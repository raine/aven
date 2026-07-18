use anyhow::Result;
use sqlx::{Row, SqliteConnection};

use crate::workspaces::Workspace;

use super::{SyncHistoryStats, sync_history_stats};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DatabaseStatsStatusCounts {
    pub inbox: i64,
    pub backlog: i64,
    pub todo: i64,
    pub active: i64,
    pub done: i64,
    pub canceled: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DatabaseStatsPriorityCounts {
    pub none: i64,
    pub low: i64,
    pub medium: i64,
    pub high: i64,
    pub urgent: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DatabaseStats {
    pub workspace_name: String,
    pub workspace_key: String,
    pub total_tasks: i64,
    pub open_tasks: i64,
    pub deleted_tasks: i64,
    pub statuses: DatabaseStatsStatusCounts,
    pub priorities: DatabaseStatsPriorityCounts,
    pub projects: i64,
    pub labels: i64,
    pub notes: i64,
    pub task_labels: i64,
    pub sync_history: SyncHistoryStats,
    pub conflicts: i64,
    pub sqlite_page_size: i64,
    pub sqlite_page_count: i64,
    pub sqlite_freelist_count: i64,
    pub latest_created_at: Option<String>,
    pub latest_updated_at: Option<String>,
}

pub(super) async fn database_stats(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
) -> Result<DatabaseStats> {
    let row = sqlx::query(
        "SELECT
         COUNT(*) AS total_tasks,
         COALESCE(SUM(CASE WHEN deleted = 0 AND status NOT IN ('done', 'canceled') THEN 1 ELSE 0 END), 0) AS open_tasks,
         COALESCE(SUM(CASE WHEN deleted != 0 THEN 1 ELSE 0 END), 0) AS deleted_tasks,
         COALESCE(SUM(CASE WHEN deleted = 0 AND status = 'inbox' THEN 1 ELSE 0 END), 0) AS inbox_tasks,
         COALESCE(SUM(CASE WHEN deleted = 0 AND status = 'backlog' THEN 1 ELSE 0 END), 0) AS backlog_tasks,
         COALESCE(SUM(CASE WHEN deleted = 0 AND status = 'todo' THEN 1 ELSE 0 END), 0) AS todo_tasks,
         COALESCE(SUM(CASE WHEN deleted = 0 AND status = 'active' THEN 1 ELSE 0 END), 0) AS active_tasks,
         COALESCE(SUM(CASE WHEN deleted = 0 AND status = 'done' THEN 1 ELSE 0 END), 0) AS done_tasks,
         COALESCE(SUM(CASE WHEN deleted = 0 AND status = 'canceled' THEN 1 ELSE 0 END), 0) AS canceled_tasks,
         COALESCE(SUM(CASE WHEN deleted = 0 AND priority = 'none' THEN 1 ELSE 0 END), 0) AS none_priority,
         COALESCE(SUM(CASE WHEN deleted = 0 AND priority = 'low' THEN 1 ELSE 0 END), 0) AS low_priority,
         COALESCE(SUM(CASE WHEN deleted = 0 AND priority = 'medium' THEN 1 ELSE 0 END), 0) AS medium_priority,
         COALESCE(SUM(CASE WHEN deleted = 0 AND priority = 'high' THEN 1 ELSE 0 END), 0) AS high_priority,
         COALESCE(SUM(CASE WHEN deleted = 0 AND priority = 'urgent' THEN 1 ELSE 0 END), 0) AS urgent_priority,
         MAX(CASE WHEN deleted = 0 THEN created_at END) AS latest_created_at,
         MAX(CASE WHEN deleted = 0 THEN updated_at END) AS latest_updated_at
         FROM tasks
         WHERE workspace_id = ?",
    )
    .bind(&workspace.id)
    .fetch_one(&mut *conn)
    .await?;

    let projects =
        sqlx::query_scalar("SELECT count(*) FROM projects WHERE workspace_id = ? AND deleted = 0")
            .bind(&workspace.id)
            .fetch_one(&mut *conn)
            .await?;
    let labels = sqlx::query_scalar("SELECT count(*) FROM labels WHERE workspace_id = ?")
        .bind(&workspace.id)
        .fetch_one(&mut *conn)
        .await?;
    let notes = sqlx::query_scalar("SELECT count(*) FROM notes WHERE workspace_id = ?")
        .bind(&workspace.id)
        .fetch_one(&mut *conn)
        .await?;
    let task_labels = sqlx::query_scalar("SELECT count(*) FROM task_labels WHERE workspace_id = ?")
        .bind(&workspace.id)
        .fetch_one(&mut *conn)
        .await?;
    let sync_history = sync_history_stats(conn).await?;
    let conflicts = sqlx::query_scalar(
        "SELECT count(*) FROM conflicts WHERE workspace_id = ? AND resolved = 0",
    )
    .bind(&workspace.id)
    .fetch_one(&mut *conn)
    .await?;

    Ok(DatabaseStats {
        workspace_name: workspace.name.clone(),
        workspace_key: workspace.key.clone(),
        total_tasks: row.get("total_tasks"),
        open_tasks: row.get("open_tasks"),
        deleted_tasks: row.get("deleted_tasks"),
        statuses: DatabaseStatsStatusCounts {
            inbox: row.get("inbox_tasks"),
            backlog: row.get("backlog_tasks"),
            todo: row.get("todo_tasks"),
            active: row.get("active_tasks"),
            done: row.get("done_tasks"),
            canceled: row.get("canceled_tasks"),
        },
        priorities: DatabaseStatsPriorityCounts {
            none: row.get("none_priority"),
            low: row.get("low_priority"),
            medium: row.get("medium_priority"),
            high: row.get("high_priority"),
            urgent: row.get("urgent_priority"),
        },
        projects,
        labels,
        notes,
        task_labels,
        sync_history,
        conflicts,
        sqlite_page_size: sqlite_pragma_i64(conn, "PRAGMA page_size").await?,
        sqlite_page_count: sqlite_pragma_i64(conn, "PRAGMA page_count").await?,
        sqlite_freelist_count: sqlite_pragma_i64(conn, "PRAGMA freelist_count").await?,
        latest_created_at: row.get("latest_created_at"),
        latest_updated_at: row.get("latest_updated_at"),
    })
}

async fn sqlite_pragma_i64(conn: &mut SqliteConnection, sql: &'static str) -> Result<i64> {
    Ok(sqlx::query_scalar(sql).fetch_one(&mut *conn).await?)
}
