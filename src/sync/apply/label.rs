use anyhow::Result;
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
    let label = str_payload(&change.payload, "label")?;
    insert_label(conn, &workspace_id, &label, &change.created_at).await?;
    sqlx::query("INSERT OR IGNORE INTO task_labels(workspace_id, task_id, label) VALUES (?, ?, ?)")
        .bind(&workspace_id)
        .bind(&change.entity_id)
        .bind(&label)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

pub(super) async fn delete_label(conn: &mut SqliteConnection, change: &ChangeWire) -> Result<()> {
    let workspace_id = workspace_id_payload(conn, change).await?;
    let name = str_payload(&change.payload, "name")?;
    sqlx::query("DELETE FROM task_labels WHERE workspace_id = ? AND label = ?")
        .bind(&workspace_id)
        .bind(&name)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM labels WHERE workspace_id = ? AND name = ?")
        .bind(&workspace_id)
        .bind(&name)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

pub(super) async fn remove_label(conn: &mut SqliteConnection, change: &ChangeWire) -> Result<()> {
    let workspace_id = workspace_id_payload(conn, change).await?;
    let label = str_payload(&change.payload, "label")?;
    sqlx::query("DELETE FROM task_labels WHERE workspace_id = ? AND task_id = ? AND label = ?")
        .bind(&workspace_id)
        .bind(&change.entity_id)
        .bind(&label)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

pub(super) async fn create_or_update_task_label(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    task_id: &str,
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

async fn insert_label(
    conn: &mut SqliteConnection,
    workspace_id: &str,
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
