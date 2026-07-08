use anyhow::Result;
use sqlx::{Row, SqliteConnection};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SyncHistoryStats {
    pub(crate) total_change_rows: i64,
    pub(crate) pending_change_rows: i64,
    pub(crate) synced_change_rows: i64,
    pub(crate) min_server_seq: Option<i64>,
    pub(crate) max_server_seq: Option<i64>,
    pub(crate) payload_bytes: i64,
}

pub(crate) async fn sync_history_stats(conn: &mut SqliteConnection) -> Result<SyncHistoryStats> {
    let row = sqlx::query(
        "SELECT
         COUNT(*) AS total_change_rows,
         COALESCE(SUM(CASE WHEN server_seq IS NULL THEN 1 ELSE 0 END), 0) AS pending_change_rows,
         COALESCE(SUM(CASE WHEN server_seq IS NOT NULL THEN 1 ELSE 0 END), 0) AS synced_change_rows,
         MIN(server_seq) AS min_server_seq,
         MAX(server_seq) AS max_server_seq,
         COALESCE(SUM(length(CAST(payload AS BLOB))), 0) AS payload_bytes
         FROM changes",
    )
    .fetch_one(&mut *conn)
    .await?;

    Ok(SyncHistoryStats {
        total_change_rows: row.get("total_change_rows"),
        pending_change_rows: row.get("pending_change_rows"),
        synced_change_rows: row.get("synced_change_rows"),
        min_server_seq: row.get("min_server_seq"),
        max_server_seq: row.get("max_server_seq"),
        payload_bytes: row.get("payload_bytes"),
    })
}
