use anyhow::{Result, bail};
use serde_json::json;
use sqlx::SqliteConnection;

use crate::choices::{PRIORITIES, STATUSES, validate_choice};
use crate::db::{conflict_exists, field_version, insert_change, set_field_version};
use crate::ids::now;
use crate::{Task, get_task, resolve_project_for_add};

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

pub(crate) fn status_for_key(ch: char) -> Option<&'static str> {
    match ch {
        '1' => Some(STATUSES[0]),
        '2' => Some(STATUSES[1]),
        '3' => Some(STATUSES[2]),
        '4' => Some(STATUSES[3]),
        '5' => Some(STATUSES[4]),
        '6' => Some(STATUSES[5]),
        _ => None,
    }
}

pub(crate) async fn set_task_field(
    conn: &mut SqliteConnection,
    task_id: &str,
    field: &str,
    value: &str,
) -> Result<()> {
    if conflict_exists(conn, task_id, field).await? {
        bail!(
            "error conflicted-field ref={} field={} hint=\"use conflict resolve\"",
            task_id,
            field
        );
    }
    let base = field_version(conn, task_id, field).await?;
    apply_field_value(conn, task_id, field, value).await?;
    let change_id = insert_change(
        conn,
        "task",
        task_id,
        Some(field),
        "set_field",
        json!({ "value": value }),
        base.as_deref(),
    )
    .await?;
    set_field_version(conn, task_id, field, &change_id).await?;
    Ok(())
}

pub(crate) async fn apply_field_value(
    conn: &mut SqliteConnection,
    task_id: &str,
    field: &str,
    value: &str,
) -> Result<()> {
    let ts = now();
    let deleted_value = value.parse::<i64>().unwrap_or(0);
    match field {
        "title" => sqlx::query!(
            "UPDATE tasks SET title = ?, updated_at = ? WHERE id = ?",
            value,
            ts,
            task_id,
        )
        .execute(&mut *conn)
        .await?
        .rows_affected(),
        "description" => sqlx::query!(
            "UPDATE tasks SET description = ?, updated_at = ? WHERE id = ?",
            value,
            ts,
            task_id,
        )
        .execute(&mut *conn)
        .await?
        .rows_affected(),
        "project" => {
            let project = resolve_project_for_add(conn, Some(value)).await?;
            sqlx::query!(
                "UPDATE tasks SET project_key = ?, updated_at = ? WHERE id = ?",
                project.key,
                ts,
                task_id,
            )
            .execute(&mut *conn)
            .await?
            .rows_affected()
        }
        "status" => {
            validate_choice("status", value, STATUSES)?;
            sqlx::query!(
                "UPDATE tasks SET status = ?, updated_at = ? WHERE id = ?",
                value,
                ts,
                task_id,
            )
            .execute(&mut *conn)
            .await?
            .rows_affected()
        }
        "priority" => {
            validate_choice("priority", value, PRIORITIES)?;
            sqlx::query!(
                "UPDATE tasks SET priority = ?, updated_at = ? WHERE id = ?",
                value,
                ts,
                task_id,
            )
            .execute(&mut *conn)
            .await?
            .rows_affected()
        }
        "deleted" => sqlx::query!(
            "UPDATE tasks SET deleted = ?, updated_at = ? WHERE id = ?",
            deleted_value,
            ts,
            task_id,
        )
        .execute(&mut *conn)
        .await?
        .rows_affected(),
        _ => bail!("error unknown-field field={field}"),
    };
    Ok(())
}
