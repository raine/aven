use anyhow::{Result, bail, ensure};
use sqlx::SqliteConnection;
use tracing::info;

use crate::change_log::op_type;
use crate::db::{begin_immediate, insert_change, set_field_version};
use crate::error::CoreError;
use crate::mutation::{apply_field_value_in_workspace, apply_project_id_in_workspace};
use crate::projects::resolve_project_for_stored_value;
use crate::refs::get_task_in_workspace;
use crate::task_fields::TaskField;
use crate::undo::{UndoCommand, UndoPayload, record_tui_undo};
use crate::workspaces::Workspace;

use super::{
    ConflictOutcome, ConflictResolutionOutcome, ConflictResolutionValue, ExpectedConflictIdentity,
};

pub(super) async fn resolve_conflict_value(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &crate::ids::TaskId,
    field: &str,
    resolution: ConflictResolutionValue<'_>,
    expected: Option<ExpectedConflictIdentity<'_>>,
    tui_summary: Option<&str>,
) -> Result<ConflictResolutionOutcome> {
    if let Some(field_id) = field.strip_prefix("metadata:") {
        ensure!(
            tui_summary.is_none(),
            "error metadata-conflicts-not-supported-in-tui"
        );
        return super::metadata::resolve_metadata_conflict_value(
            conn,
            workspace,
            task_id,
            field,
            field_id.parse()?,
            resolution,
        )
        .await;
    }
    let task_field = TaskField::parse_or_unknown(field)?;
    let field = task_field.as_str();
    let mut tx = begin_immediate(conn).await?;
    let before = crate::undo::task_field_value(&mut tx, &workspace.id, task_id, field).await?;
    let conflict_id = crate::undo::conflict_row_id(&mut tx, &workspace.id, task_id, field).await?;
    let conflict = sqlx::query_as::<_, (String, String, String, String)>(
        "SELECT local_value, remote_value, variant_a, variant_b FROM conflicts
         WHERE workspace_id = ? AND task_id = ? AND field = ? AND resolved = 0",
    )
    .bind(&workspace.id)
    .bind(task_id)
    .bind(field)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| {
        CoreError::not_found(format!(
            "error conflict-not-found task_id={task_id} field={field}"
        ))
    })?;
    if let Some(expected) = expected
        && (conflict.2 != expected.variant_a || conflict.3 != expected.variant_b)
    {
        return Err(CoreError::generation_conflict(format!(
            "error stale-conflict task_id={task_id} field={field}"
        ))
        .into());
    }
    let value = match resolution {
        ConflictResolutionValue::Explicit(value) => value.to_string(),
        ConflictResolutionValue::Local => conflict.0,
        ConflictResolutionValue::Remote => conflict.1,
    };
    if task_field == TaskField::Status {
        ensure_recurrence_outcome_status(&mut tx, workspace, task_id, &value).await?;
    }
    if task_field == TaskField::IsEpic
        && value == "0"
        && crate::operations::task_has_epic_children(&mut tx, &workspace.id, task_id).await?
    {
        bail!("error epic-has-children task_id={task_id}");
    }
    let affected_attachment_hashes: Vec<String> = if task_field == TaskField::Deleted {
        sqlx::query_scalar(
            "SELECT DISTINCT sha256 FROM task_attachments
             WHERE workspace_id = ? AND task_id = ?",
        )
        .bind(&workspace.id)
        .bind(task_id)
        .fetch_all(&mut *tx)
        .await?
    } else {
        Vec::new()
    };
    let result = sqlx::query(
        "UPDATE conflicts SET resolved = 1 WHERE workspace_id = ? AND task_id = ? AND field = ? AND resolved = 0",
    )
    .bind(&workspace.id)
    .bind(task_id)
    .bind(field)
    .execute(&mut *tx)
    .await?;
    if result.rows_affected() != 1 {
        return Err(CoreError::not_found(format!(
            "error conflict-not-found task_id={task_id} field={field}"
        ))
        .into());
    }
    let payload = if task_field.is_project() {
        let project = resolve_project_for_stored_value(&mut tx, &workspace.id, &value).await?;
        apply_project_id_in_workspace(&mut tx, &workspace.id, task_id, &project.id).await?;
        TaskField::project_payload(&workspace.id, &workspace.key, &project)
    } else {
        apply_field_value_in_workspace(&mut tx, &workspace.id, task_id, field, &value).await?;
        task_field.scalar_payload(&workspace.id, &workspace.key, &value)?
    };
    let change_id = insert_change(
        &mut tx,
        "task",
        task_id,
        Some(field),
        op_type::RESOLVE_FIELD,
        payload,
        None,
    )
    .await?;
    set_field_version(&mut tx, task_id, field, &change_id).await?;
    crate::attachments::lifecycle::reconcile_liveness_for_hashes_in_transaction(
        &mut tx,
        &affected_attachment_hashes,
        &crate::attachments::lifecycle::SystemClock,
    )
    .await?;
    let task = get_task_in_workspace(&mut tx, workspace, task_id).await?;
    let after = crate::undo::task_field_value(&mut tx, &workspace.id, task_id, field).await?;
    if let Some(summary) = tui_summary {
        record_tui_undo(
            &mut tx,
            &workspace.id,
            summary,
            UndoPayload {
                commands: vec![UndoCommand::RestoreConflictResolution {
                    task_id: task_id.clone(),
                    field: field.to_string(),
                    before: before.clone(),
                    after: after.clone(),
                    conflict_id,
                }],
            },
        )
        .await?;
    }
    tx.commit().await?;
    info!(task_id = %task_id, field = %field, "conflict resolved");
    Ok(ConflictResolutionOutcome {
        outcome: ConflictOutcome {
            task,
            field: field.to_string(),
        },
        before,
        after,
        conflict_id,
    })
}

/// A resolved recurrence occurrence owns its terminal task status, as the local
/// recurrence mutation gate enforces, so a status conflict resolves only to it.
async fn ensure_recurrence_outcome_status(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &crate::ids::TaskId,
    value: &str,
) -> Result<()> {
    let resolved: Option<String> = sqlx::query_scalar(
        "SELECT t.status FROM recurrence_occurrences o
         JOIN tasks t ON t.workspace_id = o.workspace_id AND t.id = o.task_id
         WHERE o.workspace_id = ? AND o.task_id = ? AND o.outcome <> ''",
    )
    .bind(&workspace.id)
    .bind(task_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(status) = resolved else {
        return Ok(());
    };
    if value == status {
        return Ok(());
    }
    let error = if crate::choices::TaskStatus::parse(value)?.is_open() {
        "recurrence-terminal-reopen"
    } else {
        "recurrence-outcome-final"
    };
    Err(CoreError::validation(format!(
        "error {error} task_id={task_id} hint=\"use immediate undo\""
    ))
    .into())
}
