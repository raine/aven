use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, ensure};
use chrono::{DateTime, NaiveDate, Utc};
use sqlx::{QueryBuilder, Row, Sqlite, SqliteConnection};

use crate::db::{recurrence_occurrence_from_row, recurrence_series_from_row};
use crate::ids::{TaskId, WorkspaceId};
use crate::recurrence::{
    RecurrenceOutcome, RecurrenceProjectionState, RecurrenceSchedule, RecurrenceSeriesId,
    RecurrenceSeriesState, live_slot_on, slot_values,
};
use crate::refs::DisplayRefContext;
use crate::types::{RecurrenceOccurrence, RecurrencePauseInterval, RecurrenceSeries};

use super::types::{
    RecurrenceCounts, RecurrenceHistoryEntry, RecurrenceHistoryKind, RecurrenceHistoryPage,
    RecurrenceReconciliation, RecurrenceSeriesDetail, RecurrenceSeriesSummary, RecurrenceTaskGroup,
    TaskListItem, TaskRecurrenceSummary,
};

const SQLITE_BIND_CHUNK_SIZE: usize = 900;
pub(crate) const REPORT_RECONCILE_LIMIT: usize = 256;

#[cfg(test)]
#[path = "recurrence_tests.rs"]
mod tests;

pub(crate) async fn recurrence_reconciliation_candidates(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
) -> Result<(Vec<RecurrenceSeriesId>, bool)> {
    let ids = sqlx::query_scalar::<_, RecurrenceSeriesId>(
        "SELECT id FROM recurrence_series
         WHERE workspace_id = ? AND deleted = 0 AND state = 'active'
         ORDER BY id LIMIT ?",
    )
    .bind(workspace_id)
    .bind((REPORT_RECONCILE_LIMIT + 1) as i64)
    .fetch_all(&mut *conn)
    .await?;
    let incomplete = ids.len() > REPORT_RECONCILE_LIMIT;
    Ok((
        ids.into_iter().take(REPORT_RECONCILE_LIMIT).collect(),
        incomplete,
    ))
}

pub(crate) async fn group_search_task_items(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    items: Vec<TaskListItem>,
    at: DateTime<Utc>,
) -> Result<Vec<TaskListItem>> {
    let series_ids = items
        .iter()
        .filter_map(|item| {
            item.recurrence
                .as_ref()
                .map(|value| value.series_id.clone())
        })
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if series_ids.is_empty() {
        return Ok(items);
    }
    let series = load_series_for_ids(conn, workspace_id, &series_ids).await?;
    let counts = recurrence_counts_batch(conn, workspace_id, &series, at).await?;
    let mut seen = HashSet::new();
    let mut grouped = Vec::with_capacity(items.len());
    for mut item in items {
        let Some(summary) = item.recurrence.as_ref() else {
            grouped.push(item);
            continue;
        };
        if !seen.insert(summary.series_id.clone()) {
            continue;
        }
        let mut group_counts = counts.get(&summary.series_id).cloned().unwrap_or_default();
        group_counts.series_ref = summary.series_ref.clone();
        item.recurrence_group = Some(RecurrenceTaskGroup {
            series_id: summary.series_id.clone(),
            series_ref: summary.series_ref.clone(),
            counts: group_counts,
        });
        grouped.push(item);
    }
    Ok(grouped)
}

