use std::collections::{HashMap, HashSet, VecDeque};

use anyhow::{Context, Result, ensure};
use chrono::{DateTime, NaiveDate, Utc};
use sqlx::{QueryBuilder, Row, Sqlite, SqliteConnection};

use crate::db::{
    recurrence_occurrence_from_row, recurrence_pause_interval_from_row, recurrence_series_from_row,
};
use crate::error::CoreError;
use crate::ids::{TaskId, WorkspaceId};
use crate::recurrence::{
    RecurrenceOutcome, RecurrenceProjectionState, RecurrenceSchedule, RecurrenceSeriesId,
    RecurrenceSeriesState, live_slot_on, projection_slot_at, recurrence_series_display_ref,
    slot_at_rank, slot_rank, slot_values,
};
use crate::refs::DisplayRefContext;
use crate::types::{RecurrenceOccurrence, RecurrencePauseInterval, RecurrenceSeries};

use super::tasks::ConsumerTaskProjection;
use super::types::{
    RecurrenceCounts, RecurrenceHistoryEntry, RecurrenceHistoryKind, RecurrenceHistoryPage,
    RecurrenceOccurrenceLink, RecurrenceReconciliation, RecurrenceSeriesConflict,
    RecurrenceSeriesDetail, RecurrenceSeriesLifecycleFilter, RecurrenceSeriesListItem,
    RecurrenceSeriesListQuery, RecurrenceSeriesSummary, RecurrenceTaskGroup, TaskListItem,
    TaskRecurrenceSummary,
};
use super::validate_recurrence_history_limit;

const SQLITE_BIND_CHUNK_SIZE: usize = 900;
pub(crate) const REPORT_RECONCILE_LIMIT: usize = 256;

#[cfg(test)]
#[path = "recurrence_tests.rs"]
mod tests;

pub(crate) async fn recurrence_reconciliation_candidates(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    at: DateTime<Utc>,
) -> Result<(Vec<RecurrenceSeriesId>, bool)> {
    let rows = sqlx::query(
        "SELECT workspace_id, id, title, description, project_id, priority, initial_status,
                frequency, interval, weekdays, timezone, start_on, available_local_time,
                due_policy, state, stopped_at, created_at, updated_at, deleted
         FROM recurrence_series
         WHERE workspace_id = ? AND deleted = 0 AND state = 'active'
         ORDER BY id",
    )
    .bind(workspace_id)
    .fetch_all(&mut *conn)
    .await?;
    let series = rows
        .iter()
        .map(recurrence_series_from_row)
        .collect::<Result<Vec<_>>>()?;
    let series_ids = series
        .iter()
        .map(|series| series.id.clone())
        .collect::<Vec<_>>();
    let projected = load_projected_occurrences(conn, workspace_id, &series_ids).await?;
    let blocked = sqlx::query_scalar::<_, RecurrenceSeriesId>(
        "SELECT DISTINCT entity_id FROM conflicts
         WHERE workspace_id = ? AND entity_type = 'recurrence_series'
           AND field IN ('state', 'stopped_at') AND resolved = 0",
    )
    .bind(workspace_id)
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .collect::<HashSet<_>>();
    let mut candidates = Vec::new();
    let mut blocked_candidates = Vec::new();
    for series in series {
        let target = projection_slot_at(&series.schedule(), at)?;
        if projected
            .get(&series.id)
            .is_some_and(|occurrence| occurrence.slot_on >= target)
        {
            continue;
        }
        if blocked.contains(&series.id) {
            blocked_candidates.push(series.id);
        } else {
            candidates.push(series.id);
        }
    }
    candidates.extend(blocked_candidates);
    let incomplete = candidates.len() > REPORT_RECONCILE_LIMIT;
    candidates.truncate(REPORT_RECONCILE_LIMIT);
    Ok((candidates, incomplete))
}

pub(crate) async fn group_search_task_items(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    items: Vec<TaskListItem>,
    at: DateTime<Utc>,
) -> Result<Vec<TaskListItem>> {
    group_task_items(conn, workspace_id, items, at, |_| true).await
}

pub(crate) async fn group_terminal_task_items(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    items: Vec<TaskListItem>,
    at: DateTime<Utc>,
) -> Result<Vec<TaskListItem>> {
    group_task_items(conn, workspace_id, items, at, |item| {
        item.task.status.is_terminal()
    })
    .await
}

async fn group_task_items(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    items: Vec<TaskListItem>,
    at: DateTime<Utc>,
    include: fn(&TaskListItem) -> bool,
) -> Result<Vec<TaskListItem>> {
    let series_ids = items
        .iter()
        .filter(|item| include(item))
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
        if !include(&item) {
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
    if task_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let refs = SeriesRefContext::load(conn, workspace_id).await?;
    let rows = load_task_recurrence_rows(conn, workspace_id, task_ids).await?;
    task_recurrence_summaries_from_rows(rows, &refs)
}

pub(crate) async fn task_recurrence_summaries_for_consumer(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_ids: &[TaskId],
) -> Result<HashMap<TaskId, TaskRecurrenceSummary>> {
    if task_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = load_task_recurrence_rows(conn, workspace_id, task_ids).await?;
    let series_ids = rows
        .iter()
        .map(|row| row.series_id.clone())
        .collect::<HashSet<_>>();
    let refs = SeriesRefContext::for_series_ids(conn, workspace_id, &series_ids).await?;
    task_recurrence_summaries_from_rows(rows, &refs)
}

async fn load_task_recurrence_rows(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_ids: &[TaskId],
) -> Result<Vec<TaskRecurrenceRow>> {
    let mut rows = Vec::new();
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
            rows.push(TaskRecurrenceRow {
                task_id: row.get("task_id"),
                series_id: row.get("series_id"),
                slot_on: row.get("slot_on"),
                outcome: row.get("outcome"),
                projection_state: row.get("projection_state"),
                frequency: row.get("frequency"),
                interval: row.get::<i64, _>("interval") as u32,
                weekdays: row.get("weekdays"),
                timezone: row.get("timezone"),
                state: row.get("state"),
            });
        }
    }
    Ok(rows)
}

