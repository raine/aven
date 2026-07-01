use anyhow::{Context, Result, ensure};
use serde_json::Value;
use sqlx::SqliteConnection;

use crate::choices::{TaskPriority, TaskStatus};
use crate::db::{field_version, set_field_version};
use crate::mutation::{apply_field_value_in_workspace, apply_project_id_in_workspace};
use crate::sync::wire::ChangeWire;
use crate::task_fields::TaskField;

use super::conflict;
use super::label::create_or_update_task_label;
use super::payload::CreateTaskPayload;
use super::project::ensure_project_for_payload;
use super::shared::{str_payload, task_field_workspace_id_payload, workspace_id_payload};

pub(super) async fn create_task(conn: &mut SqliteConnection, change: &ChangeWire) -> Result<()> {
    let p = CreateTaskPayload::from_change(change)?;
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
    let project_id = ensure_project_for_payload(conn, &workspace_id, &p.project_id, change).await?;
    let title = p.title;
    let description = p.description.unwrap_or_default();
    let status = match p.status {
        Some(ref value) => TaskStatus::parse(value)?,
        None => TaskStatus::Inbox,
    };
    let priority = match p.priority {
        Some(ref value) => TaskPriority::parse(value)?,
        None => TaskPriority::None,
    };
    let is_epic = match p.is_epic.as_deref() {
        Some("1") | Some("true") => 1,
        _ => 0,
    };
    let created_at = p.created_at.unwrap_or_else(|| change.created_at.clone());
    sqlx::query(
        "INSERT INTO tasks(workspace_id, id, title, description, project_id, status, priority, created_at, updated_at, queue_activity_at, is_epic)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&workspace_id)
    .bind(&change.entity_id)
    .bind(&title)
    .bind(&description)
    .bind(&project_id)
    .bind(status.as_str())
    .bind(priority.as_str())
    .bind(&created_at)
    .bind(&change.created_at)
    .bind(&change.created_at)
    .bind(is_epic)
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
    let task_field = TaskField::parse_or_unknown(field)?;
    let field = task_field.as_str();
    let mut value = str_payload(&change.payload, "value")?;
    let workspace_id = task_field_workspace_id_payload(conn, change).await?;
    if task_field.is_project() {
        let project_id = str_payload(&change.payload, "project_id")?;
        ensure!(
            value == project_id,
            "error invalid-sync-change project-value-mismatch"
        );
        value = ensure_project_for_payload(conn, &workspace_id, &project_id, change).await?;
    }
    if !force {
        let current = field_version(conn, &change.entity_id, field).await?;
        if task_field == TaskField::IsEpic
            && value == "0"
            && crate::operations::task_has_epic_children(conn, &workspace_id, &change.entity_id)
                .await?
        {
            conflict::create_conflict(
                conn,
                change,
                &workspace_id,
                field,
                &value,
                current.as_deref(),
            )
            .await?;
            return Ok(());
        }
        if current != change.base_version {
            conflict::create_conflict(
                conn,
                change,
                &workspace_id,
                field,
                &value,
                current.as_deref(),
            )
            .await?;
            return Ok(());
        }
    }
    if force
        && task_field == TaskField::IsEpic
        && value == "0"
        && crate::operations::task_has_epic_children(conn, &workspace_id, &change.entity_id).await?
    {
        anyhow::bail!("error epic-has-children task_id={}", change.entity_id);
    }
    if task_field.is_project() {
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
