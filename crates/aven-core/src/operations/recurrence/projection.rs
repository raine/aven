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
    RecurrenceProjectionState, RecurrenceProposalIds, RecurrenceSeriesId, RecurrenceSeriesState,
    derive_occurrence_identity, derive_proposal_ids, projection_slot_at, slot_values,
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

    let mut payload =
        deterministic_task_payload(workspace, series, labels, &metadata, &slot, &identity);
    let ids = generation_ids(conn, &identity, &mut payload).await?;
    insert_change_with_identity(
        conn,
        IdentifiedChange {
            change_id: &ids.task_change_id,
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
        .set("task_change_id", &ids.task_change_id)
        .set("occurrence_change_id", &ids.occurrence_change_id)
        .set("task_field_version_seed", &ids.task_field_version_seed)
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
            change_id: &ids.occurrence_change_id,
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
            &ids.task_field_version_seed,
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
            &ids.task_field_version_seed,
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
    if generates_proposals(conn).await? {
        return verify_generated_task(conn, workspace, identity, occurrence.slot_on).await;
    }
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

/// Databases bound to encrypted sync, by seed opt-in or peer enrollment, generate
/// proposal-form records. Both markers are permanent.
pub(crate) async fn generates_proposals(conn: &mut SqliteConnection) -> Result<bool> {
    let bound: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM local_seed_source)
             OR EXISTS(SELECT 1 FROM local_peer_enrollment)",
    )
    .fetch_one(&mut *conn)
    .await?;
    #[cfg(any(test, feature = "test-support"))]
    let bound = bound
        && crate::db::get_meta(conn, crate::test_support::OCCURRENCE_FORM_GENERATION)
            .await?
            .is_none();
    Ok(bound)
}

/// One generated `create_task` history row of a task.
pub(crate) struct GeneratedCreate {
    pub(crate) change_id: String,
    pub(crate) payload: Value,
}

impl GeneratedCreate {
    pub(crate) fn seed(&self) -> &str {
        self.payload["task_field_version_seed"]
            .as_str()
            .unwrap_or_default()
    }

    pub(crate) fn occurrence_change_id(&self) -> &str {
        self.payload["occurrence_change_id"]
            .as_str()
            .unwrap_or_default()
    }

    /// The value this generation gives a versioned task field.
    pub(crate) fn default_value(&self, field: TaskField) -> &str {
        match field {
            TaskField::Project => self.payload["project_id"].as_str(),
            TaskField::Deleted => Some("0"),
            other => self.payload[other.as_str()].as_str(),
        }
        .unwrap_or_default()
    }
}

/// A generated task's `create_task` history rows in accepted order, then pending order.
pub(crate) async fn generated_creates(
    conn: &mut SqliteConnection,
    task_id: &crate::ids::TaskId,
) -> Result<Vec<GeneratedCreate>> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT change_id, payload FROM changes
         WHERE entity_type = 'task' AND entity_id = ? AND op_type = 'create_task'
           AND json_extract(payload, '$.series_id') IS NOT NULL
         ORDER BY server_seq IS NULL, server_seq, local_seq",
    )
    .bind(task_id)
    .fetch_all(&mut *conn)
    .await?;
    rows.into_iter()
        .map(|(change_id, payload)| {
            Ok(GeneratedCreate {
                change_id,
                payload: serde_json::from_str(&payload)?,
            })
        })
        .collect()
}

/// Checks a bound database's generated task against the generation whose seed it
/// still carries. Fields since edited explicitly, and tasks whose baseline came from
/// an installed snapshot, have no generated value to compare. No history is written.
async fn verify_generated_task(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    identity: &crate::recurrence::RecurrenceOccurrenceIdentity,
    slot_on: NaiveDate,
) -> Result<()> {
    let conflict = |field: &str| {
        CoreError::generation_conflict(format!(
            "error recurrence-generation-conflict slot={slot_on} field={field}"
        ))
    };
    let task = get_task_in_workspace(conn, workspace, &identity.task_id).await?;
    ensure!(task.created_at == identity.created_at, conflict("task"));
    let mut versions = Vec::new();
    for field in TaskField::VERSIONED {
        let version = entity_field_version(
            conn,
            &workspace.id,
            MutableEntityType::Task,
            identity.task_id.as_str(),
            field.as_str(),
        )
        .await?;
        versions.push((field, version));
    }
    for create in generated_creates(conn, &identity.task_id).await? {
        for (field, version) in &versions {
            let field = *field;
            if version.as_deref() == Some(create.seed()) {
                ensure!(
                    field.current_value(&task) == create.default_value(field),
                    conflict(field.as_str())
                );
            }
        }
    }
    Ok(())
}

/// Chooses this database's generation form and writes its identities into the
/// generated task payload. Databases bound to encrypted sync generate proposal-form
/// records; local-only databases keep occurrence-form records.
async fn generation_ids(
    conn: &mut SqliteConnection,
    identity: &crate::recurrence::RecurrenceOccurrenceIdentity,
    payload: &mut Value,
) -> Result<RecurrenceProposalIds> {
    let ids = if generates_proposals(conn).await? {
        derive_proposal_ids(payload)?
    } else {
        RecurrenceProposalIds {
            task_change_id: identity.task_change_id.clone(),
            occurrence_change_id: identity.occurrence_change_id.clone(),
            task_field_version_seed: identity.field_version_seeds.task.clone(),
        }
    };
    payload["task_change_id"] = ids.task_change_id.clone().into();
    payload["occurrence_change_id"] = ids.occurrence_change_id.clone().into();
    payload["task_field_version_seed"] = ids.task_field_version_seed.clone().into();
    Ok(ids)
}

pub(super) fn retryable_reconcile_error(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<sqlx::Error>()
        .and_then(sqlx::Error::as_database_error)
        .and_then(|error| error.code())
        .is_some_and(|code| matches!(code.as_ref(), "5" | "6" | "19"))
}