pub(crate) async fn group_terminal_task_items(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    items: Vec<TaskListItem>,
    at: DateTime<Utc>,
) -> Result<Vec<TaskListItem>> {
    let series_ids = items
        .iter()
        .filter(|item| item.task.status.is_terminal())
        .filter_map(|item| {
            item.recurrence
                .as_ref()
                .map(|value| value.series_id.clone())
        })
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if series_ids.is_empty() {
        return Ok(items);
    }
    let series = load_series_for_ids(conn, workspace_id, &series_ids).await?;
    let counts = recurrence_counts_batch(conn, workspace_id, &series, at).await?;
    let mut seen = HashSet::new();
    let mut grouped = Vec::with_capacity(items.len());
    for mut item in items {
        let Some(summary) = item.recurrence.as_ref() else {
            grouped.push(item);
            continue;
        };
        if !item.task.status.is_terminal() {
            grouped.push(item);
            continue;
        }
        if !seen.insert(summary.series_id.clone()) {
            continue;
        }
        let mut group_counts = counts.get(&summary.series_id).cloned().unwrap_or_default();
        group_counts.series_ref = summary.series_ref.clone();
        item.recurrence_group = Some(RecurrenceTaskGroup {
            series_id: summary.series_id.clone(),
            series_ref: summary.series_ref.clone(),
            counts: group_counts,
        });
        grouped.push(item);
    }
    Ok(grouped)
}

async fn load_series_for_ids(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series_ids: &[RecurrenceSeriesId],
) -> Result<Vec<RecurrenceSeries>> {
    let mut series = Vec::new();
    for chunk in series_ids.chunks(SQLITE_BIND_CHUNK_SIZE) {
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT workspace_id, id, title, description, project_id, priority, initial_status,
                    frequency, interval, weekdays, timezone, start_on, available_local_time,
                    due_policy, state, stopped_at, created_at, updated_at, deleted
             FROM recurrence_series WHERE workspace_id = ",
        );
        query.push_bind(workspace_id);
        query.push(" AND id IN (");
        {
            let mut separated = query.separated(", ");
            for series_id in chunk {
                separated.push_bind(series_id);
            }
        }
        query.push(") ORDER BY id");
        for row in query.build().fetch_all(&mut *conn).await? {
            series.push(recurrence_series_from_row(&row)?);
        }
    }
    Ok(series)
}

pub(crate) async fn task_recurrence_summaries(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_ids: &[TaskId],
) -> Result<HashMap<TaskId, TaskRecurrenceSummary>> {
    let mut summaries = HashMap::new();
    if task_ids.is_empty() {
        return Ok(summaries);
    }
    let refs = SeriesRefContext::load(conn, workspace_id).await?;
    for chunk in task_ids.chunks(SQLITE_BIND_CHUNK_SIZE) {
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT o.task_id, o.series_id, o.slot_on, o.outcome, o.projection_state,
                    s.frequency, s.interval, s.weekdays, s.timezone, s.state
             FROM recurrence_occurrences o
             JOIN recurrence_series s
               ON s.workspace_id = o.workspace_id AND s.id = o.series_id
             WHERE o.workspace_id = ",
        );
        query.push_bind(workspace_id);
        query.push(" AND o.task_id IN (");
        {
            let mut separated = query.separated(", ");
            for task_id in chunk {
                separated.push_bind(task_id);
            }
        }
        query.push(")");
        for row in query.build().fetch_all(&mut *conn).await? {
            let task_id: TaskId = row.get("task_id");
            let series_id: RecurrenceSeriesId = row.get("series_id");
            let outcome_text: String = row.get("outcome");
            summaries.insert(
                task_id,
                TaskRecurrenceSummary {
                    series_ref: refs.display_ref(&series_id),
                    series_id,
                    slot_on: row.get("slot_on"),
                    rule_label: rule_label(
                        row.get("frequency"),
                        row.get::<i64, _>("interval") as u32,
                        row.get("weekdays"),
                    ),
                    timezone: row.get("timezone"),
                    lifecycle: RecurrenceSeriesState::parse(
                        row.get::<String, _>("state").as_str(),
                    )?,
                    outcome: if outcome_text.is_empty() {
                        None
                    } else {
                        Some(RecurrenceOutcome::parse(&outcome_text)?)
                    },
                    projection_state: RecurrenceProjectionState::parse(
                        row.get::<String, _>("projection_state").as_str(),
                    )?,
                },
            );
        }
    }
    Ok(summaries)
}

