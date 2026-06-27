use anyhow::{Result, anyhow};
use sqlx::SqliteConnection;
use tracing::info;

use crate::db::conflict_exists;
use crate::refs::get_task;
use crate::sync::wire::ChangeWire;
use crate::task_fields::TaskField;

use super::shared::workspace_id_payload;

pub(super) async fn create_conflict(
    conn: &mut SqliteConnection,
    change: &ChangeWire,
    field: &str,
    remote_value: &str,
    local_change_id: Option<&str>,
) -> Result<()> {
    let workspace_id = workspace_id_payload(conn, change).await?;
    if conflict_exists(conn, &workspace_id, &change.entity_id, field).await? {
        return Ok(());
    }
    let local_value = current_field_value(conn, &change.entity_id, field).await?;
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
    .bind(&workspace_id)
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
    task_id: &str,
    field: &str,
) -> Result<String> {
    let task = get_task(conn, task_id).await?;
    let task_field =
        TaskField::parse(field).ok_or_else(|| anyhow!("error unknown-field field={field}"))?;
    Ok(task_field.current_value(&task))
}
