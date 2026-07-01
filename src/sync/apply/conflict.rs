use anyhow::{Result, anyhow};
use sqlx::SqliteConnection;
use tracing::info;

use crate::db::conflict_exists;
use crate::sync::wire::ChangeWire;
use crate::task_fields::TaskField;

pub(super) async fn create_conflict(
    conn: &mut SqliteConnection,
    change: &ChangeWire,
    workspace_id: &str,
    field: &str,
    remote_value: &str,
    local_change_id: Option<&str>,
) -> Result<()> {
    if conflict_exists(conn, workspace_id, &change.entity_id, field).await? {
        return Ok(());
    }
    let local_value = current_field_value(conn, workspace_id, &change.entity_id, field).await?;
    let variant_a = format!(
        "v{}",
        local_change_id
            .unwrap_or("local")
            .chars()
            .take(6)
            .collect::<String>()
    );
    let variant_b = format!("v{}", change.change_id.chars().take(6).collect::<String>());
    sqlx::query(
        "INSERT OR IGNORE INTO conflicts(workspace_id, task_id, field, base_version, local_value, remote_value,
         local_change_id, remote_change_id, variant_a, variant_b, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(workspace_id)
    .bind(&change.entity_id)
    .bind(field)
    .bind(&change.base_version)
    .bind(&local_value)
    .bind(remote_value)
    .bind(local_change_id)
    .bind(&change.change_id)
    .bind(&variant_a)
    .bind(&variant_b)
    .bind(&change.created_at)
    .execute(&mut *conn)
    .await?;
    info!(
        task_id = crate::sync::apply::shared::safe_entity_id(change),
        field = %field,
        "remote change conflict created"
    );
    Ok(())
}

async fn current_field_value(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    task_id: &str,
    field: &str,
) -> Result<String> {
    let task_field = TaskField::parse_or_unknown(field)?;
    let row = sqlx::query(
        "SELECT t.id, t.workspace_id, t.title, t.description, t.project_id, p.key AS project_key,
         p.prefix AS project_prefix, t.status, t.priority, t.created_at, t.updated_at, t.queue_activity_at,
         t.deleted, t.is_epic
         FROM tasks t JOIN projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id
         WHERE t.workspace_id = ? AND t.id = ?",
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| anyhow!("error task-not-found task_id={task_id}"))?;
    let task = crate::db::task_from_row(&row)?;
    Ok(task_field.current_value(&task))
}
