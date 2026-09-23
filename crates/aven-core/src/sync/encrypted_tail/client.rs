use super::*;
use crate::db::{self, Database, begin_immediate};
use crate::sync::{persistence, wire::ChangeWire};
use anyhow::Context as _;
use sqlx::{Row, SqliteConnection};
use std::collections::HashSet;

async fn validate_binding_and_cursor(
    conn: &mut SqliteConnection,
    authority: &Authority,
) -> Result<i64> {
    valid(crate::sync::protocol::replica_protocol(conn).await? <= 18)?;
    valid(
        db::get_meta(conn, "e2ee_association").await?.as_deref()
            == Some(authority.association.as_str()),
    )?;
    valid(
        db::get_meta(conn, "sync_generation").await?.as_deref()
            == Some(authority.sync_generation.to_string().as_str()),
    )?;
    let cursor = db::get_meta(conn, "sync_cursor")
        .await?
        .context("error encrypted-tail-cursor")?
        .parse::<i64>()?;
    valid(cursor >= authority.prefix)?;
    super::dependencies::validate(conn, &authority.association, authority.prefix).await?;
    Ok(cursor)
}

pub(super) async fn load_change(
    conn: &mut SqliteConnection,
    id: &str,
) -> Result<Option<ChangeWire>> {
    let row = sqlx::query("SELECT * FROM changes WHERE change_id = ?")
        .bind(id)
        .fetch_optional(conn)
        .await?;
    row.map(|row| {
        Ok(ChangeWire {
            change_id: row.try_get("change_id")?,
            client_id: row.try_get("client_id")?,
            local_seq: row.try_get("local_seq")?,
            entity_type: row.try_get("entity_type")?,
            entity_id: row.try_get("entity_id")?,
            field: row.try_get("field")?,
            op_type: row.try_get("op_type")?,
            payload: domain::strict_value(row.try_get::<String, _>("payload")?.as_bytes())?,
            base_version: row.try_get("base_version")?,
            created_at: row.try_get("created_at")?,
            server_seq: row.try_get("server_seq")?,
        })
    })
    .transpose()
}

fn without_server_sequence(change: &ChangeWire) -> ChangeWire {
    let mut change = change.clone();
    change.server_seq = None;
    change
}

fn require_canonical_equality(local: &ChangeWire, incoming: &ChangeWire) -> Result<()> {
    domain::validate(&without_server_sequence(local))?;
    domain::validate(&without_server_sequence(incoming))?;
    ensure!(
        persistence::changes::canonical_equal(local, incoming),
        "error encrypted-tail-same-id-divergence"
    );
    Ok(())
}

async fn validate_mapping(
    conn: &mut SqliteConnection,
    mapping: &Mapping,
    cursor: i64,
) -> Result<()> {
    let rows: Vec<(String, i64, Vec<u8>)> = sqlx::query_as(
        "SELECT operation_id, sequence, commitment FROM local_e2ee_accepted
         WHERE operation_id = ? OR sequence = ?",
    )
    .bind(&mapping.operation_id)
    .bind(mapping.sequence)
    .fetch_all(&mut *conn)
    .await?;
    for (operation_id, sequence, commitment) in &rows {
        valid(
            operation_id == &mapping.operation_id
                && *sequence == mapping.sequence
                && commitment.as_slice() == mapping.commitment,
        )?;
    }
    valid(mapping.sequence > cursor || !rows.is_empty())?;
    if let Some(change) = load_change(conn, &mapping.operation_id).await? {
        valid(change.server_seq.is_none_or(|s| s == mapping.sequence))?;
    }
    let collision: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM changes WHERE server_seq = ? AND change_id != ?)",
    )
    .bind(mapping.sequence)
    .bind(&mapping.operation_id)
    .fetch_one(conn)
    .await?;
    valid(!collision)
}

