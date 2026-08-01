use std::collections::HashSet;
use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use sqlx::{QueryBuilder, Sqlite, SqliteConnection};

use super::apply::apply_remote_change;
use super::wire::{ChangeRow, ChangeWire, PushAck, SyncRequest, SyncResponse};
use crate::change_log::op_type;
use crate::db::{Database, begin_immediate, get_meta, set_meta};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncPersistenceStatus {
    pub pinned_server: Option<String>,
    pub pending_changes: i64,
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
    pub assign_ms: u128,
    pub pull_query_ms: u128,
}

impl Database {
    pub async fn sync_persistence_status(&self) -> Result<SyncPersistenceStatus> {
        let mut conn = self.acquire_writer().await?;
        let pending_changes =
            sqlx::query_scalar("SELECT count(*) FROM changes WHERE server_seq IS NULL")
                .fetch_one(&mut *conn)
                .await?;
        let conflicts = sqlx::query_scalar("SELECT count(*) FROM conflicts WHERE resolved = 0")
            .fetch_one(&mut *conn)
            .await?;
        Ok(SyncPersistenceStatus {
            pinned_server: get_meta(&mut conn, "sync_server_url").await?,
            pending_changes,
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

    pub(super) async fn pending_sync_change_count(&self) -> Result<i64> {
        let mut conn = self.acquire_reader().await?;
        Ok(
            sqlx::query_scalar("SELECT COUNT(*) FROM changes WHERE server_seq IS NULL")
                .fetch_one(&mut *conn)
                .await?,
        )
    }

    pub(super) async fn pending_blob_contracts(
        &self,
    ) -> Result<Vec<super::wire::BlobUploadContract>> {
        let mut conn = self.acquire_reader().await?;
        let changes = load_unsynced_changes(&mut conn, i64::MAX as usize).await?;
        super::blob::unique_blob_contracts(&changes)
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
    for change in &page.response.changes {
        if existing_change_ids.contains(change.change_id.as_str()) {
            verify_existing_change(&mut tx, change).await?;
            update_change_server_seq(&mut tx, &change.change_id, change.server_seq).await?;
            continue;
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
        insert_wire_change(&mut tx, change).await?;
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
    let existing_change_ids = load_existing_change_ids(&mut tx, &changes).await?;
    let unassigned_changes = changes
        .iter()
        .filter(|change| !existing_change_ids.contains(&change.change_id))
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
        ensure_attachment_blobs_present(&mut tx, blob_dir, &unassigned_changes).await?;
    }
    let mut next_server_seq = next_available_server_seq(&mut tx).await?;
    let mut accepted_count = 0_i64;
    let mut push_acks = Vec::with_capacity(changes.len());
    for change in changes {
        let existing_server_seq = sqlx::query_scalar::<_, Option<i64>>(
            "SELECT server_seq FROM changes WHERE change_id = ?",
        )
        .bind(&change.change_id)
        .fetch_optional(&mut *tx)
        .await?
        .flatten();
        let server_seq = if let Some(server_seq) = existing_server_seq {
            verify_existing_change(&mut tx, &change).await?;
            server_seq
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
            apply_server_blob_reference(&mut tx, &change).await?;
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

async fn apply_server_blob_reference(
    conn: &mut SqliteConnection,
    change: &ChangeWire,
) -> Result<()> {
    match change.op_type.as_str() {
        op_type::ATTACHMENT_ADD => {
            let workspace_id = change.payload["workspace_id"]
                .as_str()
                .context("payload missing workspace_id")?;
            let attachment_id = change.payload["attachment_id"]
                .as_str()
                .context("payload missing attachment_id")?;
            let sha256 = change.payload["sha256"]
                .as_str()
                .context("payload missing sha256")?;
            let byte_size = change.payload["byte_size"]
                .as_i64()
                .context("payload missing byte_size")?;
            sqlx::query(
                "INSERT INTO server_blob_references(
                   workspace_id, attachment_id, task_id, sha256, byte_size, deleted
                 ) VALUES (?, ?, ?, ?, ?, 0)
                 ON CONFLICT(workspace_id, attachment_id) DO UPDATE SET
                   task_id = excluded.task_id, sha256 = excluded.sha256,
                   byte_size = excluded.byte_size, deleted = 0",
            )
            .bind(workspace_id)
            .bind(attachment_id)
            .bind(&change.entity_id)
            .bind(sha256)
            .bind(byte_size)
            .execute(&mut *conn)
            .await?;
            sqlx::query(
                "DELETE FROM blob_upload_reservations WHERE workspace_id = ? AND sha256 = ?",
            )
            .bind(workspace_id)
            .bind(sha256)
            .execute(&mut *conn)
            .await?;
        }
        op_type::ATTACHMENT_DELETE => {
            sqlx::query(
                "UPDATE server_blob_references SET deleted = 1
                 WHERE workspace_id = ? AND attachment_id = ?",
            )
            .bind(
                change.payload["workspace_id"]
                    .as_str()
                    .context("payload missing workspace_id")?,
            )
            .bind(
                change.payload["attachment_id"]
                    .as_str()
                    .context("payload missing attachment_id")?,
            )
            .execute(&mut *conn)
            .await?;
        }
        op_type::SET_FIELD if change.field.as_deref() == Some("deleted") => {
            let deleted = change.payload["value"]
                .as_str()
                .is_some_and(|value| value == "1");
            sqlx::query(
                "INSERT INTO server_task_tombstones(workspace_id, task_id, deleted)
                 VALUES (?, ?, ?)
                 ON CONFLICT(workspace_id, task_id) DO UPDATE SET deleted = excluded.deleted",
            )
            .bind(
                change.payload["workspace_id"]
                    .as_str()
                    .context("payload missing workspace_id")?,
            )
            .bind(&change.entity_id)
            .bind(i64::from(deleted))
            .execute(&mut *conn)
            .await?;
        }
        _ => {}
    }
    Ok(())
}

async fn ensure_attachment_blobs_present(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    changes: &[ChangeWire],
) -> Result<()> {
    for change in changes {
        if change.op_type != op_type::ATTACHMENT_ADD {
            continue;
        }
        let contract = super::blob::attachment_blob_contract(change)?
            .context("error attachment-blob-missing")?;
        let Some(row) =
            crate::attachments::storage::blob_inventory_row(conn, &contract.sha256).await?
        else {
            bail!("error attachment-blob-missing");
        };
        if !row.available || !crate::attachments::object_path(blob_dir, &contract.sha256)?.exists()
        {
            bail!("error attachment-blob-missing");
        }
        if row.byte_size != contract.byte_size || row.media_type != contract.media_type {
            bail!("error blob-inventory-metadata-mismatch");
        }
        let bytes =
            tokio::fs::read(crate::attachments::object_path(blob_dir, &contract.sha256)?).await?;
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