fn task_recurrence_summaries_from_rows(
    rows: Vec<TaskRecurrenceRow>,
    refs: &SeriesRefContext,
) -> Result<HashMap<TaskId, TaskRecurrenceSummary>> {
    let mut summaries = HashMap::with_capacity(rows.len());
    for row in rows {
        let outcome = if row.outcome.is_empty() {
            None
        } else {
            Some(RecurrenceOutcome::parse(&row.outcome)?)
        };
        summaries.insert(
            row.task_id,
            TaskRecurrenceSummary {
                series_ref: refs.display_ref(&row.series_id),
                series_id: row.series_id,
                slot_on: row.slot_on,
                rule_label: rule_label(row.frequency, row.interval, row.weekdays),
                timezone: row.timezone,
                lifecycle: RecurrenceSeriesState::parse(&row.state)?,
                outcome,
                projection_state: RecurrenceProjectionState::parse(&row.projection_state)?,
            },
        );
    }
    Ok(summaries)
}

struct TaskRecurrenceRow {
    task_id: TaskId,
    series_id: RecurrenceSeriesId,
    slot_on: String,
    outcome: String,
    projection_state: String,
    frequency: String,
    interval: u32,
    weekdays: String,
    timezone: String,
    state: String,
}

pub(crate) async fn terminal_recurrence_groups_for_tasks(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    tasks: &[ConsumerTaskProjection],
    recurrence: &HashMap<TaskId, TaskRecurrenceSummary>,
    at: DateTime<Utc>,
) -> Result<HashMap<TaskId, RecurrenceTaskGroup>> {
    let series_ids = tasks
        .iter()
        .filter(|task| task.status.is_terminal())
        .filter_map(|task| recurrence.get(&task.id))
        .map(|summary| summary.series_id.clone())
        .collect::<HashSet<_>>();
    if series_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let series_ids = series_ids.into_iter().collect::<Vec<_>>();
    let series = load_series_for_ids(conn, workspace_id, &series_ids).await?;
    let counts = recurrence_counts_batch(conn, workspace_id, &series, at).await?;
    let mut groups = HashMap::new();
    for task in tasks.iter().filter(|task| task.status.is_terminal()) {
        let Some(summary) = recurrence.get(&task.id) else {
            continue;
        };
        let mut group_counts = counts.get(&summary.series_id).cloned().unwrap_or_default();
        group_counts.series_ref.clone_from(&summary.series_ref);
        groups.insert(
            task.id.clone(),
            RecurrenceTaskGroup {
                series_id: summary.series_id.clone(),
                series_ref: summary.series_ref.clone(),
                counts: group_counts,
            },
        );
    }
    Ok(groups)
}

