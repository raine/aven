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
        op_type::LABEL_ADD | op_type::LABEL_REMOVE => vec![text(change, "label")?.to_owned()],
        op_type::CREATE_LABEL | op_type::LABEL_DELETE | op_type::LABEL_RESTORE => {
            vec![text(change, "name")?.to_owned()]
        }
        op_type::SET_LABEL_NAME => vec![
            text(change, "name")?.to_owned(),
            text(change, "new_name")?.to_owned(),
        ],
        _ => return Ok(()),
    };
    let workspace = text(change, "workspace_id")?;
    let task = (change.entity_type == "task").then_some((change.entity_id.as_str(), 1));
    reconcile_names(conn, prefix, workspace, names, task).await
}

/// Reconciles the named labels. `task` limits the first `n` names to that task's
/// pair; other names, including labels a re-applied rename moves into, reconcile
/// every task with a retained pair command.
pub(super) async fn reconcile_names(
    conn: &mut SqliteConnection,
    prefix: i64,
    workspace: &str,
    mut names: Vec<String>,
    task: Option<(&str, usize)>,
) -> Result<()> {
    let mut i = 0;
    while i < names.len() {
        let label = names[i].clone();
        // A re-applied rename moves references, so the new name's pairs need reconciling.
        if let Some(moved) = reapply_last_removal(conn, prefix, workspace, &label).await?
            && !names.contains(&moved)
        {
            names.push(moved);
        }
        let tasks: Vec<String> = if let Some((task, _)) = task.filter(|(_, n)| i < *n) {
            vec![task.to_owned()]
        } else {
            sqlx::query_scalar(
                "SELECT DISTINCT entity_id FROM changes
                 WHERE entity_type = 'task' AND op_type IN ('label_add', 'label_remove')
                   AND json_extract(payload, '$.workspace_id') = ?
                   AND json_extract(payload, '$.label') = ?
                   AND (server_seq > ? OR server_seq IS NULL)",
            )
            .bind(workspace)
            .bind(&label)
            .bind(prefix)
            .fetch_all(&mut *conn)
            .await?
        };
        for task in tasks {
            reconcile_pair(conn, prefix, workspace, &task, &label).await?;
        }
        i += 1;
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
) -> Result<Option<String>> {
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
        return Ok(None);
    };
    let change = super::client::load_change(conn, &id)
        .await?
        .context("error encrypted-tail-label-history")?;
    let moved = match change.op_type.as_str() {
        op_type::LABEL_DELETE => None,
        op_type::SET_LABEL_NAME if change.entity_id == label => {
            Some(text(&change, "new_name")?.to_owned())
        }
        _ => return Ok(None),
    };
    crate::sync::apply::apply_remote_change_quiet(conn, &change)
        .await
        .map_err(|_| anyhow::anyhow!("error encrypted-tail-apply"))?;
    Ok(moved)
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
