use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, ensure};
use chrono::{DateTime, NaiveDate, NaiveTime};
use serde_json::Value;

use crate::choices::{TaskPriority, TaskStatus};
use crate::ids::{ProjectId, TaskId, WorkspaceId};
use crate::recurrence::{
    RecurrenceDuePolicy, RecurrenceFrequency, RecurrenceOutcome, RecurrenceProjectionState,
    RecurrenceRule, RecurrenceSchedule, RecurrenceSeriesState, TimeZoneId, WeekdaySet,
    derive_occurrence_identity, is_slot,
};
use crate::task_fields::TaskField;

use super::super::export_types::{AvenExport, ChangeRow, RecurrenceSeriesRow};

pub(in crate::data_safety) fn has_recurrence_data(export: &AvenExport) -> bool {
    !export.tables.recurrence_series.is_empty()
        || !export.tables.recurrence_series_labels.is_empty()
        || !export.tables.recurrence_series_metadata.is_empty()
        || !export.tables.recurrence_occurrences.is_empty()
        || !export.tables.recurrence_pause_intervals.is_empty()
}

// A stopped series retains its final projection, even when its slot is in the future.
// Only that occurrence may resolve after stopping, and it cannot generate a successor.
pub(in crate::data_safety) fn recurrence_stop_boundary_valid(
    stopped_at: &str,
    resolved_at: Option<&str>,
    archived_at: Option<&str>,
    retained_final: bool,
) -> bool {
    let Ok(stop) = DateTime::parse_from_rfc3339(stopped_at) else {
        return false;
    };
    [(resolved_at, retained_final), (archived_at, false)]
        .into_iter()
        .all(|(timestamp, allow_after_stop)| {
            timestamp.is_none_or(|value| {
                DateTime::parse_from_rfc3339(value).is_ok_and(|at| at <= stop || allow_after_stop)
            })
        })
}

