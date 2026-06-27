use anyhow::{Context, Result};
use serde_json::Value;
use sqlx::SqliteConnection;

use crate::db::{field_version, set_field_version};
use crate::mutation::{apply_field_value_in_workspace, apply_project_id_in_workspace};
use crate::sync::wire::ChangeWire;
use crate::task_fields::TaskField;

use super::conflict;
use super::label::create_or_update_task_label;
use super::project::ensure_project_for_payload;
use super::shared::{str_payload, workspace_id_payload};

pub(super) async fn create_task(conn: &mut SqliteConnection, change: &ChangeWire) -> Result<()> {
    let workspace_id = workspace_id_payload(conn, change).await?;
    if sqlx::query_scalar::<_, i64>("SELECT count(*) FROM tasks WHERE workspace_id = ? AND id = ?")
        .bind(&workspace_id)
        .bind(&change.entity_id)
        .fetch_one(&mut *conn)
        .await?
        > 0
    {
        return Ok(());
    }
    let project_id = str_payload(&change.payload, "project_id")?;
    let project_id = ensure_project_for_payload(conn, &workspace_id, &project_id, change).await?;
    let title = str_payload(&change.payload, "title")?;
    let description = str_payload(&change.payload, "description").unwrap_or_default();
    let status = str_payload(&change.payload, "status").unwrap_or_else(|_| "inbox".to_string());
    let priority = str_payload(&change.payload, "priority").unwrap_or_else(|_| "none".to_string());
    let created_at =
        str_payload(&change.payload, "created_at").unwrap_or_else(|_| change.created_at.clone());
    sqlx::query(
        "INSERT INTO tasks(workspace_id, id, title, description, project_id, status, priority, created_at, updated_at, queue_activity_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&workspace_id)
    .bind(&change.entity_id)
    .bind(&title)
    .bind(&description)
    .bind(&project_id)
    .bind(&status)
    .bind(&priority)
    .bind(&created_at)
    .bind(&change.created_at)
    .bind(&change.created_at)
    .execute(&mut *conn)
    .await?;
    if let Some(labels) = change.payload.get("labels").and_then(Value::as_array) {
        for label in labels.iter().filter_map(Value::as_str) {
            create_or_update_task_label(
                conn,
                &workspace_id,
                &change.entity_id,
                label,
                &change.created_at,
            )
            .await?;
        }
    }
    for field in TaskField::VERSIONED {
        set_field_version(conn, &change.entity_id, field.as_str(), &change.change_id).await?;
    }
    Ok(())
}

pub(crate) async fn set_field(
    conn: &mut SqliteConnection,
    change: &ChangeWire,
    force: bool,
) -> Result<()> {
    let field = change
        .field
        .as_deref()
        .context("field change missing field")?;
    let value = str_payload(&change.payload, "value")?;
    let workspace_id = workspace_id_payload(conn, change).await?;
    let value = if field == TaskField::Project.as_str() {
        let project_id = str_payload(&change.payload, "project_id")?;
        ensure_project_for_payload(conn, &workspace_id, &project_id, change).await?
    } else {
        value
    };
    if !force {
        let current = field_version(conn, &change.entity_id, field).await?;
        if current != change.base_version {
            conflict::create_conflict(conn, change, field, &value, current.as_deref()).await?;
            return Ok(());
        }
    }
    if field == TaskField::Project.as_str() {
        apply_project_id_in_workspace(conn, &workspace_id, &change.entity_id, &value).await?;
    } else {
        apply_field_value_in_workspace(conn, &workspace_id, &change.entity_id, field, &value)
            .await?;
    }
    set_field_version(conn, &change.entity_id, field, &change.change_id).await?;
    if force {
        sqlx::query(
            "UPDATE conflicts SET resolved = 1 WHERE workspace_id = ? AND task_id = ? AND field = ? AND resolved = 0",
        )
        .bind(&workspace_id)
        .bind(&change.entity_id)
        .bind(field)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}