pub(crate) async fn list_recurrence_series_view(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    query: &RecurrenceSeriesListQuery,
) -> Result<Vec<RecurrenceSeriesListItem>> {
    let mut sql = QueryBuilder::<Sqlite>::new(
        "SELECT s.workspace_id, s.id, s.title, s.description, s.project_id, s.priority,
                s.initial_status, s.frequency, s.interval, s.weekdays, s.timezone, s.start_on,
                s.available_local_time, s.due_policy, s.state, s.stopped_at, s.created_at,
                s.updated_at, s.deleted, p.key AS project_key
         FROM recurrence_series s
         JOIN projects p ON p.workspace_id = s.workspace_id AND p.id = s.project_id",
    );
    sql.push(" WHERE s.workspace_id = ");
    sql.push_bind(workspace_id);
    sql.push(" AND s.deleted = 0");
    match query.lifecycle {
        RecurrenceSeriesLifecycleFilter::ActiveOrPaused => {
            sql.push(" AND s.state IN ('active', 'paused')");
        }
        RecurrenceSeriesLifecycleFilter::Active => {
            sql.push(" AND s.state = 'active'");
        }
        RecurrenceSeriesLifecycleFilter::Paused => {
            sql.push(" AND s.state = 'paused'");
        }
        RecurrenceSeriesLifecycleFilter::Stopped => {
            sql.push(" AND s.state = 'stopped'");
        }
        RecurrenceSeriesLifecycleFilter::All => {}
    }
    if let Some(project) = query.project.as_deref() {
        sql.push(" AND p.key = ");
        sql.push_bind(project);
    }
    sql.push(" ORDER BY s.title COLLATE NOCASE, s.id");
    let series = sql
        .build()
        .fetch_all(&mut *conn)
        .await?
        .iter()
        .map(|row| {
            Ok((
                recurrence_series_from_row(row)?,
                row.get::<String, _>("project_key"),
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let ids = series
        .iter()
        .map(|(item, _)| item.id.clone())
        .collect::<Vec<_>>();
    let refs = SeriesRefContext::load(conn, workspace_id).await?;
    let current = load_projected_occurrences(conn, workspace_id, &ids).await?;
    let display_refs = DisplayRefContext::for_workspace(conn, workspace_id).await?;
    let current_task_ids = current
        .values()
        .filter_map(|occurrence| occurrence.task_id.clone())
        .collect::<Vec<_>>();
    let task_refs =
        load_task_display_refs(conn, workspace_id, &current_task_ids, &display_refs).await?;
    for occurrence in current.values() {
        let task_id = occurrence.task_id.as_ref().with_context(|| {
            format!(
                "projected recurrence occurrence has no task series_id={}",
                occurrence.series_id
            )
        })?;
        ensure!(
            task_refs.contains_key(task_id),
            "projected recurrence task is missing series_id={} task_id={task_id}",
            occurrence.series_id
        );
    }
    let search = query
        .search
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_lowercase);

    Ok(series
        .into_iter()
        .filter_map(|(series, project_key)| {
            let series_ref = refs.display_ref(&series.id);
            if search.as_deref().is_some_and(|needle| {
                !series.title.to_lowercase().contains(needle)
                    && !series_ref.to_lowercase().contains(needle)
            }) {
                return None;
            }
            let current_occurrence = current.get(&series.id).and_then(|occurrence| {
                let task_id = occurrence.task_id.as_ref()?;
                Some(RecurrenceOccurrenceLink {
                    slot_on: occurrence.slot_on.to_string(),
                    task_id: task_id.clone(),
                    task_ref: task_refs.get(task_id)?.clone(),
                })
            });
            Some(RecurrenceSeriesListItem {
                rule_label: rule_label(
                    series.rule.frequency().as_str().to_string(),
                    series.rule.interval(),
                    series.rule.weekdays_set().to_string(),
                ),
                series,
                series_ref,
                project_key,
                current_occurrence,
            })
        })
        .collect())
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
    .ok_or_else(|| {
        CoreError::not_found(format!(
            "error recurrence-series-not-found series_id={series_id}"
        ))
    })?;
    let series = recurrence_series_from_row(&row)?;
    let labels = sqlx::query_scalar(
        "SELECT label FROM recurrence_series_labels
         WHERE workspace_id = ? AND series_id = ? ORDER BY label",
    )
    .bind(workspace_id)
    .bind(series_id)
    .fetch_all(&mut *conn)
    .await?;
    let metadata = sqlx::query(
        "SELECT m.field_id, f.key, m.value
         FROM recurrence_series_metadata m
         JOIN metadata_fields f
           ON f.workspace_id = m.workspace_id AND f.id = m.field_id
         WHERE m.workspace_id = ? AND m.series_id = ?
         ORDER BY f.key",
    )
    .bind(workspace_id)
    .bind(series_id)
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|row| crate::metadata::TaskMetadataValue {
        field_id: row.get("field_id"),
        key: row.get("key"),
        value: row.get("value"),
    })
    .collect();
    let summary = summaries_for_series(conn, workspace_id, vec![series.clone()], at)
        .await?
        .pop()
        .expect("one series produces one summary");
    let current_occurrence =
        load_projected_occurrences(conn, workspace_id, std::slice::from_ref(series_id))
            .await?
            .remove(series_id);
    let lifecycle_conflicts = sqlx::query(
        "SELECT field, variant_a, local_value, variant_b, remote_value
         FROM conflicts
         WHERE workspace_id = ? AND entity_type = 'recurrence_series'
           AND entity_id = ? AND field IN ('state', 'stopped_at') AND resolved = 0
         ORDER BY field, id",
    )
    .bind(workspace_id)
    .bind(series_id)
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|row| RecurrenceSeriesConflict {
        field: row.get("field"),
        variant_a: row.get("variant_a"),
        local_value: row.get("local_value"),
        variant_b: row.get("variant_b"),
        remote_value: row.get("remote_value"),
    })
    .collect();
    Ok(RecurrenceSeriesDetail {
        series,
        labels,
        metadata,
        summary,
        current_occurrence,
        lifecycle_conflicts,
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
    validate_recurrence_history_limit(limit)?;
    let series = load_series_for_ids(conn, workspace_id, std::slice::from_ref(series_id))
        .await?
        .pop()
        .ok_or_else(|| {
            CoreError::not_found(format!(
                "error recurrence-series-not-found series_id={series_id}"
            ))
        })?;
    let series_ref = recurrence_series_display_ref_for_id(conn, workspace_id, series_id).await?;
    let bounds = history_bounds(conn, workspace_id, &series, at).await?;
    let counts = recurrence_counts_for_series(conn, workspace_id, &series, &bounds).await?;
    let mut occurrence_cursor = OccurrenceHistoryCursor::default();
    let mut pause_cursor = PauseHistoryCursor::default();
    let mut next_occurrence = occurrence_cursor
        .next(conn, workspace_id, series_id)
        .await?;
    let mut next_pause = pause_cursor.next(conn, workspace_id, series_id).await?;
    let mut next_rank = bounds.derived_end_rank.checked_sub(1);
    let mut remaining_offset = offset;
    let mut items = Vec::with_capacity(limit);

    while items.len() < limit
        && (next_occurrence.is_some() || next_pause.is_some() || next_rank.is_some())
    {
        let derived_slot =
            next_rank.and_then(|rank| slot_at_rank(&series.rule, series.start_on, rank));
        let source =
            newest_history_source(next_occurrence.as_ref(), next_pause.as_ref(), derived_slot);
        if remaining_offset != 0
            && counts.pause_intervals == 0
            && matches!(source, Some(HistorySource::Derived))
        {
            let lower_rank = next_occurrence
                .as_ref()
                .and_then(|occurrence| slot_rank(&series.rule, series.start_on, occurrence.slot_on))
                .and_then(|rank| rank.checked_add(1))
                .unwrap_or(0);
            if let Some(rank) = next_rank {
                let available = rank.saturating_add(1).saturating_sub(lower_rank);
                let skipped =
                    remaining_offset.min(usize::try_from(available).unwrap_or(usize::MAX));
                if skipped != 0 {
                    next_rank = rank.checked_sub(u64::try_from(skipped).unwrap());
                    remaining_offset -= skipped;
                    continue;
                }
            }
        }
        let entry = match source {
            Some(HistorySource::Occurrence) => {
                let occurrence = next_occurrence
                    .take()
                    .expect("history source has occurrence");
                if next_rank.and_then(|rank| slot_at_rank(&series.rule, series.start_on, rank))
                    == Some(occurrence.slot_on)
                {
                    next_rank = next_rank.and_then(|rank| rank.checked_sub(1));
                }
                next_occurrence = occurrence_cursor
                    .next(conn, workspace_id, series_id)
                    .await?;
                history_entry_for_occurrence(conn, workspace_id, &series, &occurrence).await?
            }
            Some(HistorySource::Pause) => {
                let pause = next_pause.take().expect("history source has pause");
                next_pause = pause_cursor.next(conn, workspace_id, series_id).await?;
                Some(history_entry_for_pause(pause))
            }
            Some(HistorySource::Derived) => {
                let rank = next_rank.take().expect("history source has rank");
                next_rank = rank.checked_sub(1);
                let slot_on = slot_at_rank(&series.rule, series.start_on, rank)
                    .context("history derived slot rank exceeded date range")?;
                if slot_is_paused_at(conn, workspace_id, series_id, &series.schedule(), slot_on)
                    .await?
                {
                    None
                } else {
                    Some(RecurrenceHistoryEntry {
                        kind: RecurrenceHistoryKind::Missed,
                        slot_on: Some(slot_on.to_string()),
                        interval_started_at: None,
                        interval_ended_at: None,
                        task_id: None,
                        task_ref: None,
                        openable: false,
                        archived_projection: false,
                        resolved_at: None,
                    })
                }
            }
            None => break,
        };
        let Some(entry) = entry else {
            continue;
        };
        if remaining_offset != 0 {
            remaining_offset -= 1;
        } else {
            items.push(entry);
        }
    }

    let task_ids = items
        .iter()
        .filter_map(|entry| entry.task_id.clone())
        .collect::<Vec<_>>();
    let display_refs = DisplayRefContext::for_task_ids(conn, workspace_id, &task_ids).await?;
    let task_refs = load_task_display_refs(conn, workspace_id, &task_ids, &display_refs).await?;
    for entry in &mut items {
        if let Some(task_id) = entry.task_id.as_ref() {
            entry.task_ref = task_refs.get(task_id).cloned();
        }
    }
    let total = counts.total();
    Ok(RecurrenceHistoryPage {
        series_ref,
        items,
        offset,
        limit,
        total,
        has_more: offset.saturating_add(limit) < total,
    })
}

async fn recurrence_series_display_ref_for_id(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series_id: &RecurrenceSeriesId,
) -> Result<String> {
    let mut ids = HashSet::from([series_id.clone()]);
    if let Some(previous) = sqlx::query_scalar::<_, RecurrenceSeriesId>(
        "SELECT id FROM recurrence_series
         WHERE workspace_id = ? AND id < ? ORDER BY id DESC LIMIT 1",
    )
    .bind(workspace_id)
    .bind(series_id)
    .fetch_optional(&mut *conn)
    .await?
    {
        ids.insert(previous);
    }
    if let Some(next) = sqlx::query_scalar::<_, RecurrenceSeriesId>(
        "SELECT id FROM recurrence_series
         WHERE workspace_id = ? AND id > ? ORDER BY id LIMIT 1",
    )
    .bind(workspace_id)
    .bind(series_id)
    .fetch_optional(&mut *conn)
    .await?
    {
        ids.insert(next);
    }
    let mut ids = ids.into_iter().collect::<Vec<_>>();
    ids.sort();
    Ok(recurrence_series_display_ref(series_id, &ids))
}

#[derive(Debug)]
struct HistoryBounds {
    derived_end_rank: u64,
}

impl RecurrenceCounts {
    fn total(&self) -> usize {
        self.completed
            .saturating_add(self.skipped)
            .saturating_add(self.missed)
            .saturating_add(self.pause_intervals)
    }
}

async fn history_bounds(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series: &RecurrenceSeries,
    at: DateTime<Utc>,
) -> Result<HistoryBounds> {
    let projected_slot = sqlx::query_scalar::<_, String>(
        "SELECT slot_on FROM recurrence_occurrences
         WHERE workspace_id = ? AND series_id = ? AND projection_state = 'projected'",
    )
    .bind(workspace_id)
    .bind(&series.id)
    .fetch_optional(&mut *conn)
    .await?
    .map(|value| value.parse::<NaiveDate>())
    .transpose()?;
    let current_slot =
        live_slot_on(&series.rule, series.start_on, at, &series.timezone).max(projected_slot);
    let Some(current_slot) = current_slot else {
        return Ok(HistoryBounds {
            derived_end_rank: 0,
        });
    };
    let schedule = series.schedule();
    let current_rank = slot_rank(&series.rule, series.start_on, current_slot)
        .context("history current slot rank exceeded date range")?;
    let mut derived_end_rank = current_rank;
    if let Some(stop_at) = series
        .stopped_at
        .as_deref()
        .map(DateTime::parse_from_rfc3339)
        .transpose()?
    {
        let stop_at = stop_at
            .with_timezone(&Utc)
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
        derived_end_rank = derived_end_rank.min(first_slot_rank_at_or_after(
            &schedule,
            current_rank,
            &stop_at,
        )?);
    }
    Ok(HistoryBounds { derived_end_rank })
}

fn first_slot_rank_at_or_after(
    schedule: &RecurrenceSchedule,
    upper: u64,
    boundary_at: &str,
) -> Result<u64> {
    let mut low = 0;
    let mut high = upper;
    while low < high {
        let rank = low + (high - low) / 2;
        let slot_on = slot_at_rank(&schedule.rule, schedule.start_on, rank)
            .context("history boundary rank exceeded date range")?;
        let boundary = slot_values(schedule, slot_on)?.boundary_at;
        if boundary.as_str() >= boundary_at {
            high = rank;
        } else {
            low = rank + 1;
        }
    }
    Ok(low)
}

async fn recurrence_counts_for_series(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series: &RecurrenceSeries,
    bounds: &HistoryBounds,
) -> Result<RecurrenceCounts> {
    let schedule = series.schedule();
    let explicit_in_derived = count_occurrences_in_rank_range(
        conn,
        workspace_id,
        series,
        0,
        bounds.derived_end_rank,
        false,
    )
    .await?;
    let grouped = sqlx::query(
        "SELECT projection_state, outcome, COUNT(*) AS count, MAX(slot_on) AS max_slot_on
         FROM recurrence_occurrences
         WHERE workspace_id = ? AND series_id = ?
         GROUP BY projection_state, outcome",
    )
    .bind(workspace_id)
    .bind(&series.id)
    .fetch_all(&mut *conn)
    .await?;
    let mut counts = RecurrenceCounts::default();
    let mut archived_total = 0usize;
    let mut max_occurrence_rank = 0;
    for row in grouped {
        let projection_state: String = row.get("projection_state");
        let outcome: String = row.get("outcome");
        let count = usize::try_from(row.get::<i64, _>("count"))?;
        if let Some(slot_on) = row
            .try_get::<Option<String>, _>("max_slot_on")?
            .map(|value| value.parse::<NaiveDate>())
            .transpose()?
        {
            let rank = slot_rank(&series.rule, series.start_on, slot_on)
                .context("history occurrence is not a schedule slot")?;
            max_occurrence_rank = max_occurrence_rank.max(rank.saturating_add(1));
        }
        match outcome.as_str() {
            "completed" => counts.completed += count,
            "skipped" => counts.skipped += count,
            _ if projection_state == "archived" => archived_total += count,
            _ => {}
        }
    }

    let mut paused_in_derived = 0usize;
    let mut explicit_in_pauses = 0usize;
    let mut archived_in_pauses = 0usize;
    let mut pause_cursor = PauseHistoryCursor::default();
    while let Some(pause) = pause_cursor.next(conn, workspace_id, &series.id).await? {
        if let Some((start, end)) = pause_rank_span(&schedule, &pause, bounds.derived_end_rank)? {
            paused_in_derived = paused_in_derived
                .saturating_add(usize::try_from(end.saturating_sub(start)).unwrap_or(usize::MAX));
            explicit_in_pauses = explicit_in_pauses.saturating_add(
                count_occurrences_in_rank_range(conn, workspace_id, series, start, end, false)
                    .await?,
            );
        }
        if archived_total != 0
            && let Some((start, end)) = pause_rank_span(&schedule, &pause, max_occurrence_rank)?
        {
            archived_in_pauses = archived_in_pauses.saturating_add(
                count_occurrences_in_rank_range(conn, workspace_id, series, start, end, true)
                    .await?,
            );
        }
    }
    counts.pause_intervals = usize::try_from(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM recurrence_pause_intervals
             WHERE workspace_id = ? AND series_id = ?",
        )
        .bind(workspace_id)
        .bind(&series.id)
        .fetch_one(&mut *conn)
        .await?,
    )?;
    counts.missed = bounds
        .derived_end_rank
        .try_into()
        .unwrap_or(usize::MAX)
        .saturating_sub(explicit_in_derived)
        .saturating_sub(paused_in_derived)
        .saturating_add(explicit_in_pauses)
        .saturating_add(archived_total.saturating_sub(archived_in_pauses));

    let latest_explicit = latest_visible_occurrence(conn, workspace_id, series).await?;
    let latest_derived = match bounds
        .derived_end_rank
        .checked_sub(1)
        .and_then(|rank| slot_at_rank(&series.rule, series.start_on, rank))
    {
        Some(last_derived)
            if latest_explicit
                .as_ref()
                .is_some_and(|(slot_on, _)| *slot_on >= last_derived) =>
        {
            None
        }
        Some(_) => latest_derived_slot(conn, workspace_id, series, bounds.derived_end_rank).await?,
        None => None,
    };
    match (latest_explicit, latest_derived) {
        (Some((slot_on, _outcome)), Some(derived)) if derived > slot_on => {
            counts.latest_slot_on = Some(derived.to_string());
            counts.latest_outcome = None;
        }
        (Some((slot_on, outcome)), _) => {
            counts.latest_slot_on = Some(slot_on.to_string());
            counts.latest_outcome = outcome;
        }
        (None, Some(derived)) => {
            counts.latest_slot_on = Some(derived.to_string());
            counts.latest_outcome = None;
        }
        (None, None) => {}
    }
    Ok(counts)
}

fn pause_rank_span(
    schedule: &RecurrenceSchedule,
    pause: &RecurrencePauseInterval,
    upper: u64,
) -> Result<Option<(u64, u64)>> {
    if upper == 0 {
        return Ok(None);
    }
    let start = first_slot_rank_at_or_after(schedule, upper, &pause.paused_at)?;
    let end = pause
        .resumed_at
        .as_deref()
        .map(|resumed_at| first_slot_rank_at_or_after(schedule, upper, resumed_at))
        .transpose()?
        .unwrap_or(upper);
    (start < end)
        .then_some((start, end))
        .map_or(Ok(None), |span| Ok(Some(span)))
}

async fn count_occurrences_in_rank_range(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series: &RecurrenceSeries,
    start: u64,
    end: u64,
    archived_only: bool,
) -> Result<usize> {
    if start >= end {
        return Ok(0);
    }
    let schedule = series.schedule();
    let start_slot = slot_at_rank(&schedule.rule, schedule.start_on, start)
        .context("history range start exceeded date range")?;
    let end_slot = slot_at_rank(&schedule.rule, schedule.start_on, end);
    let end_slot = end_slot
        .map(|slot| slot.to_string())
        .or_else(|| {
            slot_at_rank(&schedule.rule, schedule.start_on, end - 1).map(|slot| slot.to_string())
        })
        .context("history range end exceeded date range")?;
    let mut query = QueryBuilder::<Sqlite>::new(
        "SELECT COUNT(*) FROM recurrence_occurrences
         WHERE workspace_id = ",
    );
    query.push_bind(workspace_id);
    query.push(" AND series_id = ");
    query.push_bind(&series.id);
    query.push(" AND slot_on >= ");
    query.push_bind(start_slot.to_string());
    if slot_at_rank(&schedule.rule, schedule.start_on, end).is_some() {
        query.push(" AND slot_on < ");
    } else {
        query.push(" AND slot_on <= ");
    }
    query.push_bind(end_slot);
    if archived_only {
        query.push(" AND projection_state = 'archived'");
    }
    Ok(usize::try_from(
        query
            .build_query_scalar::<i64>()
            .fetch_one(&mut *conn)
            .await?,
    )?)
}

async fn latest_visible_occurrence(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series: &RecurrenceSeries,
) -> Result<Option<(NaiveDate, Option<RecurrenceOutcome>)>> {
    let mut cursor = OccurrenceHistoryCursor::default();
    while let Some(occurrence) = cursor.next(conn, workspace_id, &series.id).await? {
        match occurrence.projection_state {
            RecurrenceProjectionState::Resolved => {
                return Ok(Some((occurrence.slot_on, occurrence.outcome)));
            }
            RecurrenceProjectionState::Archived
                if !slot_is_paused_at(
                    conn,
                    workspace_id,
                    &series.id,
                    &series.schedule(),
                    occurrence.slot_on,
                )
                .await? =>
            {
                return Ok(Some((occurrence.slot_on, None)));
            }
            _ => {}
        }
    }
    Ok(None)
}

async fn latest_derived_slot(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series: &RecurrenceSeries,
    end_rank: u64,
) -> Result<Option<NaiveDate>> {
    let schedule = series.schedule();
    let mut rank = end_rank;
    while let Some(previous) = rank.checked_sub(1) {
        rank = previous;
        let slot_on = slot_at_rank(&series.rule, series.start_on, rank)
            .context("history derived slot rank exceeded date range")?;
        let explicit = sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(
                SELECT 1 FROM recurrence_occurrences
                WHERE workspace_id = ? AND series_id = ? AND slot_on = ?
            )",
        )
        .bind(workspace_id)
        .bind(&series.id)
        .bind(slot_on.to_string())
        .fetch_one(&mut *conn)
        .await?;
        if explicit == 0
            && !slot_is_paused_at(conn, workspace_id, &series.id, &schedule, slot_on).await?
        {
            return Ok(Some(slot_on));
        }
    }
    Ok(None)
}

async fn slot_is_paused_at(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series_id: &RecurrenceSeriesId,
    schedule: &RecurrenceSchedule,
    slot_on: NaiveDate,
) -> Result<bool> {
    let boundary = slot_values(schedule, slot_on)?.boundary_at;
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT EXISTS(
            SELECT 1 FROM recurrence_pause_intervals
            WHERE workspace_id = ? AND series_id = ? AND paused_at <= ?
              AND (resumed_at = '' OR resumed_at > ?)
        )",
    )
    .bind(workspace_id)
    .bind(series_id)
    .bind(&boundary)
    .bind(&boundary)
    .fetch_one(&mut *conn)
    .await?
        != 0)
}

