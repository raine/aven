use crate::ids::WorkspaceId;
use anyhow::Result;
use sqlx::{Row, SqliteConnection};

use super::ProjectListItem;
use super::fragments;

pub async fn list_project_items_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
) -> Result<Vec<ProjectListItem>> {
    let sql = format!(
        "SELECT p.key, p.name, p.prefix,
         COALESCE(SUM(CASE WHEN {} AND (t.available_at = '' OR t.available_at <= strftime('%Y-%m-%dT%H:%M:%SZ', 'now')) THEN 1 ELSE 0 END), 0) AS open_count,
         COALESCE(SUM(CASE WHEN t.deleted = 0 AND t.status = 'inbox' AND (t.available_at = '' OR t.available_at <= strftime('%Y-%m-%dT%H:%M:%SZ', 'now')) THEN 1 ELSE 0 END), 0) AS inbox_count
         FROM projects p
         LEFT JOIN tasks t ON t.workspace_id = p.workspace_id AND t.project_id = p.id
         WHERE p.workspace_id = ? AND p.deleted = 0
         GROUP BY p.key, p.name, p.prefix
         ORDER BY p.key",
        fragments::open_task_clause("t"),
    );
    let rows = sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(workspace_id)
        .fetch_all(&mut *conn)
        .await?;
    Ok(rows
        .into_iter()
        .map(|row| ProjectListItem {
            key: row.get("key"),
            name: row.get("name"),
            prefix: row.get("prefix"),
            open_count: row.get("open_count"),
            inbox_count: row.get("inbox_count"),
        })
        .collect())
}