pub(crate) async fn list_recurrence_series(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    at: DateTime<Utc>,
) -> Result<Vec<RecurrenceSeriesSummary>> {
    let rows = sqlx::query(
        "SELECT workspace_id, id, title, description, project_id, priority, initial_status,
                frequency, interval, weekdays, timezone, start_on, available_local_time,
                due_policy, state, stopped_at, created_at, updated_at, deleted
         FROM recurrence_series WHERE workspace_id = ? AND deleted = 0
         ORDER BY title COLLATE NOCASE, id",
    )
    .bind(workspace_id)
    .fetch_all(&mut *conn)
    .await?;
    let series = rows
        .iter()
        .map(recurrence_series_from_row)
        .collect::<Result<Vec<_>>>()?;
    summaries_for_series(conn, workspace_id, series, at).await
}

pub(crate) async fn recurrence_series_detail(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series_id: &RecurrenceSeriesId,
    at: DateTime<Utc>,
) -> Result<RecurrenceSeriesDetail> {
    let row = sqlx::query(
        "SELECT workspace_id, id, title, description, project_id, priority, initial_status,
                frequency, interval, weekdays, timezone, start_on, available_local_time,
                due_policy, state, stopped_at, created_at, updated_at, deleted
         FROM recurrence_series WHERE workspace_id = ? AND id = ?",
    )
    .bind(workspace_id)
    .bind(series_id)
    .fetch_optional(&mut *conn)
    .await?
    .with_context(|| format!("error recurrence-series-not-found series_id={series_id}"))?;
    let series = recurrence_series_from_row(&row)?;
    let labels = sqlx::query_scalar(
        "SELECT label FROM recurrence_series_labels
         WHERE workspace_id = ? AND series_id = ? ORDER BY label",
    )
    .bind(workspace_id)
    .bind(series_id)
    .fetch_all(&mut *conn)
    .await?;
    let summary = summaries_for_series(conn, workspace_id, vec![series.clone()], at)
        .await?
        .pop()
        .expect("one series produces one summary");
    let current_occurrence =
        load_current_occurrences(conn, workspace_id, std::slice::from_ref(series_id))
            .await?
            .remove(series_id);
    Ok(RecurrenceSeriesDetail {
        series,
        labels,
        summary,
        current_occurrence,
    })
}

pub(crate) async fn recurrence_history(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series_id: &RecurrenceSeriesId,
    at: DateTime<Utc>,
    offset: usize,
    limit: usize,
) -> Result<RecurrenceHistoryPage> {
    let detail = recurrence_series_detail(conn, workspace_id, series_id, at).await?;
    let occurrences = load_occurrences(conn, workspace_id, series_id).await?;
    let pauses = load_pause_intervals(conn, workspace_id, series_id).await?;
    let display_refs = DisplayRefContext::for_workspace(conn, workspace_id).await?;
    let task_ids = occurrences
        .iter()
        .filter_map(|occurrence| occurrence.task_id.clone())
        .chain(
            pauses
                .iter()
                .filter_map(|pause| pause.suspended_task_id.clone()),
        )
        .collect::<Vec<_>>();
    let task_refs = load_task_display_refs(conn, workspace_id, &task_ids, &display_refs).await?;
    let mut entries = build_history_entries(&detail.series, &occurrences, &pauses, at, &task_refs)?;
    entries.sort_by(|left, right| {
        right
            .sort_key()
            .cmp(left.sort_key())
            .then_with(|| right.kind.order().cmp(&left.kind.order()))
    });
    let total = entries.len();
    let limit = limit.clamp(1, 500);
    let items = entries.into_iter().skip(offset).take(limit).collect();
    Ok(RecurrenceHistoryPage {
        series_ref: detail.summary.series_ref,
        items,
        offset,
        limit,
        total,
        has_more: offset.saturating_add(limit) < total,
    })
}

