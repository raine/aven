use anyhow::{Context, Result, bail, ensure};
use chrono::{NaiveDate, NaiveTime};
use serde_json::Value;
use sqlx::{Row, SqliteConnection};

use crate::choices::{TaskPriority, TaskStatus};
use crate::db::{entity_conflict_exists, entity_field_version, set_entity_field_version};
use crate::ids::{ProjectId, TaskId, WorkspaceId};
use crate::recurrence::{
    RecurrenceDuePolicy, RecurrenceFrequency, RecurrenceOutcome, RecurrenceRule,
    RecurrenceSchedule, RecurrenceSeriesId, RecurrenceSeriesState, TimeZoneId, WeekdaySet,
    derive_occurrence_identity, is_slot, slot_values,
};
use crate::sync::wire::ChangeWire;
use crate::task_fields::TaskField;
use crate::types::MutableEntityType;

use super::shared::str_payload;

const SERIES_FIELDS: &[&str] = &[
    "title",
    "description",
    "project",
    "priority",
    "initial_status",
    "labels",
    "available_local_time",
    "due_policy",
    "state",
    "stopped_at",
    "deleted",
];

pub(super) async fn create_series(conn: &mut SqliteConnection, change: &ChangeWire) -> Result<()> {
    let workspace_id = workspace_id(change)?;
    let series_id = series_id(change)?;
    let schedule = schedule(&change.payload)?;
    let title = str_payload(&change.payload, "title")?;
    let description = str_payload(&change.payload, "description")?;
    let project_id: ProjectId = str_payload(&change.payload, "project_id")?.parse()?;
    let priority = TaskPriority::parse(&str_payload(&change.payload, "priority")?)?;
    let initial_status = TaskStatus::parse(&str_payload(&change.payload, "initial_status")?)?;
    ensure!(
        initial_status.is_open(),
        "error invalid-sync-change recurrence-terminal-template"
    );
    let created_at = str_payload(&change.payload, "created_at")?;
    let updated_at = str_payload(&change.payload, "updated_at")?;
    let labels = labels(&change.payload)?;
    ensure_project_exists(conn, &workspace_id, &project_id).await?;

    let existing = sqlx::query(
        "SELECT title, description, project_id, priority, initial_status, frequency, interval,
                weekdays, timezone, start_on, available_local_time, due_policy, state,
                stopped_at, created_at, updated_at, deleted
         FROM recurrence_series WHERE workspace_id = ? AND id = ?",
    )
    .bind(&workspace_id)
    .bind(&series_id)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(row) = existing {
        let equal = row.get::<String, _>("title") == title
            && row.get::<String, _>("description") == description
            && row.get::<ProjectId, _>("project_id") == project_id
            && row.get::<String, _>("priority") == priority.as_str()
            && row.get::<String, _>("initial_status") == initial_status.as_str()
            && row.get::<String, _>("frequency") == schedule.rule.frequency().as_str()
            && row.get::<i64, _>("interval") == i64::from(schedule.rule.interval())
            && row.get::<String, _>("weekdays") == schedule.rule.weekdays_set().to_string()
            && row.get::<String, _>("timezone") == schedule.timezone.as_str()
            && row.get::<String, _>("start_on") == schedule.start_on.to_string()
            && row.get::<String, _>("available_local_time")
                == format_time(schedule.available_local_time)
            && row.get::<String, _>("due_policy") == schedule.due_policy.as_str()
            && row.get::<String, _>("state") == "active"
            && row.get::<String, _>("stopped_at").is_empty()
            && row.get::<String, _>("created_at") == created_at
            && row.get::<String, _>("updated_at") == updated_at
            && row.get::<i64, _>("deleted") == 0
            && load_series_labels(conn, &workspace_id, &series_id).await? == labels;
        ensure!(
            equal,
            "error recurrence-generation-conflict series_id={series_id}"
        );
        return Ok(());
    }

    sqlx::query(
        "INSERT INTO recurrence_series(
            workspace_id, id, title, description, project_id, priority, initial_status,
            frequency, interval, weekdays, timezone, start_on, available_local_time,
            due_policy, state, stopped_at, created_at, updated_at, deleted
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'active', '', ?, ?, 0)",
    )
    .bind(&workspace_id)
    .bind(&series_id)
    .bind(title)
    .bind(description)
    .bind(project_id)
    .bind(priority.as_str())
    .bind(initial_status.as_str())
    .bind(schedule.rule.frequency().as_str())
    .bind(i64::from(schedule.rule.interval()))
    .bind(schedule.rule.weekdays_set().to_string())
    .bind(schedule.timezone.as_str())
    .bind(schedule.start_on.to_string())
    .bind(format_time(schedule.available_local_time))
    .bind(schedule.due_policy.as_str())
    .bind(created_at)
    .bind(updated_at)
    .execute(&mut *conn)
    .await?;
    replace_series_labels(conn, &workspace_id, &series_id, &labels).await?;
    for field in SERIES_FIELDS {
        set_entity_field_version(
            conn,
            &workspace_id,
            MutableEntityType::RecurrenceSeries,
            series_id.as_str(),
            field,
            &change.change_id,
        )
        .await?;
    }
    Ok(())
}