async fn history_entry_for_occurrence(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series: &RecurrenceSeries,
    occurrence: &RecurrenceOccurrence,
) -> Result<Option<RecurrenceHistoryEntry>> {
    let kind = match occurrence.outcome {
        Some(RecurrenceOutcome::Completed) => RecurrenceHistoryKind::Completed,
        Some(RecurrenceOutcome::Skipped) => RecurrenceHistoryKind::Skipped,
        None if matches!(
            occurrence.projection_state,
            RecurrenceProjectionState::Archived
        ) && !slot_is_paused_at(
            conn,
            workspace_id,
            &series.id,
            &series.schedule(),
            occurrence.slot_on,
        )
        .await? =>
        {
            RecurrenceHistoryKind::Missed
        }
        _ => return Ok(None),
    };
    Ok(Some(RecurrenceHistoryEntry {
        kind,
        slot_on: Some(occurrence.slot_on.to_string()),
        interval_started_at: None,
        interval_ended_at: None,
        task_id: occurrence.task_id.clone(),
        task_ref: None,
        openable: occurrence.task_id.is_some(),
        archived_projection: matches!(
            occurrence.projection_state,
            RecurrenceProjectionState::Archived
        ),
        resolved_at: occurrence.resolved_at.clone(),
    }))
}

fn history_entry_for_pause(pause: RecurrencePauseInterval) -> RecurrenceHistoryEntry {
    let openable = pause.suspended_task_id.is_some();
    RecurrenceHistoryEntry {
        kind: RecurrenceHistoryKind::Paused,
        slot_on: None,
        interval_started_at: Some(pause.paused_at),
        interval_ended_at: pause.resumed_at,
        task_id: pause.suspended_task_id,
        task_ref: None,
        openable,
        archived_projection: false,
        resolved_at: None,
    }
}