async fn summaries_for_series(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series: Vec<RecurrenceSeries>,
    at: DateTime<Utc>,
) -> Result<Vec<RecurrenceSeriesSummary>> {
    let ids = series
        .iter()
        .map(|item| item.id.clone())
        .collect::<Vec<_>>();
    let refs = SeriesRefContext::load(conn, workspace_id).await?;
    let current = load_current_occurrences(conn, workspace_id, &ids).await?;
    let display_refs = DisplayRefContext::for_workspace(conn, workspace_id).await?;
    let current_task_ids = current
        .values()
        .filter_map(|occurrence| occurrence.task_id.clone())
        .collect::<Vec<_>>();
    let task_refs =
        load_task_display_refs(conn, workspace_id, &current_task_ids, &display_refs).await?;
    let counts = recurrence_counts_batch(conn, workspace_id, &series, at).await?;
    Ok(series
        .into_iter()
        .map(|item| {
            let occurrence = current.get(&item.id);
            RecurrenceSeriesSummary {
                series_ref: refs.display_ref(&item.id),
                current_task_ref: occurrence
                    .and_then(|value| value.task_id.as_ref())
                    .and_then(|task_id| task_refs.get(task_id))
                    .cloned(),
                current_slot_on: occurrence.map(|value| value.slot_on.to_string()),
                counts: counts.get(&item.id).cloned().unwrap_or_default(),
                rule_label: rule_label(
                    item.rule.frequency().as_str().to_string(),
                    item.rule.interval(),
                    item.rule.weekdays_set().to_string(),
                ),
                series: item,
            }
        })
        .collect())
}

async fn recurrence_counts_batch(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series: &[RecurrenceSeries],
    at: DateTime<Utc>,
) -> Result<HashMap<RecurrenceSeriesId, RecurrenceCounts>> {
    let ids = series
        .iter()
        .map(|item| item.id.clone())
        .collect::<Vec<_>>();
    let occurrences = load_occurrences_for_series(conn, workspace_id, &ids).await?;
    let pauses = load_pauses_for_series(conn, workspace_id, &ids).await?;
    let empty_refs = HashMap::new();
    let mut result = HashMap::new();
    for item in series {
        let entries = build_history_entries(
            item,
            occurrences
                .get(&item.id)
                .map(Vec::as_slice)
                .unwrap_or_default(),
            pauses.get(&item.id).map(Vec::as_slice).unwrap_or_default(),
            at,
            &empty_refs,
        )?;
        result.insert(item.id.clone(), counts_from_entries(&entries));
    }
    Ok(result)
}

fn counts_from_entries(entries: &[RecurrenceHistoryEntry]) -> RecurrenceCounts {
    let mut counts = RecurrenceCounts::default();
    for entry in entries {
        match entry.kind {
            RecurrenceHistoryKind::Completed => counts.completed += 1,
            RecurrenceHistoryKind::Skipped => counts.skipped += 1,
            RecurrenceHistoryKind::Missed => counts.missed += 1,
            RecurrenceHistoryKind::Paused => counts.pause_intervals += 1,
        }
        if !matches!(entry.kind, RecurrenceHistoryKind::Paused)
            && counts
                .latest_slot_on
                .as_deref()
                .is_none_or(|slot| entry.slot_on.as_deref().is_some_and(|entry| entry > slot))
        {
            counts.latest_slot_on = entry.slot_on.clone();
            counts.latest_outcome = match entry.kind {
                RecurrenceHistoryKind::Completed => Some(RecurrenceOutcome::Completed),
                RecurrenceHistoryKind::Skipped => Some(RecurrenceOutcome::Skipped),
                RecurrenceHistoryKind::Missed | RecurrenceHistoryKind::Paused => None,
            };
        }
    }
    counts
}

