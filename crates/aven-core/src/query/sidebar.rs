use crate::ids::WorkspaceId;
use anyhow::Result;
use chrono::Local;
use sqlx::{Row, SqliteConnection};

use crate::projects::resolve_existing_project_in_workspace;

use super::SidebarCounts;
use super::fragments;

fn sidebar_task_count_columns() -> String {
    let now = "?1";
    let today = "?2";
    let available = fragments::available_task_clause("t", now);
    let open = fragments::open_task_clause("t");
    let ready = format!(
        "{open} AND {available} AND t.is_epic = 0 AND {}",
        fragments::ready_dependency_clause("t")
    );
    let blocked = format!(
        "{open} AND {available} AND {}",
        fragments::unresolved_blocker_clause("t")
    );
    let overdue = format!(
        "{}{} AND {available}",
        fragments::overdue_task_prefix("t"),
        today
    );
    format!(
        "\
COALESCE(SUM(CASE WHEN {open} AND {available} THEN 1 ELSE 0 END), 0) AS open_count,
COALESCE(SUM(CASE WHEN t.deleted = 0 AND t.status = 'inbox' AND {available} THEN 1 ELSE 0 END), 0) AS inbox_count,
COALESCE(SUM(CASE WHEN t.deleted = 0 AND t.status = 'active' AND {available} THEN 1 ELSE 0 END), 0) AS active_count,
COALESCE(SUM(CASE WHEN t.deleted = 0 AND t.status = 'backlog' AND {available} THEN 1 ELSE 0 END), 0) AS backlog_count,
COALESCE(SUM(CASE WHEN t.deleted = 0 AND t.status = 'todo' AND {available} THEN 1 ELSE 0 END), 0) AS todo_count,
COALESCE(SUM(CASE WHEN {ready} THEN 1 ELSE 0 END), 0) AS ready_count,
COALESCE(SUM(CASE WHEN {blocked} THEN 1 ELSE 0 END), 0) AS blocked_count,
COALESCE(SUM(CASE WHEN {overdue} THEN 1 ELSE 0 END), 0) AS overdue_count,
COALESCE(SUM(CASE WHEN {} AND NOT EXISTS (
    SELECT 1 FROM recurrence_occurrences done_occurrence
    WHERE done_occurrence.workspace_id = t.workspace_id AND done_occurrence.task_id = t.id
) THEN 1 ELSE 0 END), 0) AS done_count,
COALESCE(SUM(CASE WHEN t.deleted = 0 AND t.status NOT IN ('done', 'canceled') AND t.available_at != '' AND t.available_at > {now} THEN 1 ELSE 0 END), 0) AS upcoming_count",
        fragments::terminal_status_clause("t"),
    )
}

fn sidebar_counts_sql(project_scoped: bool) -> String {
    // Clock values occupy ?1 and ?2 so every work count shares one time and date pair.
    let (
        conflict_workspace,
        conflict_project,
        epic_workspace,
        epic_project,
        task_workspace,
        task_project,
    ) = if project_scoped {
        (
            "?3",
            " AND COALESCE(ct.project_id, cs.project_id) = ?4",
            "?5",
            " AND ep.project_id = ?6",
            "?7",
            " AND t.project_id = ?8",
        )
    } else {
        ("?3", "", "?4", "", "?5", "")
    };
    format!(
        "SELECT {},
         (SELECT COUNT(*) FROM (
              SELECT DISTINCT c.entity_type, c.entity_id
              FROM conflicts c
              LEFT JOIN tasks ct ON c.entity_type = 'task'
                AND ct.workspace_id = c.workspace_id AND ct.id = c.entity_id
              LEFT JOIN recurrence_series cs ON c.entity_type = 'recurrence_series'
                AND cs.workspace_id = c.workspace_id AND cs.id = c.entity_id
              WHERE c.workspace_id = {conflict_workspace} AND c.resolved = 0
                AND COALESCE(ct.deleted, cs.deleted, 1) = 0{conflict_project}
          )) AS conflicts_count,
         (SELECT COUNT(*)
          FROM tasks ep
          WHERE ep.workspace_id = {epic_workspace}{epic_project}
            AND ep.deleted = 0 AND ep.status NOT IN ('done', 'canceled') AND ep.is_epic = 1
            AND {}
            AND (ep.available_at = '' OR ep.available_at <= strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))) AS epics_count
         FROM tasks t
         WHERE t.workspace_id = {task_workspace}{task_project} AND {}",
        sidebar_task_count_columns(),
        fragments::ordinary_task_clause("ep"),
        fragments::ordinary_task_clause("t"),
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
    let mut q = sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(crate::ids::now())
        .bind(Local::now().date_naive().format("%Y-%m-%d").to_string())
        .bind(workspace_id);
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
    let recurring_done = if let Some(project_id) = project_id.as_ref() {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(DISTINCT o.series_id)
             FROM recurrence_occurrences o
             JOIN recurrence_series s
               ON s.workspace_id = o.workspace_id AND s.id = o.series_id
             WHERE o.workspace_id = ? AND s.project_id = ? AND s.deleted = 0
               AND o.outcome IN ('completed', 'skipped')",
        )
        .bind(workspace_id)
        .bind(project_id)
        .fetch_one(&mut *conn)
        .await?
    } else {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(DISTINCT o.series_id)
             FROM recurrence_occurrences o
             JOIN recurrence_series s
               ON s.workspace_id = o.workspace_id AND s.id = o.series_id
             WHERE o.workspace_id = ? AND s.deleted = 0
               AND o.outcome IN ('completed', 'skipped')",
        )
        .bind(workspace_id)
        .fetch_one(&mut *conn)
        .await?
    };
    let recurring = if let Some(project_id) = project_id.as_ref() {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM recurrence_series
             WHERE workspace_id = ? AND project_id = ? AND deleted = 0
               AND state IN ('active', 'paused')",
        )
        .bind(workspace_id)
        .bind(project_id)
        .fetch_one(&mut *conn)
        .await?
    } else {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM recurrence_series
             WHERE workspace_id = ? AND deleted = 0 AND state IN ('active', 'paused')",
        )
        .bind(workspace_id)
        .fetch_one(&mut *conn)
        .await?
    };
    Ok(SidebarCounts {
        open: row.get("open_count"),
        inbox: row.get("inbox_count"),
        active: row.get("active_count"),
        backlog: row.get("backlog_count"),
        todo: row.get("todo_count"),
        ready: row.get("ready_count"),
        blocked: row.get("blocked_count"),
        overdue: row.get("overdue_count"),
        conflicts: row.get("conflicts_count"),
        done: row.get::<i64, _>("done_count") + recurring_done,
        epics: row.get("epics_count"),
        recurring,
        upcoming: row.get("upcoming_count"),
    })
}
