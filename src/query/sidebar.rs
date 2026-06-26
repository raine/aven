use anyhow::Result;
use sqlx::{Row, SqliteConnection};

use crate::projects::resolve_existing_project_in_workspace;
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
    sidebar_counts_for_scope_in_workspace(conn, workspace_id, None).await
}

pub(crate) async fn sidebar_counts_for_scope_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    project_key: Option<&str>,
) -> Result<SidebarCounts> {
    let project_id = if let Some(project_key) = project_key {
        Some(
            resolve_existing_project_in_workspace(conn, workspace_id, project_key)
                .await?
                .id,
        )
    } else {
        None
    };
    let row = match project_id {
        Some(project_id) => {
            sqlx::query(
                "SELECT
                 COALESCE(SUM(CASE WHEN t.deleted = 0 AND t.status NOT IN ('done', 'canceled') THEN 1 ELSE 0 END), 0) AS open_count,
                 COALESCE(SUM(CASE WHEN t.deleted = 0 AND t.status = 'inbox' THEN 1 ELSE 0 END), 0) AS inbox_count,
                 COALESCE(SUM(CASE WHEN t.deleted = 0 AND t.status = 'active' THEN 1 ELSE 0 END), 0) AS active_count,
                 COALESCE(SUM(CASE WHEN t.deleted = 0 AND t.status = 'backlog' THEN 1 ELSE 0 END), 0) AS backlog_count,
                 COALESCE(SUM(CASE WHEN t.deleted = 0 AND t.status = 'todo' THEN 1 ELSE 0 END), 0) AS todo_count,
                 COALESCE(SUM(CASE WHEN t.deleted = 0 AND t.status IN ('done', 'canceled') THEN 1 ELSE 0 END), 0) AS done_count,
                 (SELECT COUNT(DISTINCT c.task_id)
                  FROM conflicts c
                  JOIN tasks ct ON ct.workspace_id = c.workspace_id AND ct.id = c.task_id
                  WHERE c.workspace_id = ? AND c.resolved = 0 AND ct.deleted = 0
                  AND ct.project_id = ?) AS conflicts_count
                 FROM tasks t
                 WHERE t.workspace_id = ? AND t.project_id = ?",
            )
            .bind(workspace_id)
            .bind(&project_id)
            .bind(workspace_id)
            .bind(&project_id)
            .fetch_one(&mut *conn)
            .await?
        }
        None => {
            sqlx::query(
                "SELECT
                 COALESCE(SUM(CASE WHEN t.deleted = 0 AND t.status NOT IN ('done', 'canceled') THEN 1 ELSE 0 END), 0) AS open_count,
                 COALESCE(SUM(CASE WHEN t.deleted = 0 AND t.status = 'inbox' THEN 1 ELSE 0 END), 0) AS inbox_count,
                 COALESCE(SUM(CASE WHEN t.deleted = 0 AND t.status = 'active' THEN 1 ELSE 0 END), 0) AS active_count,
                 COALESCE(SUM(CASE WHEN t.deleted = 0 AND t.status = 'backlog' THEN 1 ELSE 0 END), 0) AS backlog_count,
                 COALESCE(SUM(CASE WHEN t.deleted = 0 AND t.status = 'todo' THEN 1 ELSE 0 END), 0) AS todo_count,
                 COALESCE(SUM(CASE WHEN t.deleted = 0 AND t.status IN ('done', 'canceled') THEN 1 ELSE 0 END), 0) AS done_count,
                 (SELECT COUNT(DISTINCT c.task_id)
                  FROM conflicts c
                  JOIN tasks ct ON ct.workspace_id = c.workspace_id AND ct.id = c.task_id
                  WHERE c.workspace_id = ? AND c.resolved = 0 AND ct.deleted = 0) AS conflicts_count
                 FROM tasks t
                 WHERE t.workspace_id = ?",
            )
            .bind(workspace_id)
            .bind(workspace_id)
            .fetch_one(&mut *conn)
            .await?
        }
    };
    Ok(SidebarCounts {
        open: row.get("open_count"),
        inbox: row.get("inbox_count"),
        active: row.get("active_count"),
        backlog: row.get("backlog_count"),
        todo: row.get("todo_count"),
        conflicts: row.get("conflicts_count"),
        done: row.get("done_count"),
    })
}