fn build_history_entries(
    series: &RecurrenceSeries,
    occurrences: &[RecurrenceOccurrence],
    pauses: &[RecurrencePauseInterval],
    at: DateTime<Utc>,
    task_refs: &HashMap<TaskId, String>,
) -> Result<Vec<RecurrenceHistoryEntry>> {
    let schedule = schedule_for_series(series);
    let live_slot = live_slot_on(&series.rule, series.start_on, at, &series.timezone);
    let projected_slot = occurrences
        .iter()
        .find(|occurrence| {
            matches!(
                occurrence.projection_state,
                RecurrenceProjectionState::Projected
            )
        })
        .map(|occurrence| occurrence.slot_on);
    let current_slot = live_slot.max(projected_slot);
    let created_on = DateTime::parse_from_rfc3339(&series.created_at)?
        .with_timezone(&series.timezone.timezone())
        .date_naive()
        .max(series.start_on);
    let stop_at = series
        .stopped_at
        .as_deref()
        .map(DateTime::parse_from_rfc3339)
        .transpose()?;
    let stop_at_utc = stop_at.as_ref().map(|stop| {
        stop.with_timezone(&Utc)
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string()
    });
    let explicit_slots = occurrences
        .iter()
        .map(|occurrence| occurrence.slot_on)
        .collect::<HashSet<_>>();
    let mut entries = Vec::new();

    for occurrence in occurrences {
        if matches!(
            occurrence.projection_state,
            RecurrenceProjectionState::Projected
        ) {
            continue;
        }
        let paused = slot_is_paused(&schedule, occurrence.slot_on, pauses)?;
        let kind = match occurrence.outcome {
            Some(RecurrenceOutcome::Completed) => RecurrenceHistoryKind::Completed,
            Some(RecurrenceOutcome::Skipped) => RecurrenceHistoryKind::Skipped,
            None if matches!(
                occurrence.projection_state,
                RecurrenceProjectionState::Archived
            ) && !paused =>
            {
                RecurrenceHistoryKind::Missed
            }
            None => continue,
        };
        let task_ref = occurrence
            .task_id
            .as_ref()
            .and_then(|task_id| task_refs.get(task_id))
            .cloned();
        entries.push(RecurrenceHistoryEntry {
            kind,
            slot_on: Some(occurrence.slot_on.to_string()),
            interval_started_at: None,
            interval_ended_at: None,
            task_id: occurrence.task_id.clone(),
            task_ref,
            openable: occurrence.task_id.is_some(),
            corrected: matches!(
                occurrence.projection_state,
                RecurrenceProjectionState::Corrected
            ),
            archived_projection: matches!(
                occurrence.projection_state,
                RecurrenceProjectionState::Archived
            ),
            resolved_at: occurrence.resolved_at.clone(),
        });
    }

    if let Some(current_slot) = current_slot {
        for slot_on in schedule.slots_on_or_after(created_on) {
            if slot_on >= current_slot {
                break;
            }
            let boundary = slot_values(&schedule, slot_on)?.boundary_at;
            if stop_at_utc
                .as_deref()
                .is_some_and(|stop| boundary.as_str() >= stop)
            {
                break;
            }
            if explicit_slots.contains(&slot_on) || slot_is_paused(&schedule, slot_on, pauses)? {
                continue;
            }
            entries.push(RecurrenceHistoryEntry {
                kind: RecurrenceHistoryKind::Missed,
                slot_on: Some(slot_on.to_string()),
                interval_started_at: None,
                interval_ended_at: None,
                task_id: None,
                task_ref: None,
                openable: false,
                corrected: false,
                archived_projection: false,
                resolved_at: None,
            });
        }
    }

    entries.extend(pauses.iter().map(|pause| {
        RecurrenceHistoryEntry {
            kind: RecurrenceHistoryKind::Paused,
            slot_on: None,
            interval_started_at: Some(pause.paused_at.clone()),
            interval_ended_at: pause.resumed_at.clone(),
            task_id: pause.suspended_task_id.clone(),
            task_ref: pause
                .suspended_task_id
                .as_ref()
                .and_then(|task_id| task_refs.get(task_id))
                .cloned(),
            openable: pause.suspended_task_id.is_some(),
            corrected: false,
            archived_projection: false,
            resolved_at: None,
        }
    }));
    Ok(entries)
}