pub(super) async fn update_template(
    conn: &mut SqliteConnection,
    change: &ChangeWire,
) -> Result<()> {
    let workspace_id = workspace_id(change)?;
    let series_id = series_id(change)?;
    ensure_series_exists(conn, &workspace_id, &series_id).await?;
    let force = change
        .payload
        .get("conflict_resolution")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let fields = change
        .payload
        .get("fields")
        .and_then(Value::as_array)
        .context("payload missing fields")?;
    let updated_at = str_payload(&change.payload, "updated_at")?;
    for pair in fields {
        let pair = pair
            .as_array()
            .context("invalid recurrence template field pair")?;
        ensure!(
            pair.len() == 2,
            "error invalid-sync-change recurrence-template-field-pair"
        );
        let field = pair[0]
            .as_str()
            .context("invalid recurrence template field")?;
        let value = pair[1]
            .as_str()
            .context("invalid recurrence template value")?;
        ensure!(
            matches!(
                field,
                "title"
                    | "description"
                    | "project"
                    | "priority"
                    | "initial_status"
                    | "available_local_time"
                    | "due_policy"
            ),
            "error invalid-sync-change recurrence-template-field={field}"
        );
        let current = entity_field_version(
            conn,
            &workspace_id,
            MutableEntityType::RecurrenceSeries,
            series_id.as_str(),
            field,
        )
        .await?;
        let base_version = template_base_version(&change.payload, field)?;
        if !force
            && current != base_version
            && current.as_deref() != Some(change.change_id.as_str())
        {
            let local = series_field_value(conn, &workspace_id, &series_id, field).await?;
            create_series_conflict(
                conn,
                change,
                &workspace_id,
                &series_id,
                field,
                &local,
                value,
                current.as_deref(),
            )
            .await?;
            continue;
        }
        apply_series_field(conn, &workspace_id, &series_id, field, value, &updated_at).await?;
        set_entity_field_version(
            conn,
            &workspace_id,
            MutableEntityType::RecurrenceSeries,
            series_id.as_str(),
            field,
            &change.change_id,
        )
        .await?;
        if force {
            sqlx::query("UPDATE conflicts SET resolved = 1 WHERE workspace_id = ? AND entity_type = 'recurrence_series' AND entity_id = ? AND field = ? AND resolved = 0")
                .bind(&workspace_id).bind(&series_id).bind(field).execute(&mut *conn).await?;
        }
    }
    if change
        .payload
        .get("labels_changed")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let label_values = change
            .payload
            .get("labels")
            .context("invalid recurrence labels")?;
        let labels = label_values
            .as_array()
            .context("invalid recurrence labels")?
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_string)
                    .context("invalid recurrence label")
            })
            .collect::<Result<Vec<_>>>()?;
        let current = entity_field_version(
            conn,
            &workspace_id,
            MutableEntityType::RecurrenceSeries,
            series_id.as_str(),
            "labels",
        )
        .await?;
        let base_version = template_base_version(&change.payload, "labels")?;
        if force || current == base_version || current.as_deref() == Some(change.change_id.as_str())
        {
            replace_series_labels(conn, &workspace_id, &series_id, &labels).await?;
            set_entity_field_version(
                conn,
                &workspace_id,
                MutableEntityType::RecurrenceSeries,
                series_id.as_str(),
                "labels",
                &change.change_id,
            )
            .await?;
            if force {
                sqlx::query("UPDATE conflicts SET resolved = 1 WHERE workspace_id = ? AND entity_type = 'recurrence_series' AND entity_id = ? AND field = 'labels' AND resolved = 0")
                    .bind(&workspace_id).bind(&series_id).execute(&mut *conn).await?;
            }
        } else {
            let local =
                serde_json::to_string(&load_series_labels(conn, &workspace_id, &series_id).await?)?;
            let remote = serde_json::to_string(&labels)?;
            create_series_conflict(
                conn,
                change,
                &workspace_id,
                &series_id,
                "labels",
                &local,
                &remote,
                current.as_deref(),
            )
            .await?;
        }
    }
    Ok(())
}

