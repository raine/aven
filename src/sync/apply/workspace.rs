use anyhow::{Context, Result};
use sqlx::SqliteConnection;

use crate::ids::now;
use crate::sync::wire::ChangeWire;

use super::shared::str_payload;

pub(super) async fn create_workspace(
    conn: &mut SqliteConnection,
    change: &ChangeWire,
) -> Result<()> {
    let key = str_payload(&change.payload, "key")?;
    let name = str_payload(&change.payload, "name")?;
    let created_at = str_payload(&change.payload, "created_at").unwrap_or_else(|_| now());
    sqlx::query(
        "INSERT OR IGNORE INTO workspaces(id, key, name, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&change.entity_id)
    .bind(key)
    .bind(name)
    .bind(&created_at)
    .bind(&created_at)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

pub(super) async fn set_workspace_field(
    conn: &mut SqliteConnection,
    change: &ChangeWire,
) -> Result<()> {
    let field = change
        .field
        .as_deref()
        .context("workspace field change missing field")?;
    let value = str_payload(&change.payload, "value")?;
    let ts = now();
    match field {
        "name" => {
            sqlx::query("UPDATE workspaces SET name = ?, updated_at = ? WHERE id = ?")
                .bind(value)
                .bind(&ts)
                .bind(&change.entity_id)
                .execute(&mut *conn)
                .await?;
        }
        "key" => {
            sqlx::query("UPDATE workspaces SET key = ?, updated_at = ? WHERE id = ?")
                .bind(value)
                .bind(&ts)
                .bind(&change.entity_id)
                .execute(&mut *conn)
                .await?;
        }
        _ => anyhow::bail!("error invalid-sync-change field={field}"),
    }
    Ok(())
}
