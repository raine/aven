use anyhow::{Context, Result, ensure};
use chrono::{DateTime, NaiveDate, Utc};
use serde_json::Value;
use sqlx::SqliteConnection;

use crate::change_log::{ChangePayload, op_type};
use crate::db::{
    IdentifiedChange, begin_immediate, entity_field_version, insert_change_with_identity,
    set_entity_field_version,
};
use crate::error::CoreError;
use crate::recurrence::{
    RecurrenceProjectionState, RecurrenceSeriesId, RecurrenceSeriesState,
    derive_occurrence_identity, projection_slot_at, slot_values,
};
use crate::refs::get_task_in_workspace;
use crate::task_fields::TaskField;
use crate::types::{MutableEntityType, RecurrenceOccurrence, RecurrenceSeries};
use crate::workspaces::Workspace;

use super::{
    RecurrenceReconcileOutcome, format_local_time, format_utc, lifecycle_conflict_exists,
    load_occurrence, load_projected_occurrence, load_series, load_series_labels,
    load_series_metadata,
};

pub(super) async fn reconcile_recurrence_series_once(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    series_id: &RecurrenceSeriesId,
    at: DateTime<Utc>,
) -> Result<RecurrenceReconcileOutcome> {
    let mut tx = begin_immediate(conn).await?;
    let outcome =
        reconcile_recurrence_series_in_transaction(&mut tx, workspace, series_id, at).await?;
    tx.commit().await?;
    Ok(outcome)
}

pub(crate) async fn reconcile_recurrence_series_in_transaction(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    series_id: &RecurrenceSeriesId,
    at: DateTime<Utc>,
) -> Result<RecurrenceReconcileOutcome> {
    let series = load_series(conn, &workspace.id, series_id).await?;
    let projected = load_projected_occurrence(conn, &workspace.id, series_id).await?;
    if !matches!(series.state, RecurrenceSeriesState::Active) {
        return Ok(RecurrenceReconcileOutcome {
            series,
            occurrence: projected,
            changed: false,
            lifecycle_blocked: false,
        });
    }
    if lifecycle_conflict_exists(conn, &workspace.id, series_id).await? {
        return Ok(RecurrenceReconcileOutcome {
            series,
            occurrence: projected,
            changed: false,
            lifecycle_blocked: true,
        });
    }

    let schedule = series.schedule();
    let target = projection_slot_at(&schedule, at)?;
    if projected
        .as_ref()
        .is_some_and(|occurrence| occurrence.slot_on >= target)
    {
        return Ok(RecurrenceReconcileOutcome {
            series,
            occurrence: projected,
            changed: false,
            lifecycle_blocked: false,
        });
    }

    let changed_at = format_utc(at);
    if let Some(projected) = projected {
        sqlx::query(
            "UPDATE recurrence_occurrences
             SET projection_state = 'archived', archived_at = ?
             WHERE workspace_id = ? AND series_id = ? AND slot_on = ?
             AND projection_state = 'projected'",
        )
        .bind(&changed_at)
        .bind(&workspace.id)
        .bind(series_id)
        .bind(projected.slot_on.format("%Y-%m-%d").to_string())
        .execute(&mut *conn)
        .await?;
    }
    if let Some(existing) = load_occurrence(conn, &workspace.id, series_id, target).await?
        && matches!(
            existing.projection_state,
            RecurrenceProjectionState::Archived
        )
        && existing.outcome.is_none()
    {
        let expected = derive_occurrence_identity(&workspace.id, series_id, &schedule, target)?;
        ensure!(
            existing.task_id.as_ref() == Some(&expected.task_id),
            CoreError::generation_conflict(format!(
                "error recurrence-generation-conflict slot={target} field=task_id"
            ))
        );
        let task = get_task_in_workspace(conn, workspace, &expected.task_id).await?;
        ensure!(
            task.status.is_open() && !task.deleted,
            CoreError::generation_conflict(format!(
                "error recurrence-generation-conflict slot={target} field=task"
            ))
        );
        sqlx::query(
            "UPDATE recurrence_occurrences
             SET projection_state = 'projected', archived_at = ''
             WHERE workspace_id = ? AND series_id = ? AND slot_on = ?
             AND projection_state = 'archived' AND outcome = ''",
        )
        .bind(&workspace.id)
        .bind(series_id)
        .bind(target.format("%Y-%m-%d").to_string())
        .execute(&mut *conn)
        .await?;
        let occurrence = load_occurrence(conn, &workspace.id, series_id, target)
            .await?
            .context("promoted recurrence occurrence missing")?;
        return Ok(RecurrenceReconcileOutcome {
            series,
            occurrence: Some(occurrence),
            changed: true,
            lifecycle_blocked: false,
        });
    }
    let labels = load_series_labels(conn, &workspace.id, series_id).await?;
    let occurrence = materialize_occurrence(conn, workspace, &series, &labels, target).await?;
    Ok(RecurrenceReconcileOutcome {
        series,
        occurrence: Some(occurrence),
        changed: true,
        lifecycle_blocked: false,
    })
}

