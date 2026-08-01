use crate::ids::WorkspaceId;
use anyhow::{Context, Result};
use serde_json::Value;
use sqlx::SqliteConnection;

use crate::sync::wire::ChangeWire;

use super::shared::str_payload;
use super::shared::workspace_id_payload;

pub(super) async fn create_label(conn: &mut SqliteConnection, change: &ChangeWire) -> Result<()> {
    let workspace_id = workspace_id_payload(conn, change).await?;
    let name = str_payload(&change.payload, "name")?;
    let created_at =
        str_payload(&change.payload, "created_at").unwrap_or_else(|_| crate::ids::now());
    insert_label(conn, &workspace_id, &name, &created_at).await
}

pub(super) async fn add_label(conn: &mut SqliteConnection, change: &ChangeWire) -> Result<()> {
    let workspace_id = workspace_id_payload(conn, change).await?;
    let task_id = super::shared::task_id(change)?;
    let label = str_payload(&change.payload, "label")?;
    insert_label(conn, &workspace_id, &label, &change.created_at).await?;
    sqlx::query("INSERT OR IGNORE INTO task_labels(workspace_id, task_id, label) VALUES (?, ?, ?)")
        .bind(&workspace_id)
        .bind(&task_id)
        .bind(&label)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

pub(super) async fn set_label_name(conn: &mut SqliteConnection, change: &ChangeWire) -> Result<()> {
    let workspace_id = workspace_id_payload(conn, change).await?;
    let name = str_payload(&change.payload, "name")?;
    let new_name = str_payload(&change.payload, "new_name")?;
    let created_at: Option<String> =
        sqlx::query_scalar("SELECT created_at FROM labels WHERE workspace_id = ? AND name = ?")
            .bind(&workspace_id)
            .bind(&name)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(created_at) = created_at else {
        return Ok(());
    };
    insert_label(conn, &workspace_id, &new_name, &created_at).await?;
    sqlx::query(
        "INSERT OR IGNORE INTO task_labels(workspace_id, task_id, label)
         SELECT workspace_id, task_id, ? FROM task_labels WHERE workspace_id = ? AND label = ?",
    )
    .bind(&new_name)
    .bind(&workspace_id)
    .bind(&name)
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "INSERT OR IGNORE INTO recurrence_series_labels(workspace_id, series_id, label)
         SELECT workspace_id, series_id, ? FROM recurrence_series_labels WHERE workspace_id = ? AND label = ?",
    )
    .bind(&new_name)
    .bind(&workspace_id)
    .bind(&name)
    .execute(&mut *conn)
    .await?;
    delete_label_rows(conn, &workspace_id, &name).await
}

pub(super) async fn restore_label(conn: &mut SqliteConnection, change: &ChangeWire) -> Result<()> {
    let workspace_id = workspace_id_payload(conn, change).await?;
    let name = str_payload(&change.payload, "name")?;
    let created_at = str_payload(&change.payload, "created_at")?;
    insert_label(conn, &workspace_id, &name, &created_at).await?;
    for task_id in string_array_payload(&change.payload, "task_ids")? {
        sqlx::query(
            "INSERT OR IGNORE INTO task_labels(workspace_id, task_id, label)
             SELECT ?, id, ? FROM tasks WHERE workspace_id = ? AND id = ?",
        )
        .bind(&workspace_id)
        .bind(&name)
        .bind(&workspace_id)
        .bind(task_id)
        .execute(&mut *conn)
        .await?;
    }
    for series_id in string_array_payload(&change.payload, "series_ids")? {
        sqlx::query(
            "INSERT OR IGNORE INTO recurrence_series_labels(workspace_id, series_id, label)
             SELECT ?, id, ? FROM recurrence_series WHERE workspace_id = ? AND id = ?",
        )
        .bind(&workspace_id)
        .bind(&name)
        .bind(&workspace_id)
        .bind(series_id)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

pub(super) async fn delete_label(conn: &mut SqliteConnection, change: &ChangeWire) -> Result<()> {
    let workspace_id = workspace_id_payload(conn, change).await?;
    let name = str_payload(&change.payload, "name")?;
    delete_label_rows(conn, &workspace_id, &name).await
}

async fn delete_label_rows(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    name: &str,
) -> Result<()> {
    sqlx::query("DELETE FROM task_labels WHERE workspace_id = ? AND label = ?")
        .bind(workspace_id)
        .bind(name)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM recurrence_series_labels WHERE workspace_id = ? AND label = ?")
        .bind(workspace_id)
        .bind(name)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM labels WHERE workspace_id = ? AND name = ?")
        .bind(workspace_id)
        .bind(name)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

pub(super) async fn remove_label(conn: &mut SqliteConnection, change: &ChangeWire) -> Result<()> {
    let workspace_id = workspace_id_payload(conn, change).await?;
    let task_id = super::shared::task_id(change)?;
    let label = str_payload(&change.payload, "label")?;
    sqlx::query("DELETE FROM task_labels WHERE workspace_id = ? AND task_id = ? AND label = ?")
        .bind(&workspace_id)
        .bind(&task_id)
        .bind(&label)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

pub(super) async fn create_or_update_task_label(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: &crate::ids::TaskId,
    label: &str,
    created_at: &str,
) -> Result<()> {
    insert_label(conn, workspace_id, label, created_at).await?;
    sqlx::query("INSERT OR IGNORE INTO task_labels(workspace_id, task_id, label) VALUES (?, ?, ?)")
        .bind(workspace_id)
        .bind(task_id)
        .bind(label)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

fn string_array_payload(payload: &Value, key: &str) -> Result<Vec<String>> {
    payload
        .get(key)
        .and_then(Value::as_array)
        .context("payload missing label references")?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .context("invalid label reference")
        })
        .collect()
}

async fn insert_label(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    name: &str,
    created_at: &str,
) -> Result<()> {
    sqlx::query("INSERT OR IGNORE INTO labels(workspace_id, name, created_at) VALUES (?, ?, ?)")
        .bind(workspace_id)
        .bind(name)
        .bind(created_at)
        .execute(&mut *conn)
        .await?;
    Ok(())
}
