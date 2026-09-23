use std::collections::HashSet;
use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use sqlx::SqliteConnection;

use super::{ServerSyncPage, ServerSyncResult};
use crate::change_log::op_type;
use crate::db::{Database, begin_immediate};
#[cfg(test)]
use crate::sync::wire::SyncRequest;
use crate::sync::wire::{ChangeRow, ChangeWire, PushAck};

impl Database {
    pub async fn persist_server_sync_page(&self, page: ServerSyncPage) -> Result<ServerSyncResult> {
        self.persist_server_sync_page_inner(page, None, crate::sync::wire::SYNC_PROTOCOL_VERSION)
            .await
    }

    pub async fn persist_server_sync_page_with_blobs(
        &self,
        page: ServerSyncPage,
        blob_dir: &Path,
    ) -> Result<ServerSyncResult> {
        self.persist_server_sync_page_inner(
            page,
            Some(blob_dir),
            crate::sync::wire::SYNC_PROTOCOL_VERSION,
        )
        .await
    }

    #[cfg(test)]
    pub(in crate::sync) async fn persist_test_protocol_page(
        &self,
        request: SyncRequest,
        active_protocol: u32,
    ) -> Result<ServerSyncResult> {
        self.persist_server_sync_page_inner(ServerSyncPage { request }, None, active_protocol)
            .await
    }

    async fn persist_server_sync_page_inner(
        &self,
        page: ServerSyncPage,
        blob_dir: Option<&Path>,
        active_protocol: u32,
    ) -> Result<ServerSyncResult> {
        let _installation = self.plaintext_installation_guard()?;
        let envelope =
            crate::sync::wire::validate_request_at_protocol(&page.request, active_protocol)?;
        for change in &page.request.changes {
            super::super::protocol::validate_change(active_protocol, change)?;
            crate::sync::wire::validate_local_change_shape(change)?;
        }
        if blob_dir.is_none()
            && page
                .request
                .changes
                .iter()
                .any(|change| change.op_type == op_type::ATTACHMENT_ADD)
        {
            bail!("error attachment-blob-storage-required");
        }
        let blob_prepare_started = Instant::now();
        if let Some(blob_dir) = blob_dir {
            let mut conn = self.acquire_reader().await?;
            super::prepare_server_blobs(&mut conn, blob_dir, &page.request.changes).await?;
        }
        let blob_prepare_ms = blob_prepare_started.elapsed().as_millis();
        let mut conn = self.acquire_writer().await?;
        super::super::shared_state::adoption::ensure_unbound(&mut conn).await?;
        let assign_started = Instant::now();
        let (accepted_count, push_acks) =
            super::assign_server_sequences(&mut conn, page.request.changes, blob_dir).await?;
        let assign_ms = assign_started.elapsed().as_millis();
        let pull_query_started = Instant::now();
        let (changes, has_more) =
            load_server_changes_after(&mut conn, envelope.after, envelope.pull_limit).await?;
        let pull_query_ms = pull_query_started.elapsed().as_millis();
        Ok(ServerSyncResult {
            accepted_count,
            push_acks,
            changes,
            has_more,
            blob_prepare_ms,
            assign_ms,
            pull_query_ms,
        })
    }
}

pub(super) async fn assign_server_sequences(
    conn: &mut SqliteConnection,
    changes: Vec<ChangeWire>,
    blob_dir: Option<&Path>,
) -> Result<(i64, Vec<PushAck>)> {
    if changes.is_empty() {
        return Ok((0, Vec::new()));
    }
    let mut tx = begin_immediate(conn).await?;
    super::super::shared_state::adoption::ensure_unbound(&mut tx).await?;
    let assigned_change_ids = super::load_assigned_change_ids(&mut tx, &changes).await?;
    let unassigned_changes = changes
        .iter()
        .filter(|change| !assigned_change_ids.contains(&change.change_id))
        .cloned()
        .collect::<Vec<_>>();
    if unassigned_changes
        .iter()
        .any(|change| change.op_type == op_type::ATTACHMENT_ADD)
        && blob_dir.is_none()
    {
        bail!("error attachment-blob-storage-required");
    }
    if let Some(blob_dir) = blob_dir {
        super::ensure_attachment_blobs_admitted(&mut tx, blob_dir, &unassigned_changes).await?;
    }
    let mut next_server_seq = next_available_server_seq(&mut tx).await?;
    let mut accepted_count = 0_i64;
    let mut push_acks = Vec::with_capacity(changes.len());
    let mut affected_attachment_hashes = HashSet::new();
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
            super::apply_server_blob_reference(&mut tx, &change, &mut affected_attachment_hashes)
                .await?;
            accepted_count += 1;
            server_seq
        };
        push_acks.push(PushAck {
            change_id: change.change_id,
            server_seq,
        });
    }
    let affected_attachment_hashes = affected_attachment_hashes.into_iter().collect::<Vec<_>>();
    crate::attachments::lifecycle::reconcile_liveness_for_hashes_in_transaction(
        &mut tx,
        &affected_attachment_hashes,
        &crate::attachments::lifecycle::SystemClock,
    )
    .await?;
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