#[derive(Clone, Copy)]
enum HistorySource {
    Occurrence,
    Pause,
    Derived,
}

fn newest_history_source(
    occurrence: Option<&RecurrenceOccurrence>,
    pause: Option<&RecurrencePauseInterval>,
    derived_slot: Option<NaiveDate>,
) -> Option<HistorySource> {
    let mut best: Option<(String, HistorySource)> =
        occurrence.map(|occurrence| (occurrence.slot_on.to_string(), HistorySource::Occurrence));
    if let Some(pause) = pause
        && best
            .as_ref()
            .is_none_or(|(current, _)| pause.paused_at.as_str() > current.as_str())
    {
        best = Some((pause.paused_at.clone(), HistorySource::Pause));
    }
    if let Some(derived_slot) = derived_slot {
        let key = derived_slot.to_string();
        if best.as_ref().is_none_or(|(current, _)| key > *current) {
            best = Some((key, HistorySource::Derived));
        }
    }
    best.map(|(_, source)| source)
}

#[derive(Default)]
struct OccurrenceHistoryCursor {
    after_slot: Option<NaiveDate>,
    buffer: VecDeque<RecurrenceOccurrence>,
    exhausted: bool,
}

impl OccurrenceHistoryCursor {
    async fn next(
        &mut self,
        conn: &mut SqliteConnection,
        workspace_id: &WorkspaceId,
        series_id: &RecurrenceSeriesId,
    ) -> Result<Option<RecurrenceOccurrence>> {
        if self.buffer.is_empty() && !self.exhausted {
            let mut query = QueryBuilder::<Sqlite>::new(
                "SELECT workspace_id, series_id, slot_on, task_id, outcome, resolved_at,
                        outcome_change_id, projection_state, archived_at
                 FROM recurrence_occurrences
                 WHERE workspace_id = ",
            );
            query.push_bind(workspace_id);
            query.push(" AND series_id = ");
            query.push_bind(series_id);
            if let Some(after_slot) = self.after_slot {
                query.push(" AND slot_on < ");
                query.push_bind(after_slot.to_string());
            }
            query.push(" ORDER BY slot_on DESC LIMIT ");
            query.push_bind(i64::try_from(HISTORY_CURSOR_CHUNK_SIZE).unwrap());
            let rows = query.build().fetch_all(&mut *conn).await?;
            self.exhausted = rows.len() < HISTORY_CURSOR_CHUNK_SIZE;
            for row in rows {
                self.buffer.push_back(recurrence_occurrence_from_row(&row)?);
            }
            self.after_slot = self.buffer.back().map(|occurrence| occurrence.slot_on);
        }
        Ok(self.buffer.pop_front())
    }
}