pub(super) fn validate_recurrence_snapshot(
    export: &AvenExport,
    workspace_ids: &HashSet<WorkspaceId>,
    project_ids: &HashMap<WorkspaceId, HashSet<ProjectId>>,
    label_keys: &HashSet<(WorkspaceId, String)>,
    task_ids: &HashMap<WorkspaceId, HashSet<TaskId>>,
) -> Result<()> {
    struct ValidatedSeries<'a> {
        row: &'a RecurrenceSeriesRow,
        schedule: RecurrenceSchedule,
        state: RecurrenceSeriesState,
        created_at: DateTime<chrono::FixedOffset>,
        stopped_at: Option<DateTime<chrono::FixedOffset>>,
    }

    let task_rows = export
        .tables
        .tasks
        .iter()
        .map(|task| ((task.workspace_id.clone(), task.id.clone()), task))
        .collect::<HashMap<_, _>>();
    let mut change_rows = HashMap::new();
    for change in &export.tables.changes {
        ensure!(
            change_rows
                .insert(change.change_id.as_str(), change)
                .is_none(),
            "error invalid-export-snapshot change.change_id={} is duplicated",
            change.change_id
        );
    }
    let mut task_creates: HashMap<&str, Vec<&ChangeRow>> = HashMap::new();
    for change in &export.tables.changes {
        if change.entity_type == "task" && change.op_type == "create_task" {
            task_creates
                .entry(change.entity_id.as_str())
                .or_default()
                .push(change);
        }
    }
    let field_versions = export
        .tables
        .field_versions
        .iter()
        .map(|row| {
            (
                (
                    row.workspace_id.clone(),
                    row.entity_type.as_str(),
                    row.entity_id.as_str(),
                    row.field.as_str(),
                ),
                row.version.as_str(),
            )
        })
        .collect::<HashMap<_, _>>();

    let mut series_by_id = HashMap::new();
    for row in &export.tables.recurrence_series {
        ensure!(
            workspace_ids.contains(&row.workspace_id),
            "error invalid-export-snapshot recurrence_series.workspace_id={} is missing",
            row.workspace_id
        );
        ensure!(
            project_ids
                .get(&row.workspace_id)
                .is_some_and(|projects| projects.contains(&row.project_id)),
            "error invalid-export-snapshot recurrence_series.project_id={} is missing in workspace {}",
            row.project_id,
            row.workspace_id
        );
        TaskPriority::parse(&row.priority).context("invalid recurrence series priority")?;
        let initial_status =
            TaskStatus::parse(&row.initial_status).context("invalid recurrence initial status")?;
        ensure!(
            initial_status.is_open(),
            "error invalid-export-snapshot recurrence initial status must be open"
        );
        let frequency = RecurrenceFrequency::parse(&row.frequency)?;
        let interval = u32::try_from(row.interval).context("invalid recurrence interval")?;
        let weekdays = row
            .weekdays
            .parse::<WeekdaySet>()
            .map_err(anyhow::Error::msg)?;
        let rule = RecurrenceRule::new(frequency, interval, weekdays)?;
        let timezone = row.timezone.parse::<TimeZoneId>()?;
        let start_on = row
            .start_on
            .parse::<NaiveDate>()
            .context("invalid recurrence start date")?;
        let available_local_time = optional_import_text(&row.available_local_time)
            .map(|value| value.parse::<NaiveTime>())
            .transpose()
            .context("invalid recurrence availability time")?;
        let due_policy = RecurrenceDuePolicy::parse(&row.due_policy)?;
        let state = RecurrenceSeriesState::parse(&row.state)?;
        let stopped_at = optional_import_text(&row.stopped_at)
            .map(DateTime::parse_from_rfc3339)
            .transpose()
            .context("invalid recurrence stop time")?;
        ensure!(
            matches!(state, RecurrenceSeriesState::Stopped) == stopped_at.is_some(),
            "error invalid-export-snapshot recurrence stopped state and stop time disagree"
        );
        let created_at = DateTime::parse_from_rfc3339(&row.created_at)
            .context("invalid recurrence creation time")?;
        DateTime::parse_from_rfc3339(&row.updated_at).context("invalid recurrence update time")?;
        ensure!(
            matches!(row.deleted, 0 | 1),
            "error invalid-export-snapshot recurrence deleted value must be zero or one"
        );
        let key = (row.workspace_id.clone(), row.id.clone());
        ensure!(
            !series_by_id.contains_key(&key),
            "error invalid-export-snapshot recurrence series identity is duplicated"
        );
        series_by_id.insert(
            key,
            ValidatedSeries {
                row,
                schedule: RecurrenceSchedule::new(
                    rule,
                    timezone,
                    start_on,
                    available_local_time,
                    due_policy,
                ),
                state,
                created_at,
                stopped_at,
            },
        );
    }

    let mut series_label_keys = HashSet::new();
    for row in &export.tables.recurrence_series_labels {
        let series_key = (row.workspace_id.clone(), row.series_id.clone());
        ensure!(
            series_by_id.contains_key(&series_key),
            "error invalid-export-snapshot recurrence series label has no series"
        );
        ensure!(
            label_keys.contains(&(row.workspace_id.clone(), row.label.clone())),
            "error invalid-export-snapshot recurrence series label={} is missing",
            row.label
        );
        ensure!(
            series_label_keys.insert((row.workspace_id.clone(), row.series_id.clone(), &row.label)),
            "error invalid-export-snapshot recurrence series label is duplicated"
        );
    }

    let mut final_slots = HashMap::new();
    for row in &export.tables.recurrence_occurrences {
        let slot = row.slot_on.parse::<NaiveDate>()?;
        final_slots
            .entry((row.workspace_id.clone(), row.series_id.clone()))
            .and_modify(|last: &mut NaiveDate| *last = (*last).max(slot))
            .or_insert(slot);
    }
    let mut occurrence_keys = HashSet::new();
    let mut occurrence_tasks = HashSet::new();
    let mut projected_series = HashSet::new();
    for row in &export.tables.recurrence_occurrences {
        let series_key = (row.workspace_id.clone(), row.series_id.clone());
        let series = series_by_id
            .get(&series_key)
            .context("error invalid-export-snapshot recurrence occurrence has no series")?;
        let slot_on = row
            .slot_on
            .parse::<NaiveDate>()
            .context("invalid recurrence slot date")?;
        ensure!(
            occurrence_keys.insert((row.workspace_id.clone(), row.series_id.clone(), slot_on)),
            "error invalid-export-snapshot recurrence occurrence identity is duplicated"
        );
        ensure!(
            is_slot(&series.schedule.rule, series.schedule.start_on, slot_on),
            "error invalid-export-snapshot recurrence slot={} is outside the series lattice",
            slot_on
        );
        let creation_date = series
            .created_at
            .with_timezone(&series.schedule.timezone.timezone())
            .date_naive();
        ensure!(
            slot_on >= creation_date,
            "error invalid-export-snapshot recurrence slot={} precedes the series lifecycle",
            slot_on
        );
        let projection_state = RecurrenceProjectionState::parse(&row.projection_state)?;
        let outcome = optional_import_text(&row.outcome)
            .map(RecurrenceOutcome::parse)
            .transpose()?;
        let task_id = optional_import_text(&row.task_id)
            .map(str::parse::<TaskId>)
            .transpose()?;
        let resolved_at = optional_import_text(&row.resolved_at);
        let outcome_change_id = optional_import_text(&row.outcome_change_id);
        let archived_at = optional_import_text(&row.archived_at);
        let valid_shape = match projection_state {
            RecurrenceProjectionState::Projected => {
                task_id.is_some()
                    && outcome.is_none()
                    && resolved_at.is_none()
                    && outcome_change_id.is_none()
                    && archived_at.is_none()
            }
            RecurrenceProjectionState::Resolved => {
                task_id.is_some()
                    && outcome.is_some()
                    && resolved_at.is_some()
                    && outcome_change_id.is_some()
                    && archived_at.is_none()
            }
            RecurrenceProjectionState::Archived => {
                task_id.is_some()
                    && outcome.is_none()
                    && resolved_at.is_none()
                    && outcome_change_id.is_none()
                    && archived_at.is_some()
            }
        };
        ensure!(
            valid_shape,
            "error invalid-export-snapshot recurrence occurrence state and fields disagree"
        );
        if matches!(projection_state, RecurrenceProjectionState::Projected) {
            ensure!(
                projected_series.insert(series_key.clone()),
                "error invalid-export-snapshot recurrence projection is not unique"
            );
        }
        for value in [resolved_at, archived_at].into_iter().flatten() {
            DateTime::parse_from_rfc3339(value)
                .context("invalid recurrence occurrence timestamp")?;
        }
        if let Some(stopped_at) = series.stopped_at {
            let no_successor = outcome_change_id
                .and_then(|id| change_rows.get(id))
                .and_then(|change| serde_json::from_str::<Value>(&change.payload).ok())
                .is_some_and(|payload| {
                    payload.get("successor_task_id").and_then(Value::as_str) == Some("")
                });
            ensure!(
                recurrence_stop_boundary_valid(
                    &stopped_at.to_rfc3339(),
                    resolved_at,
                    archived_at,
                    final_slots.get(&series_key) == Some(&slot_on) && no_successor,
                ),
                "error invalid-export-snapshot recurrence activity exceeds the stop boundary"
            );
        }

        if let Some(task_id) = task_id {
            ensure!(
                task_ids
                    .get(&row.workspace_id)
                    .is_some_and(|tasks| tasks.contains(&task_id)),
                "error invalid-export-snapshot recurrence task={} is missing",
                task_id
            );
            ensure!(
                occurrence_tasks.insert((row.workspace_id.clone(), task_id.clone())),
                "error invalid-export-snapshot recurrence task link is duplicated"
            );
            let identity = derive_occurrence_identity(
                &row.workspace_id,
                &row.series_id,
                &series.schedule,
                slot_on,
            )?;
            ensure!(
                identity.task_id == task_id,
                "error invalid-export-snapshot recurrence deterministic task identity mismatch slot={slot_on}"
            );
            let task = task_rows
                .get(&(row.workspace_id.clone(), task_id.clone()))
                .context("error invalid-export-snapshot recurrence task row is missing")?;
            ensure!(
                task.created_at == identity.created_at,
                "error invalid-export-snapshot recurrence deterministic task timestamp mismatch slot={slot_on}"
            );
            match outcome {
                Some(RecurrenceOutcome::Completed) => ensure!(
                    task.status == "done",
                    "error invalid-export-snapshot recurrence completed outcome requires done task"
                ),
                Some(RecurrenceOutcome::Skipped) => ensure!(
                    task.status == "canceled",
                    "error invalid-export-snapshot recurrence skipped outcome requires canceled task"
                ),
                None if matches!(projection_state, RecurrenceProjectionState::Projected) => {
                    ensure!(
                        TaskStatus::parse(&task.status)?.is_open(),
                        "error invalid-export-snapshot recurrence projected task must be open"
                    );
                }
                None => {}
            }
            // Every generation of the occurrence validates in its own form, and at
            // least one is linked to its projection.
            let mut seeds = Vec::new();
            let mut linked = false;
            for create in task_creates.get(task_id.as_str()).into_iter().flatten() {
                let payload: Value = serde_json::from_str(&create.payload)
                    .context("invalid recurrence deterministic change payload")?;
                if payload.get("series_id").is_none() {
                    continue;
                }
                let ids = validate_deterministic_change(Some(create), &identity, slot_on, false)?;
                if let Some(projection) = change_rows.get(ids.occurrence_change_id.as_str()) {
                    validate_deterministic_change(Some(projection), &identity, slot_on, true)?;
                    linked = true;
                }
                seeds.push(ids.task_field_version_seed);
            }
            ensure!(
                linked,
                "error invalid-export-snapshot recurrence deterministic change is missing"
            );
            for field in TaskField::VERSIONED {
                let version = field_versions
                    .get(&(
                        row.workspace_id.clone(),
                        "task",
                        task_id.as_str(),
                        field.as_str(),
                    ))
                    .context(
                        "error invalid-export-snapshot recurrence task field version is missing",
                    )?;
                if seeds.iter().any(|seed| seed == version) {
                    continue;
                }
                ensure!(
                    change_rows.contains_key(version),
                    "error invalid-export-snapshot recurrence task field version has no change"
                );
            }
        }

        if let Some(change_id) = outcome_change_id {
            let change = change_rows
                .get(change_id)
                .context("error invalid-export-snapshot recurrence outcome change is missing")?;
            ensure!(
                change.entity_type == "recurrence_series"
                    && change.entity_id == row.series_id.as_str()
                    && change.field.as_deref() == Some("outcome")
                    && change.op_type == "resolve_recurrence_occurrence",
                "error invalid-export-snapshot recurrence outcome change identity mismatch"
            );
            let payload: Value = serde_json::from_str(&change.payload)
                .context("invalid recurrence outcome change payload")?;
            ensure!(
                payload.get("slot_on").and_then(Value::as_str) == Some(row.slot_on.as_str())
                    && payload.get("outcome").and_then(Value::as_str)
                        == outcome.map(RecurrenceOutcome::as_str)
                    && payload.get("resolved_at").and_then(Value::as_str) == resolved_at,
                "error invalid-export-snapshot recurrence outcome change payload mismatch"
            );
        }
    }

    let mut pause_ids = HashSet::new();
    let mut pauses_by_series: HashMap<_, Vec<_>> = HashMap::new();
    for row in &export.tables.recurrence_pause_intervals {
        let series_key = (row.workspace_id.clone(), row.series_id.clone());
        let series = series_by_id
            .get(&series_key)
            .context("error invalid-export-snapshot recurrence pause has no series")?;
        ensure!(
            pause_ids.insert((row.workspace_id.clone(), row.id.as_str())),
            "error invalid-export-snapshot recurrence pause identity is duplicated"
        );
        let paused_at = DateTime::parse_from_rfc3339(&row.paused_at)
            .context("invalid recurrence pause time")?;
        let resumed_at = optional_import_text(&row.resumed_at)
            .map(DateTime::parse_from_rfc3339)
            .transpose()
            .context("invalid recurrence resume time")?;
        ensure!(
            resumed_at.is_none_or(|resumed| resumed > paused_at),
            "error invalid-export-snapshot recurrence pause interval is inverted"
        );
        ensure!(
            paused_at >= series.created_at,
            "error invalid-export-snapshot recurrence pause precedes the series lifecycle"
        );
        if let Some(stopped_at) = series.stopped_at {
            ensure!(
                paused_at <= stopped_at && resumed_at.is_none_or(|resumed| resumed <= stopped_at),
                "error invalid-export-snapshot recurrence pause exceeds the stop boundary"
            );
        }
        ensure!(
            resumed_at.is_some() != row.resolved_by_change_id.is_empty(),
            "error invalid-export-snapshot recurrence pause resolution fields disagree"
        );
        ensure!(
            row.suspended_slot_on.is_empty() == row.suspended_task_id.is_empty(),
            "error invalid-export-snapshot recurrence suspended task fields disagree"
        );
        if !row.suspended_slot_on.is_empty() {
            let slot_on = row.suspended_slot_on.parse::<NaiveDate>()?;
            let task_id = row.suspended_task_id.parse::<TaskId>()?;
            ensure!(
                occurrence_keys.contains(&(
                    row.workspace_id.clone(),
                    row.series_id.clone(),
                    slot_on
                )) && occurrence_tasks.contains(&(row.workspace_id.clone(), task_id)),
                "error invalid-export-snapshot recurrence suspended task link is missing"
            );
        }
        for change_id in [
            Some(row.created_by_change_id.as_str()),
            optional_import_text(&row.resolved_by_change_id),
        ]
        .into_iter()
        .flatten()
        {
            let change = change_rows
                .get(change_id)
                .context("error invalid-export-snapshot recurrence pause change is missing")?;
            ensure!(
                change.entity_type == "recurrence_series"
                    && change.entity_id == row.series_id.as_str(),
                "error invalid-export-snapshot recurrence pause change identity mismatch"
            );
        }
        pauses_by_series
            .entry(series_key)
            .or_default()
            .push((paused_at, resumed_at));
    }
    for pauses in pauses_by_series.values_mut() {
        pauses.sort_by_key(|(paused_at, _)| *paused_at);
        for pair in pauses.windows(2) {
            ensure!(
                pair[0].1.is_some_and(|resumed_at| resumed_at <= pair[1].0),
                "error invalid-export-snapshot recurrence pause intervals overlap"
            );
        }
    }

    for series in series_by_id.values() {
        let has_lifecycle_conflict = export.tables.conflicts.iter().any(|conflict| {
            conflict.workspace_id == series.row.workspace_id
                && conflict.entity_type == "recurrence_series"
                && conflict.entity_id == series.row.id.as_str()
                && conflict.field == "state"
                && conflict.resolved == 0
        });
        let has_open_pause = pauses_by_series
            .get(&(series.row.workspace_id.clone(), series.row.id.clone()))
            .is_some_and(|pauses| pauses.iter().any(|(_, resumed_at)| resumed_at.is_none()));
        if !has_lifecycle_conflict {
            ensure!(
                matches!(series.state, RecurrenceSeriesState::Paused) == has_open_pause,
                "error invalid-export-snapshot recurrence state and open pause disagree"
            );
        }
    }

    Ok(())
}

