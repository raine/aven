use anyhow::Result;
use sqlx::SqliteConnection;

use crate::ids::WorkspaceId;

pub struct WorkspaceTaskCounts {
    pub visible: i64,
    pub total: i64,
}

pub async fn workspace_task_counts(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
) -> Result<WorkspaceTaskCounts> {
    let visible =
        sqlx::query_scalar("SELECT count(*) FROM tasks WHERE workspace_id = ? AND deleted = 0")
            .bind(workspace_id)
            .fetch_one(&mut *conn)
            .await?;
    let total = sqlx::query_scalar("SELECT count(*) FROM tasks WHERE workspace_id = ?")
        .bind(workspace_id)
        .fetch_one(&mut *conn)
        .await?;
    Ok(WorkspaceTaskCounts { visible, total })
}

pub async fn unresolved_conflict_count(conn: &mut SqliteConnection) -> Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM conflicts WHERE resolved = 0")
            .fetch_one(&mut *conn)
            .await?,
    )
}
