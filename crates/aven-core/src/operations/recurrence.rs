use anyhow::{Context, Result, ensure};
use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use sqlx::{Row, SqliteConnection};

#[cfg(test)]
use crate::choices::{TaskPriority, TaskStatus};
use crate::db::{
    Database, begin_immediate, entity_conflict_exists, recurrence_occurrence_from_row,
    recurrence_series_from_row,
};
use crate::error::CoreError;
use crate::ids::{TaskId, WorkspaceId, now, now_utc};
#[cfg(test)]
use crate::recurrence::RecurrenceProjectionState;
#[cfg(test)]
use crate::recurrence::RecurrenceSeriesState;
use crate::recurrence::{
    RecurrenceDuePolicy, RecurrenceOutcome, RecurrenceSchedule, RecurrenceSeriesId,
    SERIES_REF_PREFIX, recurrence_series_display_ref,
};
use crate::refs::get_task_in_workspace;
#[cfg(test)]
use crate::task_fields::TaskField;
use crate::types::{MutableEntityType, RecurrenceOccurrence, RecurrenceSeries, Task};
use crate::undo::{UndoCommand, UndoContext, UndoPayload, record_tui_undo};
use crate::workspaces::Workspace;

#[cfg(test)]
#[path = "recurrence_tests.rs"]
mod tests;

mod lifecycle;
mod projection;
mod resolution;
mod template;

pub use lifecycle::{pause_recurrence_series, resume_recurrence_series, stop_recurrence_series};
pub(crate) use lifecycle::{
    stop_recurrence_series_for_project_delete_in_transaction, timestamp_strictly_after,
};
pub(crate) use projection::reconcile_recurrence_series_in_transaction;
use projection::{materialize_occurrence, verify_materialized_occurrence};
use projection::{reconcile_recurrence_series_once, retryable_reconcile_error};
pub(crate) use resolution::resolve_recurrence_occurrence_in_transaction;
pub(crate) use resolution::{
    RecurrenceMutationOutcome, RecurrenceStructuralMutation, RecurrenceTaskMutation,
    route_recurrence_task_mutation, undo_recurrence_resolution,
    validate_consumer_recurrence_mutation,
};
pub use template::{create_recurrence_series, update_recurrence_template};

