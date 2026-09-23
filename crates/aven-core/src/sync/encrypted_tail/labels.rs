use anyhow::{Context, Result};
use sqlx::SqliteConnection;

use crate::change_log::op_type;
use crate::sync::wire::ChangeWire;

pub(super) async fn reconcile(
    conn: &mut SqliteConnection,
    prefix: i64,
    change: &ChangeWire,
) -> Result<()> {
    if !matches!(
        change.op_type.as_str(),
        op_type::LABEL_ADD | op_type::LABEL_REMOVE
    ) {
        return Ok(());
    }
    let workspace = change.payload["workspace_id"]
        .as_str()
        .context("error encrypted-tail-label-history")?;
    let label = change.payload["label"]
        .as_str()
        .context("error encrypted-tail-label-history")?;
    // Each command assigns presence, so only the last tail command is needed.
    // Pending work follows accepted order; prefix history is not the snapshot.
    let last: Option<String> = sqlx::query_scalar(
        "SELECT op_type FROM changes
         WHERE entity_type = 'task' AND entity_id = ?
           AND op_type IN ('label_add', 'label_remove')
           AND json_extract(payload, '$.workspace_id') = ?
           AND json_extract(payload, '$.label') = ?
           AND (server_seq > ? OR server_seq IS NULL)
         ORDER BY server_seq IS NULL DESC, server_seq DESC,
                  local_seq DESC, created_at DESC, change_id DESC
         LIMIT 1",
    )
    .bind(&change.entity_id)
    .bind(workspace)
    .bind(label)
    .bind(prefix)
    .fetch_optional(&mut *conn)
    .await?;
    match last.as_deref() {
        Some(op_type::LABEL_ADD) => {
            sqlx::query(
                "INSERT OR IGNORE INTO task_labels(workspace_id, task_id, label) VALUES (?, ?, ?)",
            )
            .bind(workspace)
            .bind(&change.entity_id)
            .bind(label)
            .execute(conn)
            .await?;
        }
        Some(op_type::LABEL_REMOVE) => {
            sqlx::query(
                "DELETE FROM task_labels WHERE workspace_id = ? AND task_id = ? AND label = ?",
            )
            .bind(workspace)
            .bind(&change.entity_id)
            .bind(label)
            .execute(conn)
            .await?;
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests;