fn slot_is_paused(
    schedule: &RecurrenceSchedule,
    slot_on: NaiveDate,
    pauses: &[RecurrencePauseInterval],
) -> Result<bool> {
    let boundary = slot_values(schedule, slot_on)?.boundary_at;
    Ok(pauses.iter().any(|pause| {
        boundary >= pause.paused_at
            && pause
                .resumed_at
                .as_deref()
                .is_none_or(|resumed_at| boundary.as_str() < resumed_at)
    }))
}

async fn load_current_occurrences(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series_ids: &[RecurrenceSeriesId],
) -> Result<HashMap<RecurrenceSeriesId, RecurrenceOccurrence>> {
    let all = load_occurrences_for_series(conn, workspace_id, series_ids).await?;
    Ok(all
        .into_iter()
        .filter_map(|(series_id, occurrences)| {
            occurrences
                .into_iter()
                .find(|value| {
                    matches!(value.projection_state, RecurrenceProjectionState::Projected)
                })
                .map(|value| (series_id, value))
        })
        .collect())
}

async fn load_occurrences(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series_id: &RecurrenceSeriesId,
) -> Result<Vec<RecurrenceOccurrence>> {
    Ok(
        load_occurrences_for_series(conn, workspace_id, std::slice::from_ref(series_id))
            .await?
            .remove(series_id)
            .unwrap_or_default(),
    )
}

async fn load_occurrences_for_series(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series_ids: &[RecurrenceSeriesId],
) -> Result<HashMap<RecurrenceSeriesId, Vec<RecurrenceOccurrence>>> {
    let mut by_series = HashMap::new();
    if series_ids.is_empty() {
        return Ok(by_series);
    }
    for chunk in series_ids.chunks(SQLITE_BIND_CHUNK_SIZE) {
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT workspace_id, series_id, slot_on, task_id, outcome, resolved_at,
                    outcome_change_id, projection_state, archived_at
             FROM recurrence_occurrences WHERE workspace_id = ",
        );
        query.push_bind(workspace_id);
        query.push(" AND series_id IN (");
        {
            let mut separated = query.separated(", ");
            for series_id in chunk {
                separated.push_bind(series_id);
            }
        }
        query.push(") ORDER BY series_id, slot_on DESC");
        for row in query.build().fetch_all(&mut *conn).await? {
            let occurrence = recurrence_occurrence_from_row(&row)?;
            by_series
                .entry(occurrence.series_id.clone())
                .or_insert_with(Vec::new)
                .push(occurrence);
        }
    }
    Ok(by_series)
}

async fn load_pause_intervals(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series_id: &RecurrenceSeriesId,
) -> Result<Vec<RecurrencePauseInterval>> {
    Ok(
        load_pauses_for_series(conn, workspace_id, std::slice::from_ref(series_id))
            .await?
            .remove(series_id)
            .unwrap_or_default(),
    )
}

async fn load_pauses_for_series(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series_ids: &[RecurrenceSeriesId],
) -> Result<HashMap<RecurrenceSeriesId, Vec<RecurrencePauseInterval>>> {
    let mut by_series = HashMap::new();
    if series_ids.is_empty() {
        return Ok(by_series);
    }
    for chunk in series_ids.chunks(SQLITE_BIND_CHUNK_SIZE) {
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT workspace_id, id, series_id, paused_at, resumed_at, suspended_slot_on,
                    suspended_task_id, created_by_change_id, resolved_by_change_id
             FROM recurrence_pause_intervals WHERE workspace_id = ",
        );
        query.push_bind(workspace_id);
        query.push(" AND series_id IN (");
        {
            let mut separated = query.separated(", ");
            for series_id in chunk {
                separated.push_bind(series_id);
            }
        }
        query.push(") ORDER BY series_id, paused_at DESC");
        for row in query.build().fetch_all(&mut *conn).await? {
            let series_id: RecurrenceSeriesId = row.get("series_id");
            let resumed_at: String = row.get("resumed_at");
            let suspended_slot_on: String = row.get("suspended_slot_on");
            let suspended_task_id: String = row.get("suspended_task_id");
            let resolved_by_change_id: String = row.get("resolved_by_change_id");
            by_series
                .entry(series_id.clone())
                .or_insert_with(Vec::new)
                .push(RecurrencePauseInterval {
                    workspace_id: row.get("workspace_id"),
                    id: row.get("id"),
                    series_id,
                    paused_at: row.get("paused_at"),
                    resumed_at: (!resumed_at.is_empty()).then_some(resumed_at),
                    suspended_slot_on: (!suspended_slot_on.is_empty())
                        .then(|| suspended_slot_on.parse())
                        .transpose()?,
                    suspended_task_id: (!suspended_task_id.is_empty())
                        .then(|| suspended_task_id.parse())
                        .transpose()?,
                    created_by_change_id: row.get("created_by_change_id"),
                    resolved_by_change_id: (!resolved_by_change_id.is_empty())
                        .then_some(resolved_by_change_id),
                });
        }
    }
    Ok(by_series)
}