const RECONCILE_ATTEMPTS: usize = 3;
const SERIES_TEMPLATE_FIELDS: &[&str] = &[
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

#[derive(Debug, Clone)]
pub struct RecurrenceSeriesDraft {
    pub title: String,
    pub description: String,
    pub project: String,
    pub priority: String,
    pub initial_status: String,
    pub labels: Vec<String>,
    pub metadata: Vec<crate::metadata::TaskMetadataInput>,
    pub schedule: RecurrenceSchedule,
}

#[derive(Debug, Clone)]
pub struct CreateRecurrenceSeriesParams {
    pub draft: RecurrenceSeriesDraft,
    at: Option<DateTime<Utc>>,
    create_missing_labels: bool,
}

impl CreateRecurrenceSeriesParams {
    pub fn new(draft: RecurrenceSeriesDraft) -> Self {
        Self {
            draft,
            at: None,
            create_missing_labels: false,
        }
    }

    pub fn at(mut self, at: DateTime<Utc>) -> Self {
        self.at = Some(at);
        self
    }

    pub fn with_create_missing_labels(mut self) -> Self {
        self.create_missing_labels = true;
        self
    }

    fn resolve_at_with(mut self, clock: impl FnOnce() -> Result<DateTime<Utc>>) -> Result<Self> {
        if self.at.is_none() {
            self.at = Some(clock()?);
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Default)]
pub struct RecurrenceTemplateUpdate {
    pub title: Option<String>,
    pub description: Option<String>,
    pub project: Option<String>,
    pub priority: Option<String>,
    pub initial_status: Option<String>,
    pub labels: Option<Vec<String>>,
    pub set_metadata: Vec<crate::metadata::TaskMetadataInput>,
    pub remove_metadata: Vec<String>,
    pub available_local_time: Option<Option<NaiveTime>>,
    pub due_policy: Option<RecurrenceDuePolicy>,
}

#[derive(Debug, Clone)]
pub struct UpdateRecurrenceTemplateParams {
    pub update: RecurrenceTemplateUpdate,
    create_missing_labels: bool,
}

impl UpdateRecurrenceTemplateParams {
    pub fn new(update: RecurrenceTemplateUpdate) -> Self {
        Self {
            update,
            create_missing_labels: false,
        }
    }

    pub fn with_create_missing_labels(mut self) -> Self {
        self.create_missing_labels = true;
        self
    }
}

#[derive(Debug, Clone)]
pub struct RecurrenceCreateOutcome {
    pub series: RecurrenceSeries,
    pub series_ref: String,
    pub occurrence: RecurrenceOccurrence,
    pub task: Task,
}

#[derive(Debug, Clone)]
pub struct RecurrenceTemplateUpdateOutcome {
    pub series: RecurrenceSeries,
    pub changed: bool,
}

#[derive(Debug, Clone)]
pub struct RecurrenceReconcileOutcome {
    pub series: RecurrenceSeries,
    pub occurrence: Option<RecurrenceOccurrence>,
    pub changed: bool,
    pub lifecycle_blocked: bool,
}

#[derive(Debug, Clone)]
pub struct RecurrenceResolveOutcome {
    pub series: RecurrenceSeries,
    pub resolved: RecurrenceOccurrence,
    pub task: Task,
    pub successor: Option<Task>,
}

#[derive(Debug, Clone)]
pub struct RecurrenceStateOutcome {
    pub series: RecurrenceSeries,
    pub occurrence: Option<RecurrenceOccurrence>,
}

impl Database {
    pub async fn create_recurrence_series(
        &self,
        workspace: &Workspace,
        params: CreateRecurrenceSeriesParams,
    ) -> Result<RecurrenceCreateOutcome> {
        self.create_recurrence_series_with_clock(workspace, params, utc_now)
            .await
    }

    async fn create_recurrence_series_with_clock(
        &self,
        workspace: &Workspace,
        params: CreateRecurrenceSeriesParams,
        clock: impl FnOnce() -> Result<DateTime<Utc>>,
    ) -> Result<RecurrenceCreateOutcome> {
        let params = params.resolve_at_with(clock)?;
        let mut conn = self.acquire_writer().await?;
        create_recurrence_series(&mut conn, workspace, params).await
    }

    pub async fn update_recurrence_template(
        &self,
        workspace: &Workspace,
        series_id: &RecurrenceSeriesId,
        params: UpdateRecurrenceTemplateParams,
    ) -> Result<RecurrenceTemplateUpdateOutcome> {
        let mut conn = self.acquire_writer().await?;
        update_recurrence_template(&mut conn, workspace, series_id, params).await
    }

    pub async fn reconcile_recurrence_series(
        &self,
        workspace: &Workspace,
        series_id: &RecurrenceSeriesId,
        at: DateTime<Utc>,
    ) -> Result<RecurrenceReconcileOutcome> {
        let mut conn = self.acquire_writer().await?;
        let mut last_error = None;
        for _ in 0..RECONCILE_ATTEMPTS {
            match reconcile_recurrence_series_once(&mut conn, workspace, series_id, at).await {
                Ok(outcome) => return Ok(outcome),
                Err(error) if retryable_reconcile_error(&error) => last_error = Some(error),
                Err(error) => return Err(error),
            }
        }
        Err(last_error.expect("bounded reconciliation records retry errors"))
    }

    pub async fn resolve_recurrence_occurrence(
        &self,
        workspace: &Workspace,
        task_id: &TaskId,
        outcome: RecurrenceOutcome,
    ) -> Result<RecurrenceResolveOutcome> {
        self.resolve_recurrence_occurrence_with_undo(workspace, task_id, outcome, UndoContext::None)
            .await
    }

    pub async fn resolve_recurrence_occurrence_with_undo(
        &self,
        workspace: &Workspace,
        task_id: &TaskId,
        outcome: RecurrenceOutcome,
        undo: UndoContext,
    ) -> Result<RecurrenceResolveOutcome> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let before = get_task_in_workspace(&mut tx, workspace, task_id).await?;
        let result = resolve_recurrence_occurrence_in_transaction(
            &mut tx,
            workspace,
            task_id,
            outcome,
            &now(),
        )
        .await?;
        if before.status != result.task.status
            && let UndoContext::Tui = undo
        {
            record_tui_undo(
                &mut tx,
                &workspace.id,
                UndoPayload {
                    commands: vec![UndoCommand::SetTaskField {
                        task_id: task_id.clone(),
                        field: "status".to_string(),
                        before: before.status.as_str().to_string(),
                        after: result.task.status.as_str().to_string(),
                        queue_activity_before: Some(before.queue_activity_at.clone()),
                        queue_activity_after: Some(result.task.queue_activity_at.clone()),
                    }],
                },
            )
            .await?;
        }
        tx.commit().await?;
        Ok(result)
    }

    pub async fn pause_recurrence_series(
        &self,
        workspace: &Workspace,
        series_id: &RecurrenceSeriesId,
    ) -> Result<RecurrenceStateOutcome> {
        let mut conn = self.acquire_writer().await?;
        pause_recurrence_series(&mut conn, workspace, series_id, &now()).await
    }

    pub async fn resume_recurrence_series(
        &self,
        workspace: &Workspace,
        series_id: &RecurrenceSeriesId,
        at: DateTime<Utc>,
    ) -> Result<RecurrenceStateOutcome> {
        let mut conn = self.acquire_writer().await?;
        resume_recurrence_series(&mut conn, workspace, series_id, at).await
    }

    pub async fn stop_recurrence_series(
        &self,
        workspace: &Workspace,
        series_id: &RecurrenceSeriesId,
        skip_current: bool,
    ) -> Result<RecurrenceStateOutcome> {
        let mut conn = self.acquire_writer().await?;
        stop_recurrence_series(&mut conn, workspace, series_id, skip_current, &now()).await
    }

    pub async fn resolve_recurrence_ref(
        &self,
        workspace: &Workspace,
        input: &str,
    ) -> Result<RecurrenceSeries> {
        let mut conn = self.acquire_reader().await?;
        resolve_recurrence_ref(&mut conn, workspace, input).await
    }

    pub async fn recurrence_series_ref(
        &self,
        workspace_id: &WorkspaceId,
        series_id: &RecurrenceSeriesId,
    ) -> Result<String> {
        let mut conn = self.acquire_reader().await?;
        recurrence_series_ref(&mut conn, workspace_id, series_id).await
    }
}

async fn load_series(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series_id: &RecurrenceSeriesId,
) -> Result<RecurrenceSeries> {
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
    recurrence_series_from_row(&row)
}

async fn load_series_labels(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series_id: &RecurrenceSeriesId,
) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT label FROM recurrence_series_labels
         WHERE workspace_id = ? AND series_id = ? ORDER BY label",
    )
    .bind(workspace_id)
    .bind(series_id)
    .fetch_all(&mut *conn)
    .await?)
}