#[derive(Default)]
struct PauseHistoryCursor {
    after: Option<(String, String)>,
    buffer: VecDeque<RecurrencePauseInterval>,
    exhausted: bool,
}

impl PauseHistoryCursor {
    async fn next(
        &mut self,
        conn: &mut SqliteConnection,
        workspace_id: &WorkspaceId,
        series_id: &RecurrenceSeriesId,
    ) -> Result<Option<RecurrencePauseInterval>> {
        if self.buffer.is_empty() && !self.exhausted {
            let mut query = QueryBuilder::<Sqlite>::new(
                "SELECT workspace_id, id, series_id, paused_at, resumed_at, suspended_slot_on,
                        suspended_task_id, created_by_change_id, resolved_by_change_id
                 FROM recurrence_pause_intervals
                 WHERE workspace_id = ",
            );
            query.push_bind(workspace_id);
            query.push(" AND series_id = ");
            query.push_bind(series_id);
            if let Some((at, id)) = self.after.as_ref() {
                query.push(" AND (paused_at < ");
                query.push_bind(at);
                query.push(" OR (paused_at = ");
                query.push_bind(at);
                query.push(" AND id < ");
                query.push_bind(id);
                query.push("))");
            }
            query.push(" ORDER BY paused_at DESC, id DESC LIMIT ");
            query.push_bind(i64::try_from(HISTORY_CURSOR_CHUNK_SIZE).unwrap());
            let rows = query.build().fetch_all(&mut *conn).await?;
            self.exhausted = rows.len() < HISTORY_CURSOR_CHUNK_SIZE;
            for row in rows {
                self.buffer
                    .push_back(recurrence_pause_interval_from_row(&row)?);
            }
            self.after = self
                .buffer
                .back()
                .map(|pause| (pause.paused_at.clone(), pause.id.clone()));
        }
        Ok(self.buffer.pop_front())
    }
}

