use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Utc};
use sqlx::SqliteConnection;

use crate::change_log::{ChangeEntity, ChangePayload, append_change, op_type};
use crate::choices::TaskStatus;
use crate::db::{entity_field_version, insert_change, set_entity_field_version, set_field_version};
use crate::error::CoreError;
use crate::ids::{TaskId, WorkspaceId};
use crate::mutation::apply_field_value_in_workspace;
use crate::recurrence::{
    RecurrenceOutcome, RecurrenceProjectionState, RecurrenceSeriesId, RecurrenceSeriesState,
    derive_occurrence_identity, next_slot_after, slot_values,
};
use crate::refs::get_task_in_workspace;
use crate::task_fields::TaskField;
use crate::types::{MutableEntityType, RecurrenceOccurrence, RecurrenceSeries};
use crate::workspaces::Workspace;

use super::projection::{generated_creates, generates_proposals};
use super::{
    RecurrenceResolveOutcome, format_local_time, load_occurrence, load_occurrence_for_task,
    load_projected_occurrence, load_series, load_series_labels, load_series_metadata,
    materialize_occurrence, reconcile_recurrence_series_in_transaction,
    verify_materialized_occurrence,
};

pub(crate) async fn resolve_recurrence_occurrence_in_transaction(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &TaskId,
    outcome: RecurrenceOutcome,
    resolved_at: &str,
) -> Result<RecurrenceResolveOutcome> {
    let mut occurrence = load_occurrence_for_task(conn, &workspace.id, task_id)
        .await?
        .ok_or_else(|| CoreError::not_found("error recurrence-occurrence-not-found"))?;
    if matches!(
        occurrence.projection_state,
        RecurrenceProjectionState::Projected
    ) {
        reconcile_recurrence_series_in_transaction(
            conn,
            workspace,
            &occurrence.series_id,
            DateTime::parse_from_rfc3339(resolved_at)
                .context("invalid recurrence resolution time")?
                .with_timezone(&Utc),
        )
        .await?;
        occurrence = load_occurrence_for_task(conn, &workspace.id, task_id)
            .await?
            .ok_or_else(|| CoreError::not_found("error recurrence-occurrence-not-found"))?;
    }
    ensure!(
        matches!(
            occurrence.projection_state,
            RecurrenceProjectionState::Projected
        ),
        CoreError::validation(format!(
            "error recurrence-occurrence-not-current task_id={task_id}"
        ))
    );
    let task = get_task_in_workspace(conn, workspace, task_id).await?;
    ensure!(
        task.status.is_open(),
        CoreError::validation(format!(
            "error recurrence-occurrence-terminal task_id={task_id}"
        ))
    );
    let series = load_series(conn, &workspace.id, &occurrence.series_id).await?;
    let target_status = match outcome {
        RecurrenceOutcome::Completed => TaskStatus::Done,
        RecurrenceOutcome::Skipped => TaskStatus::Canceled,
    };
    let status_change_id = write_task_status(
        conn,
        workspace,
        task_id,
        task.status,
        target_status,
        resolved_at,
    )
    .await?;

    let successor_slot = if matches!(series.state, RecurrenceSeriesState::Active) {
        Some(
            next_slot_after(&series.rule, series.start_on, occurrence.slot_on)
                .context("recurrence schedule has no representable successor")?,
        )
    } else {
        None
    };
    let successor_task_id = successor_slot
        .map(|slot| {
            derive_occurrence_identity(&workspace.id, &series.id, &series.schedule(), slot)
                .map(|identity| identity.task_id)
        })
        .transpose()?;
    let outcome_change_id = append_change(
        conn,
        ChangeEntity::RecurrenceSeries,
        series.id.as_str(),
        Some("outcome"),
        op_type::RESOLVE_RECURRENCE_OCCURRENCE,
        ChangePayload::workspace(workspace)
            .set("slot_on", occurrence.slot_on.format("%Y-%m-%d").to_string())
            .set("task_id", task_id.as_str())
            .set("outcome", outcome.as_str())
            .set("task_status", target_status.as_str())
            .set("resolved_at", resolved_at)
            .set("task_status_change_id", &status_change_id)
            .set(
                "successor_task_id",
                successor_task_id.as_ref().map(TaskId::as_str).unwrap_or(""),
            )
            .set("frequency", series.rule.frequency().as_str())
            .set("interval", series.rule.interval())
            .set("weekdays", series.rule.weekdays_set().to_string())
            .set("timezone", series.timezone.as_str())
            .set("start_on", series.start_on.format("%Y-%m-%d").to_string())
            .set(
                "available_local_time",
                format_local_time(series.available_local_time),
            )
            .set("due_policy", series.due_policy.as_str()),
    )
    .await?;
    sqlx::query(
        "UPDATE recurrence_occurrences
         SET outcome = ?, resolved_at = ?, outcome_change_id = ?, projection_state = 'resolved'
         WHERE workspace_id = ? AND series_id = ? AND slot_on = ?
         AND projection_state = 'projected'",
    )
    .bind(outcome.as_str())
    .bind(resolved_at)
    .bind(&outcome_change_id)
    .bind(&workspace.id)
    .bind(&series.id)
    .bind(occurrence.slot_on.format("%Y-%m-%d").to_string())
    .execute(&mut *conn)
    .await?;

    let labels = load_series_labels(conn, &workspace.id, &series.id).await?;
    let successor = if let Some(next_slot) = successor_slot {
        let successor_occurrence =
            materialize_occurrence(conn, workspace, &series, &labels, next_slot).await?;
        let successor_id = successor_occurrence
            .task_id
            .as_ref()
            .expect("materialized successor has a task");
        Some(get_task_in_workspace(conn, workspace, successor_id).await?)
    } else {
        None
    };
    let resolved = load_occurrence(conn, &workspace.id, &series.id, occurrence.slot_on)
        .await?
        .expect("resolved occurrence remains stored");
    let task = get_task_in_workspace(conn, workspace, task_id).await?;
    Ok(RecurrenceResolveOutcome {
        series,
        resolved,
        task,
        successor,
    })
}
/// Local write intent checked against the recurrence aggregate in its owner transaction.
#[derive(Debug, Clone, Copy)]
pub(crate) enum RecurrenceTaskMutation<'a> {
    Scalar { field: TaskField, value: &'a str },
    Structural(RecurrenceStructuralMutation),
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum RecurrenceStructuralMutation {
    Labels,
    Notes,
    Attachments,
    Dependencies,
    EpicMembership,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecurrenceMutationOutcome {
    Proceed,
    NoChange,
    Handled,
}

pub(crate) async fn route_recurrence_task_mutation(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &TaskId,
    request: RecurrenceTaskMutation<'_>,
    at: &str,
) -> Result<RecurrenceMutationOutcome> {
    let Some(mut occurrence) = load_occurrence_for_task(conn, &workspace.id, task_id).await? else {
        return Ok(RecurrenceMutationOutcome::Proceed);
    };
    if matches!(
        occurrence.projection_state,
        RecurrenceProjectionState::Projected
    ) {
        reconcile_recurrence_series_in_transaction(
            conn,
            workspace,
            &occurrence.series_id,
            DateTime::parse_from_rfc3339(at)?.with_timezone(&Utc),
        )
        .await?;
        occurrence = load_occurrence_for_task(conn, &workspace.id, task_id)
            .await?
            .ok_or_else(|| CoreError::not_found("error recurrence-occurrence-not-found"))?;
    }
    if matches!(
        occurrence.projection_state,
        RecurrenceProjectionState::Archived
    ) {
        return Err(CoreError::validation(format!(
            "error recurrence-occurrence-archived task_id={task_id}"
        ))
        .into());
    }
    let (field, value) = match request {
        RecurrenceTaskMutation::Scalar { field, value } => (field, value),
        RecurrenceTaskMutation::Structural(
            RecurrenceStructuralMutation::Labels
            | RecurrenceStructuralMutation::Notes
            | RecurrenceStructuralMutation::Attachments
            | RecurrenceStructuralMutation::Dependencies
            | RecurrenceStructuralMutation::EpicMembership,
        ) => return Ok(RecurrenceMutationOutcome::Proceed),
    };
    if field == TaskField::Status {
        let task = get_task_in_workspace(conn, workspace, task_id).await?;
        let target = TaskStatus::parse(value)?;
        if task.status == target {
            return Ok(RecurrenceMutationOutcome::NoChange);
        }
        if task.status.is_terminal() {
            if target.is_open() {
                return Err(CoreError::validation(format!(
                    "error recurrence-terminal-reopen task_id={task_id} hint=\"use immediate undo\""
                ))
                .into());
            }
            return Err(CoreError::validation(format!(
                "error recurrence-outcome-final task_id={task_id} hint=\"use immediate undo\""
            ))
            .into());
        }
        if target.is_terminal() {
            ensure!(
                matches!(
                    occurrence.projection_state,
                    RecurrenceProjectionState::Projected
                ),
                "error recurrence-occurrence-not-current task_id={task_id}"
            );
            let outcome = if target == TaskStatus::Done {
                RecurrenceOutcome::Completed
            } else {
                RecurrenceOutcome::Skipped
            };
            resolve_recurrence_occurrence_in_transaction(conn, workspace, task_id, outcome, at)
                .await?;
            return Ok(RecurrenceMutationOutcome::Handled);
        }
        return Ok(RecurrenceMutationOutcome::Proceed);
    }
    if field == TaskField::Deleted
        && matches!(
            occurrence.projection_state,
            RecurrenceProjectionState::Projected
        )
        && value == "1"
    {
        let series = load_series(conn, &workspace.id, &occurrence.series_id).await?;
        let guidance = match series.state {
            RecurrenceSeriesState::Active => "skip, pause, or stop the series",
            RecurrenceSeriesState::Paused => "skip the occurrence or stop the series",
            RecurrenceSeriesState::Stopped => "complete or cancel the final occurrence",
        };
        return Err(CoreError::validation(format!(
            "error recurrence-current-delete task_id={task_id} hint=\"{guidance}\""
        ))
        .into());
    }
    Ok(RecurrenceMutationOutcome::Proceed)
}

pub(crate) async fn undo_recurrence_resolution(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: &TaskId,
    before_status: &str,
    after_status: &str,
) -> Result<bool> {
    let Some(occurrence) = load_occurrence_for_task(conn, workspace_id, task_id).await? else {
        return Ok(false);
    };
    if !matches!(
        occurrence.projection_state,
        RecurrenceProjectionState::Resolved
    ) || occurrence.outcome.is_none()
    {
        return Ok(false);
    }
    let task_status: String =
        sqlx::query_scalar("SELECT status FROM tasks WHERE workspace_id = ? AND id = ?")
            .bind(workspace_id)
            .bind(task_id)
            .fetch_one(&mut *conn)
            .await?;
    ensure!(
        task_status == after_status,
        "error undo-state-changed task_id={task_id} field=status"
    );
    let series = load_series(conn, workspace_id, &occurrence.series_id).await?;
    let outcome_change_id = occurrence
        .outcome_change_id
        .as_deref()
        .context("error recurrence-undo-missing-outcome-change")?;
    ensure_change_is_latest_series_transition(conn, workspace_id, &series.id, outcome_change_id)
        .await?;
    ensure_change_unsynced(conn, outcome_change_id).await?;
    let status_change_id = entity_field_version(
        conn,
        workspace_id,
        MutableEntityType::Task,
        task_id.as_str(),
        "status",
    )
    .await?
    .context("error recurrence-undo-missing-status-change")?;
    ensure_change_unsynced(conn, &status_change_id).await?;
    let prior_status_version: Option<String> =
        sqlx::query_scalar("SELECT base_version FROM changes WHERE change_id = ?")
            .bind(&status_change_id)
            .fetch_one(&mut *conn)
            .await?;
    crate::sync::shared_state::ensure_changes_not_local_capture_protected(
        conn,
        &[outcome_change_id, &status_change_id],
    )
    .await?;

    let successor = load_projected_occurrence(conn, workspace_id, &series.id).await?;
    match series.state {
        RecurrenceSeriesState::Active => {
            let successor = successor.context("error recurrence-undo-successor-missing")?;
            let expected_slot = next_slot_after(&series.rule, series.start_on, occurrence.slot_on)
                .context("recurrence schedule has no representable successor")?;
            ensure!(
                successor.slot_on == expected_slot,
                "error recurrence-undo-successor-changed"
            );
            let generated =
                ensure_successor_untouched(conn, workspace_id, &series, &successor).await?;
            remove_materialized_occurrence(conn, workspace_id, &series, &successor, &generated)
                .await?;
        }
        RecurrenceSeriesState::Paused | RecurrenceSeriesState::Stopped => {
            ensure!(
                successor.is_none(),
                "error recurrence-undo-successor-exists"
            );
        }
    }

    apply_field_value_in_workspace(conn, workspace_id, task_id, "status", before_status).await?;
    if let Some(version) = prior_status_version {
        set_entity_field_version(
            conn,
            workspace_id,
            MutableEntityType::Task,
            task_id.as_str(),
            "status",
            &version,
        )
        .await?;
    } else {
        sqlx::query(
            "DELETE FROM field_versions
             WHERE workspace_id = ? AND entity_type = 'task' AND entity_id = ? AND field = 'status'",
        )
        .bind(workspace_id)
        .bind(task_id)
        .execute(&mut *conn)
        .await?;
    }
    sqlx::query(
        "UPDATE recurrence_occurrences
         SET outcome = '', resolved_at = '', outcome_change_id = '', projection_state = 'projected'
         WHERE workspace_id = ? AND series_id = ? AND slot_on = ?",
    )
    .bind(workspace_id)
    .bind(&series.id)
    .bind(occurrence.slot_on.format("%Y-%m-%d").to_string())
    .execute(&mut *conn)
    .await?;
    sqlx::query("DELETE FROM changes WHERE change_id IN (?, ?)")
        .bind(&status_change_id)
        .bind(outcome_change_id)
        .execute(&mut *conn)
        .await?;
    Ok(true)
}
async fn write_task_status(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &TaskId,
    before: TaskStatus,
    after: TaskStatus,
    resolved_at: &str,
) -> Result<String> {
    let field = "status";
    ensure!(
        !crate::db::conflict_exists(conn, &workspace.id, task_id, field).await?,
        "error conflicted-field ref={} field=status hint=\"use conflict resolve\"",
        task_id
    );
    let base = entity_field_version(
        conn,
        &workspace.id,
        MutableEntityType::Task,
        task_id.as_str(),
        field,
    )
    .await?;
    apply_field_value_in_workspace(conn, &workspace.id, task_id, field, after.as_str()).await?;
    let change_id = insert_change(
        conn,
        "task",
        task_id.as_str(),
        Some(field),
        op_type::SET_FIELD,
        TaskField::Status.scalar_payload(&workspace.id, &workspace.key, after.as_str())?,
        base.as_deref(),
    )
    .await?;
    set_field_version(conn, task_id, field, &change_id).await?;
    ensure!(
        before.is_open() && after.is_terminal(),
        "error recurrence-invalid-terminal-transition"
    );
    sqlx::query(
        "UPDATE tasks SET updated_at = ?, queue_activity_at = ?
         WHERE workspace_id = ? AND id = ?",
    )
    .bind(resolved_at)
    .bind(resolved_at)
    .bind(&workspace.id)
    .bind(task_id)
    .execute(&mut *conn)
    .await?;
    Ok(change_id)
}
async fn ensure_change_is_latest_series_transition(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series_id: &RecurrenceSeriesId,
    change_id: &str,
) -> Result<()> {
    let latest: Option<String> = sqlx::query_scalar(
        "SELECT change_id FROM changes
         WHERE entity_type = 'recurrence_series' AND entity_id = ?
         AND json_extract(payload, '$.workspace_id') = ?
         AND op_type != 'project_recurrence_occurrence'
         ORDER BY local_seq DESC LIMIT 1",
    )
    .bind(series_id)
    .bind(workspace_id.as_str())
    .fetch_optional(&mut *conn)
    .await?;
    ensure!(
        latest.as_deref() == Some(change_id),
        "error recurrence-undo-later-operation"
    );
    Ok(())
}

async fn ensure_change_unsynced(conn: &mut SqliteConnection, change_id: &str) -> Result<()> {
    let server_seq: Option<i64> =
        sqlx::query_scalar("SELECT server_seq FROM changes WHERE change_id = ?")
            .bind(change_id)
            .fetch_one(&mut *conn)
            .await?;
    ensure!(server_seq.is_none(), "error recurrence-undo-already-synced");
    Ok(())
}

async fn ensure_successor_untouched(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series: &RecurrenceSeries,
    occurrence: &RecurrenceOccurrence,
) -> Result<[String; 2]> {
    let task_id = occurrence
        .task_id
        .as_ref()
        .context("error recurrence-undo-successor-missing-task")?;
    let identity = derive_occurrence_identity(
        workspace_id,
        &series.id,
        &series.schedule(),
        occurrence.slot_on,
    )?;
    ensure!(
        task_id == &identity.task_id,
        "error recurrence-undo-successor-changed"
    );
    let generated = if generates_proposals(conn).await? {
        ensure_generation_untouched(conn, workspace_id, task_id).await?
    } else {
        let workspace = crate::workspaces::workspace_for_id(conn, workspace_id).await?;
        let labels = load_series_labels(conn, workspace_id, &series.id).await?;
        let metadata = load_series_metadata(conn, workspace_id, &series.id).await?;
        let slot = slot_values(&series.schedule(), occurrence.slot_on)?;
        verify_materialized_occurrence(
            conn, &workspace, series, &labels, &metadata, &slot, &identity, occurrence,
        )
        .await
        .map_err(|_| anyhow::anyhow!("error recurrence-undo-successor-touched"))?;
        [
            identity.task_change_id.clone(),
            identity.occurrence_change_id.clone(),
        ]
    };
    ensure_change_unsynced(conn, &generated[0]).await?;
    ensure_change_unsynced(conn, &generated[1]).await?;
    let extra_changes: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM changes
         WHERE entity_type = 'task' AND entity_id = ? AND change_id != ?",
    )
    .bind(task_id)
    .bind(&generated[0])
    .fetch_one(&mut *conn)
    .await?;
    let notes: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notes WHERE workspace_id = ? AND task_id = ?")
            .bind(workspace_id)
            .bind(task_id)
            .fetch_one(&mut *conn)
            .await?;
    let attachments: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM task_attachments WHERE workspace_id = ? AND task_id = ?",
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_one(&mut *conn)
    .await?;
    let dependencies: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM task_dependencies
         WHERE workspace_id = ? AND (task_id = ? OR depends_on_task_id = ?)",
    )
    .bind(workspace_id)
    .bind(task_id)
    .bind(task_id)
    .fetch_one(&mut *conn)
    .await?;
    let epics: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM task_epic_links
         WHERE workspace_id = ? AND (child_task_id = ? OR epic_task_id = ?)",
    )
    .bind(workspace_id)
    .bind(task_id)
    .bind(task_id)
    .fetch_one(&mut *conn)
    .await?;
    let related: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM task_related_links
         WHERE workspace_id = ? AND (task_a_id = ? OR task_b_id = ?)",
    )
    .bind(workspace_id)
    .bind(task_id)
    .bind(task_id)
    .fetch_one(&mut *conn)
    .await?;
    ensure!(
        extra_changes == 0
            && notes == 0
            && attachments == 0
            && dependencies == 0
            && epics == 0
            && related == 0,
        "error recurrence-undo-successor-touched"
    );
    Ok(generated)
}