pub(super) async fn project_occurrence(
    conn: &mut SqliteConnection,
    change: &ChangeWire,
) -> Result<()> {
    let workspace_id = workspace_id(change)?;
    let series_id = series_id(change)?;
    let slot_on = date_payload(&change.payload, "slot_on")?;
    let task_id: TaskId = str_payload(&change.payload, "task_id")?.parse()?;
    let schedule = schedule(&change.payload)?;
    ensure!(
        is_slot(&schedule.rule, schedule.start_on, slot_on),
        "error recurrence-slot-off-lattice slot={slot_on}"
    );
    verify_series_schedule(conn, &workspace_id, &series_id, &schedule).await?;
    let identity = derive_occurrence_identity(&workspace_id, &series_id, &schedule, slot_on)?;
    ensure!(
        identity.task_id == task_id,
        "error recurrence-generation-conflict slot={slot_on} field=task_id"
    );
    ensure!(
        str_payload(&change.payload, "projected_at")? == identity.occurrence_link.projected_at,
        "error recurrence-generation-conflict slot={slot_on} field=projected_at"
    );
    ensure!(
        str_payload(&change.payload, "task_change_id")? == identity.task_change_id,
        "error recurrence-generation-conflict slot={slot_on} field=task_change_id"
    );
    ensure!(
        str_payload(&change.payload, "occurrence_change_id")? == identity.occurrence_change_id,
        "error recurrence-generation-conflict slot={slot_on} field=occurrence_change_id"
    );
    ensure!(
        str_payload(&change.payload, "task_field_version_seed")?
            == identity.field_version_seeds.task,
        "error recurrence-generation-conflict slot={slot_on} field=task_field_version_seed"
    );
    ensure!(
        str_payload(&change.payload, "occurrence_field_version_seed")?
            == identity.field_version_seeds.occurrence,
        "error recurrence-generation-conflict slot={slot_on} field=occurrence_field_version_seed"
    );
    ensure_task_identity(conn, &workspace_id, &task_id, &identity).await?;

    if let Some(row) = sqlx::query(
        "SELECT task_id, projection_state, outcome, resolved_at, archived_at
         FROM recurrence_occurrences WHERE workspace_id = ? AND series_id = ? AND slot_on = ?",
    )
    .bind(&workspace_id)
    .bind(&series_id)
    .bind(slot_on.to_string())
    .fetch_optional(&mut *conn)
    .await?
    {
        ensure!(
            row.get::<TaskId, _>("task_id") == task_id,
            "error recurrence-generation-conflict slot={slot_on} field=occurrence-link"
        );
        return Ok(());
    }

    let archived_at = identity.occurrence_link.projected_at.clone();
    sqlx::query(
        "UPDATE recurrence_occurrences SET projection_state = 'archived', archived_at = ?
         WHERE workspace_id = ? AND series_id = ? AND projection_state = 'projected' AND slot_on < ?",
    )
    .bind(&archived_at)
    .bind(&workspace_id)
    .bind(&series_id)
    .bind(slot_on.to_string())
    .execute(&mut *conn)
    .await?;
    let newer: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM recurrence_occurrences
         WHERE workspace_id = ? AND series_id = ? AND projection_state = 'projected' AND slot_on > ?)",
    )
    .bind(&workspace_id)
    .bind(&series_id)
    .bind(slot_on.to_string())
    .fetch_one(&mut *conn)
    .await?;
    let (projection_state, archived_at) = if newer {
        ("archived", archived_at.as_str())
    } else {
        ("projected", "")
    };
    sqlx::query(
        "INSERT INTO recurrence_occurrences(
            workspace_id, series_id, slot_on, task_id, outcome, resolved_at,
            outcome_change_id, projection_state, archived_at
         ) VALUES (?, ?, ?, ?, '', '', '', ?, ?)",
    )
    .bind(&workspace_id)
    .bind(&series_id)
    .bind(slot_on.to_string())
    .bind(task_id)
    .bind(projection_state)
    .bind(archived_at)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

pub(super) async fn resolve_occurrence(
    conn: &mut SqliteConnection,
    change: &ChangeWire,
) -> Result<()> {
    apply_outcome(conn, change, true).await
}

pub(super) async fn record_outcome(conn: &mut SqliteConnection, change: &ChangeWire) -> Result<()> {
    apply_outcome(conn, change, false).await
}

