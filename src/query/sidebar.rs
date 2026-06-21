use anyhow::Result;
use sqlx::{Row, SqliteConnection};

use crate::workspaces::active_workspace_id;

use super::SidebarCounts;

#[allow(dead_code)]
pub(crate) async fn sidebar_counts(conn: &mut SqliteConnection) -> Result<SidebarCounts> {
    let workspace_id = active_workspace_id();
    sidebar_counts_in_workspace(conn, &workspace_id).await
}

pub(crate) async fn sidebar_counts_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &str,
) -> Result<SidebarCounts> {
    let row = sqlx::query(
        "SELECT
         COALESCE(SUM(CASE WHEN deleted = 0 AND status NOT IN ('done', 'canceled') THEN 1 ELSE 0 END), 0) AS all_count,
         COALESCE(SUM(CASE WHEN deleted = 0 AND status = 'inbox' THEN 1 ELSE 0 END), 0) AS inbox_count,
         COALESCE(SUM(CASE WHEN deleted = 0 AND status = 'active' THEN 1 ELSE 0 END), 0) AS active_count,
         COALESCE(SUM(CASE WHEN deleted = 0 AND status = 'backlog' THEN 1 ELSE 0 END), 0) AS backlog_count,
         COALESCE(SUM(CASE WHEN deleted = 0 AND status = 'todo' THEN 1 ELSE 0 END), 0) AS todo_count,
         COALESCE(SUM(CASE WHEN deleted = 0 AND status = 'done' THEN 1 ELSE 0 END), 0) AS done_count,
         (SELECT COUNT(DISTINCT c.task_id)
          FROM conflicts c
          JOIN tasks t ON t.workspace_id = c.workspace_id AND t.id = c.task_id
          WHERE c.workspace_id = ? AND c.resolved = 0 AND t.deleted = 0) AS conflicts_count
         FROM tasks
         WHERE workspace_id = ?",
    )
    .bind(workspace_id)
    .bind(workspace_id)
    .fetch_one(&mut *conn)
    .await?;
    Ok(SidebarCounts {
        all: row.get("all_count"),
        inbox: row.get("inbox_count"),
        active: row.get("active_count"),
        backlog: row.get("backlog_count"),
        todo: row.get("todo_count"),
        conflicts: row.get("conflicts_count"),
        done: row.get("done_count"),
    })
}
