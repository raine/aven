use anyhow::{Context, Result, bail};
use sqlx::SqliteConnection;

use super::{ServerSyncPage, ServerSyncResult};
use crate::change_log::op_type;
use crate::db::{Database, begin_immediate};
use crate::sync::wire::{ChangeRow, ChangeWire, PushAck};

impl Database {
    /// Accepts one pushed page and returns one bounded pull page, assigning
    /// server sequences like a sync server. Attachment additions are refused.
    pub async fn persist_server_sync_page(&self, page: ServerSyncPage) -> Result<ServerSyncResult> {
        let active_protocol = crate::sync::wire::SYNC_PROTOCOL_VERSION;
        let envelope =
            crate::sync::wire::validate_request_at_protocol(&page.request, active_protocol)?;
        for change in &page.request.changes {
            super::super::protocol::validate_change(active_protocol, change)?;
            crate::sync::wire::validate_local_change_shape(change)?;
        }
        if page
            .request
            .changes
            .iter()
            .any(|change| change.op_type == op_type::ATTACHMENT_ADD)
        {
            bail!("error attachment-blob-storage-required");
        }
        let mut conn = self.acquire_writer().await?;
        let (accepted_count, push_acks) =
            assign_server_sequences(&mut conn, page.request.changes).await?;
        let (changes, has_more) =
            load_server_changes_after(&mut conn, envelope.after, envelope.pull_limit).await?;
        Ok(ServerSyncResult {
            accepted_count,
            push_acks,
            changes,
            has_more,
        })
    }
}

async fn assign_server_sequences(
    conn: &mut SqliteConnection,
    changes: Vec<ChangeWire>,
) -> Result<(i64, Vec<PushAck>)> {
    if changes.is_empty() {
        return Ok((0, Vec::new()));
    }
    let mut tx = begin_immediate(conn).await?;
    let mut next_server_seq = next_available_server_seq(&mut tx).await?;
    let mut accepted_count = 0_i64;
    let mut push_acks = Vec::with_capacity(changes.len());
    for change in changes {
        let existing_server_seq = sqlx::query_scalar::<_, Option<i64>>(
            "SELECT server_seq FROM changes WHERE change_id = ?",
        )
        .bind(&change.change_id)
        .fetch_optional(&mut *tx)
        .await?;
        let server_seq = if let Some(existing_server_seq) = existing_server_seq {
            super::verify_existing_change(&mut tx, &change).await?;
            existing_server_seq.context("existing server change missing sequence")?
        } else {
            let server_seq = next_server_seq;
            next_server_seq += 1;
            let payload = change.payload.to_string();
            sqlx::query(
                "INSERT INTO changes(change_id, client_id, local_seq, entity_type, entity_id, field,
                 op_type, payload, base_version, created_at, server_seq)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&change.change_id)
            .bind(&change.client_id)
            .bind(change.local_seq)
            .bind(&change.entity_type)
            .bind(&change.entity_id)
            .bind(&change.field)
            .bind(&change.op_type)
            .bind(payload)
            .bind(&change.base_version)
            .bind(&change.created_at)
            .bind(server_seq)
            .execute(&mut *tx)
            .await?;
            accepted_count += 1;
            server_seq
        };
        push_acks.push(PushAck {
            change_id: change.change_id,
            server_seq,
        });
    }
    tx.commit().await?;
    Ok((accepted_count, push_acks))
}

async fn next_available_server_seq(conn: &mut SqliteConnection) -> Result<i64> {
    Ok(sqlx::query_scalar!(
        r#"SELECT COALESCE(MAX(server_seq), 0) + 1 AS "seq!: i64" FROM changes"#
    )
    .fetch_one(&mut *conn)
    .await?)
}

async fn load_server_changes_after(
    conn: &mut SqliteConnection,
    after: i64,
    pull_limit: u32,
) -> Result<(Vec<ChangeWire>, bool)> {
    let fetch_limit = i64::from(pull_limit) + 1;
    let rows = sqlx::query_as!(
        ChangeRow,
        r#"SELECT change_id AS "change_id!: String", client_id AS "client_id!: String",
         local_seq AS "local_seq!: i64", entity_type AS "entity_type!: String",
         entity_id AS "entity_id!: String", field, op_type AS "op_type!: String",
         payload AS "payload!: String", base_version, created_at AS "created_at!: String",
         server_seq
         FROM changes WHERE server_seq > ? ORDER BY server_seq LIMIT ?"#,
        after,
        fetch_limit,
    )
    .fetch_all(&mut *conn)
    .await?;
    let has_more = rows.len() > pull_limit as usize;
    let changes = rows
        .into_iter()
        .take(pull_limit as usize)
        .map(ChangeRow::into_wire)
        .collect();
    Ok((changes, has_more))
}