/// A bound database's successor is untouched when this device's generation is its
/// only one and every field still carries that generation's seed and value.
async fn ensure_generation_untouched(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: &TaskId,
) -> Result<[String; 2]> {
    let creates = generated_creates(conn, task_id).await?;
    let [create] = creates.as_slice() else {
        anyhow::bail!("error recurrence-undo-successor-touched");
    };
    let workspace = crate::workspaces::workspace_for_id(conn, workspace_id).await?;
    let task = get_task_in_workspace(conn, &workspace, task_id).await?;
    for field in TaskField::VERSIONED {
        let version = entity_field_version(
            conn,
            workspace_id,
            MutableEntityType::Task,
            task_id.as_str(),
            field.as_str(),
        )
        .await?;
        ensure!(
            version.as_deref() == Some(create.seed())
                && field.current_value(&task) == create.default_value(field),
            "error recurrence-undo-successor-touched"
        );
    }
    let labels: Vec<String> = sqlx::query_scalar(
        "SELECT label FROM task_labels WHERE workspace_id = ? AND task_id = ? ORDER BY label",
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_all(&mut *conn)
    .await?;
    ensure!(
        serde_json::to_value(&labels)? == create.payload["labels"],
        "error recurrence-undo-successor-touched"
    );
    let metadata_versions: Vec<Option<String>> = sqlx::query_scalar(
        "SELECT fv.version FROM task_metadata m
         LEFT JOIN field_versions fv ON fv.workspace_id = m.workspace_id
           AND fv.entity_type = 'task' AND fv.entity_id = m.task_id
           AND fv.field = 'metadata:' || m.field_id
         WHERE m.workspace_id = ? AND m.task_id = ?",
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_all(&mut *conn)
    .await?;
    ensure!(
        metadata_versions.len() == create.payload["metadata"].as_array().map_or(0, Vec::len)
            && metadata_versions
                .iter()
                .all(|version| version.as_deref() == Some(create.seed())),
        "error recurrence-undo-successor-touched"
    );
    Ok([
        create.change_id.clone(),
        create.occurrence_change_id().to_owned(),
    ])
}

async fn remove_materialized_occurrence(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series: &RecurrenceSeries,
    occurrence: &RecurrenceOccurrence,
    generated: &[String; 2],
) -> Result<()> {
    let task_id = occurrence
        .task_id
        .as_ref()
        .expect("verified successor has task");
    crate::sync::shared_state::ensure_changes_not_local_capture_protected(
        conn,
        &[&generated[0], &generated[1]],
    )
    .await?;
    sqlx::query(
        "DELETE FROM recurrence_occurrences
         WHERE workspace_id = ? AND series_id = ? AND slot_on = ?",
    )
    .bind(workspace_id)
    .bind(&series.id)
    .bind(occurrence.slot_on.format("%Y-%m-%d").to_string())
    .execute(&mut *conn)
    .await?;
    sqlx::query("DELETE FROM task_labels WHERE workspace_id = ? AND task_id = ?")
        .bind(workspace_id)
        .bind(task_id)
        .execute(&mut *conn)
        .await?;
    sqlx::query(
        "DELETE FROM field_versions
         WHERE workspace_id = ? AND entity_type = 'task' AND entity_id = ?",
    )
    .bind(workspace_id)
    .bind(task_id)
    .execute(&mut *conn)
    .await?;
    sqlx::query("DELETE FROM tasks WHERE workspace_id = ? AND id = ?")
        .bind(workspace_id)
        .bind(task_id)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM changes WHERE change_id IN (?, ?)")
        .bind(&generated[0])
        .bind(&generated[1])
        .execute(&mut *conn)
        .await?;
    Ok(())
}