pub(super) async fn materialize_occurrence(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    series: &RecurrenceSeries,
    labels: &[String],
    slot_on: NaiveDate,
) -> Result<RecurrenceOccurrence> {
    let schedule = series.schedule();
    let slot = slot_values(&schedule, slot_on)?;
    let identity = derive_occurrence_identity(&workspace.id, &series.id, &schedule, slot_on)?;
    let metadata = load_series_metadata(conn, &workspace.id, &series.id).await?;
    if let Some(existing) = load_occurrence(conn, &workspace.id, &series.id, slot_on).await? {
        verify_materialized_occurrence(
            conn, workspace, series, labels, &metadata, &slot, &identity, &existing,
        )
        .await?;
        return Ok(existing);
    }

    sqlx::query(
        "INSERT INTO tasks(
            workspace_id, id, title, description, project_id, status, priority,
            created_at, updated_at, queue_activity_at, available_at, due_on, deleted, is_epic
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, 0)",
    )
    .bind(&workspace.id)
    .bind(&identity.task_id)
    .bind(&series.title)
    .bind(&series.description)
    .bind(&series.project_id)
    .bind(series.initial_status.as_str())
    .bind(series.priority.as_str())
    .bind(&identity.created_at)
    .bind(&identity.updated_at)
    .bind(&identity.created_at)
    .bind(&slot.available_at)
    .bind(slot.due_on.as_deref().unwrap_or(""))
    .execute(&mut *conn)
    .await?;
    for label in labels {
        sqlx::query("INSERT INTO task_labels(workspace_id, task_id, label) VALUES (?, ?, ?)")
            .bind(&workspace.id)
            .bind(&identity.task_id)
            .bind(label)
            .execute(&mut *conn)
            .await?;
    }
    for value in &metadata {
        sqlx::query(
            "INSERT INTO task_metadata(
                 workspace_id, task_id, field_id, value, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&workspace.id)
        .bind(&identity.task_id)
        .bind(&value.field_id)
        .bind(&value.value)
        .bind(&identity.created_at)
        .bind(&identity.updated_at)
        .execute(&mut *conn)
        .await?;
    }
    sqlx::query(
        "INSERT INTO recurrence_occurrences(
            workspace_id, series_id, slot_on, task_id, outcome, resolved_at,
            outcome_change_id, projection_state, archived_at
         ) VALUES (?, ?, ?, ?, '', '', '', 'projected', '')",
    )
    .bind(&workspace.id)
    .bind(&series.id)
    .bind(slot_on.format("%Y-%m-%d").to_string())
    .bind(&identity.task_id)
    .execute(&mut *conn)
    .await?;

    let payload =
        deterministic_task_payload(workspace, series, labels, &metadata, &slot, &identity);
    insert_change_with_identity(
        conn,
        IdentifiedChange {
            change_id: &identity.task_change_id,
            entity_type: "task",
            entity_id: identity.task_id.as_str(),
            field: None,
            op_type: op_type::CREATE_TASK,
            payload,
            base_version: None,
            created_at: &identity.created_at,
        },
    )
    .await?;
    let occurrence_payload = ChangePayload::workspace(workspace)
        .set("series_id", series.id.as_str())
        .set("slot_on", slot_on.format("%Y-%m-%d").to_string())
        .set("task_id", identity.task_id.as_str())
        .set("projected_at", &identity.occurrence_link.projected_at)
        .set("task_change_id", &identity.task_change_id)
        .set("occurrence_change_id", &identity.occurrence_change_id)
        .set(
            "task_field_version_seed",
            &identity.field_version_seeds.task,
        )
        .set(
            "occurrence_field_version_seed",
            &identity.field_version_seeds.occurrence,
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
        .set("due_policy", series.due_policy.as_str())
        .into_value();
    insert_change_with_identity(
        conn,
        IdentifiedChange {
            change_id: &identity.occurrence_change_id,
            entity_type: "recurrence_series",
            entity_id: series.id.as_str(),
            field: Some("projection"),
            op_type: op_type::PROJECT_RECURRENCE_OCCURRENCE,
            payload: occurrence_payload,
            base_version: None,
            created_at: &identity.occurrence_link.projected_at,
        },
    )
    .await?;
    for field in TaskField::VERSIONED {
        set_entity_field_version(
            conn,
            &workspace.id,
            MutableEntityType::Task,
            identity.task_id.as_str(),
            field.as_str(),
            &identity.field_version_seeds.task,
        )
        .await?;
    }
    for value in &metadata {
        set_entity_field_version(
            conn,
            &workspace.id,
            MutableEntityType::Task,
            identity.task_id.as_str(),
            &format!("metadata:{}", value.field_id),
            &identity.field_version_seeds.task,
        )
        .await?;
    }
    load_occurrence(conn, &workspace.id, &series.id, slot_on)
        .await?
        .context("materialized recurrence occurrence missing")
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn verify_materialized_occurrence(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    series: &RecurrenceSeries,
    labels: &[String],
    metadata: &[crate::metadata::ResolvedMetadataValue],
    slot: &crate::recurrence::RecurrenceSlot,
    identity: &crate::recurrence::RecurrenceOccurrenceIdentity,
    occurrence: &RecurrenceOccurrence,
) -> Result<()> {
    ensure!(
        occurrence.task_id.as_ref() == Some(&identity.task_id),
        CoreError::generation_conflict(format!(
            "error recurrence-generation-conflict slot={} field=task_id",
            occurrence.slot_on
        ))
    );
    let task = get_task_in_workspace(conn, workspace, &identity.task_id).await?;
    ensure!(
        task.title == series.title
            && task.description == series.description
            && task.project_id == series.project_id
            && task.status == series.initial_status
            && task.priority == series.priority
            && task.created_at == identity.created_at
            && task.updated_at == identity.updated_at
            && task.queue_activity_at == identity.created_at
            && task.available_at.as_deref() == Some(slot.available_at.as_str())
            && task.due_on == slot.due_on
            && !task.deleted
            && !task.is_epic,
        CoreError::generation_conflict(format!(
            "error recurrence-generation-conflict slot={} field=task",
            occurrence.slot_on
        ))
    );
    let stored_labels: Vec<String> = sqlx::query_scalar(
        "SELECT label FROM task_labels WHERE workspace_id = ? AND task_id = ? ORDER BY label",
    )
    .bind(&workspace.id)
    .bind(&identity.task_id)
    .fetch_all(&mut *conn)
    .await?;
    ensure!(
        stored_labels == labels,
        CoreError::generation_conflict(format!(
            "error recurrence-generation-conflict slot={} field=labels",
            occurrence.slot_on
        ))
    );
    let stored_metadata: Vec<(crate::ids::MetadataFieldId, String)> = sqlx::query_as(
        "SELECT field_id, value FROM task_metadata
         WHERE workspace_id = ? AND task_id = ? ORDER BY field_id",
    )
    .bind(&workspace.id)
    .bind(&identity.task_id)
    .fetch_all(&mut *conn)
    .await?;
    let mut expected_metadata = metadata
        .iter()
        .map(|value| (value.field_id.clone(), value.value.clone()))
        .collect::<Vec<_>>();
    expected_metadata.sort_by(|left, right| left.0.as_str().cmp(right.0.as_str()));
    ensure!(
        stored_metadata == expected_metadata,
        CoreError::generation_conflict(format!(
            "error recurrence-generation-conflict slot={} field=metadata",
            occurrence.slot_on
        ))
    );
    for field in TaskField::VERSIONED {
        let version = entity_field_version(
            conn,
            &workspace.id,
            MutableEntityType::Task,
            identity.task_id.as_str(),
            field.as_str(),
        )
        .await?;
        ensure!(
            version.as_deref() == Some(identity.field_version_seeds.task.as_str()),
            CoreError::generation_conflict(format!(
                "error recurrence-generation-conflict slot={} field=field_versions",
                occurrence.slot_on
            ))
        );
    }
    for value in metadata {
        let version = entity_field_version(
            conn,
            &workspace.id,
            MutableEntityType::Task,
            identity.task_id.as_str(),
            &format!("metadata:{}", value.field_id),
        )
        .await?;
        ensure!(
            version.as_deref() == Some(identity.field_version_seeds.task.as_str()),
            CoreError::generation_conflict(format!(
                "error recurrence-generation-conflict slot={} field=metadata_versions",
                occurrence.slot_on
            ))
        );
    }
    let payload = deterministic_task_payload(workspace, series, labels, metadata, slot, identity);
    insert_change_with_identity(
        conn,
        IdentifiedChange {
            change_id: &identity.task_change_id,
            entity_type: "task",
            entity_id: identity.task_id.as_str(),
            field: None,
            op_type: op_type::CREATE_TASK,
            payload,
            base_version: None,
            created_at: &identity.created_at,
        },
    )
    .await?;
    Ok(())
}

fn deterministic_task_payload(
    workspace: &Workspace,
    series: &RecurrenceSeries,
    labels: &[String],
    metadata: &[crate::metadata::ResolvedMetadataValue],
    slot: &crate::recurrence::RecurrenceSlot,
    identity: &crate::recurrence::RecurrenceOccurrenceIdentity,
) -> Value {
    ChangePayload::workspace(workspace)
        .set("task_id", identity.task_id.as_str())
        .set("series_id", series.id.as_str())
        .set("slot_on", slot.scheduled_on.format("%Y-%m-%d").to_string())
        .set("title", &series.title)
        .set("description", &series.description)
        .set("project_id", series.project_id.as_str())
        .set("status", series.initial_status.as_str())
        .set("priority", series.priority.as_str())
        .set("available_at", &slot.available_at)
        .set("due_on", slot.due_on.as_deref().unwrap_or(""))
        .set("is_epic", "0")
        .set("labels", labels)
        .set("metadata", metadata)
        .set("created_at", &identity.created_at)
        .set("updated_at", &identity.updated_at)
        .set("task_change_id", &identity.task_change_id)
        .set("occurrence_change_id", &identity.occurrence_change_id)
        .set(
            "task_field_version_seed",
            &identity.field_version_seeds.task,
        )
        .set(
            "occurrence_field_version_seed",
            &identity.field_version_seeds.occurrence,
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
        .set("due_policy", series.due_policy.as_str())
        .into_value()
}

pub(super) fn retryable_reconcile_error(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<sqlx::Error>()
        .and_then(sqlx::Error::as_database_error)
        .and_then(|error| error.code())
        .is_some_and(|code| matches!(code.as_ref(), "5" | "6" | "19"))
}