async fn apply_outcome(
    conn: &mut SqliteConnection,
    change: &ChangeWire,
    task_backed: bool,
) -> Result<()> {
    let workspace_id = workspace_id(change)?;
    let series_id = series_id(change)?;
    let slot_on = date_payload(&change.payload, "slot_on")?;
    let outcome = RecurrenceOutcome::parse(&str_payload(&change.payload, "outcome")?)?;
    let resolved_at = str_payload(&change.payload, "resolved_at")?;
    let stored_schedule = load_schedule(conn, &workspace_id, &series_id).await?;
    ensure!(
        stored_schedule == schedule(&change.payload)?,
        "error recurrence-generation-conflict series_id={series_id} field=schedule"
    );
    let schedule = stored_schedule;
    ensure!(
        is_slot(&schedule.rule, schedule.start_on, slot_on),
        "error recurrence-slot-off-lattice slot={slot_on}"
    );
    ensure!(
        resolved_at >= slot_values(&schedule, slot_on)?.boundary_at,
        "error invalid-sync-change recurrence-resolution-before-slot"
    );
    let task_id = if task_backed {
        Some(str_payload(&change.payload, "task_id")?.parse::<TaskId>()?)
    } else {
        None
    };
    let existing = sqlx::query(
        "SELECT task_id, outcome, resolved_at, outcome_change_id, projection_state
         FROM recurrence_occurrences WHERE workspace_id = ? AND series_id = ? AND slot_on = ?",
    )
    .bind(&workspace_id)
    .bind(&series_id)
    .bind(slot_on.to_string())
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(row) = existing {
        let conflict_resolution = change
            .payload
            .get("conflict_resolution")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let local_outcome = row.get::<String, _>("outcome");
        if conflict_resolution {
            let projection_state = if row.get::<String, _>("task_id").is_empty() {
                "corrected"
            } else {
                "resolved"
            };
            update_outcome(
                conn,
                &workspace_id,
                &series_id,
                slot_on,
                outcome,
                &resolved_at,
                &change.change_id,
                projection_state,
            )
            .await?;
            if let Some(task_id) = task_id.as_ref() {
                let status = match outcome {
                    RecurrenceOutcome::Completed => "done",
                    RecurrenceOutcome::Skipped => "canceled",
                };
                sqlx::query("UPDATE tasks SET status = ?, updated_at = ?, queue_activity_at = ? WHERE workspace_id = ? AND id = ?")
                    .bind(status).bind(&resolved_at).bind(&resolved_at).bind(&workspace_id).bind(task_id).execute(&mut *conn).await?;
                set_entity_field_version(
                    conn,
                    &workspace_id,
                    MutableEntityType::Task,
                    task_id.as_str(),
                    "status",
                    &change.change_id,
                )
                .await?;
            }
            sqlx::query("UPDATE conflicts SET resolved = 1 WHERE workspace_id = ? AND entity_type = 'recurrence_series' AND entity_id = ? AND field = ? AND resolved = 0")
                .bind(&workspace_id).bind(&series_id).bind(outcome_field(slot_on)).execute(&mut *conn).await?;
            return Ok(());
        }
        if local_outcome.is_empty() {
            ensure!(
                task_backed
                    && row.get::<String, _>("task_id")
                        == task_id.as_ref().map(TaskId::as_str).unwrap_or(""),
                "error recurrence-outcome-task-mismatch"
            );
            update_outcome(
                conn,
                &workspace_id,
                &series_id,
                slot_on,
                outcome,
                &resolved_at,
                &change.change_id,
                "resolved",
            )
            .await?;
            ensure_task_outcome(conn, &workspace_id, task_id.as_ref(), outcome).await?;
            return Ok(());
        }
        let local = RecurrenceOutcome::parse(&local_outcome)?;
        if local == outcome {
            if outcome == RecurrenceOutcome::Completed {
                let current_at = row.get::<String, _>("resolved_at");
                if resolved_at < current_at {
                    sqlx::query(
                        "UPDATE recurrence_occurrences SET resolved_at = ?
                         WHERE workspace_id = ? AND series_id = ? AND slot_on = ?",
                    )
                    .bind(&resolved_at)
                    .bind(&workspace_id)
                    .bind(&series_id)
                    .bind(slot_on.to_string())
                    .execute(&mut *conn)
                    .await?;
                }
            }
            ensure_task_outcome(conn, &workspace_id, task_id.as_ref(), outcome).await?;
            return Ok(());
        }
        create_series_conflict(
            conn,
            change,
            &workspace_id,
            &series_id,
            &outcome_field(slot_on),
            local.as_str(),
            outcome.as_str(),
            Some(row.get::<String, _>("outcome_change_id").as_str()),
        )
        .await?;
        return Ok(());
    }
    ensure!(
        !task_backed,
        "error recurrence-occurrence-not-found slot={slot_on}"
    );
    sqlx::query(
        "INSERT INTO recurrence_occurrences(
            workspace_id, series_id, slot_on, task_id, outcome, resolved_at,
            outcome_change_id, projection_state, archived_at
         ) VALUES (?, ?, ?, '', ?, ?, ?, 'corrected', '')",
    )
    .bind(&workspace_id)
    .bind(&series_id)
    .bind(slot_on.to_string())
    .bind(outcome.as_str())
    .bind(resolved_at)
    .bind(&change.change_id)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

