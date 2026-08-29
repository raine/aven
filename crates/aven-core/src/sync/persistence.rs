use std::collections::HashSet;
use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use sqlx::{QueryBuilder, Sqlite, SqliteConnection};

use super::apply::apply_remote_change;
use super::wire::{
    AttachmentAddPayload, ChangeRow, ChangeWire, PushAck, SyncRequest, SyncResponse,
};
use crate::change_log::op_type;
use crate::db::{Database, begin_immediate, get_meta, set_meta};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncPersistenceStatus {
    pub pinned_server: Option<String>,
    pub pending_changes: i64,
    pub pending_attachment_uploads: i64,
    pub pending_attachment_upload_bytes: i64,
    pub conflicts: i64,
    pub sync_cursor: Option<String>,
    pub local_sequence: Option<String>,
    pub last_attempt: Option<String>,
    pub last_success: Option<String>,
    pub last_error: Option<String>,
    pub last_pushed: Option<String>,
    pub last_pulled: Option<String>,
    pub last_cursor: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ClientSyncPage {
    pub request: SyncRequest,
    pub pending: usize,
}

#[derive(Debug)]
pub struct ApplySyncPage {
    pub request: SyncRequest,
    pub response: SyncResponse,
    pub attempted_at: String,
    pub previous_pushed: i64,
    pub previous_pulled: usize,
}

#[derive(Debug)]
pub struct ServerSyncPage {
    pub request: SyncRequest,
}

#[derive(Debug)]
pub struct ServerSyncResult {
    pub accepted_count: i64,
    pub push_acks: Vec<PushAck>,
    pub changes: Vec<ChangeWire>,
    pub has_more: bool,
    pub blob_prepare_ms: u128,
    pub assign_ms: u128,
    pub pull_query_ms: u128,
}

impl Database {
    pub async fn sync_persistence_status(&self) -> Result<SyncPersistenceStatus> {
        let mut conn = self.acquire_reader().await?;
        let pending_changes =
            sqlx::query_scalar("SELECT count(*) FROM changes WHERE server_seq IS NULL")
                .fetch_one(&mut *conn)
                .await?;
        let (pending_attachment_uploads, pending_attachment_upload_bytes): (i64, i64) =
            sqlx::query_as(
                "SELECT COUNT(*), COALESCE(SUM(byte_size), 0)
                 FROM (
                   SELECT json_extract(payload, '$.workspace_id') AS workspace_id,
                          json_extract(payload, '$.sha256') AS sha256,
                          MAX(CAST(json_extract(payload, '$.byte_size') AS INTEGER)) AS byte_size
                   FROM changes
                   WHERE server_seq IS NULL AND op_type = 'attachment_add'
                   GROUP BY workspace_id, sha256
                 )",
            )
            .fetch_one(&mut *conn)
            .await?;
        let conflicts = sqlx::query_scalar("SELECT count(*) FROM conflicts WHERE resolved = 0")
            .fetch_one(&mut *conn)
            .await?;
        Ok(SyncPersistenceStatus {
            pinned_server: get_meta(&mut conn, "sync_server_url").await?,
            pending_changes,
            pending_attachment_uploads,
            pending_attachment_upload_bytes,
            conflicts,
            sync_cursor: get_meta(&mut conn, "sync_cursor").await?,
            local_sequence: get_meta(&mut conn, "local_seq").await?,
            last_attempt: get_meta(&mut conn, "sync_last_attempt_at").await?,
            last_success: get_meta(&mut conn, "sync_last_success_at").await?,
            last_error: get_meta(&mut conn, "sync_last_error").await?,
            last_pushed: get_meta(&mut conn, "sync_last_pushed").await?,
            last_pulled: get_meta(&mut conn, "sync_last_pulled").await?,
            last_cursor: get_meta(&mut conn, "sync_last_cursor").await?,
        })
    }

    pub async fn begin_sync_attempt(&self, attempted_at: String) -> Result<()> {
        let mut conn = self.acquire_writer().await?;
        set_meta(&mut conn, "sync_last_attempt_at", &attempted_at).await
    }

    pub async fn record_sync_error(&self, error: String) -> Result<()> {
        let mut conn = self.acquire_writer().await?;
        set_meta(&mut conn, "sync_last_error", &error).await
    }

    pub(super) async fn pending_sync_changes_exist(&self) -> Result<bool> {
        let mut conn = self.acquire_reader().await?;
        Ok(
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM changes WHERE server_seq IS NULL)")
                .fetch_one(&mut *conn)
                .await?,
        )
    }

    pub(super) async fn pending_blob_counts(
        &self,
        known_server_blobs: &HashSet<String>,
    ) -> Result<crate::attachments::lifecycle::ByteCount> {
        let mut conn = self.acquire_reader().await?;
        let known = serde_json::to_string(known_server_blobs)?;
        let (count, bytes): (i64, i64) = sqlx::query_as(
            "WITH known(sha256) AS (
               SELECT value FROM json_each(?)
             ), pending AS (
               SELECT json_extract(payload, '$.workspace_id') AS workspace_id,
                      json_extract(payload, '$.sha256') AS sha256,
                      MAX(CAST(json_extract(payload, '$.byte_size') AS INTEGER)) AS byte_size
               FROM changes
               WHERE server_seq IS NULL AND op_type = 'attachment_add'
               GROUP BY workspace_id, sha256
             )
             SELECT COUNT(*), COALESCE(SUM(byte_size), 0)
             FROM pending LEFT JOIN known USING (sha256)
             WHERE known.sha256 IS NULL",
        )
        .bind(known)
        .fetch_one(&mut *conn)
        .await?;
        Ok(crate::attachments::lifecycle::ByteCount {
            count: u64::try_from(count)?,
            bytes: u64::try_from(bytes)?,
        })
    }

    pub async fn prepare_client_sync_page(
        &self,
        server: String,
        push_limit: usize,
        pull_limit: u32,
    ) -> Result<ClientSyncPage> {
        let mut conn = self.acquire_writer().await?;
        validate_sync_server(&mut conn, &server).await?;
        let client_id = get_meta(&mut conn, "client_id")
            .await?
            .context("missing client id")?;
        let after = sync_cursor(&mut conn).await?;
        let changes = load_unsynced_changes(&mut conn, push_limit).await?;
        let pending = changes.len();
        Ok(ClientSyncPage {
            request: SyncRequest {
                protocol_version: Some(super::wire::SYNC_PROTOCOL_VERSION),
                client_id,
                after,
                pull_limit: Some(pull_limit),
                changes,
            },
            pending,
        })
    }

    pub async fn apply_client_sync_page(&self, page: ApplySyncPage) -> Result<usize> {
        let envelope = super::wire::validate_sync_request_envelope(&page.request)?;
        let request_change_ids = page
            .request
            .changes
            .iter()
            .map(|change| change.change_id.clone())
            .collect::<Vec<_>>();
        super::wire::validate_sync_response_for_request(
            envelope.after,
            envelope.pull_limit,
            &request_change_ids,
            &page.response,
        )?;
        let mut conn = self.acquire_writer().await?;
        apply_sync_response(&mut conn, page).await
    }

    pub async fn persist_server_sync_page(&self, page: ServerSyncPage) -> Result<ServerSyncResult> {
        self.persist_server_sync_page_inner(page, None).await
    }

    pub async fn persist_server_sync_page_with_blobs(
        &self,
        page: ServerSyncPage,
        blob_dir: &Path,
    ) -> Result<ServerSyncResult> {
        self.persist_server_sync_page_inner(page, Some(blob_dir))
            .await
    }

    async fn persist_server_sync_page_inner(
        &self,
        page: ServerSyncPage,
        blob_dir: Option<&Path>,
    ) -> Result<ServerSyncResult> {
        let envelope = super::wire::validate_sync_request_envelope(&page.request)?;
        for change in &page.request.changes {
            super::wire::validate_pushed_change(change)?;
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
            prepare_server_blobs(&mut conn, blob_dir, &page.request.changes).await?;
        }
        let blob_prepare_ms = blob_prepare_started.elapsed().as_millis();
        let mut conn = self.acquire_writer().await?;
        let assign_started = Instant::now();
        let (accepted_count, push_acks) =
            assign_server_sequences(&mut conn, page.request.changes, blob_dir).await?;
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

async fn sync_cursor(conn: &mut SqliteConnection) -> Result<i64> {
    Ok(get_meta(conn, "sync_cursor")
        .await?
        .unwrap_or_else(|| "0".to_string())
        .parse::<i64>()?)
}

async fn validate_sync_server(conn: &mut SqliteConnection, server: &str) -> Result<()> {
    let normalized = server.trim_end_matches('/');
    if let Some(existing) = get_meta(conn, "sync_server_url").await? {
        if existing != normalized {
            bail!(
                "error sync-server-changed existing={} requested={} hint=\"use a fresh database for a different sync server\"",
                existing,
                normalized
            );
        }
    } else {
        set_meta(conn, "sync_server_url", normalized).await?;
    }
    Ok(())
}

async fn load_unsynced_changes(
    conn: &mut SqliteConnection,
    limit: usize,
) -> Result<Vec<ChangeWire>> {
    let limit = limit as i64;
    let rows = sqlx::query_as!(
        ChangeRow,
        r#"SELECT change_id AS "change_id!: String", client_id AS "client_id!: String",
         local_seq AS "local_seq!: i64", entity_type AS "entity_type!: String",
         entity_id AS "entity_id!: String", field, op_type AS "op_type!: String",
         payload AS "payload!: String", base_version, created_at AS "created_at!: String",
         server_seq
         FROM changes WHERE server_seq IS NULL ORDER BY local_seq, created_at LIMIT ?"#,
        limit,
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows.into_iter().map(ChangeRow::into_wire).collect())
}

async fn apply_sync_response(conn: &mut SqliteConnection, page: ApplySyncPage) -> Result<usize> {
    let mut applied = 0;
    let mut tx = begin_immediate(conn).await?;
    let current_cursor = sync_cursor(&mut tx).await?;
    if current_cursor != page.request.after {
        bail!(
            "error stale-sync-page expected_cursor={} request_cursor={}",
            current_cursor,
            page.request.after
        );
    }
    update_change_server_seqs_if_missing(&mut tx, &page.response.push_acks).await?;
    let existing_change_ids = load_existing_change_ids(&mut tx, &page.response.changes).await?;
    let mut affected_series = HashSet::new();
    let mut affected_attachment_hashes = HashSet::new();
    for change in &page.response.changes {
        if existing_change_ids.contains(change.change_id.as_str()) {
            verify_existing_change(&mut tx, change).await?;
            update_change_server_seq(&mut tx, &change.change_id, change.server_seq).await?;
            continue;
        }
        collect_attachment_liveness_hashes(&mut tx, change, &mut affected_attachment_hashes)
            .await?;
        let related_mutation = matches!(
            change.op_type.as_str(),
            op_type::RELATED_ADD | op_type::RELATED_REMOVE
        );
        if related_mutation {
            insert_wire_change(&mut tx, change).await?;
        }
        apply_remote_change(&mut tx, change).await?;
        if change.entity_type == "recurrence_series" {
            let workspace_id = change
                .payload
                .get("workspace_id")
                .and_then(serde_json::Value::as_str)
                .context("recurrence change missing workspace_id")?;
            affected_series.insert((workspace_id.to_string(), change.entity_id.clone()));
        } else if let Some(series_id) = change
            .payload
            .get("series_id")
            .and_then(serde_json::Value::as_str)
        {
            let workspace_id = change
                .payload
                .get("workspace_id")
                .and_then(serde_json::Value::as_str)
                .context("recurrence task change missing workspace_id")?;
            affected_series.insert((workspace_id.to_string(), series_id.to_string()));
        }
        if !related_mutation {
            insert_wire_change(&mut tx, change).await?;
        }
        applied += 1;
    }
    for (workspace_id, series_id) in affected_series {
        let workspace_id: crate::ids::WorkspaceId = workspace_id.parse()?;
        let series_id: crate::recurrence::RecurrenceSeriesId = series_id.parse()?;
        let workspace = crate::workspaces::workspace_for_id(&mut tx, &workspace_id).await?;
        let at =
            chrono::DateTime::parse_from_rfc3339(&page.attempted_at)?.with_timezone(&chrono::Utc);
        crate::operations::recurrence::reconcile_recurrence_series_in_transaction(
            &mut tx, &workspace, &series_id, at,
        )
        .await?;
    }
    let affected_attachment_hashes = affected_attachment_hashes.into_iter().collect::<Vec<_>>();
    crate::attachments::lifecycle::reconcile_liveness_for_hashes_in_transaction(
        &mut tx,
        &affected_attachment_hashes,
        &crate::attachments::lifecycle::SystemClock,
    )
    .await?;
    let pushed = page.previous_pushed + page.response.push_acks.len() as i64;
    let pulled = page.previous_pulled + applied;
    set_meta(&mut tx, "sync_cursor", &page.response.cursor.to_string()).await?;
    set_meta(&mut tx, "sync_last_success_at", &page.attempted_at).await?;
    set_meta(&mut tx, "sync_last_error", "").await?;
    set_meta(&mut tx, "sync_last_pushed", &pushed.to_string()).await?;
    set_meta(&mut tx, "sync_last_pulled", &pulled.to_string()).await?;
    set_meta(
        &mut tx,
        "sync_last_cursor",
        &page.response.cursor.to_string(),
    )
    .await?;
    tx.commit().await?;
    Ok(applied)
}

async fn load_existing_change_ids(
    conn: &mut SqliteConnection,
    changes: &[ChangeWire],
) -> Result<HashSet<String>> {
    if changes.is_empty() {
        return Ok(HashSet::new());
    }
    let mut query_builder =
        QueryBuilder::<Sqlite>::new("SELECT change_id FROM changes WHERE change_id IN (");
    let mut separated = query_builder.separated(", ");
    for change in changes {
        separated.push_bind(&change.change_id);
    }
    separated.push_unseparated(")");
    Ok(query_builder
        .build_query_scalar::<String>()
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .collect())
}

async fn load_assigned_change_ids(
    conn: &mut SqliteConnection,
    changes: &[ChangeWire],
) -> Result<HashSet<String>> {
    if changes.is_empty() {
        return Ok(HashSet::new());
    }
    let mut query_builder = QueryBuilder::<Sqlite>::new(
        "SELECT change_id FROM changes WHERE server_seq IS NOT NULL AND change_id IN (",
    );
    let mut separated = query_builder.separated(", ");
    for change in changes {
        separated.push_bind(&change.change_id);
    }
    separated.push_unseparated(")");
    Ok(query_builder
        .build_query_scalar::<String>()
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .collect())
}

async fn verify_existing_change(conn: &mut SqliteConnection, incoming: &ChangeWire) -> Result<()> {
    let row = sqlx::query(
        "SELECT entity_type, entity_id, field, op_type, payload, base_version, created_at
         FROM changes WHERE change_id = ?",
    )
    .bind(&incoming.change_id)
    .fetch_one(&mut *conn)
    .await?;
    use sqlx::Row;
    let stored_payload: String = row.try_get("payload")?;
    let stored_payload: serde_json::Value = serde_json::from_str(&stored_payload)?;
    let equal = row.try_get::<String, _>("entity_type")? == incoming.entity_type
        && row.try_get::<String, _>("entity_id")? == incoming.entity_id
        && row.try_get::<Option<String>, _>("field")? == incoming.field
        && row.try_get::<String, _>("op_type")? == incoming.op_type
        && stored_payload == incoming.payload
        && row.try_get::<Option<String>, _>("base_version")? == incoming.base_version
        && row.try_get::<String, _>("created_at")? == incoming.created_at;
    if !equal {
        bail!(
            "error sync-change-id-payload-mismatch change_id={}",
            incoming.change_id
        );
    }
    Ok(())
}

async fn update_change_server_seq(
    conn: &mut SqliteConnection,
    change_id: &str,
    server_seq: Option<i64>,
) -> Result<()> {
    if let Some(server_seq) = server_seq {
        sqlx::query!(
            "UPDATE changes SET server_seq = ? WHERE change_id = ? AND server_seq IS NULL",
            server_seq,
            change_id,
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

async fn update_change_server_seqs_if_missing(
    conn: &mut SqliteConnection,
    push_acks: &[PushAck],
) -> Result<()> {
    if push_acks.is_empty() {
        return Ok(());
    }
    let mut query_builder = QueryBuilder::<Sqlite>::new("WITH updates(change_id, server_seq) AS (");
    query_builder.push_values(push_acks, |mut row, ack| {
        row.push_bind(&ack.change_id).push_bind(ack.server_seq);
    });
    query_builder.push(
        ") UPDATE changes
         SET server_seq = (
             SELECT updates.server_seq
             FROM updates
             WHERE updates.change_id = changes.change_id
         )
         WHERE server_seq IS NULL
           AND change_id IN (SELECT change_id FROM updates)",
    );
    query_builder.build().execute(&mut *conn).await?;
    Ok(())
}

async fn insert_wire_change(conn: &mut SqliteConnection, change: &ChangeWire) -> Result<()> {
    let payload = change.payload.to_string();
    sqlx::query!(
        "INSERT OR IGNORE INTO changes(change_id, client_id, local_seq, entity_type, entity_id, field,
         op_type, payload, base_version, created_at, server_seq)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        change.change_id,
        change.client_id,
        change.local_seq,
        change.entity_type,
        change.entity_id,
        change.field,
        change.op_type,
        payload,
        change.base_version,
        change.created_at,
        change.server_seq,
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

async fn assign_server_sequences(
    conn: &mut SqliteConnection,
    changes: Vec<ChangeWire>,
    blob_dir: Option<&Path>,
) -> Result<(i64, Vec<PushAck>)> {
    if changes.is_empty() {
        return Ok((0, Vec::new()));
    }
    let mut tx = begin_immediate(conn).await?;
    let assigned_change_ids = load_assigned_change_ids(&mut tx, &changes).await?;
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
        ensure_attachment_blobs_admitted(&mut tx, blob_dir, &unassigned_changes).await?;
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
            verify_existing_change(&mut tx, &change).await?;
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
            apply_server_blob_reference(&mut tx, &change, &mut affected_attachment_hashes).await?;
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

async fn apply_server_blob_reference(
    conn: &mut SqliteConnection,
    change: &ChangeWire,
    affected_attachment_hashes: &mut HashSet<String>,
) -> Result<()> {
    match change.op_type.as_str() {
        op_type::ATTACHMENT_ADD => {
            let payload = AttachmentAddPayload::from_change(change)?;
            let previous_sha256: Option<String> = sqlx::query_scalar(
                "SELECT sha256 FROM server_blob_references
                 WHERE workspace_id = ? AND attachment_id = ?",
            )
            .bind(&payload.workspace_id)
            .bind(&payload.attachment_id)
            .fetch_optional(&mut *conn)
            .await?;
            if let Some(previous_sha256) = previous_sha256 {
                affected_attachment_hashes.insert(previous_sha256);
            }
            affected_attachment_hashes.insert(payload.sha256.clone());
            sqlx::query(
                "INSERT INTO server_blob_references(
                   workspace_id, attachment_id, task_id, sha256, byte_size, deleted
                 ) VALUES (?, ?, ?, ?, ?, 0)
                 ON CONFLICT(workspace_id, attachment_id) DO UPDATE SET
                   task_id = excluded.task_id, sha256 = excluded.sha256,
                   byte_size = excluded.byte_size, deleted = 0",
            )
            .bind(&payload.workspace_id)
            .bind(&payload.attachment_id)
            .bind(&change.entity_id)
            .bind(&payload.sha256)
            .bind(payload.byte_size)
            .execute(&mut *conn)
            .await?;
            sqlx::query(
                "DELETE FROM blob_upload_reservations WHERE workspace_id = ? AND sha256 = ?",
            )
            .bind(&payload.workspace_id)
            .bind(&payload.sha256)
            .execute(&mut *conn)
            .await?;
        }
        op_type::ATTACHMENT_DELETE => {
            let workspace_id = change.payload["workspace_id"]
                .as_str()
                .context("payload missing workspace_id")?;
            let attachment_id = change.payload["attachment_id"]
                .as_str()
                .context("payload missing attachment_id")?;
            let sha256: Option<String> = sqlx::query_scalar(
                "SELECT sha256 FROM server_blob_references
                 WHERE workspace_id = ? AND attachment_id = ?",
            )
            .bind(workspace_id)
            .bind(attachment_id)
            .fetch_optional(&mut *conn)
            .await?;
            if let Some(sha256) = sha256 {
                affected_attachment_hashes.insert(sha256);
            }
            sqlx::query(
                "UPDATE server_blob_references SET deleted = 1
                 WHERE workspace_id = ? AND attachment_id = ?",
            )
            .bind(workspace_id)
            .bind(attachment_id)
            .execute(&mut *conn)
            .await?;
        }
        op_type::SET_FIELD | op_type::RESOLVE_FIELD
            if change.field.as_deref() == Some("deleted") =>
        {
            let workspace_id = change.payload["workspace_id"]
                .as_str()
                .context("payload missing workspace_id")?;
            let hashes: Vec<String> = sqlx::query_scalar(
                "SELECT DISTINCT sha256 FROM server_blob_references
                 WHERE workspace_id = ? AND task_id = ?",
            )
            .bind(workspace_id)
            .bind(&change.entity_id)
            .fetch_all(&mut *conn)
            .await?;
            affected_attachment_hashes.extend(hashes);
            let deleted = change.payload["value"]
                .as_str()
                .is_some_and(|value| value == "1");
            sqlx::query(
                "INSERT INTO server_task_tombstones(workspace_id, task_id, deleted)
                 VALUES (?, ?, ?)
                 ON CONFLICT(workspace_id, task_id) DO UPDATE SET deleted = excluded.deleted",
            )
            .bind(workspace_id)
            .bind(&change.entity_id)
            .bind(i64::from(deleted))
            .execute(&mut *conn)
            .await?;
        }
        _ => {}
    }
    Ok(())
}

async fn collect_attachment_liveness_hashes(
    conn: &mut SqliteConnection,
    change: &ChangeWire,
    affected_attachment_hashes: &mut HashSet<String>,
) -> Result<()> {
    match change.op_type.as_str() {
        op_type::ATTACHMENT_ADD => {
            let payload = AttachmentAddPayload::from_change(change)?;
            affected_attachment_hashes.insert(payload.sha256);
        }
        op_type::ATTACHMENT_DELETE => {
            let workspace_id = change.payload["workspace_id"]
                .as_str()
                .context("payload missing workspace_id")?;
            let attachment_id = change.payload["attachment_id"]
                .as_str()
                .context("payload missing attachment_id")?;
            let sha256: Option<String> = sqlx::query_scalar(
                "SELECT sha256 FROM task_attachments
                 WHERE workspace_id = ? AND attachment_id = ?",
            )
            .bind(workspace_id)
            .bind(attachment_id)
            .fetch_optional(&mut *conn)
            .await?;
            if let Some(sha256) = sha256 {
                affected_attachment_hashes.insert(sha256);
            }
        }
        op_type::SET_FIELD | op_type::RESOLVE_FIELD
            if change.field.as_deref() == Some("deleted") =>
        {
            let workspace_id = change.payload["workspace_id"]
                .as_str()
                .context("payload missing workspace_id")?;
            let hashes: Vec<String> = sqlx::query_scalar(
                "SELECT DISTINCT sha256 FROM task_attachments
                 WHERE workspace_id = ? AND task_id = ?",
            )
            .bind(workspace_id)
            .bind(&change.entity_id)
            .fetch_all(&mut *conn)
            .await?;
            affected_attachment_hashes.extend(hashes);
        }
        _ => {}
    }
    Ok(())
}

async fn prepare_server_blobs(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    changes: &[ChangeWire],
) -> Result<()> {
    let assigned_change_ids = load_assigned_change_ids(conn, changes).await?;
    let contracts = changes
        .iter()
        .filter(|change| {
            change.op_type == op_type::ATTACHMENT_ADD
                && !assigned_change_ids.contains(&change.change_id)
        })
        .map(|change| {
            super::blob::attachment_blob_contract(change)?.context("error attachment-blob-missing")
        })
        .collect::<Result<Vec<_>>>()?;
    for contract in super::blob::unique_blob_content_contracts(&contracts)? {
        validate_server_blob_before_writer(conn, blob_dir, &contract).await?;
    }
    Ok(())
}

async fn validate_server_blob_before_writer(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    contract: &super::wire::BlobUploadContract,
) -> Result<()> {
    let Some(row) = crate::attachments::storage::blob_inventory_row(conn, &contract.sha256).await?
    else {
        bail!("error attachment-blob-missing");
    };
    validate_server_blob_inventory(&row, contract)?;
    let path = crate::attachments::object_path(blob_dir, &contract.sha256)?;
    let bytes = match tokio::fs::read(path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            bail!("error attachment-blob-missing")
        }
        Err(error) => return Err(error.into()),
    };
    if i64::try_from(bytes.len()).ok() != Some(contract.byte_size)
        || crate::attachments::storage::sha256_hex(&bytes) != contract.sha256
    {
        bail!("error attachment-blob-content-mismatch");
    }
    let validated =
        crate::attachments::decode::validate_image(bytes, Some(contract.media_type.clone()))
            .await?;
    if (validated.facts.width, validated.facts.height) != (contract.width, contract.height) {
        bail!("error blob-inventory-metadata-mismatch");
    }
    Ok(())
}

fn validate_server_blob_inventory(
    row: &crate::types::BlobInventoryRow,
    contract: &super::wire::BlobUploadContract,
) -> Result<()> {
    if !row.available {
        bail!("error attachment-blob-missing");
    }
    if row.byte_size != contract.byte_size || row.media_type != contract.media_type {
        bail!("error blob-inventory-metadata-mismatch");
    }
    Ok(())
}

async fn ensure_attachment_blobs_admitted(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    changes: &[ChangeWire],
) -> Result<()> {
    let contracts = super::blob::attachment_blob_contracts(changes)?;
    for contract in super::blob::unique_blob_content_contracts(&contracts)? {
        let Some(row) =
            crate::attachments::storage::blob_inventory_row(conn, &contract.sha256).await?
        else {
            bail!("error attachment-blob-missing");
        };
        validate_server_blob_inventory(&row, &contract)?;
        if !crate::attachments::object_path(blob_dir, &contract.sha256)?.exists() {
            bail!("error attachment-blob-missing");
        }
    }
    for contract in super::blob::unique_blob_admission_contracts(&contracts) {
        let admitted: bool = sqlx::query_scalar(
            "SELECT EXISTS(
               SELECT 1 FROM server_blob_references sbr
               LEFT JOIN server_task_tombstones st
                 ON st.workspace_id = sbr.workspace_id AND st.task_id = sbr.task_id
               WHERE sbr.workspace_id = ? AND sbr.sha256 = ? AND sbr.deleted = 0
                 AND COALESCE(st.deleted, 0) = 0
             ) OR EXISTS(
               SELECT 1 FROM blob_upload_reservations
               WHERE workspace_id = ? AND sha256 = ? AND byte_size = ? AND expires_at > ?
             )",
        )
        .bind(&contract.workspace_id)
        .bind(&contract.sha256)
        .bind(&contract.workspace_id)
        .bind(&contract.sha256)
        .bind(contract.byte_size)
        .bind(crate::ids::now())
        .fetch_one(&mut *conn)
        .await?;
        if !admitted {
            bail!("error attachment-blob-unreserved");
        }
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::time::Duration;

    use image::{DynamicImage, ImageFormat};
    use serde_json::json;

    use super::*;
    use crate::attachments::storage::{object_path, sha256_hex, upsert_inventory_available};
    use crate::sync::wire::{ChangeWire, SYNC_PROTOCOL_VERSION, SyncRequest};

    #[tokio::test]
    async fn server_task_deletion_operations_reconcile_attachment_liveness() {
        let (_temp, mut conn) = crate::test_support::test_conn().await;
        for (index, operation) in [op_type::SET_FIELD, op_type::RESOLVE_FIELD]
            .into_iter()
            .enumerate()
        {
            let task_id = format!("BBBBBBBBBBBBBBB{index}");
            let attachment_id = format!("CCCCCCCCCCCCCCC{index}");
            let sha256 = format!("{index:064x}");
            upsert_inventory_available(&mut conn, &sha256, 1, "image/png")
                .await
                .unwrap();
            sqlx::query("INSERT INTO blob_lifecycle(sha256, unreferenced_at) VALUES (?, NULL)")
                .bind(&sha256)
                .execute(&mut *conn)
                .await
                .unwrap();
            sqlx::query(
                "INSERT INTO server_blob_references(
                   workspace_id, attachment_id, task_id, sha256, byte_size, deleted
                 ) VALUES ('0000000000000000', ?, ?, ?, 1, 0)",
            )
            .bind(attachment_id)
            .bind(&task_id)
            .bind(&sha256)
            .execute(&mut *conn)
            .await
            .unwrap();

            let deletion_change = |value: &str| ChangeWire {
                change_id: format!("AAAAAAAAAAAAAA{index}{value}"),
                client_id: "client".to_string(),
                local_seq: 1,
                entity_type: "task".to_string(),
                entity_id: task_id.clone(),
                field: Some("deleted".to_string()),
                op_type: operation.to_string(),
                payload: json!({
                    "workspace_id": "0000000000000000",
                    "workspace_key": "default",
                    "value": value,
                }),
                base_version: None,
                created_at: "2026-01-01T00:00:00Z".to_string(),
                server_seq: None,
            };

            apply_server_blob_reference(&mut conn, &deletion_change("1"))
                .await
                .unwrap();
            let deleted: bool = sqlx::query_scalar(
                "SELECT deleted FROM server_task_tombstones
                 WHERE workspace_id = '0000000000000000' AND task_id = ?",
            )
            .bind(&task_id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
            let unreferenced_at: Option<String> =
                sqlx::query_scalar("SELECT unreferenced_at FROM blob_lifecycle WHERE sha256 = ?")
                    .bind(&sha256)
                    .fetch_one(&mut *conn)
                    .await
                    .unwrap();
            assert!(deleted, "{operation} must apply live-to-deleted state");
            assert!(unreferenced_at.is_some());

            apply_server_blob_reference(&mut conn, &deletion_change("0"))
                .await
                .unwrap();
            let deleted: bool = sqlx::query_scalar(
                "SELECT deleted FROM server_task_tombstones
                 WHERE workspace_id = '0000000000000000' AND task_id = ?",
            )
            .bind(&task_id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
            let unreferenced_at: Option<String> =
                sqlx::query_scalar("SELECT unreferenced_at FROM blob_lifecycle WHERE sha256 = ?")
                    .bind(&sha256)
                    .fetch_one(&mut *conn)
                    .await
                    .unwrap();
            assert!(!deleted, "{operation} must apply deleted-to-live state");
            assert_eq!(unreferenced_at, None);
        }
    }

    #[tokio::test]
    async fn related_comparison_observes_push_acknowledgement_first() {
        let (_temp, mut conn) = crate::test_support::test_conn().await;
        let workspace = crate::workspaces::Workspace::default();
        let project = crate::projects::create_project(&mut conn, &workspace, "related-ack")
            .await
            .unwrap();
        let task_id: crate::ids::TaskId = "AAAA000000000001".parse().unwrap();
        let related_task_id: crate::ids::TaskId = "BBBB000000000002".parse().unwrap();
        for id in [&task_id, &related_task_id] {
            sqlx::query(
                "INSERT INTO tasks(id, workspace_id, title, description, project_id, status, priority, created_at, updated_at)
                 VALUES (?, ?, 'task', '', ?, 'todo', 'none', 't', 't')",
            )
            .bind(id)
            .bind(&workspace.id)
            .bind(&project.id)
            .execute(&mut *conn)
            .await
            .unwrap();
        }
        let local = crate::operations::set_task_related_link_in_transaction(
            &mut conn,
            &workspace,
            &task_id,
            &related_task_id,
            true,
        )
        .await
        .unwrap();
        let local_change_id = local.change_id.unwrap();
        set_meta(&mut conn, "sync_cursor", "3").await.unwrap();

        let remote = ChangeWire {
            change_id: "CCCC000000000003".to_string(),
            client_id: "remote".to_string(),
            local_seq: 1,
            entity_type: "task".to_string(),
            entity_id: task_id.to_string(),
            field: Some("related".to_string()),
            op_type: op_type::RELATED_REMOVE.to_string(),
            payload: json!({
                "workspace_id": workspace.id,
                "workspace_key": workspace.key,
                "related_task_id": related_task_id,
            }),
            base_version: None,
            created_at: "2026-08-22T00:00:00Z".to_string(),
            server_seq: Some(4),
        };
        let page = ApplySyncPage {
            request: SyncRequest {
                protocol_version: Some(SYNC_PROTOCOL_VERSION),
                client_id: "local".to_string(),
                after: 3,
                pull_limit: Some(100),
                changes: Vec::new(),
            },
            response: SyncResponse {
                protocol_version: SYNC_PROTOCOL_VERSION,
                cursor: 4,
                has_more: false,
                push_acks: vec![PushAck {
                    change_id: local_change_id.clone(),
                    server_seq: 5,
                }],
                changes: vec![remote],
            },
            attempted_at: "2026-08-22T00:00:01Z".to_string(),
            previous_pushed: 0,
            previous_pulled: 0,
        };

        apply_sync_response(&mut conn, page).await.unwrap();

        let state: (i64, String) = sqlx::query_as(
            "SELECT linked, last_change_id FROM task_related_links
             WHERE workspace_id = ? AND task_a_id = ? AND task_b_id = ?",
        )
        .bind(&workspace.id)
        .bind(&task_id)
        .bind(&related_task_id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(state, (1, local_change_id.clone()));
        let acknowledged: Option<i64> =
            sqlx::query_scalar("SELECT server_seq FROM changes WHERE change_id = ?")
                .bind(local_change_id)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(acknowledged, Some(5));
    }

    async fn corrupt_attachment_page() -> (
        tempfile::TempDir,
        Database,
        std::path::PathBuf,
        ServerSyncPage,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let database = Database::open(&temp.path().join("server.sqlite"))
            .await
            .unwrap();
        let blob_dir = temp.path().join("blobs");
        let expected = b"expected";
        let sha256 = sha256_hex(expected);
        {
            let mut conn = database.acquire_writer().await.unwrap();
            upsert_inventory_available(&mut conn, &sha256, expected.len() as i64, "image/png")
                .await
                .unwrap();
        }
        let path = object_path(&blob_dir, &sha256).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"corrupt!").unwrap();
        let change = ChangeWire {
            change_id: "0123456789ABCDEF".to_string(),
            client_id: "client-a".to_string(),
            local_seq: 1,
            entity_type: "task".to_string(),
            entity_id: "0123456789ABCDE0".to_string(),
            field: Some("attachments".to_string()),
            op_type: op_type::ATTACHMENT_ADD.to_string(),
            payload: json!({
                "workspace_id": "0000000000000000",
                "workspace_key": "default",
                "attachment_id": "7KQ9A1X4MV2P8D6R",
                "sha256": sha256,
                "byte_size": expected.len(),
                "media_type": "image/png",
                "filename": "photo.png",
                "alt_text": "photo",
                "width": 1,
                "height": 1,
                "created_at": "2026-01-01T00:00:00Z"
            }),
            base_version: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            server_seq: None,
        };
        let page = ServerSyncPage {
            request: SyncRequest {
                protocol_version: Some(SYNC_PROTOCOL_VERSION),
                client_id: "client-a".to_string(),
                after: 0,
                pull_limit: Some(100),
                changes: vec![change],
            },
        };
        (temp, database, blob_dir, page)
    }

    fn page_for_contract(contract: &super::super::wire::BlobUploadContract) -> ServerSyncPage {
        let change = ChangeWire {
            change_id: "0123456789ABCDEF".to_string(),
            client_id: "client-a".to_string(),
            local_seq: 1,
            entity_type: "task".to_string(),
            entity_id: "0123456789ABCDE0".to_string(),
            field: Some("attachments".to_string()),
            op_type: op_type::ATTACHMENT_ADD.to_string(),
            payload: json!({
                "workspace_id": contract.workspace_id,
                "workspace_key": "default",
                "attachment_id": "7KQ9A1X4MV2P8D6R",
                "sha256": contract.sha256,
                "byte_size": contract.byte_size,
                "media_type": contract.media_type,
                "filename": "photo.png",
                "alt_text": "photo",
                "width": contract.width,
                "height": contract.height,
                "created_at": "2026-01-01T00:00:00Z"
            }),
            base_version: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            server_seq: None,
        };
        ServerSyncPage {
            request: SyncRequest {
                protocol_version: Some(SYNC_PROTOCOL_VERSION),
                client_id: "client-a".to_string(),
                after: 0,
                pull_limit: Some(100),
                changes: vec![change],
            },
        }
    }

    #[tokio::test]
    async fn corrupt_server_blob_is_rejected_before_writer_acquisition() {
        let (_temp, database, blob_dir, page) = corrupt_attachment_page().await;
        let writer = database.acquire_writer().await.unwrap();
        let task = tokio::spawn({
            let database = database.clone();
            async move {
                database
                    .persist_server_sync_page_with_blobs(page, &blob_dir)
                    .await
            }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            task.is_finished(),
            "corrupt content validation should not wait for the writer gate"
        );
        drop(writer);
        let error = task.await.unwrap().unwrap_err();
        assert!(
            error
                .to_string()
                .contains("attachment-blob-content-mismatch")
        );
    }

    #[tokio::test]
    async fn reservation_is_rechecked_after_blob_preparation() {
        let temp = tempfile::tempdir().unwrap();
        let database = Database::open(&temp.path().join("server.sqlite"))
            .await
            .unwrap();
        let blob_dir = temp.path().join("blobs");
        let mut encoded = Cursor::new(Vec::new());
        DynamicImage::new_rgba8(1, 1)
            .write_to(&mut encoded, ImageFormat::Png)
            .unwrap();
        let bytes = encoded.into_inner();
        let contract = super::super::wire::BlobUploadContract {
            workspace_id: "0000000000000000".to_string(),
            sha256: sha256_hex(&bytes),
            byte_size: bytes.len() as i64,
            media_type: "image/png".to_string(),
            width: 1,
            height: 1,
        };
        database
            .store_server_blob(
                &blob_dir,
                crate::attachments::lifecycle::LifecyclePolicy::default(),
                &contract,
                bytes,
            )
            .await
            .unwrap();
        let page = page_for_contract(&contract);
        {
            let mut reader = database.acquire_reader().await.unwrap();
            prepare_server_blobs(&mut reader, &blob_dir, &page.request.changes)
                .await
                .unwrap();
        }
        {
            let mut writer = database.acquire_writer().await.unwrap();
            sqlx::query("DELETE FROM blob_upload_reservations")
                .execute(&mut *writer)
                .await
                .unwrap();
        }
        let mut writer = database.acquire_writer().await.unwrap();
        let error = assign_server_sequences(&mut writer, page.request.changes, Some(&blob_dir))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("attachment-blob-unreserved"));
        let accepted: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM changes")
            .fetch_one(&mut *writer)
            .await
            .unwrap();
        assert_eq!(accepted, 0);
    }
}
