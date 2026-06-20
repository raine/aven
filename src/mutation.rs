use anyhow::{Result, bail};
use serde_json::json;
use sqlx::SqliteConnection;
use tracing::{debug, info};

use crate::choices::{PRIORITIES, STATUSES, validate_choice};
use crate::db::{conflict_exists, field_version, insert_change, set_field_version};
use crate::ids::now;
use crate::projects::resolve_project_for_add_in_workspace;
use crate::refs::get_task;
use crate::types::Task;
use crate::workspaces::active_workspace_id;

pub(crate) async fn set_status(
    conn: &mut SqliteConnection,
    task: &Task,
    status: &str,
) -> Result<Task> {
    set_task_field(conn, &task.id, "status", status).await?;
    get_task(conn, &task.id).await
}

pub(crate) async fn set_priority(
    conn: &mut SqliteConnection,
    task: &Task,
    priority: &str,
) -> Result<Task> {
    set_task_field(conn, &task.id, "priority", priority).await?;
    get_task(conn, &task.id).await
}

pub(crate) async fn cycle_priority(
    conn: &mut SqliteConnection,
    task: &Task,
    reverse: bool,
) -> Result<Task> {
    let index = PRIORITIES
        .iter()
        .position(|priority| *priority == task.priority)
        .unwrap_or(0);
    let next = if reverse {
        (index + PRIORITIES.len() - 1) % PRIORITIES.len()
    } else {
        (index + 1) % PRIORITIES.len()
    };
    set_priority(conn, task, PRIORITIES[next]).await
}

pub(crate) async fn set_deleted(
    conn: &mut SqliteConnection,
    task: &Task,
    deleted: bool,
) -> Result<Task> {
    set_task_field(conn, &task.id, "deleted", if deleted { "1" } else { "0" }).await?;
    get_task(conn, &task.id).await
}

pub(crate) async fn set_task_field(
    conn: &mut SqliteConnection,
    task_id: &str,
    field: &str,
    value: &str,
) -> Result<()> {
    let workspace_id = active_workspace_id();
    if conflict_exists(conn, &workspace_id, task_id, field).await? {
        bail!(
            "error conflicted-field ref={} field={} hint=\"use conflict resolve\"",
            task_id,
            field
        );
    }
    debug!(task_id = %task_id, field = %field, "task field mutation started");
    let base = field_version(conn, task_id, field).await?;
    apply_field_value_in_workspace(conn, &workspace_id, task_id, field, value).await?;
    let change_id = insert_change(
        conn,
        "task",
        task_id,
        Some(field),
        "set_field",
        json!({
            "workspace_id": workspace_id,
            "workspace_key": crate::workspaces::active_workspace().key,
            "value": value,
        }),
        base.as_deref(),
    )
    .await?;
    set_field_version(conn, task_id, field, &change_id).await?;
    info!(
        task_id = %task_id,
        field = %field,
        change_id = %change_id,
        "task field mutated"
    );
    Ok(())
}

#[allow(dead_code)]
pub(crate) async fn apply_field_value(
    conn: &mut SqliteConnection,
    task_id: &str,
    field: &str,
    value: &str,
) -> Result<()> {
    apply_field_value_in_workspace(conn, active_workspace_id().as_str(), task_id, field, value).await
}

pub(crate) async fn apply_field_value_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    task_id: &str,
    field: &str,
    value: &str,
) -> Result<()> {
    let ts = now();
    let deleted_value = value.parse::<i64>().unwrap_or(0);
    match field {
        "title" => sqlx::query(
            "UPDATE tasks SET title = ?, updated_at = ? WHERE workspace_id = ? AND id = ?",
        )
        .bind(value)
        .bind(&ts)
        .bind(workspace_id)
        .bind(task_id)
        .execute(&mut *conn)
        .await?
        .rows_affected(),
        "description" => sqlx::query(
            "UPDATE tasks SET description = ?, updated_at = ? WHERE workspace_id = ? AND id = ?",
        )
        .bind(value)
        .bind(&ts)
        .bind(workspace_id)
        .bind(task_id)
        .execute(&mut *conn)
        .await?
        .rows_affected(),
        "project" => {
            let project = resolve_project_for_add_in_workspace(conn, workspace_id, Some(value)).await?;
            sqlx::query(
                "UPDATE tasks SET project_key = ?, updated_at = ? WHERE workspace_id = ? AND id = ?",
            )
            .bind(project.key)
            .bind(&ts)
            .bind(workspace_id)
            .bind(task_id)
            .execute(&mut *conn)
            .await?
            .rows_affected()
        }
        "status" => {
            validate_choice("status", value, STATUSES)?;
            sqlx::query(
                "UPDATE tasks SET status = ?, updated_at = ? WHERE workspace_id = ? AND id = ?",
            )
            .bind(value)
            .bind(&ts)
            .bind(workspace_id)
            .bind(task_id)
            .execute(&mut *conn)
            .await?
            .rows_affected()
        }
        "priority" => {
            validate_choice("priority", value, PRIORITIES)?;
            sqlx::query(
                "UPDATE tasks SET priority = ?, updated_at = ? WHERE workspace_id = ? AND id = ?",
            )
            .bind(value)
            .bind(&ts)
            .bind(workspace_id)
            .bind(task_id)
            .execute(&mut *conn)
            .await?
            .rows_affected()
        }
        "deleted" => sqlx::query(
            "UPDATE tasks SET deleted = ?, updated_at = ? WHERE workspace_id = ? AND id = ?",
        )
        .bind(deleted_value)
        .bind(&ts)
        .bind(workspace_id)
        .bind(task_id)
        .execute(&mut *conn)
        .await?
        .rows_affected(),
        _ => bail!("error unknown-field field={field}"),
    };
    Ok(())
}