pub(super) async fn set_state(conn: &mut SqliteConnection, change: &ChangeWire) -> Result<()> {
    let workspace_id = workspace_id(change)?;
    let series_id = series_id(change)?;
    let remote = RecurrenceSeriesState::parse(&str_payload(&change.payload, "state")?)?;
    let stopped_at = str_payload(&change.payload, "stopped_at")?;
    let changed_at = str_payload(&change.payload, "changed_at")?;
    ensure!(
        matches!(remote, RecurrenceSeriesState::Stopped) != stopped_at.is_empty(),
        "error invalid-sync-change recurrence-stop-state-mismatch"
    );
    let row = sqlx::query(
        "SELECT state, stopped_at FROM recurrence_series WHERE workspace_id = ? AND id = ?",
    )
    .bind(&workspace_id)
    .bind(&series_id)
    .fetch_one(&mut *conn)
    .await?;
    let local = RecurrenceSeriesState::parse(&row.get::<String, _>("state"))?;
    let current = entity_field_version(
        conn,
        &workspace_id,
        MutableEntityType::RecurrenceSeries,
        series_id.as_str(),
        "state",
    )
    .await?;
    let conflict_resolution = change
        .payload
        .get("conflict_resolution")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !conflict_resolution
        && current != change.base_version
        && current.as_deref() != Some(change.change_id.as_str())
        && local == remote
    {
        return Ok(());
    }
    if !conflict_resolution
        && current != change.base_version
        && current.as_deref() != Some(change.change_id.as_str())
        && local != remote
    {
        create_series_conflict(
            conn,
            change,
            &workspace_id,
            &series_id,
            "state",
            local.as_str(),
            remote.as_str(),
            current.as_deref(),
        )
        .await?;
        return Ok(());
    }
    let local_stopped_at = row.get::<String, _>("stopped_at");
    let merged_stopped_at =
        if remote == RecurrenceSeriesState::Stopped && !local_stopped_at.is_empty() {
            local_stopped_at.min(stopped_at)
        } else {
            stopped_at
        };
    sqlx::query("UPDATE recurrence_series SET state = ?, stopped_at = ?, updated_at = ? WHERE workspace_id = ? AND id = ?")
        .bind(remote.as_str())
        .bind(&merged_stopped_at)
        .bind(changed_at)
        .bind(&workspace_id)
        .bind(&series_id)
        .execute(&mut *conn)
        .await?;
    set_entity_field_version(
        conn,
        &workspace_id,
        MutableEntityType::RecurrenceSeries,
        series_id.as_str(),
        "state",
        &change.change_id,
    )
    .await?;
    if remote == RecurrenceSeriesState::Stopped {
        set_entity_field_version(
            conn,
            &workspace_id,
            MutableEntityType::RecurrenceSeries,
            series_id.as_str(),
            "stopped_at",
            &change.change_id,
        )
        .await?;
    }
    if conflict_resolution {
        sqlx::query("UPDATE conflicts SET resolved = 1 WHERE workspace_id = ? AND entity_type = 'recurrence_series' AND entity_id = ? AND field = 'state' AND resolved = 0")
            .bind(&workspace_id).bind(&series_id).execute(&mut *conn).await?;
        let workspace = crate::workspaces::workspace_for_id(conn, &workspace_id).await?;
        crate::operations::conflicts::cleanup_recurrence_projections(
            conn,
            &workspace,
            &series_id,
            remote,
            &merged_stopped_at,
        )
        .await?;
    }
    Ok(())
}

pub(super) async fn open_pause(conn: &mut SqliteConnection, change: &ChangeWire) -> Result<()> {
    let workspace_id = workspace_id(change)?;
    let series_id = series_id(change)?;
    let interval_id = str_payload(&change.payload, "interval_id")?;
    let paused_at = str_payload(&change.payload, "paused_at")?;
    let suspended_slot_on = str_payload(&change.payload, "suspended_slot_on")?;
    let suspended_task_id = change
        .payload
        .get("suspended_task_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    if let Some(row) = sqlx::query("SELECT series_id, paused_at, suspended_slot_on, suspended_task_id, created_by_change_id FROM recurrence_pause_intervals WHERE workspace_id = ? AND id = ?")
        .bind(&workspace_id)
        .bind(&interval_id)
        .fetch_optional(&mut *conn)
        .await?
    {
        ensure!(row.get::<String, _>("series_id") == series_id.as_str()
            && row.get::<String, _>("paused_at") == paused_at
            && row.get::<String, _>("suspended_slot_on") == suspended_slot_on
            && row.get::<String, _>("suspended_task_id") == suspended_task_id,
            "error recurrence-generation-conflict interval_id={interval_id}");
        return Ok(());
    }
    let open_interval_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM recurrence_pause_intervals
         WHERE workspace_id = ? AND series_id = ? AND resumed_at = '')",
    )
    .bind(&workspace_id)
    .bind(&series_id)
    .fetch_one(&mut *conn)
    .await?;
    if open_interval_exists {
        return Ok(());
    }
    sqlx::query("INSERT INTO recurrence_pause_intervals(workspace_id, id, series_id, paused_at, resumed_at, suspended_slot_on, suspended_task_id, created_by_change_id, resolved_by_change_id) VALUES (?, ?, ?, ?, '', ?, ?, ?, '')")
        .bind(&workspace_id)
        .bind(interval_id)
        .bind(series_id)
        .bind(paused_at)
        .bind(suspended_slot_on)
        .bind(suspended_task_id)
        .bind(&change.change_id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

pub(super) async fn close_pause(conn: &mut SqliteConnection, change: &ChangeWire) -> Result<()> {
    let workspace_id = workspace_id(change)?;
    let series_id = series_id(change)?;
    let resumed_at = str_payload(&change.payload, "resumed_at")?;
    let interval_id = change.payload.get("interval_id").and_then(Value::as_str);
    let result = if let Some(interval_id) = interval_id {
        sqlx::query("UPDATE recurrence_pause_intervals SET resumed_at = ?, resolved_by_change_id = ? WHERE workspace_id = ? AND id = ? AND (resumed_at = '' OR resumed_at = ?)")
            .bind(&resumed_at).bind(&change.change_id).bind(&workspace_id).bind(interval_id).bind(&resumed_at).execute(&mut *conn).await?
    } else {
        sqlx::query("UPDATE recurrence_pause_intervals SET resumed_at = ?, resolved_by_change_id = ? WHERE workspace_id = ? AND series_id = ? AND resumed_at = ''")
            .bind(&resumed_at).bind(&change.change_id).bind(&workspace_id).bind(&series_id).execute(&mut *conn).await?
    };
    ensure!(
        result.rows_affected() <= 1,
        "error invalid-sync-change recurrence-pause-ordering"
    );
    Ok(())
}

pub(super) async fn suppress_recurrence_status_conflict(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: &TaskId,
    value: &str,
    versions_match: bool,
) -> Result<bool> {
    if versions_match || !matches!(value, "done" | "canceled") {
        return Ok(false);
    }
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM recurrence_occurrences WHERE workspace_id = ? AND task_id = ?)",
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_one(&mut *conn)
    .await?)
}

