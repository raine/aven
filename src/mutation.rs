use anyhow::{Result, bail, ensure};
use sqlx::SqliteConnection;
use tracing::{debug, info};

use crate::change_log::op_type;
use crate::choices::TaskPriority;
use crate::db::{conflict_exists, field_version, insert_change, set_field_version, task_from_row};
use crate::ids::now;
use crate::projects::resolve_project_for_add_in_workspace;
use crate::refs::get_task;
use crate::task_fields::TaskField;
use crate::types::{Project, Task};
use crate::workspaces::{active_workspace, active_workspace_id};

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
    let index = TaskPriority::ALL
        .iter()
        .position(|priority| *priority == task.priority)
        .unwrap_or(0);
    let next = if reverse {
        (index + TaskPriority::ALL.len() - 1) % TaskPriority::ALL.len()
    } else {
        (index + 1) % TaskPriority::ALL.len()
    };
    set_priority(conn, task, TaskPriority::ALL[next].as_str()).await
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
) -> Result<bool> {
    let task_field = TaskField::parse_or_unknown(field)?;
    let workspace = active_workspace();
    if task_field.is_project() {
        let project =
            resolve_project_for_add_in_workspace(conn, &workspace.id, Some(value)).await?;
        set_task_project(conn, task_id, &project).await
    } else {
        set_task_scalar_field(conn, task_id, task_field, value).await
    }
}

pub(crate) async fn set_task_project(
    conn: &mut SqliteConnection,
    task_id: &str,
    project: &Project,
) -> Result<bool> {
    let workspace = active_workspace();
    let field = TaskField::Project.as_str();
    let current = current_task(conn, &workspace.id, task_id).await?;
    if current.project_id == project.id {
        return Ok(false);
    }
    if conflict_exists(conn, &workspace.id, task_id, field).await? {
        bail!(
            "error conflicted-field ref={} field={} hint=\"use conflict resolve\"",
            task_id,
            field
        );
    }
    debug!(task_id = %task_id, field = %field, "task field mutation started");
    let base = field_version(conn, task_id, field).await?;
    apply_project_id_in_workspace(conn, &workspace.id, task_id, &project.id).await?;
    let payload = TaskField::project_payload(&workspace.id, &workspace.key, project);
    finish_task_field_change(conn, task_id, field, payload, base.as_deref()).await?;
    Ok(true)
}

async fn set_task_scalar_field(
    conn: &mut SqliteConnection,
    task_id: &str,
    task_field: TaskField,
    value: &str,
) -> Result<bool> {
    task_field.validate_value(value)?;

    let workspace = active_workspace();
    let field = task_field.as_str();
    let current = current_task(conn, &workspace.id, task_id).await?;
    if task_field.current_value(&current) == value {
        return Ok(false);
    }
    if conflict_exists(conn, &workspace.id, task_id, field).await? {
        bail!(
            "error conflicted-field ref={} field={} hint=\"use conflict resolve\"",
            task_id,
            field
        );
    }
    debug!(task_id = %task_id, field = %field, "task field mutation started");
    let base = field_version(conn, task_id, field).await?;
    apply_scalar_field_value_in_workspace(conn, &workspace.id, task_id, task_field, value).await?;
    let payload = task_field.scalar_payload(&workspace.id, &workspace.key, value)?;
    finish_task_field_change(conn, task_id, field, payload, base.as_deref()).await?;
    Ok(true)
}

