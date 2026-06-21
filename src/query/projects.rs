use anyhow::Result;
use sqlx::{Row, SqliteConnection};

use crate::workspaces::active_workspace_id;

use super::ProjectListItem;

#[allow(dead_code)]
pub(crate) async fn list_project_items(
    conn: &mut SqliteConnection,
) -> Result<Vec<ProjectListItem>> {
    let workspace_id = active_workspace_id();
    list_project_items_in_workspace(conn, &workspace_id).await
}

pub(crate) async fn list_project_items_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &str,
) -> Result<Vec<ProjectListItem>> {
    let rows = sqlx::query(
        "SELECT p.key, p.name, p.prefix,
         COALESCE(SUM(CASE WHEN t.deleted = 0 AND t.status NOT IN ('done', 'canceled') THEN 1 ELSE 0 END), 0) AS open_count,
         COALESCE(SUM(CASE WHEN t.deleted = 0 AND t.status = 'inbox' THEN 1 ELSE 0 END), 0) AS inbox_count
         FROM projects p
         LEFT JOIN tasks t ON t.workspace_id = p.workspace_id AND t.project_key = p.key
         WHERE p.workspace_id = ? AND p.deleted = 0
         GROUP BY p.key, p.name, p.prefix
         ORDER BY p.key",
    )
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