const HISTORY_CURSOR_CHUNK_SIZE: usize = 64;

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
    let current = load_projected_occurrences(conn, workspace_id, &ids).await?;
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
    let mut result = HashMap::with_capacity(series.len());
    for item in series {
        let bounds = history_bounds(conn, workspace_id, item, at).await?;
        result.insert(
            item.id.clone(),
            recurrence_counts_for_series(conn, workspace_id, item, &bounds).await?,
        );
    }
    Ok(result)
}

async fn load_projected_occurrences(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series_ids: &[RecurrenceSeriesId],
) -> Result<HashMap<RecurrenceSeriesId, RecurrenceOccurrence>> {
    let mut by_series = HashMap::new();
    for chunk in series_ids.chunks(SQLITE_BIND_CHUNK_SIZE) {
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT workspace_id, series_id, slot_on, task_id, outcome, resolved_at,
                    outcome_change_id, projection_state, archived_at
             FROM recurrence_occurrences WHERE workspace_id = ",
        );
        query.push_bind(workspace_id);
        query.push(" AND projection_state = 'projected' AND series_id IN (");
        {
            let mut separated = query.separated(", ");
            for series_id in chunk {
                separated.push_bind(series_id);
            }
        }
        query.push(") ORDER BY series_id");
        for row in query.build().fetch_all(&mut *conn).await? {
            let occurrence = recurrence_occurrence_from_row(&row)?;
            by_series.insert(occurrence.series_id.clone(), occurrence);
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

fn rule_label(frequency: String, interval: u32, weekdays: String) -> String {
    match (frequency.as_str(), interval, weekdays.as_str()) {
        ("daily", 1, _) => "daily".to_string(),
        ("daily", interval, _) => format!("every {interval} days"),
        ("monthly", 1, _) => "monthly".to_string(),
        ("monthly", interval, _) => format!("every {interval} months"),
        ("yearly", 1, _) => "yearly".to_string(),
        ("yearly", interval, _) => format!("every {interval} years"),
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

    async fn for_series_ids(
        conn: &mut SqliteConnection,
        workspace_id: &WorkspaceId,
        series_ids: &HashSet<RecurrenceSeriesId>,
    ) -> Result<Self> {
        if series_ids.is_empty() {
            return Ok(Self { ids: Vec::new() });
        }
        let requested = series_ids.iter().cloned().collect::<Vec<_>>();
        let mut ids = HashSet::new();
        for chunk in requested.chunks(SQLITE_BIND_CHUNK_SIZE.saturating_sub(2)) {
            let mut query = QueryBuilder::<Sqlite>::new("WITH requested(id) AS (VALUES ");
            for (index, series_id) in chunk.iter().enumerate() {
                if index != 0 {
                    query.push(", ");
                }
                query.push("(").push_bind(series_id).push(")");
            }
            query.push(
                ")
                 SELECT id FROM requested
                 UNION
                 SELECT MAX(s.id) FROM recurrence_series s
                 JOIN requested r ON s.id < r.id
                 WHERE s.workspace_id = ",
            );
            query.push_bind(workspace_id);
            query.push(
                " GROUP BY r.id
                  UNION
                  SELECT MIN(s.id) FROM recurrence_series s
                  JOIN requested r ON s.id > r.id
                  WHERE s.workspace_id = ",
            );
            query.push_bind(workspace_id);
            for series_id in query
                .build_query_scalar::<Option<RecurrenceSeriesId>>()
                .fetch_all(&mut *conn)
                .await?
                .into_iter()
                .flatten()
            {
                ids.insert(series_id);
            }
        }
        let mut ids = ids.into_iter().collect::<Vec<_>>();
        ids.sort();
        Ok(Self { ids })
    }

    pub(crate) fn display_ref(&self, series_id: &RecurrenceSeriesId) -> String {
        recurrence_series_display_ref(series_id, &self.ids)
    }
}

pub(crate) fn validate_reconciliation(result: &RecurrenceReconciliation) -> Result<()> {
    ensure!(
        result.examined <= REPORT_RECONCILE_LIMIT,
        "recurrence report reconciliation exceeded its candidate bound"
    );
    Ok(())
}