fn validate_deterministic_change(
    change: Option<&ChangeRow>,
    identity: &crate::recurrence::RecurrenceOccurrenceIdentity,
    slot_on: NaiveDate,
    projection: bool,
) -> Result<crate::recurrence::RecurrenceProposalIds> {
    let change = change
        .context("error invalid-export-snapshot recurrence deterministic change is missing")?;
    let (entity_type, entity_id, field, op_type, created_at) = if projection {
        (
            "recurrence_series",
            identity.occurrence_link.series_id.as_str(),
            Some("projection"),
            "project_recurrence_occurrence",
            identity.occurrence_link.projected_at.as_str(),
        )
    } else {
        (
            "task",
            identity.task_id.as_str(),
            None,
            "create_task",
            identity.created_at.as_str(),
        )
    };
    ensure!(
        change.entity_type == entity_type
            && change.entity_id == entity_id
            && change.field.as_deref() == field
            && change.op_type == op_type
            && change.created_at == created_at,
        "error invalid-export-snapshot recurrence deterministic change identity mismatch"
    );
    let payload: Value = serde_json::from_str(&change.payload)
        .context("invalid recurrence deterministic change payload")?;
    let ids = identity
        .stored_generation_ids(&payload, slot_on, projection)
        .context("error invalid-export-snapshot recurrence deterministic payload mismatch")?;
    let own_id = if projection {
        &ids.occurrence_change_id
    } else {
        &ids.task_change_id
    };
    ensure!(
        change.change_id == *own_id,
        "error invalid-export-snapshot recurrence deterministic change identity mismatch"
    );
    Ok(ids)
}

fn optional_import_text(value: &str) -> Option<&str> {
    (!value.is_empty()).then_some(value)
}
