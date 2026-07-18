use crate::ids::WorkspaceId;
use anyhow::Result;
use sqlx::{Row, SqliteConnection};

use crate::projects::resolve_existing_project_in_workspace;

use super::SidebarCounts;
use super::fragments;

fn sidebar_task_count_columns() -> String {
    format!(
        "\
COALESCE(SUM(CASE WHEN {} AND (t.available_at = '' OR t.available_at <= strftime('%Y-%m-%dT%H:%M:%SZ', 'now')) THEN 1 ELSE 0 END), 0) AS open_count,
COALESCE(SUM(CASE WHEN t.deleted = 0 AND t.status = 'inbox' AND (t.available_at = '' OR t.available_at <= strftime('%Y-%m-%dT%H:%M:%SZ', 'now')) THEN 1 ELSE 0 END), 0) AS inbox_count,
COALESCE(SUM(CASE WHEN t.deleted = 0 AND t.status = 'active' AND (t.available_at = '' OR t.available_at <= strftime('%Y-%m-%dT%H:%M:%SZ', 'now')) THEN 1 ELSE 0 END), 0) AS active_count,
COALESCE(SUM(CASE WHEN t.deleted = 0 AND t.status = 'backlog' AND (t.available_at = '' OR t.available_at <= strftime('%Y-%m-%dT%H:%M:%SZ', 'now')) THEN 1 ELSE 0 END), 0) AS backlog_count,
COALESCE(SUM(CASE WHEN t.deleted = 0 AND t.status = 'todo' AND (t.available_at = '' OR t.available_at <= strftime('%Y-%m-%dT%H:%M:%SZ', 'now')) THEN 1 ELSE 0 END), 0) AS todo_count,
COALESCE(SUM(CASE WHEN {} THEN 1 ELSE 0 END), 0) AS done_count,
COALESCE(SUM(CASE WHEN t.deleted = 0 AND t.status NOT IN ('done', 'canceled') AND t.available_at != '' AND t.available_at > strftime('%Y-%m-%dT%H:%M:%SZ', 'now') THEN 1 ELSE 0 END), 0) AS upcoming_count",
        fragments::open_task_clause("t"),
        fragments::terminal_status_clause("t"),
    )
}

fn sidebar_counts_sql(project_scoped: bool) -> String {
    let conflict_project = if project_scoped {
        " AND ct.project_id = ?"
    } else {
        ""
    };
    let task_project = if project_scoped {
        " AND t.project_id = ?"
    } else {
        ""
    };
    format!(
        "SELECT {},
         (SELECT COUNT(DISTINCT c.task_id)
          FROM conflicts c
          JOIN tasks ct ON ct.workspace_id = c.workspace_id AND ct.id = c.task_id
          WHERE c.workspace_id = ? AND c.resolved = 0 AND ct.deleted = 0{conflict_project}) AS conflicts_count,
         (SELECT COUNT(*)
          FROM tasks ep
          WHERE ep.workspace_id = ?{task_project}
            AND ep.deleted = 0 AND ep.status NOT IN ('done', 'canceled') AND ep.is_epic = 1
            AND (ep.available_at = '' OR ep.available_at <= strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))) AS epics_count
         FROM tasks t
         WHERE t.workspace_id = ?{task_project}",
        sidebar_task_count_columns(),
    )
}

pub async fn sidebar_counts_for_scope_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
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
    let project_scoped = project_id.is_some();
    let sql = sidebar_counts_sql(project_scoped);
    let mut q = sqlx::query(sqlx::AssertSqlSafe(sql.as_str())).bind(workspace_id);
    if let Some(ref pid) = project_id {
        q = q.bind(pid);
    }
    q = q.bind(workspace_id);
    if let Some(ref pid) = project_id {
        q = q.bind(pid);
    }
    q = q.bind(workspace_id);
    if let Some(ref pid) = project_id {
        q = q.bind(pid);
    }
    let row = q.fetch_one(&mut *conn).await?;
    Ok(SidebarCounts {
        open: row.get("open_count"),
        inbox: row.get("inbox_count"),
        active: row.get("active_count"),
        backlog: row.get("backlog_count"),
        todo: row.get("todo_count"),
        conflicts: row.get("conflicts_count"),
        done: row.get("done_count"),
        epics: row.get("epics_count"),
        upcoming: row.get("upcoming_count"),
    })
}