async fn record_acceptance_and_clear_outbox(
    conn: &mut SqliteConnection,
    record: &Accepted,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO local_e2ee_accepted(operation_id, sequence, commitment, record)
         VALUES (?, ?, ?, ?) ON CONFLICT(operation_id) DO NOTHING",
    )
    .bind(&record.mapping.operation_id)
    .bind(record.mapping.sequence)
    .bind(record.mapping.commitment.as_slice())
    .bind(&record.record)
    .execute(&mut *conn)
    .await?;
    sqlx::query("DELETE FROM local_e2ee_outbox WHERE operation_id = ?")
        .bind(&record.mapping.operation_id)
        .execute(conn)
        .await?;
    Ok(())
}

fn open_accepted_change(authority: &Authority, accepted: &Accepted) -> Result<ChangeWire> {
    valid(
        accepted.mapping.sequence > authority.prefix
            && hash(&accepted.record) == accepted.mapping.commitment,
    )?;
    let change = codec::open(authority, &accepted.record)?;
    valid(change.change_id == accepted.mapping.operation_id)?;
    Ok(change)
}

async fn load_observed_outcome(
    conn: &mut SqliteConnection,
    operation_id: &str,
) -> Result<Option<(Option<i64>, Option<Vec<u8>>)>> {
    Ok(sqlx::query_as(
        "SELECT observed_sequence, observed_commitment FROM local_e2ee_outbox
         WHERE operation_id = ?",
    )
    .bind(operation_id)
    .fetch_optional(conn)
    .await?)
}

async fn apply_new_remote_change(
    conn: &mut SqliteConnection,
    change: &ChangeWire,
    attachment_hashes: &mut HashSet<String>,
) -> Result<()> {
    persistence::collect_attachment_liveness_hashes(conn, change, attachment_hashes).await?;
    if persistence::is_epic_change(change) {
        crate::epic_membership::capture_snapshot_baseline(
            conn,
            persistence::epic_change_workspace(change)?,
            &change.entity_id,
        )
        .await?;
    }
    let related = matches!(change.op_type.as_str(), "related_add" | "related_remove");
    // Related links reference their establishing change through a foreign key,
    // so their history row must exist before applying the presence register.
    if related {
        persistence::insert_wire_change(conn, change).await?;
    }
    crate::sync::apply::apply_remote_change_quiet(conn, change)
        .await
        .map_err(|_| anyhow::anyhow!("error encrypted-tail-apply"))?;
    if !related {
        persistence::insert_wire_change(conn, change).await?;
    }
    persistence::reconcile_epic_change(conn, change).await?;
    Ok(())
}

