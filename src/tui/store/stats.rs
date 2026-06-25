use anyhow::Result;
use sqlx::{Row, SqliteConnection};

use super::TuiStore;
use super::types::{DatabaseStatsPriorityCounts, DatabaseStatsStatusCounts, TuiDatabaseStats};
use crate::workspaces::Workspace;

impl TuiStore {
    pub(crate) async fn load_database_stats(&mut self) -> Result<()> {
        let mut conn = self.pool.acquire().await?;
        self.db_stats = load_database_stats(&mut conn, &self.active_workspace).await?;
        Ok(())
    }
}

async fn load_database_stats(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
) -> Result<TuiDatabaseStats> {
    let workspace_id = workspace.id.as_str();
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
    .bind(workspace_id)
    .fetch_one(&mut *conn)
    .await?;

    let projects =
        sqlx::query_scalar("SELECT count(*) FROM projects WHERE workspace_id = ? AND deleted = 0")
            .bind(workspace_id)
            .fetch_one(&mut *conn)
            .await?;
    let labels = sqlx::query_scalar("SELECT count(*) FROM labels WHERE workspace_id = ?")
        .bind(workspace_id)
        .fetch_one(&mut *conn)
        .await?;
    let notes = sqlx::query_scalar("SELECT count(*) FROM notes WHERE workspace_id = ?")
        .bind(workspace_id)
        .fetch_one(&mut *conn)
        .await?;
    let task_labels = sqlx::query_scalar("SELECT count(*) FROM task_labels WHERE workspace_id = ?")
        .bind(workspace_id)
        .fetch_one(&mut *conn)
        .await?;
    let pending_changes: i64 =
        sqlx::query_scalar("SELECT count(*) FROM changes WHERE server_seq IS NULL")
            .fetch_one(&mut *conn)
            .await?;
    let conflicts = sqlx::query_scalar(
        "SELECT count(*) FROM conflicts WHERE workspace_id = ? AND resolved = 0",
    )
    .bind(workspace_id)
    .fetch_one(&mut *conn)
    .await?;
    let sqlite_page_size = sqlite_pragma_i64(conn, SqlitePragma::PageSize).await?;
    let sqlite_page_count = sqlite_pragma_i64(conn, SqlitePragma::PageCount).await?;
    let sqlite_freelist_count = sqlite_pragma_i64(conn, SqlitePragma::FreelistCount).await?;

    Ok(TuiDatabaseStats {
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
        pending_changes,
        conflicts,
        sqlite_page_size,
        sqlite_page_count,
        sqlite_freelist_count,
        latest_created_at: row.get("latest_created_at"),
        latest_updated_at: row.get("latest_updated_at"),
    })
}

#[derive(Debug, Clone, Copy)]
enum SqlitePragma {
    PageSize,
    PageCount,
    FreelistCount,
}

impl SqlitePragma {
    fn sql(self) -> &'static str {
        match self {
            Self::PageSize => "PRAGMA page_size",
            Self::PageCount => "PRAGMA page_count",
            Self::FreelistCount => "PRAGMA freelist_count",
        }
    }
}

async fn sqlite_pragma_i64(conn: &mut SqliteConnection, pragma: SqlitePragma) -> Result<i64> {
    Ok(sqlx::query_scalar(pragma.sql())
        .fetch_one(&mut *conn)
        .await?)
}