async fn load_series_metadata(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series_id: &RecurrenceSeriesId,
) -> Result<Vec<crate::metadata::ResolvedMetadataValue>> {
    let rows = sqlx::query(
        "SELECT m.field_id, f.key, m.value
         FROM recurrence_series_metadata m
         JOIN metadata_fields f ON f.workspace_id = m.workspace_id AND f.id = m.field_id
         WHERE m.workspace_id = ? AND m.series_id = ? ORDER BY f.key",
    )
    .bind(workspace_id)
    .bind(series_id)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| crate::metadata::ResolvedMetadataValue {
            field_id: row.get("field_id"),
            key: row.get("key"),
            value: row.get("value"),
        })
        .collect())
}

async fn load_occurrence(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series_id: &RecurrenceSeriesId,
    slot_on: NaiveDate,
) -> Result<Option<RecurrenceOccurrence>> {
    let row = sqlx::query(
        "SELECT workspace_id, series_id, slot_on, task_id, outcome, resolved_at,
                outcome_change_id, projection_state, archived_at
         FROM recurrence_occurrences
         WHERE workspace_id = ? AND series_id = ? AND slot_on = ?",
    )
    .bind(workspace_id)
    .bind(series_id)
    .bind(slot_on.format("%Y-%m-%d").to_string())
    .fetch_optional(&mut *conn)
    .await?;
    row.as_ref().map(recurrence_occurrence_from_row).transpose()
}

async fn load_projected_occurrence(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series_id: &RecurrenceSeriesId,
) -> Result<Option<RecurrenceOccurrence>> {
    let row = sqlx::query(
        "SELECT workspace_id, series_id, slot_on, task_id, outcome, resolved_at,
                outcome_change_id, projection_state, archived_at
         FROM recurrence_occurrences
         WHERE workspace_id = ? AND series_id = ? AND projection_state = 'projected'",
    )
    .bind(workspace_id)
    .bind(series_id)
    .fetch_optional(&mut *conn)
    .await?;
    row.as_ref().map(recurrence_occurrence_from_row).transpose()
}

