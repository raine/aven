use anyhow::{Context, Result};
use sqlx::SqliteConnection;
use tracing::info;

use crate::change_log::op_type;
use crate::db::{begin_immediate, insert_change, set_field_version};
use crate::error::CoreError;
use crate::ids::{MetadataFieldId, TaskId, now};
use crate::metadata::{
    TaskMetadataInput, decode_metadata_conflict_value, encode_metadata_conflict_value,
    metadata_field_by_id, validate_task_metadata_result,
};
use crate::refs::get_task_in_workspace;
use crate::workspaces::Workspace;

use super::{ConflictOutcome, ConflictResolutionOutcome, ConflictResolutionValue};

pub(super) async fn resolve_metadata_conflict_value(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &TaskId,
    identity: &str,
    field_id: MetadataFieldId,
    resolution: ConflictResolutionValue<'_>,
) -> Result<ConflictResolutionOutcome> {
    let mut tx = begin_immediate(conn).await?;
    let field = metadata_field_by_id(&mut tx, &workspace.id, &field_id)
        .await?
        .context("error metadata-field-not-found")?;
    let conflict = sqlx::query_as::<_, (i64, String, String)>(
        "SELECT id, local_value, remote_value FROM conflicts
         WHERE workspace_id = ? AND entity_type = 'task' AND entity_id = ?
           AND field = ? AND resolved = 0",
    )
    .bind(&workspace.id)
    .bind(task_id)
    .bind(identity)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| {
        CoreError::not_found(format!(
            "error conflict-not-found task_id={task_id} field={identity}"
        ))
    })?;
    let current: Option<String> = sqlx::query_scalar(
        "SELECT value FROM task_metadata
         WHERE workspace_id = ? AND task_id = ? AND field_id = ?",
    )
    .bind(&workspace.id)
    .bind(task_id)
    .bind(&field_id)
    .fetch_optional(&mut *tx)
    .await?;
    let before = encode_metadata_conflict_value(current.as_deref())?;
    let encoded = match resolution {
        ConflictResolutionValue::Local => conflict.1.clone(),
        ConflictResolutionValue::Remote => conflict.2.clone(),
        ConflictResolutionValue::Explicit(value) if value == conflict.1 || value == conflict.2 => {
            value.to_string()
        }
        ConflictResolutionValue::Explicit(value) => encode_metadata_conflict_value(Some(value))?,
    };
    let value = decode_metadata_conflict_value(&encoded)?;
    let (set, remove) = match &value {
        Some(value) => (
            vec![TaskMetadataInput {
                expected_field_id: Some(field_id.clone()),
                key: field.key.clone(),
                value: value.clone(),
            }],
            Vec::new(),
        ),
        None => (Vec::new(), vec![field.key.clone()]),
    };
    validate_task_metadata_result(&mut tx, &workspace.id, task_id, &set, &remove).await?;
    let changed_at = now();
    let (op, payload) = if let Some(value) = value.as_deref() {
        sqlx::query(
            "INSERT INTO task_metadata(
                 workspace_id, task_id, field_id, value, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT(workspace_id, task_id, field_id)
             DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        )
        .bind(&workspace.id)
        .bind(task_id)
        .bind(&field_id)
        .bind(value)
        .bind(&changed_at)
        .bind(&changed_at)
        .execute(&mut *tx)
        .await?;
        (
            op_type::SET_TASK_METADATA,
            crate::change_log::ChangePayload::workspace(workspace)
                .set("field_id", &field_id)
                .set("key", &field.key)
                .set("value", value)
                .set("conflict_resolution", true)
                .into_value(),
        )
    } else {
        sqlx::query(
            "DELETE FROM task_metadata
             WHERE workspace_id = ? AND task_id = ? AND field_id = ?",
        )
        .bind(&workspace.id)
        .bind(task_id)
        .bind(&field_id)
        .execute(&mut *tx)
        .await?;
        (
            op_type::REMOVE_TASK_METADATA,
            crate::change_log::ChangePayload::workspace(workspace)
                .set("field_id", &field_id)
                .set("key", &field.key)
                .set("conflict_resolution", true)
                .into_value(),
        )
    };
    sqlx::query("UPDATE tasks SET updated_at = ? WHERE workspace_id = ? AND id = ?")
        .bind(&changed_at)
        .bind(&workspace.id)
        .bind(task_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE conflicts SET resolved = 1 WHERE id = ?")
        .bind(conflict.0)
        .execute(&mut *tx)
        .await?;
    let change_id =
        insert_change(&mut tx, "task", task_id, Some(identity), op, payload, None).await?;
    set_field_version(&mut tx, task_id, identity, &change_id).await?;
    let task = get_task_in_workspace(&mut tx, workspace, task_id).await?;
    tx.commit().await?;
    info!(task_id = %task_id, field_id = %field_id, "metadata conflict resolved");
    Ok(ConflictResolutionOutcome {
        outcome: ConflictOutcome {
            task,
            field: identity.to_string(),
        },
        before,
        after: encoded,
        conflict_id: conflict.0,
    })
}

#[cfg(test)]
mod tests;