async fn load_task_display_refs(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_ids: &[TaskId],
    display_refs: &DisplayRefContext,
) -> Result<HashMap<TaskId, String>> {
    let mut refs = HashMap::new();
    for chunk in task_ids.chunks(SQLITE_BIND_CHUNK_SIZE) {
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT t.id, p.prefix
             FROM tasks t
             JOIN projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id
             WHERE t.workspace_id = ",
        );
        query.push_bind(workspace_id);
        query.push(" AND t.id IN (");
        {
            let mut separated = query.separated(", ");
            for task_id in chunk {
                separated.push_bind(task_id);
            }
        }
        query.push(")");
        for row in query.build().fetch_all(&mut *conn).await? {
            let task_id: TaskId = row.get("id");
            let prefix: String = row.get("prefix");
            refs.insert(
                task_id.clone(),
                display_refs.display_ref_for_id(workspace_id, &prefix, &task_id),
            );
        }
    }
    Ok(refs)
}

fn schedule_for_series(series: &RecurrenceSeries) -> RecurrenceSchedule {
    RecurrenceSchedule::new(
        series.rule,
        series.timezone.clone(),
        series.start_on,
        series.available_local_time,
        series.due_policy,
    )
}

fn rule_label(frequency: String, interval: u32, weekdays: String) -> String {
    match (frequency.as_str(), interval, weekdays.as_str()) {
        ("daily", _, _) => "daily".to_string(),
        ("weekly", 1, "mon,tue,wed,thu,fri") => "weekdays".to_string(),
        ("weekly", 1, days) => format!("weekly on {days}"),
        ("weekly", interval, days) => format!("every {interval} weeks on {days}"),
        _ => frequency,
    }
}

pub(crate) struct SeriesRefContext {
    ids: Vec<RecurrenceSeriesId>,
}

impl SeriesRefContext {
    pub(crate) async fn load(
        conn: &mut SqliteConnection,
        workspace_id: &WorkspaceId,
    ) -> Result<Self> {
        let ids = sqlx::query_scalar(
            "SELECT id FROM recurrence_series WHERE workspace_id = ? ORDER BY id",
        )
        .bind(workspace_id)
        .fetch_all(&mut *conn)
        .await?;
        Ok(Self { ids })
    }

    pub(crate) fn display_ref(&self, series_id: &RecurrenceSeriesId) -> String {
        let id = series_id.as_str();
        let shared = self
            .ids
            .iter()
            .filter(|candidate| candidate.as_str() != id)
            .map(|candidate| common_prefix_len(id, candidate.as_str()))
            .max()
            .unwrap_or(0);
        let length = 4.max(shared.saturating_add(1)).min(id.len());
        format!("RCR-{}", &id[..length])
    }
}

fn common_prefix_len(left: &str, right: &str) -> usize {
    left.bytes()
        .zip(right.bytes())
        .take_while(|(left, right)| left == right)
        .count()
}

pub(crate) fn validate_reconciliation(result: &RecurrenceReconciliation) -> Result<()> {
    ensure!(
        result.examined <= REPORT_RECONCILE_LIMIT,
        "recurrence report reconciliation exceeded its candidate bound"
    );
    Ok(())
}