impl Database {
    pub async fn encrypted_tail_cursor(&self, authority: &Authority) -> Result<i64> {
        let mut conn = self.acquire_reader().await?;
        validate_binding_and_cursor(&mut conn, authority).await
    }
    /// Observes local work without validating, freezing or claiming its history.
    pub async fn encrypted_tail_idle(&self, authority: &Authority) -> Result<bool> {
        let mut conn = self.acquire_reader().await?;
        let mut tx = sqlx::Connection::begin(&mut *conn).await?;
        validate_binding_and_cursor(&mut tx, authority).await?;
        let pending: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM local_e2ee_outbox)
                 OR EXISTS(SELECT 1 FROM changes WHERE server_seq IS NULL)",
        )
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(!pending)
    }
    pub async fn prepare_encrypted_tail(&self, authority: &Authority) -> Result<Option<Vec<u8>>> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        validate_binding_and_cursor(&mut tx, authority).await?;
        let frozen: Option<(String, i64, Vec<u8>, bool)> = sqlx::query_as(
            "SELECT association, sync_generation, record, blocked
             FROM local_e2ee_outbox WHERE singleton = 1",
        )
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((association, generation, record, blocked)) = frozen {
            valid(association == authority.association && generation == authority.sync_generation)?;
            ensure!(!blocked, "error encrypted-tail-integrity-blocked");
            let change = codec::open(authority, &record)?;
            require_canonical_equality(
                &change,
                &load_change(&mut tx, &change.change_id)
                    .await?
                    .context("error encrypted-tail-history-lost")?,
            )?;
            tx.commit().await?;
            return Ok(Some(record));
        }
        let pending_ids: Vec<String> = sqlx::query_scalar(
            "SELECT change_id FROM changes WHERE server_seq IS NULL
             ORDER BY local_seq, created_at, change_id LIMIT 4097",
        )
        .fetch_all(&mut *tx)
        .await?;
        ensure!(
            pending_ids.len() <= 4096,
            "error encrypted-tail-preflight-limit"
        );
        let mut preflight_bytes = 0;
        let mut first_pending = None;
        for id in pending_ids {
            let change = load_change(&mut tx, &id)
                .await?
                .context("error encrypted-tail-history-lost")?;
            preflight_bytes += serde_json::to_vec(&change)?.len();
            ensure!(
                preflight_bytes <= 16 * 1048576,
                "error encrypted-tail-preflight-limit"
            );
            domain::validate_state(&mut tx, &change).await?;
            if first_pending.is_none() {
                first_pending = Some(change)
            }
        }
        let record = if let Some(change) = first_pending {
            let record = codec::seal(authority, &change)?;
            // Check the full decoder contract before committing dispatch authority.
            require_canonical_equality(&change, &codec::open(authority, &record)?)?;
            sqlx::query(
                "INSERT INTO local_e2ee_outbox(
                     singleton, operation_id, association, sync_generation, record
                 ) VALUES (1, ?, ?, ?, ?)",
            )
            .bind(change.change_id)
            .bind(&authority.association)
            .bind(authority.sync_generation)
            .bind(&record)
            .execute(&mut *tx)
            .await?;
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
    pub async fn observe_encrypted_tail(
        &self,
        authority: &Authority,
        mapping: &Mapping,
    ) -> Result<Vec<u8>> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let cursor = validate_binding_and_cursor(&mut tx, authority).await?;
        validate_mapping(&mut tx, mapping, cursor).await?;
        valid(mapping.sequence > authority.prefix)?;
        let (operation_id, record, observed_sequence, observed_commitment): (
            String,
            Vec<u8>,
            Option<i64>,
            Option<Vec<u8>>,
        ) = sqlx::query_as(
            "SELECT operation_id, record, observed_sequence, observed_commitment
             FROM local_e2ee_outbox WHERE singleton = 1",
        )
        .fetch_one(&mut *tx)
        .await?;
        valid(
            operation_id == mapping.operation_id
                && observed_sequence.is_none_or(|sequence| sequence == mapping.sequence)
                && observed_commitment
                    .as_ref()
                    .is_none_or(|h| h.as_slice() == mapping.commitment),
        )?;
        sqlx::query(
            "UPDATE local_e2ee_outbox SET observed_sequence = ?, observed_commitment = ?
             WHERE singleton = 1",
        )
        .bind(mapping.sequence)
        .bind(mapping.commitment.as_slice())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(record)
    }
    pub async fn verify_encrypted_tail_outcome(
        &self,
        authority: &Authority,
        accepted: &Accepted,
    ) -> Result<()> {
        let result = self
            .commit_encrypted_tail_outcome(authority, accepted)
            .await;
        if result
            .as_ref()
            .err()
            .is_some_and(|error| error.to_string() == "error encrypted-tail-same-id-divergence")
        {
            let mut conn = self.acquire_writer().await?;
            sqlx::query("UPDATE local_e2ee_outbox SET blocked = 1 WHERE operation_id = ?")
                .bind(&accepted.mapping.operation_id)
                .execute(&mut *conn)
                .await?;
        }
        result
    }
    async fn commit_encrypted_tail_outcome(
        &self,
        authority: &Authority,
        accepted: &Accepted,
    ) -> Result<()> {
        let change = open_accepted_change(authority, accepted)?;
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let cursor = validate_binding_and_cursor(&mut tx, authority).await?;
        validate_mapping(&mut tx, &accepted.mapping, cursor).await?;
        let observed = load_observed_outcome(&mut tx, &change.change_id).await?;
        if let Some((sequence, commitment)) = observed {
            valid(
                sequence == Some(accepted.mapping.sequence)
                    && commitment.as_deref() == Some(accepted.mapping.commitment.as_slice()),
            )?;
        }
        let local = load_change(&mut tx, &change.change_id)
            .await?
            .context("error encrypted-tail-local-missing")?;
        require_canonical_equality(&local, &change)?;
        domain::validate_state(&mut tx, &change).await?;
        persistence::update_change_server_seq(
            &mut tx,
            &change.change_id,
            Some(accepted.mapping.sequence),
        )
        .await?;
        persistence::reconcile_epic_change(&mut tx, &change).await?;
        super::notes::reconcile(&mut tx, authority.prefix, &change).await?;
        super::labels::reconcile(&mut tx, authority.prefix, &change).await?;
        if let Some(workspace) = super::dependencies::affected_workspace(&change)? {
            super::dependencies::reconcile(&mut tx, authority.prefix, workspace).await?;
        }
        record_acceptance_and_clear_outbox(&mut tx, accepted).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn apply_encrypted_tail_page(
        &self,
        authority: &Authority,
        page: &Page,
    ) -> Result<()> {
        valid(
            page.records.len() <= PAGE_COUNT
                && page.after >= authority.prefix
                && page.watermark >= page.after
                && page
                    .records
                    .iter()
                    .map(|accepted| accepted.record.len())
                    .sum::<usize>()
                    <= PAGE_BYTES,
        )?;
        let mut operation_ids = HashSet::new();
        let mut previous_sequence = page.after;
        let mut changes = Vec::new();
        for accepted in &page.records {
            valid(
                accepted.mapping.sequence > previous_sequence
                    && accepted.mapping.sequence <= page.watermark
                    && operation_ids.insert(&accepted.mapping.operation_id),
            )?;
            changes.push(open_accepted_change(authority, accepted)?);
            previous_sequence = accepted.mapping.sequence;
        }
        valid(
            page.cursor == previous_sequence && (!page.has_more || previous_sequence > page.after),
        )?;
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        valid(validate_binding_and_cursor(&mut tx, authority).await? == page.after)?;
        let mut attachment_hashes = HashSet::new();
        let mut dependency_workspaces = HashSet::new();
        // Validate every mapping and local comparison before any domain effects.
        let mut local_presence = Vec::with_capacity(changes.len());
        for (accepted, change) in page.records.iter().zip(&changes) {
            validate_mapping(&mut tx, &accepted.mapping, page.after).await?;
            let local = load_change(&mut tx, &change.change_id).await?;
            if let Some(local) = &local {
                require_canonical_equality(local, change)?;
            }
            let observed = load_observed_outcome(&mut tx, &change.change_id).await?;
            if let Some((Some(sequence), Some(commitment))) = observed {
                valid(
                    sequence == accepted.mapping.sequence
                        && commitment.as_slice() == accepted.mapping.commitment,
                )?;
            }
            local_presence.push(local.is_some());
        }
        // All local ranks precede incoming relation application. Page IDs are unique,
        // and ranking does not insert history, so validated presence remains stable.
        for ((accepted, change), is_local) in page.records.iter().zip(&changes).zip(&local_presence)
        {
            if *is_local {
                persistence::update_change_server_seq(
                    &mut tx,
                    &change.change_id,
                    Some(accepted.mapping.sequence),
                )
                .await?;
                persistence::reconcile_epic_change(&mut tx, change).await?;
            }
        }
        for ((accepted, mut change), is_local) in
            page.records.iter().zip(changes).zip(local_presence)
        {
            domain::validate_state(&mut tx, &change).await?;
            if !is_local {
                change.server_seq = Some(accepted.mapping.sequence);
                apply_new_remote_change(&mut tx, &change, &mut attachment_hashes).await?;
            }
            if let Some(workspace) = super::dependencies::affected_workspace(&change)? {
                dependency_workspaces.insert(workspace.to_owned());
            }
            super::notes::reconcile(&mut tx, authority.prefix, &change).await?;
            super::labels::reconcile(&mut tx, authority.prefix, &change).await?;
            record_acceptance_and_clear_outbox(&mut tx, accepted).await?;
        }
        for workspace in dependency_workspaces {
            super::dependencies::reconcile(&mut tx, authority.prefix, &workspace).await?;
        }
        crate::attachments::lifecycle::reconcile_liveness_for_hashes_in_transaction(
            &mut tx,
            &attachment_hashes.into_iter().collect::<Vec<_>>(),
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
