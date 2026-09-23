use super::*;
use crate::db::{self, Database, begin_immediate};
use crate::sync::{persistence as p, wire::ChangeWire};
use anyhow::Context as _;
use sqlx::{Row, SqliteConnection};
use std::collections::HashSet;

async fn state(conn: &mut SqliteConnection, a: &Authority) -> Result<i64> {
    valid(crate::sync::protocol::replica_protocol(conn).await? <= 18)?;
    valid(
        db::get_meta(conn, "e2ee_association").await?.as_deref() == Some(a.association.as_str()),
    )?;
    valid(
        db::get_meta(conn, "sync_generation").await?.as_deref()
            == Some(a.sync_generation.to_string().as_str()),
    )?;
    let cursor = db::get_meta(conn, "sync_cursor")
        .await?
        .context("error encrypted-tail-cursor")?
        .parse::<i64>()?;
    valid(cursor >= a.prefix)?;
    Ok(cursor)
}
async fn load(conn: &mut SqliteConnection, id: &str) -> Result<Option<ChangeWire>> {
    let row = sqlx::query("SELECT * FROM changes WHERE change_id=?")
        .bind(id)
        .fetch_optional(conn)
        .await?;
    row.map(|r| {
        Ok(ChangeWire {
            change_id: r.try_get("change_id")?,
            client_id: r.try_get("client_id")?,
            local_seq: r.try_get("local_seq")?,
            entity_type: r.try_get("entity_type")?,
            entity_id: r.try_get("entity_id")?,
            field: r.try_get("field")?,
            op_type: r.try_get("op_type")?,
            payload: domain::strict_value(r.try_get::<String, _>("payload")?.as_bytes())?,
            base_version: r.try_get("base_version")?,
            created_at: r.try_get("created_at")?,
            server_seq: r.try_get("server_seq")?,
        })
    })
    .transpose()
}
fn plain(c: &ChangeWire) -> ChangeWire {
    let mut c = c.clone();
    c.server_seq = None;
    c
}
fn equal(a: &ChangeWire, b: &ChangeWire) -> Result<()> {
    domain::validate(&plain(a))?;
    domain::validate(&plain(b))?;
    ensure!(
        p::changes::canonical_equal(a, b),
        "error encrypted-tail-same-id-divergence"
    );
    Ok(())
}
async fn mapping(conn: &mut SqliteConnection, m: &Mapping, cursor: i64) -> Result<()> {
    let rows:Vec<(String,i64,Vec<u8>)>=sqlx::query_as("SELECT operation_id,sequence,commitment FROM local_e2ee_accepted WHERE operation_id=? OR sequence=?")
        .bind(&m.operation_id).bind(m.sequence).fetch_all(&mut *conn).await?;
    for (id, seq, hash) in &rows {
        valid(id == &m.operation_id && *seq == m.sequence && hash.as_slice() == m.commitment)?;
    }
    valid(m.sequence > cursor || !rows.is_empty())?;
    if let Some(c) = load(conn, &m.operation_id).await? {
        valid(c.server_seq.is_none_or(|s| s == m.sequence))?;
    }
    let collision: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM changes WHERE server_seq=? AND change_id!=?)",
    )
    .bind(m.sequence)
    .bind(&m.operation_id)
    .fetch_one(conn)
    .await?;
    valid(!collision)
}
async fn save(conn: &mut SqliteConnection, record: &Accepted) -> Result<()> {
    sqlx::query("INSERT INTO local_e2ee_accepted(operation_id,sequence,commitment,record) VALUES(?,?,?,?) ON CONFLICT(operation_id) DO NOTHING")
        .bind(&record.mapping.operation_id).bind(record.mapping.sequence).bind(record.mapping.commitment.as_slice()).bind(&record.record).execute(&mut *conn).await?;
    sqlx::query("DELETE FROM local_e2ee_outbox WHERE operation_id=?")
        .bind(&record.mapping.operation_id)
        .execute(conn)
        .await?;
    Ok(())
}
fn verify(a: &Authority, r: &Accepted) -> Result<ChangeWire> {
    valid(r.mapping.sequence > a.prefix && hash(&r.record) == r.mapping.commitment)?;
    let c = codec::open(a, &r.record)?;
    valid(c.change_id == r.mapping.operation_id)?;
    Ok(c)
}
impl Database {
    pub async fn encrypted_tail_cursor(&self, a: &Authority) -> Result<i64> {
        let mut conn = self.acquire_reader().await?;
        state(&mut conn, a).await
    }
    pub async fn prepare_encrypted_tail(&self, a: &Authority) -> Result<Option<Vec<u8>>> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        state(&mut tx, a).await?;
        let frozen:Option<(String,i64,Vec<u8>,bool)>=sqlx::query_as("SELECT association,sync_generation,record,blocked FROM local_e2ee_outbox WHERE singleton=1").fetch_optional(&mut *tx).await?;
        if let Some((association, generation, record, blocked)) = frozen {
            valid(association == a.association && generation == a.sync_generation)?;
            ensure!(!blocked, "error encrypted-tail-integrity-blocked");
            let c = codec::open(a, &record)?;
            equal(
                &c,
                &load(&mut tx, &c.change_id)
                    .await?
                    .context("error encrypted-tail-history-lost")?,
            )?;
            tx.commit().await?;
            return Ok(Some(record));
        }
        let ids:Vec<String>=sqlx::query_scalar("SELECT change_id FROM changes WHERE server_seq IS NULL ORDER BY local_seq,created_at,change_id LIMIT 4097").fetch_all(&mut *tx).await?;
        ensure!(ids.len() <= 4096, "error encrypted-tail-preflight-limit");
        let mut bytes = 0;
        let mut first = None;
        for id in ids {
            let c = load(&mut tx, &id)
                .await?
                .context("error encrypted-tail-history-lost")?;
            bytes += serde_json::to_vec(&c)?.len();
            ensure!(
                bytes <= 16 * 1048576,
                "error encrypted-tail-preflight-limit"
            );
            domain::validate_state(&mut tx, &c).await?;
            if first.is_none() {
                first = Some(c)
            }
        }
        let record = if let Some(c) = first {
            let record = codec::seal(a, &c)?;
            // Check the full decoder contract before committing dispatch authority.
            equal(&c, &codec::open(a, &record)?)?;
            sqlx::query("INSERT INTO local_e2ee_outbox(singleton,operation_id,association,sync_generation,record) VALUES(1,?,?,?,?)")
                .bind(c.change_id).bind(&a.association).bind(a.sync_generation).bind(&record).execute(&mut *tx).await?;
            Some(record)
        } else {
            None
        };
        tx.commit().await?;
        #[cfg(any(test, feature = "test-support"))]
        super::crash_at("frozen");
        Ok(record)
    }
    /// Retain an observed immutable outcome before fetching a different representation.
    pub async fn observe_encrypted_tail(&self, a: &Authority, m: &Mapping) -> Result<Vec<u8>> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let cursor = state(&mut tx, a).await?;
        mapping(&mut tx, m, cursor).await?;
        valid(m.sequence > a.prefix)?;
        let (id,record,old_seq,old_hash):(String,Vec<u8>,Option<i64>,Option<Vec<u8>>)=sqlx::query_as("SELECT operation_id,record,observed_sequence,observed_commitment FROM local_e2ee_outbox WHERE singleton=1").fetch_one(&mut *tx).await?;
        valid(
            id == m.operation_id
                && old_seq.is_none_or(|s| s == m.sequence)
                && old_hash
                    .as_ref()
                    .is_none_or(|h| h.as_slice() == m.commitment),
        )?;
        sqlx::query("UPDATE local_e2ee_outbox SET observed_sequence=?,observed_commitment=? WHERE singleton=1").bind(m.sequence).bind(m.commitment.as_slice()).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(record)
    }
    pub async fn verify_encrypted_tail_outcome(&self, a: &Authority, r: &Accepted) -> Result<()> {
        let result = self.commit_encrypted_tail_outcome(a, r).await;
        if result
            .as_ref()
            .err()
            .is_some_and(|e| e.to_string() == "error encrypted-tail-same-id-divergence")
        {
            let mut conn = self.acquire_writer().await?;
            sqlx::query("UPDATE local_e2ee_outbox SET blocked=1 WHERE operation_id=?")
                .bind(&r.mapping.operation_id)
                .execute(&mut *conn)
                .await?;
        }
        result
    }
    async fn commit_encrypted_tail_outcome(&self, a: &Authority, r: &Accepted) -> Result<()> {
        let c = verify(a, r)?;
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let cursor = state(&mut tx, a).await?;
        mapping(&mut tx, &r.mapping, cursor).await?;
        let observed:Option<(Option<i64>,Option<Vec<u8>>)>=sqlx::query_as("SELECT observed_sequence,observed_commitment FROM local_e2ee_outbox WHERE operation_id=?").bind(&c.change_id).fetch_optional(&mut *tx).await?;
        if let Some((seq, hash)) = observed {
            valid(
                seq == Some(r.mapping.sequence)
                    && hash.as_deref() == Some(r.mapping.commitment.as_slice()),
            )?;
        }
        let local = load(&mut tx, &c.change_id)
            .await?
            .context("error encrypted-tail-local-missing")?;
        equal(&local, &c)?;
        domain::validate_state(&mut tx, &c).await?;
        p::update_change_server_seq(&mut tx, &c.change_id, Some(r.mapping.sequence)).await?;
        p::reconcile_epic_change(&mut tx, &c).await?;
        save(&mut tx, r).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn apply_encrypted_tail_page(&self, a: &Authority, page: &Page) -> Result<()> {
        valid(
            page.records.len() <= PAGE_COUNT
                && page.after >= a.prefix
                && page.watermark >= page.after
                && page.records.iter().map(|r| r.record.len()).sum::<usize>() <= PAGE_BYTES,
        )?;
        let mut ids = HashSet::new();
        let mut previous = page.after;
        let mut changes = Vec::new();
        for r in &page.records {
            valid(
                r.mapping.sequence > previous
                    && r.mapping.sequence <= page.watermark
                    && ids.insert(&r.mapping.operation_id),
            )?;
            changes.push(verify(a, r)?);
            previous = r.mapping.sequence;
        }
        valid(page.cursor == previous && (!page.has_more || previous > page.after))?;
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        valid(state(&mut tx, a).await? == page.after)?;
        let mut hashes = HashSet::new();
        // Validate every mapping and local comparison before any domain effects.
        for (r, c) in page.records.iter().zip(&changes) {
            mapping(&mut tx, &r.mapping, page.after).await?;
            if let Some(local) = load(&mut tx, &c.change_id).await? {
                equal(&local, c)?;
            }
            let observed:Option<(Option<i64>,Option<Vec<u8>>)>=sqlx::query_as("SELECT observed_sequence,observed_commitment FROM local_e2ee_outbox WHERE operation_id=?").bind(&c.change_id).fetch_optional(&mut *tx).await?;
            if let Some((Some(seq), Some(hash))) = observed {
                valid(seq == r.mapping.sequence && hash.as_slice() == r.mapping.commitment)?;
            }
        }
        // Local ranks precede incoming relation application, just as plaintext apply.
        for (r, c) in page.records.iter().zip(&changes) {
            if load(&mut tx, &c.change_id).await?.is_some() {
                p::update_change_server_seq(&mut tx, &c.change_id, Some(r.mapping.sequence))
                    .await?;
                p::reconcile_epic_change(&mut tx, c).await?;
            }
        }
        for (r, mut c) in page.records.iter().zip(changes) {
            domain::validate_state(&mut tx, &c).await?;
            if load(&mut tx, &c.change_id).await?.is_none() {
                c.server_seq = Some(r.mapping.sequence);
                p::collect_attachment_liveness_hashes(&mut tx, &c, &mut hashes).await?;
                if p::is_epic_change(&c) {
                    crate::epic_membership::capture_snapshot_baseline(
                        &mut tx,
                        p::epic_change_workspace(&c)?,
                        &c.entity_id,
                    )
                    .await?;
                }
                let related = matches!(c.op_type.as_str(), "related_add" | "related_remove");
                if related {
                    p::insert_wire_change(&mut tx, &c).await?;
                }
                crate::sync::apply::apply_remote_change_quiet(&mut tx, &c)
                    .await
                    .map_err(|_| anyhow::anyhow!("error encrypted-tail-apply"))?;
                if !related {
                    p::insert_wire_change(&mut tx, &c).await?;
                }
                p::reconcile_epic_change(&mut tx, &c).await?;
            }
            save(&mut tx, r).await?;
        }
        crate::attachments::lifecycle::reconcile_liveness_for_hashes_in_transaction(
            &mut tx,
            &hashes.into_iter().collect::<Vec<_>>(),
            &crate::attachments::lifecycle::SystemClock,
        )
        .await?;
        crate::sync::protocol::establish_protocol(&mut tx, 18).await?;
        db::set_meta(&mut tx, "sync_cursor", &page.cursor.to_string()).await?;
        #[cfg(any(test, feature = "test-support"))]
        super::crash_at("before-page-commit");
        tx.commit().await?;
        #[cfg(any(test, feature = "test-support"))]
        super::crash_at("after-page-commit");
        Ok(())
    }
}
