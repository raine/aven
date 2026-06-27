use anyhow::Result;
use sqlx::SqliteConnection;

use crate::sync::wire::ChangeWire;

use super::shared::{str_payload, task_field_workspace_id_payload, workspace_id_payload};

pub(super) async fn delete_note(conn: &mut SqliteConnection, change: &ChangeWire) -> Result<()> {
    let workspace_id = task_field_workspace_id_payload(conn, change).await?;
    let note_id = str_payload(&change.payload, "note_id")?;
    sqlx::query("DELETE FROM notes WHERE workspace_id = ? AND task_id = ? AND id = ?")
        .bind(&workspace_id)
        .bind(&change.entity_id)
        .bind(&note_id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

pub(super) async fn add_note(conn: &mut SqliteConnection, change: &ChangeWire) -> Result<()> {
    let workspace_id = workspace_id_payload(conn, change).await?;
    let note_id = str_payload(&change.payload, "note_id")?;
    let body = str_payload(&change.payload, "body")?;
    let created_at =
        str_payload(&change.payload, "created_at").unwrap_or_else(|_| change.created_at.clone());
    sqlx::query(
        "INSERT OR IGNORE INTO notes(workspace_id, id, task_id, body, created_at, change_id)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&workspace_id)
    .bind(&note_id)
    .bind(&change.entity_id)
    .bind(&body)
    .bind(&created_at)
    .bind(&change.change_id)
    .execute(&mut *conn)
    .await?;
    sqlx::query("UPDATE tasks SET queue_activity_at = ? WHERE workspace_id = ? AND id = ?")
        .bind(&created_at)
        .bind(&workspace_id)
        .bind(&change.entity_id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}