async fn finish_task_field_change(
    conn: &mut SqliteConnection,
    task_id: &str,
    field: &str,
    payload: serde_json::Value,
    base: Option<&str>,
) -> Result<()> {
    let change_id = insert_change(
        conn,
        "task",
        task_id,
        Some(field),
        op_type::SET_FIELD,
        payload,
        base,
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

async fn current_task(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    task_id: &str,
) -> Result<Task> {
    let row = sqlx::query(
        "SELECT t.id, t.workspace_id, t.title, t.description, t.project_id,
                p.key AS project_key, p.prefix AS project_prefix, t.status,
                t.priority, t.created_at, t.updated_at, t.queue_activity_at,
                t.deleted, t.is_epic
         FROM tasks t
         JOIN projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id
         WHERE t.workspace_id = ? AND t.id = ?",
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| {
        anyhow::anyhow!(
            "error task-not-found task_id={} workspace_id={}",
            task_id,
            workspace_id
        )
    })?;
    task_from_row(&row)
}

#[allow(dead_code)]
pub(crate) async fn apply_field_value(
    conn: &mut SqliteConnection,
    task_id: &str,
    field: &str,
    value: &str,
) -> Result<()> {
    apply_field_value_in_workspace(conn, active_workspace_id().as_str(), task_id, field, value)
        .await
}

pub(crate) async fn apply_project_id_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    task_id: &str,
    project_id: &str,
) -> Result<()> {
    let project_exists = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM projects WHERE workspace_id = ? AND id = ? AND deleted = 0",
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_one(&mut *conn)
    .await?
        > 0;
    if !project_exists {
        bail!("error unknown-project-id id={project_id}");
    }
    let ts = now();
    let rows_affected = sqlx::query(
        "UPDATE tasks SET project_id = ?, updated_at = ? WHERE workspace_id = ? AND id = ?",
    )
    .bind(project_id)
    .bind(&ts)
    .bind(workspace_id)
    .bind(task_id)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    ensure!(
        rows_affected == 1,
        "error task-not-found task_id={} workspace_id={}",
        task_id,
        workspace_id
    );
    Ok(())
}

pub(crate) async fn apply_field_value_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    task_id: &str,
    field: &str,
    value: &str,
) -> Result<()> {
    let task_field = TaskField::parse_or_unknown(field)?;
    apply_scalar_field_value_in_workspace(conn, workspace_id, task_id, task_field, value).await
}

async fn apply_scalar_field_value_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    task_id: &str,
    task_field: TaskField,
    value: &str,
) -> Result<()> {
    task_field.validate_value(value)?;

    let ts = now();
    let activity_at = if task_field.updates_queue_activity() {
        ts.as_str()
    } else {
        ""
    };
    let deleted_value = i64::from(value == "1");
    let epic_value = i64::from(value == "1");
    let rows_affected = match task_field {
        TaskField::Title => sqlx::query(
            "UPDATE tasks SET title = ?, updated_at = ?, queue_activity_at = COALESCE(NULLIF(?, ''), queue_activity_at) WHERE workspace_id = ? AND id = ?",
        )
        .bind(value)
        .bind(&ts)
        .bind(activity_at)
        .bind(workspace_id)
        .bind(task_id)
        .execute(&mut *conn)
        .await?
        .rows_affected(),
        TaskField::Description => sqlx::query(
            "UPDATE tasks SET description = ?, updated_at = ?, queue_activity_at = COALESCE(NULLIF(?, ''), queue_activity_at) WHERE workspace_id = ? AND id = ?",
        )
        .bind(value)
        .bind(&ts)
        .bind(activity_at)
        .bind(workspace_id)
        .bind(task_id)
        .execute(&mut *conn)
        .await?
        .rows_affected(),
        TaskField::Project => bail!("error project-update-requires-project-id"),
        TaskField::Status => sqlx::query(
            "UPDATE tasks SET status = ?, updated_at = ?, queue_activity_at = COALESCE(NULLIF(?, ''), queue_activity_at) WHERE workspace_id = ? AND id = ?",
        )
        .bind(value)
        .bind(&ts)
        .bind(activity_at)
        .bind(workspace_id)
        .bind(task_id)
        .execute(&mut *conn)
        .await?
        .rows_affected(),
        TaskField::Priority => sqlx::query(
            "UPDATE tasks SET priority = ?, updated_at = ?, queue_activity_at = COALESCE(NULLIF(?, ''), queue_activity_at) WHERE workspace_id = ? AND id = ?",
        )
        .bind(value)
        .bind(&ts)
        .bind(activity_at)
        .bind(workspace_id)
        .bind(task_id)
        .execute(&mut *conn)
        .await?
        .rows_affected(),
        TaskField::Deleted => sqlx::query(
            "UPDATE tasks SET deleted = ?, updated_at = ?, queue_activity_at = COALESCE(NULLIF(?, ''), queue_activity_at) WHERE workspace_id = ? AND id = ?",
        )
        .bind(deleted_value)
        .bind(&ts)
        .bind(activity_at)
        .bind(workspace_id)
        .bind(task_id)
        .execute(&mut *conn)
        .await?
        .rows_affected(),
        TaskField::IsEpic => sqlx::query(
            "UPDATE tasks SET is_epic = ?, updated_at = ? WHERE workspace_id = ? AND id = ?",
        )
        .bind(epic_value)
        .bind(&ts)
        .bind(workspace_id)
        .bind(task_id)
        .execute(&mut *conn)
        .await?
        .rows_affected(),
    };
    ensure!(
        rows_affected == 1,
        "error task-not-found task_id={} workspace_id={}",
        task_id,
        workspace_id
    );
    Ok(())
}