#[allow(clippy::too_many_arguments)]
async fn update_outcome(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series_id: &RecurrenceSeriesId,
    slot_on: NaiveDate,
    outcome: RecurrenceOutcome,
    resolved_at: &str,
    change_id: &str,
    projection_state: &str,
) -> Result<()> {
    sqlx::query("UPDATE recurrence_occurrences SET outcome = ?, resolved_at = ?, outcome_change_id = ?, projection_state = ?, archived_at = '' WHERE workspace_id = ? AND series_id = ? AND slot_on = ?")
        .bind(outcome.as_str()).bind(resolved_at).bind(change_id).bind(projection_state).bind(workspace_id).bind(series_id).bind(slot_on.to_string()).execute(&mut *conn).await?;
    Ok(())
}

async fn ensure_task_outcome(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: Option<&TaskId>,
    outcome: RecurrenceOutcome,
) -> Result<()> {
    let Some(task_id) = task_id else {
        return Ok(());
    };
    let status: String =
        sqlx::query_scalar("SELECT status FROM tasks WHERE workspace_id = ? AND id = ?")
            .bind(workspace_id)
            .bind(task_id)
            .fetch_one(&mut *conn)
            .await?;
    let expected = match outcome {
        RecurrenceOutcome::Completed => "done",
        RecurrenceOutcome::Skipped => "canceled",
    };
    ensure!(
        status == expected,
        "error recurrence-outcome-status-mismatch task_id={task_id}"
    );
    Ok(())
}

