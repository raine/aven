use anyhow::Result;

use crate::db::{Database, get_meta};

use super::SyncPersistenceStatus;

impl Database {
    /// Local sync bookkeeping; reads no protected keys and contacts no server.
    pub async fn sync_persistence_status(&self) -> Result<SyncPersistenceStatus> {
        let mut conn = self.acquire_reader().await?;
        let pending_changes =
            sqlx::query_scalar("SELECT count(*) FROM changes WHERE server_seq IS NULL")
                .fetch_one(&mut *conn)
                .await?;
        let conflicts = sqlx::query_scalar("SELECT count(*) FROM conflicts WHERE resolved = 0")
            .fetch_one(&mut *conn)
            .await?;
        Ok(SyncPersistenceStatus {
            pending_changes,
            conflicts,
            sync_cursor: get_meta(&mut conn, "sync_cursor").await?,
            local_sequence: get_meta(&mut conn, "local_seq").await?,
        })
    }
}
