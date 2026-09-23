use anyhow::Result;

use crate::db::{Database, get_meta, set_meta};

use super::SyncPersistenceStatus;

impl Database {
    pub async fn sync_persistence_status(&self) -> Result<SyncPersistenceStatus> {
        let mut conn = self.acquire_reader().await?;
        super::sync_persistence_status(&mut conn).await
    }

    pub async fn sync_facts(&self) -> Result<crate::api::SyncFacts> {
        let mut conn = self.acquire_reader().await?;
        let mut tx = sqlx::Connection::begin(&mut *conn).await?;
        let status = super::sync_persistence_status(&mut tx).await?;
        let missing = super::super::blob::missing_local_blob_counts(&mut tx).await?;
        let facts = crate::api::SyncFacts {
            compatibility_block: status.blocked_protocol.map(|server_protocol| {
                super::super::protocol::SyncCompatibilityError {
                    server_protocol,
                    client_protocol: super::super::wire::SYNC_PROTOCOL_VERSION,
                }
            }),
            pending_changes: status.pending_changes,
            attachment_uploads: status.pending_attachment_uploads,
            attachment_downloads: missing.count,
            metadata_confirmed_at: get_meta(&mut tx, "sync_metadata_confirmed_at").await?,
            metadata_caught_up: get_meta(&mut tx, "sync_metadata_caught_up")
                .await?
                .as_deref()
                == Some("1"),
        };
        tx.commit().await?;
        Ok(facts)
    }

    pub async fn begin_sync_attempt(&self, attempted_at: String) -> Result<()> {
        let _installation = self.plaintext_installation_guard()?;
        let mut conn = self.acquire_writer().await?;
        super::super::shared_state::adoption::ensure_unbound(&mut conn).await?;
        set_meta(&mut conn, "sync_last_attempt_at", &attempted_at).await
    }

    pub async fn record_sync_error(&self, error: String) -> Result<()> {
        let _installation = self.plaintext_installation_guard()?;
        let mut conn = self.acquire_writer().await?;
        super::super::shared_state::adoption::ensure_unbound(&mut conn).await?;
        set_meta(&mut conn, "sync_last_error", &error).await
    }
}

pub(super) async fn sync_persistence_status(
    conn: &mut sqlx::SqliteConnection,
) -> Result<SyncPersistenceStatus> {
    let pending_changes =
        sqlx::query_scalar("SELECT count(*) FROM changes WHERE server_seq IS NULL")
            .fetch_one(&mut *conn)
            .await?;
    let (pending_attachment_uploads, pending_attachment_upload_bytes): (i64, i64) = sqlx::query_as(
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
        pinned_server: get_meta(conn, "sync_server_url").await?,
        established_protocol: super::super::protocol::replica_protocol(conn).await?,
        blocked_protocol: get_meta(conn, "sync_blocked_protocol")
            .await?
            .filter(|v| !v.is_empty())
            .map(|v| v.parse())
            .transpose()?,
        pending_changes,
        pending_attachment_uploads,
        pending_attachment_upload_bytes,
        conflicts,
        sync_cursor: get_meta(conn, "sync_cursor").await?,
        local_sequence: get_meta(conn, "local_seq").await?,
        last_attempt: get_meta(conn, "sync_last_attempt_at").await?,
        last_success: get_meta(conn, "sync_last_success_at").await?,
        last_error: get_meta(conn, "sync_last_error").await?,
        last_pushed: get_meta(conn, "sync_last_pushed").await?,
        last_pulled: get_meta(conn, "sync_last_pulled").await?,
        last_cursor: get_meta(conn, "sync_last_cursor").await?,
    })
}