async fn ensure_task_identity(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: &TaskId,
    identity: &crate::recurrence::RecurrenceOccurrenceIdentity,
) -> Result<()> {
    let row =
        sqlx::query("SELECT created_at, updated_at FROM tasks WHERE workspace_id = ? AND id = ?")
            .bind(workspace_id)
            .bind(task_id)
            .fetch_one(&mut *conn)
            .await?;
    ensure!(
        row.get::<String, _>("created_at") == identity.created_at
            && row.get::<String, _>("updated_at") == identity.updated_at,
        "error recurrence-generation-conflict task_id={task_id} field=timestamps"
    );
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
            version.as_deref() == Some(identity.field_version_seeds.task.as_str()),
            "error recurrence-generation-conflict task_id={task_id} field=field_versions"
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn create_series_conflict(
    conn: &mut SqliteConnection,
    change: &ChangeWire,
    workspace_id: &WorkspaceId,
    series_id: &RecurrenceSeriesId,
    field: &str,
    local_value: &str,
    remote_value: &str,
    local_change_id: Option<&str>,
) -> Result<()> {
    if entity_conflict_exists(
        conn,
        workspace_id,
        MutableEntityType::RecurrenceSeries,
        series_id.as_str(),
        field,
    )
    .await?
    {
        return Ok(());
    }
    let variant_a = format!(
        "v{}",
        local_change_id
            .unwrap_or("local")
            .chars()
            .take(6)
            .collect::<String>()
    );
    let variant_b = format!("v{}", change.change_id.chars().take(6).collect::<String>());
    sqlx::query("INSERT OR IGNORE INTO conflicts(workspace_id, entity_type, entity_id, task_id, field, base_version, local_value, remote_value, local_change_id, remote_change_id, variant_a, variant_b, created_at) VALUES (?, 'recurrence_series', ?, '', ?, ?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(workspace_id).bind(series_id).bind(field).bind(&change.base_version).bind(local_value).bind(remote_value).bind(local_change_id).bind(&change.change_id).bind(variant_a).bind(variant_b).bind(&change.created_at).execute(&mut *conn).await?;
    Ok(())
}

fn template_base_version(payload: &Value, field: &str) -> Result<Option<String>> {
    match payload
        .get("base_versions")
        .and_then(Value::as_object)
        .and_then(|versions| versions.get(field))
    {
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(Value::Null) => Ok(None),
        _ => bail!("error invalid-sync-change recurrence-template-base-version field={field}"),
    }
}

fn workspace_id(change: &ChangeWire) -> Result<WorkspaceId> {
    str_payload(&change.payload, "workspace_id")?
        .parse()
        .context("invalid recurrence workspace ID")
}
fn series_id(change: &ChangeWire) -> Result<RecurrenceSeriesId> {
    change
        .entity_id
        .parse()
        .context("invalid recurrence series ID")
}
fn date_payload(payload: &Value, key: &str) -> Result<NaiveDate> {
    str_payload(payload, key)?
        .parse()
        .with_context(|| format!("invalid recurrence {key}"))
}
fn format_time(time: Option<NaiveTime>) -> String {
    time.map(|value| value.format("%H:%M:%S").to_string())
        .unwrap_or_default()
}
fn outcome_field(slot_on: NaiveDate) -> String {
    format!("outcome:{slot_on}")
}

fn schedule(payload: &Value) -> Result<RecurrenceSchedule> {
    let frequency = RecurrenceFrequency::parse(&str_payload(payload, "frequency")?)?;
    let interval = payload
        .get("interval")
        .and_then(Value::as_u64)
        .context("payload missing interval")?;
    let interval = u32::try_from(interval).context("recurrence interval exceeds u32")?;
    let weekdays = str_payload(payload, "weekdays")?
        .parse::<WeekdaySet>()
        .map_err(|err| anyhow::anyhow!(err))?;
    let rule = RecurrenceRule::new(frequency, interval, weekdays)?;
    let timezone: TimeZoneId = str_payload(payload, "timezone")?.parse()?;
    let start_on = date_payload(payload, "start_on")?;
    let available = str_payload(payload, "available_local_time")?;
    let available_local_time = if available.is_empty() {
        None
    } else {
        Some(NaiveTime::parse_from_str(&available, "%H:%M:%S")?)
    };
    let due_policy = RecurrenceDuePolicy::parse(&str_payload(payload, "due_policy")?)?;
    Ok(RecurrenceSchedule::new(
        rule,
        timezone,
        start_on,
        available_local_time,
        due_policy,
    ))
}

async fn load_schedule(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series_id: &RecurrenceSeriesId,
) -> Result<RecurrenceSchedule> {
    let row = sqlx::query("SELECT frequency, interval, weekdays, timezone, start_on, available_local_time, due_policy FROM recurrence_series WHERE workspace_id = ? AND id = ?").bind(workspace_id).bind(series_id).fetch_one(&mut *conn).await?;
    let payload = serde_json::json!({"frequency": row.get::<String, _>("frequency"), "interval": row.get::<i64, _>("interval"), "weekdays": row.get::<String, _>("weekdays"), "timezone": row.get::<String, _>("timezone"), "start_on": row.get::<String, _>("start_on"), "available_local_time": row.get::<String, _>("available_local_time"), "due_policy": row.get::<String, _>("due_policy")});
    schedule(&payload)
}

async fn verify_series_schedule(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series_id: &RecurrenceSeriesId,
    expected: &RecurrenceSchedule,
) -> Result<()> {
    ensure!(
        load_schedule(conn, workspace_id, series_id).await? == *expected,
        "error recurrence-generation-conflict series_id={series_id} field=schedule"
    );
    Ok(())
}
async fn ensure_series_exists(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series_id: &RecurrenceSeriesId,
) -> Result<()> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM recurrence_series WHERE workspace_id = ? AND id = ?)",
    )
    .bind(workspace_id)
    .bind(series_id)
    .fetch_one(&mut *conn)
    .await?;
    ensure!(
        exists,
        "error recurrence-series-not-found series_id={series_id}"
    );
    Ok(())
}
async fn ensure_project_exists(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    project_id: &ProjectId,
) -> Result<()> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM projects WHERE workspace_id = ? AND id = ?)",
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_one(&mut *conn)
    .await?;
    ensure!(
        exists,
        "error recurrence-project-not-found project_id={project_id}"
    );
    Ok(())
}
async fn load_series_labels(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series_id: &RecurrenceSeriesId,
) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar("SELECT label FROM recurrence_series_labels WHERE workspace_id = ? AND series_id = ? ORDER BY label").bind(workspace_id).bind(series_id).fetch_all(&mut *conn).await?)
}
fn labels(payload: &Value) -> Result<Vec<String>> {
    payload
        .get("labels")
        .and_then(Value::as_array)
        .context("payload missing labels")?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .context("invalid recurrence label")
        })
        .collect()
}
async fn replace_series_labels(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series_id: &RecurrenceSeriesId,
    labels: &[String],
) -> Result<()> {
    sqlx::query("DELETE FROM recurrence_series_labels WHERE workspace_id = ? AND series_id = ?")
        .bind(workspace_id)
        .bind(series_id)
        .execute(&mut *conn)
        .await?;
    for label in labels {
        sqlx::query("INSERT INTO labels(workspace_id, name, created_at) VALUES (?, ?, ?) ON CONFLICT(workspace_id, name) DO NOTHING").bind(workspace_id).bind(label).bind(crate::ids::now()).execute(&mut *conn).await?;
        sqlx::query(
            "INSERT INTO recurrence_series_labels(workspace_id, series_id, label) VALUES (?, ?, ?)",
        )
        .bind(workspace_id)
        .bind(series_id)
        .bind(label)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

async fn series_field_value(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series_id: &RecurrenceSeriesId,
    field: &str,
) -> Result<String> {
    let query = match field {
        "title" => "SELECT title FROM recurrence_series WHERE workspace_id = ? AND id = ?",
        "description" => {
            "SELECT description FROM recurrence_series WHERE workspace_id = ? AND id = ?"
        }
        "project" => "SELECT project_id FROM recurrence_series WHERE workspace_id = ? AND id = ?",
        "priority" => "SELECT priority FROM recurrence_series WHERE workspace_id = ? AND id = ?",
        "initial_status" => {
            "SELECT initial_status FROM recurrence_series WHERE workspace_id = ? AND id = ?"
        }
        "available_local_time" => {
            "SELECT available_local_time FROM recurrence_series WHERE workspace_id = ? AND id = ?"
        }
        "due_policy" => {
            "SELECT due_policy FROM recurrence_series WHERE workspace_id = ? AND id = ?"
        }
        "state" => "SELECT state FROM recurrence_series WHERE workspace_id = ? AND id = ?",
        "stopped_at" => {
            "SELECT stopped_at FROM recurrence_series WHERE workspace_id = ? AND id = ?"
        }
        _ => bail!("invalid recurrence series field: {field}"),
    };
    Ok(sqlx::query_scalar(query)
        .bind(workspace_id)
        .bind(series_id)
        .fetch_one(&mut *conn)
        .await?)
}

async fn apply_series_field(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series_id: &RecurrenceSeriesId,
    field: &str,
    value: &str,
    updated_at: &str,
) -> Result<()> {
    match field {
        "priority" => {
            TaskPriority::parse(value)?;
        }
        "initial_status" => {
            ensure!(
                TaskStatus::parse(value)?.is_open(),
                "error recurrence-initial-status-terminal"
            );
        }
        "available_local_time" if !value.is_empty() => {
            NaiveTime::parse_from_str(value, "%H:%M:%S")?;
        }
        "due_policy" => {
            RecurrenceDuePolicy::parse(value)?;
        }
        "project" => {
            let project: ProjectId = value.parse()?;
            ensure_project_exists(conn, workspace_id, &project).await?;
        }
        _ => {}
    }
    let query = match field {
        "title" => {
            "UPDATE recurrence_series SET title = ?, updated_at = ? WHERE workspace_id = ? AND id = ?"
        }
        "description" => {
            "UPDATE recurrence_series SET description = ?, updated_at = ? WHERE workspace_id = ? AND id = ?"
        }
        "project" => {
            "UPDATE recurrence_series SET project_id = ?, updated_at = ? WHERE workspace_id = ? AND id = ?"
        }
        "priority" => {
            "UPDATE recurrence_series SET priority = ?, updated_at = ? WHERE workspace_id = ? AND id = ?"
        }
        "initial_status" => {
            "UPDATE recurrence_series SET initial_status = ?, updated_at = ? WHERE workspace_id = ? AND id = ?"
        }
        "available_local_time" => {
            "UPDATE recurrence_series SET available_local_time = ?, updated_at = ? WHERE workspace_id = ? AND id = ?"
        }
        "due_policy" => {
            "UPDATE recurrence_series SET due_policy = ?, updated_at = ? WHERE workspace_id = ? AND id = ?"
        }
        _ => bail!("invalid recurrence template field: {field}"),
    };
    sqlx::query(query)
        .bind(value)
        .bind(updated_at)
        .bind(workspace_id)
        .bind(series_id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}