async fn load_occurrence_for_task(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: &TaskId,
) -> Result<Option<RecurrenceOccurrence>> {
    let row = sqlx::query(
        "SELECT workspace_id, series_id, slot_on, task_id, outcome, resolved_at,
                outcome_change_id, projection_state, archived_at
         FROM recurrence_occurrences WHERE workspace_id = ? AND task_id = ?",
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_optional(&mut *conn)
    .await?;
    row.as_ref().map(recurrence_occurrence_from_row).transpose()
}

async fn lifecycle_conflict_exists(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series_id: &RecurrenceSeriesId,
) -> Result<bool> {
    for field in ["state", "stopped_at"] {
        if entity_conflict_exists(
            conn,
            workspace_id,
            MutableEntityType::RecurrenceSeries,
            series_id.as_str(),
            field,
        )
        .await?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn ensure_no_series_conflict(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series_id: &RecurrenceSeriesId,
    field: &str,
) -> Result<()> {
    ensure!(
        !entity_conflict_exists(
            conn,
            workspace_id,
            MutableEntityType::RecurrenceSeries,
            series_id.as_str(),
            field,
        )
        .await?,
        CoreError::open_conflict(format!(
            "error recurrence-conflicted-field series_id={series_id} field={field}"
        ))
    );
    Ok(())
}

pub async fn resolve_recurrence_ref(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    input: &str,
) -> Result<RecurrenceSeries> {
    let (hint, suffix) = split_ref(input);
    if suffix.len() < 3 {
        return Err(CoreError::validation(format!(
            "error recurrence-ref-too-short input={input} minimum=3"
        ))
        .into());
    }
    let series_rows = sqlx::query(
        "SELECT workspace_id, id, title, description, project_id, priority, initial_status,
                frequency, interval, weekdays, timezone, start_on, available_local_time,
                due_policy, state, stopped_at, created_at, updated_at, deleted
         FROM recurrence_series WHERE workspace_id = ? AND id LIKE ? || '%' ORDER BY id",
    )
    .bind(&workspace.id)
    .bind(&suffix)
    .fetch_all(&mut *conn)
    .await?;
    let mut series = series_rows
        .iter()
        .map(recurrence_series_from_row)
        .collect::<Result<Vec<_>>>()?;
    if hint
        .as_deref()
        .is_some_and(|value| value != SERIES_REF_PREFIX)
    {
        series.clear();
    }
    if series.len() == 1 {
        return Ok(series.remove(0));
    }
    if series.len() > 1 {
        return Err(
            CoreError::validation(format!("error ambiguous-recurrence-ref input={input}")).into(),
        );
    }
    let task = crate::refs::resolve_task_ref_in_workspace(conn, workspace, input).await?;
    let occurrence = load_occurrence_for_task(conn, &workspace.id, &task.id)
        .await?
        .context("error task-is-not-recurring")?;
    load_series(conn, &workspace.id, &occurrence.series_id).await
}

async fn recurrence_series_ref(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series_id: &RecurrenceSeriesId,
) -> Result<String> {
    let ids = sqlx::query_scalar::<_, RecurrenceSeriesId>(
        "SELECT id FROM recurrence_series WHERE workspace_id = ? ORDER BY id",
    )
    .bind(workspace_id)
    .fetch_all(&mut *conn)
    .await?;
    ensure!(
        ids.iter().any(|candidate| candidate == series_id),
        CoreError::not_found(format!(
            "error recurrence-series-not-found series_id={series_id}"
        ))
    );
    Ok(recurrence_series_display_ref(series_id, &ids))
}

fn split_ref(input: &str) -> (Option<String>, String) {
    let (hint, suffix) = input
        .split_once('-')
        .map_or((None, input), |(hint, suffix)| (Some(hint), suffix));
    let suffix = suffix
        .chars()
        .filter(|value| value.is_ascii_alphanumeric())
        .map(|value| match value.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            value => value,
        })
        .collect();
    (hint.map(|value| value.to_ascii_uppercase()), suffix)
}

fn format_local_time(value: Option<NaiveTime>) -> String {
    value
        .map(|value| value.format("%H:%M:%S").to_string())
        .unwrap_or_default()
}

fn format_utc(value: DateTime<Utc>) -> String {
    value.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn utc_now() -> Result<DateTime<Utc>> {
    Ok(now_utc())
}
