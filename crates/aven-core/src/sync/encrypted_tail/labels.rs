use anyhow::{Context, Result};
use sqlx::SqliteConnection;

use crate::change_log::op_type;
use crate::sync::wire::ChangeWire;

// Selects the last matching tail command: accepted order followed by the local
// pending push order. Prefix history is not the snapshot, so it never assigns state.
macro_rules! last_tail_command {
    ($select:literal, $filter:literal) => {
        concat!(
            $select,
            " FROM changes WHERE json_extract(payload, '$.workspace_id') = ? AND ",
            $filter,
            " AND (server_seq > ? OR server_seq IS NULL)
             ORDER BY server_seq IS NULL DESC, server_seq DESC,
                      local_seq DESC, created_at DESC, change_id DESC
             LIMIT 1"
        )
    };
}

pub(super) async fn reconcile(
    conn: &mut SqliteConnection,
    prefix: i64,
    change: &ChangeWire,
) -> Result<()> {
    let names = match change.op_type.as_str() {
        op_type::LABEL_ADD | op_type::LABEL_REMOVE => vec![text(change, "label")?],
        op_type::CREATE_LABEL | op_type::LABEL_DELETE | op_type::LABEL_RESTORE => {
            vec![text(change, "name")?]
        }
        op_type::SET_LABEL_NAME => vec![text(change, "name")?, text(change, "new_name")?],
        _ => return Ok(()),
    };
    let workspace = text(change, "workspace_id")?;
    for label in names {
        reapply_last_removal(conn, prefix, workspace, label).await?;
        let tasks: Vec<String> = if change.entity_type == "task" {
            vec![change.entity_id.clone()]
        } else {
            sqlx::query_scalar(
                "SELECT DISTINCT entity_id FROM changes
                 WHERE entity_type = 'task' AND op_type IN ('label_add', 'label_remove')
                   AND json_extract(payload, '$.workspace_id') = ?
                   AND json_extract(payload, '$.label') = ?
                   AND (server_seq > ? OR server_seq IS NULL)",
            )
            .bind(workspace)
            .bind(label)
            .bind(prefix)
            .fetch_all(&mut *conn)
            .await?
        };
        for task in tasks {
            reconcile_pair(conn, prefix, workspace, &task, label).await?;
        }
    }
    Ok(())
}

/// A label deleted or renamed away by its last tail command stays absent even when
/// an earlier remote command recreated it after the local command was recorded.
async fn reapply_last_removal(
    conn: &mut SqliteConnection,
    prefix: i64,
    workspace: &str,
    label: &str,
) -> Result<()> {
    let last: Option<String> = sqlx::query_scalar(last_tail_command!(
        "SELECT change_id",
        "((entity_type = 'label' AND entity_id = ?
                 AND op_type IN ('create_label', 'set_label_name', 'label_delete', 'label_restore'))
             OR (entity_type = 'label' AND op_type = 'set_label_name'
                 AND json_extract(payload, '$.new_name') = ?)
             OR (entity_type = 'task' AND op_type = 'label_add'
                 AND json_extract(payload, '$.label') = ?))"
    ))
    .bind(workspace)
    .bind(label)
    .bind(label)
    .bind(label)
    .bind(prefix)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(id) = last else {
        return Ok(());
    };
    let change = super::client::load_change(conn, &id)
        .await?
        .context("error encrypted-tail-label-history")?;
    let removal = change.op_type == op_type::LABEL_DELETE
        || (change.op_type == op_type::SET_LABEL_NAME && change.entity_id == label);
    if removal {
        crate::sync::apply::apply_remote_change_quiet(conn, &change)
            .await
            .map_err(|_| anyhow::anyhow!("error encrypted-tail-apply"))?;
    }
    Ok(())
}

/// Assigns one task-label pair from its last absolute tail command. A rename into
/// the label carries the old label's presence, which local apply order already holds.
async fn reconcile_pair(
    conn: &mut SqliteConnection,
    prefix: i64,
    workspace: &str,
    task: &str,
    label: &str,
) -> Result<()> {
    let last: Option<(String, String, String)> = sqlx::query_as(last_tail_command!(
        "SELECT op_type, entity_id,
                COALESCE(json_extract(payload, '$.created_at'), created_at)",
        "((entity_type = 'task' AND entity_id = ?
                 AND op_type IN ('label_add', 'label_remove')
                 AND json_extract(payload, '$.label') = ?)
             OR (entity_type = 'label' AND entity_id = ?
                 AND op_type IN ('set_label_name', 'label_delete'))
             OR (entity_type = 'label' AND op_type = 'set_label_name'
                 AND json_extract(payload, '$.new_name') = ?)
             OR (entity_type = 'label' AND entity_id = ? AND op_type = 'label_restore'
                 AND EXISTS(SELECT 1 FROM json_each(payload, '$.task_ids') WHERE value = ?)))"
    ))
    .bind(workspace)
    .bind(task)
    .bind(label)
    .bind(label)
    .bind(label)
    .bind(label)
    .bind(task)
    .bind(prefix)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((op, entity, created_at)) = last else {
        return Ok(());
    };
    let present = match op.as_str() {
        op_type::LABEL_ADD | op_type::LABEL_RESTORE => true,
        op_type::SET_LABEL_NAME if entity != label => return Ok(()),
        _ => false,
    };
    if present {
        sqlx::query(
            "INSERT OR IGNORE INTO labels(workspace_id, name, created_at) VALUES (?, ?, ?)",
        )
        .bind(workspace)
        .bind(label)
        .bind(&created_at)
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "INSERT OR IGNORE INTO task_labels(workspace_id, task_id, label) VALUES (?, ?, ?)",
        )
        .bind(workspace)
        .bind(task)
        .bind(label)
        .execute(conn)
        .await?;
    } else {
        sqlx::query("DELETE FROM task_labels WHERE workspace_id = ? AND task_id = ? AND label = ?")
            .bind(workspace)
            .bind(task)
            .bind(label)
            .execute(conn)
            .await?;
    }
    Ok(())
}

fn text<'a>(change: &'a ChangeWire, key: &str) -> Result<&'a str> {
    change.payload[key]
        .as_str()
        .context("error encrypted-tail-label-history")
}

#[cfg(test)]
mod tests;
