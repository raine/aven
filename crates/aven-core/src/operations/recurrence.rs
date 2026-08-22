use std::collections::BTreeMap;

use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use serde_json::Value;
use sqlx::{Row, SqliteConnection};

use crate::change_log::{ChangeEntity, ChangePayload, append_change, op_type};
use crate::choices::{TaskPriority, TaskStatus};
use crate::db::{
    Database, IdentifiedChange, begin_immediate, entity_conflict_exists, entity_field_version,
    insert_change, insert_change_with_identity, recurrence_occurrence_from_row,
    recurrence_series_from_row, set_entity_field_version, set_field_version,
};
use crate::error::CoreError;
use crate::ids::{TaskId, WorkspaceId, new_id, now, now_utc};
use crate::labels::{resolve_labels_in_workspace, resolve_or_create_labels_in_workspace};
use crate::mutation::apply_field_value_in_workspace;
use crate::projects::resolve_or_create_project_in_workspace;
use crate::recurrence::{
    RecurrenceDuePolicy, RecurrenceOutcome, RecurrenceProjectionState, RecurrenceSchedule,
    RecurrenceSeriesId, RecurrenceSeriesState, SERIES_REF_PREFIX, derive_occurrence_identity,
    next_slot_after, projection_slot_at, recurrence_series_display_ref, slot_cutoff, slot_values,
};
use crate::refs::get_task_in_workspace;
use crate::task_fields::TaskField;
use crate::types::{MutableEntityType, RecurrenceOccurrence, RecurrenceSeries, Task};
use crate::undo::{UndoCommand, UndoContext, UndoPayload, record_tui_undo};
use crate::workspaces::Workspace;

#[cfg(test)]
#[path = "recurrence_tests.rs"]
mod tests;

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
            && let UndoContext::Tui { summary } = undo
        {
            record_tui_undo(
                &mut tx,
                &workspace.id,
                &summary,
                UndoPayload {
                    commands: vec![UndoCommand::SetTaskField {
                        task_id: task_id.clone(),
                        field: "status".to_string(),
                        before: before.status.as_str().to_string(),
                        after: result.task.status.as_str().to_string(),
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

pub async fn create_recurrence_series(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    params: CreateRecurrenceSeriesParams,
) -> Result<RecurrenceCreateOutcome> {
    let CreateRecurrenceSeriesParams {
        draft,
        at,
        create_missing_labels,
    } = params;
    let at = at.map_or_else(utc_now, Ok)?;
    let priority = TaskPriority::parse(&draft.priority)?;
    let initial_status = TaskStatus::parse(&draft.initial_status)?;
    ensure!(
        initial_status.is_open(),
        CoreError::validation(format!(
            "error recurrence-initial-status-terminal status={}",
            initial_status.as_str()
        ))
    );
    let creation_date = at
        .with_timezone(&draft.schedule.timezone.timezone())
        .date_naive();
    let first_slot = draft
        .schedule
        .slots_on_or_after(draft.schedule.start_on.max(creation_date))
        .next()
        .context("recurrence schedule has no representable first slot")?;
    let series_id = RecurrenceSeriesId::new();
    let created_at = format_utc(at);

    let mut tx = begin_immediate(conn).await?;
    let project =
        resolve_or_create_project_in_workspace(&mut tx, &workspace.id, draft.project.as_str())
            .await?;
    let labels = if create_missing_labels {
        resolve_or_create_labels_in_workspace(&mut tx, workspace, &draft.labels)
            .await?
            .names
    } else {
        resolve_labels_in_workspace(&mut tx, &workspace.id, &draft.labels).await?
    };
    let series_metadata =
        crate::metadata::resolve_metadata_inputs(&mut tx, workspace, &draft.metadata).await?;
    sqlx::query(
        "INSERT INTO recurrence_series(
            workspace_id, id, title, description, project_id, priority, initial_status,
            frequency, interval, weekdays, timezone, start_on, available_local_time,
            due_policy, state, stopped_at, created_at, updated_at, deleted
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'active', '', ?, ?, 0)",
    )
    .bind(&workspace.id)
    .bind(&series_id)
    .bind(&draft.title)
    .bind(&draft.description)
    .bind(&project.id)
    .bind(priority.as_str())
    .bind(initial_status.as_str())
    .bind(draft.schedule.rule.frequency().as_str())
    .bind(i64::from(draft.schedule.rule.interval()))
    .bind(draft.schedule.rule.weekdays_set().to_string())
    .bind(draft.schedule.timezone.as_str())
    .bind(draft.schedule.start_on.format("%Y-%m-%d").to_string())
    .bind(format_local_time(draft.schedule.available_local_time))
    .bind(draft.schedule.due_policy.as_str())
    .bind(&created_at)
    .bind(&created_at)
    .execute(&mut *tx)
    .await?;
    for label in &labels {
        sqlx::query(
            "INSERT INTO recurrence_series_labels(workspace_id, series_id, label)
             VALUES (?, ?, ?)",
        )
        .bind(&workspace.id)
        .bind(&series_id)
        .bind(label)
        .execute(&mut *tx)
        .await?;
    }
    for value in &series_metadata {
        sqlx::query(
            "INSERT INTO recurrence_series_metadata(
                 workspace_id, series_id, field_id, value, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&workspace.id)
        .bind(&series_id)
        .bind(&value.field_id)
        .bind(&value.value)
        .bind(&created_at)
        .bind(&created_at)
        .execute(&mut *tx)
        .await?;
    }
    let create_change_id = append_change(
        &mut tx,
        ChangeEntity::RecurrenceSeries,
        series_id.as_str(),
        None,
        op_type::CREATE_RECURRENCE_SERIES,
        ChangePayload::workspace(workspace)
            .set("series_id", series_id.as_str())
            .set("title", &draft.title)
            .set("description", &draft.description)
            .set("project_id", project.id.as_str())
            .set("project_key", &project.key)
            .set("project_name", &project.name)
            .set("project_prefix", &project.prefix)
            .set("priority", priority.as_str())
            .set("initial_status", initial_status.as_str())
            .set("frequency", draft.schedule.rule.frequency().as_str())
            .set("interval", draft.schedule.rule.interval())
            .set("weekdays", draft.schedule.rule.weekdays_set().to_string())
            .set("timezone", draft.schedule.timezone.as_str())
            .set(
                "start_on",
                draft.schedule.start_on.format("%Y-%m-%d").to_string(),
            )
            .set(
                "available_local_time",
                format_local_time(draft.schedule.available_local_time),
            )
            .set("due_policy", draft.schedule.due_policy.as_str())
            .set("labels", &labels)
            .set("metadata", &series_metadata)
            .set("state", "active")
            .set("stopped_at", "")
            .set("created_at", &created_at)
            .set("updated_at", &created_at),
    )
    .await?;
    for field in SERIES_TEMPLATE_FIELDS {
        set_entity_field_version(
            &mut tx,
            &workspace.id,
            MutableEntityType::RecurrenceSeries,
            series_id.as_str(),
            field,
            &create_change_id,
        )
        .await?;
    }
    for value in &series_metadata {
        set_entity_field_version(
            &mut tx,
            &workspace.id,
            MutableEntityType::RecurrenceSeries,
            series_id.as_str(),
            &format!("metadata:{}", value.field_id),
            &create_change_id,
        )
        .await?;
    }
    let series = load_series(&mut tx, &workspace.id, &series_id).await?;
    let occurrence =
        materialize_occurrence(&mut tx, workspace, &series, &labels, first_slot).await?;
    let task_id = occurrence
        .task_id
        .as_ref()
        .expect("materialized occurrence has a task")
        .clone();
    let task = get_task_in_workspace(&mut tx, workspace, &task_id).await?;
    let series_ref = recurrence_series_ref(&mut tx, &workspace.id, &series_id).await?;
    tx.commit().await?;
    Ok(RecurrenceCreateOutcome {
        series,
        series_ref,
        occurrence,
        task,
    })
}

pub async fn update_recurrence_template(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    series_id: &RecurrenceSeriesId,
    params: UpdateRecurrenceTemplateParams,
) -> Result<RecurrenceTemplateUpdateOutcome> {
    let UpdateRecurrenceTemplateParams {
        update,
        create_missing_labels,
    } = params;
    if let Some(priority) = update.priority.as_deref() {
        TaskPriority::parse(priority)?;
    }
    if let Some(status) = update.initial_status.as_deref() {
        let status = TaskStatus::parse(status)?;
        ensure!(
            status.is_open(),
            CoreError::validation(format!(
                "error recurrence-initial-status-terminal status={}",
                status.as_str()
            ))
        );
    }

    let mut tx = begin_immediate(conn).await?;
    let current = load_series(&mut tx, &workspace.id, series_id).await?;
    crate::metadata::validate_recurrence_metadata_result(
        &mut tx,
        &workspace.id,
        series_id,
        &update.set_metadata,
        &update.remove_metadata,
    )
    .await?;
    let mut values = Vec::<(&str, String)>::new();
    if let Some(title) = update.title
        && title != current.title
    {
        values.push(("title", title));
    }
    if let Some(description) = update.description
        && description != current.description
    {
        values.push(("description", description));
    }
    if let Some(priority) = update.priority
        && priority != current.priority.as_str()
    {
        values.push(("priority", priority));
    }
    if let Some(status) = update.initial_status
        && status != current.initial_status.as_str()
    {
        values.push(("initial_status", status));
    }
    if let Some(time) = update.available_local_time {
        let value = format_local_time(time);
        if value != format_local_time(current.available_local_time) {
            values.push(("available_local_time", value));
        }
    }
    if let Some(due_policy) = update.due_policy
        && due_policy != current.due_policy
    {
        values.push(("due_policy", due_policy.as_str().to_string()));
    }
    if let Some(project) = update.project {
        let project =
            resolve_or_create_project_in_workspace(&mut tx, &workspace.id, &project).await?;
        if project.id != current.project_id {
            values.push(("project", project.id.to_string()));
        }
    }

    let current_labels = load_series_labels(&mut tx, &workspace.id, series_id).await?;
    let target_labels = if let Some(labels) = update.labels {
        if create_missing_labels {
            resolve_or_create_labels_in_workspace(&mut tx, workspace, &labels)
                .await?
                .names
        } else {
            resolve_labels_in_workspace(&mut tx, &workspace.id, &labels).await?
        }
    } else {
        current_labels.clone()
    };
    let labels_changed = current_labels != target_labels;
    let mut metadata_changed = false;
    for input in &update.set_metadata {
        metadata_changed |=
            crate::metadata::set_recurrence_metadata(&mut tx, workspace, series_id, input).await?;
    }
    for key in &update.remove_metadata {
        metadata_changed |=
            crate::metadata::remove_recurrence_metadata(&mut tx, workspace, series_id, key).await?;
    }
    if values.is_empty() && !labels_changed && !metadata_changed {
        tx.commit().await?;
        return Ok(RecurrenceTemplateUpdateOutcome {
            series: current,
            changed: false,
        });
    }
    if values.is_empty() && !labels_changed {
        let series = load_series(&mut tx, &workspace.id, series_id).await?;
        tx.commit().await?;
        return Ok(RecurrenceTemplateUpdateOutcome {
            series,
            changed: true,
        });
    }

    for (field, _) in &values {
        ensure_no_series_conflict(&mut tx, &workspace.id, series_id, field).await?;
    }
    if labels_changed {
        ensure_no_series_conflict(&mut tx, &workspace.id, series_id, "labels").await?;
    }
    let mut base_versions = BTreeMap::new();
    for (field, _) in &values {
        base_versions.insert(
            (*field).to_string(),
            entity_field_version(
                &mut tx,
                &workspace.id,
                MutableEntityType::RecurrenceSeries,
                series_id.as_str(),
                field,
            )
            .await?,
        );
    }
    if labels_changed {
        base_versions.insert(
            "labels".to_string(),
            entity_field_version(
                &mut tx,
                &workspace.id,
                MutableEntityType::RecurrenceSeries,
                series_id.as_str(),
                "labels",
            )
            .await?,
        );
    }
    let updated_at = now();
    for (field, value) in &values {
        update_series_template_scalar(&mut tx, &workspace.id, series_id, field, value, &updated_at)
            .await?;
    }
    if labels_changed {
        sqlx::query(
            "DELETE FROM recurrence_series_labels WHERE workspace_id = ? AND series_id = ?",
        )
        .bind(&workspace.id)
        .bind(series_id)
        .execute(&mut *tx)
        .await?;
        for label in &target_labels {
            sqlx::query(
                "INSERT INTO recurrence_series_labels(workspace_id, series_id, label)
                 VALUES (?, ?, ?)",
            )
            .bind(&workspace.id)
            .bind(series_id)
            .bind(label)
            .execute(&mut *tx)
            .await?;
        }
    }
    let change_id = append_change(
        &mut tx,
        ChangeEntity::RecurrenceSeries,
        series_id.as_str(),
        None,
        op_type::UPDATE_RECURRENCE_TEMPLATE,
        ChangePayload::workspace(workspace)
            .set("fields", &values)
            .set("base_versions", &base_versions)
            .set("labels_changed", labels_changed)
            .set("labels", &target_labels)
            .set("updated_at", &updated_at),
    )
    .await?;
    for (field, _) in &values {
        set_entity_field_version(
            &mut tx,
            &workspace.id,
            MutableEntityType::RecurrenceSeries,
            series_id.as_str(),
            field,
            &change_id,
        )
        .await?;
    }
    if labels_changed {
        set_entity_field_version(
            &mut tx,
            &workspace.id,
            MutableEntityType::RecurrenceSeries,
            series_id.as_str(),
            "labels",
            &change_id,
        )
        .await?;
    }
    let series = load_series(&mut tx, &workspace.id, series_id).await?;
    tx.commit().await?;
    Ok(RecurrenceTemplateUpdateOutcome {
        series,
        changed: true,
    })
}

async fn reconcile_recurrence_series_once(
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

pub async fn pause_recurrence_series(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    series_id: &RecurrenceSeriesId,
    paused_at: &str,
) -> Result<RecurrenceStateOutcome> {
    let paused_at_utc = DateTime::parse_from_rfc3339(paused_at)
        .context("invalid recurrence pause time")?
        .with_timezone(&Utc);
    let mut tx = begin_immediate(conn).await?;
    let series = load_series(&mut tx, &workspace.id, series_id).await?;
    ensure!(
        matches!(series.state, RecurrenceSeriesState::Active),
        CoreError::validation(format!(
            "error recurrence-pause-invalid-state state={}",
            series.state.as_str()
        ))
    );
    ensure_no_series_conflict(&mut tx, &workspace.id, series_id, "state").await?;
    reconcile_recurrence_series_in_transaction(&mut tx, workspace, series_id, paused_at_utc)
        .await?;
    let projected = load_projected_occurrence(&mut tx, &workspace.id, series_id).await?;
    let change_id = set_series_state(
        &mut tx,
        workspace,
        series_id,
        RecurrenceSeriesState::Paused,
        None,
        paused_at,
        op_type::SET_RECURRENCE_STATE,
    )
    .await?;
    let interval_id = new_id();
    sqlx::query(
        "INSERT INTO recurrence_pause_intervals(
            workspace_id, id, series_id, paused_at, resumed_at, suspended_slot_on,
            suspended_task_id, created_by_change_id, resolved_by_change_id
         ) VALUES (?, ?, ?, ?, '', ?, ?, ?, '')",
    )
    .bind(&workspace.id)
    .bind(&interval_id)
    .bind(series_id)
    .bind(paused_at)
    .bind(
        projected
            .as_ref()
            .map(|occurrence| occurrence.slot_on.format("%Y-%m-%d").to_string())
            .unwrap_or_default(),
    )
    .bind(
        projected
            .as_ref()
            .and_then(|occurrence| occurrence.task_id.as_ref())
            .map(TaskId::as_str)
            .unwrap_or(""),
    )
    .bind(&change_id)
    .execute(&mut *tx)
    .await?;
    append_change(
        &mut tx,
        ChangeEntity::RecurrenceSeries,
        series_id.as_str(),
        Some("pause"),
        op_type::OPEN_RECURRENCE_PAUSE,
        ChangePayload::workspace(workspace)
            .set("interval_id", &interval_id)
            .set("paused_at", paused_at)
            .set(
                "suspended_slot_on",
                projected
                    .as_ref()
                    .map(|occurrence| occurrence.slot_on.format("%Y-%m-%d").to_string())
                    .unwrap_or_default(),
            )
            .set(
                "suspended_task_id",
                projected
                    .as_ref()
                    .and_then(|occurrence| occurrence.task_id.as_ref())
                    .map(TaskId::as_str)
                    .unwrap_or(""),
            ),
    )
    .await?;
    let series = load_series(&mut tx, &workspace.id, series_id).await?;
    tx.commit().await?;
    Ok(RecurrenceStateOutcome {
        series,
        occurrence: projected,
    })
}

pub async fn resume_recurrence_series(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    series_id: &RecurrenceSeriesId,
    at: DateTime<Utc>,
) -> Result<RecurrenceStateOutcome> {
    let mut resumed_at = format_utc(at);
    let mut tx = begin_immediate(conn).await?;
    let series = load_series(&mut tx, &workspace.id, series_id).await?;
    ensure!(
        matches!(series.state, RecurrenceSeriesState::Paused),
        CoreError::validation(format!(
            "error recurrence-resume-invalid-state state={}",
            series.state.as_str()
        ))
    );
    ensure_no_series_conflict(&mut tx, &workspace.id, series_id, "state").await?;
    let interval = sqlx::query(
        "SELECT id, paused_at, suspended_slot_on, suspended_task_id
         FROM recurrence_pause_intervals
         WHERE workspace_id = ? AND series_id = ? AND resumed_at = ''",
    )
    .bind(&workspace.id)
    .bind(series_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| CoreError::validation("error recurrence-open-pause-missing"))?;
    let interval_id: String = interval.get("id");
    let paused_at: String = interval.get("paused_at");
    resumed_at = timestamp_strictly_after(&paused_at, &resumed_at)?;
    let suspended_slot_text: String = interval.get("suspended_slot_on");
    let suspended_slot = (!suspended_slot_text.is_empty())
        .then(|| suspended_slot_text.parse::<NaiveDate>())
        .transpose()?;

    let close_change_id = append_change(
        &mut tx,
        ChangeEntity::RecurrenceSeries,
        series_id.as_str(),
        Some("pause"),
        op_type::CLOSE_RECURRENCE_PAUSE,
        ChangePayload::workspace(workspace)
            .set("interval_id", &interval_id)
            .set("paused_at", &paused_at)
            .set("resumed_at", &resumed_at),
    )
    .await?;
    sqlx::query(
        "UPDATE recurrence_pause_intervals SET resumed_at = ?, resolved_by_change_id = ?
         WHERE workspace_id = ? AND id = ? AND resumed_at = ''",
    )
    .bind(&resumed_at)
    .bind(&close_change_id)
    .bind(&workspace.id)
    .bind(&interval_id)
    .execute(&mut *tx)
    .await?;
    set_series_state(
        &mut tx,
        workspace,
        series_id,
        RecurrenceSeriesState::Active,
        None,
        &resumed_at,
        op_type::SET_RECURRENCE_STATE,
    )
    .await?;

    let schedule = series.schedule();
    let mut occurrence = load_projected_occurrence(&mut tx, &workspace.id, series_id).await?;
    let suspended_still_live = if let Some(suspended_slot) = suspended_slot {
        occurrence
            .as_ref()
            .is_some_and(|value| value.slot_on == suspended_slot)
            && at < slot_cutoff(&schedule, suspended_slot)?
    } else {
        false
    };
    if occurrence.is_some() && !suspended_still_live {
        let slot = occurrence.as_ref().expect("checked occurrence").slot_on;
        sqlx::query(
            "UPDATE recurrence_occurrences
             SET projection_state = 'archived', archived_at = ?
             WHERE workspace_id = ? AND series_id = ? AND slot_on = ?
             AND projection_state = 'projected'",
        )
        .bind(&resumed_at)
        .bind(&workspace.id)
        .bind(series_id)
        .bind(slot.format("%Y-%m-%d").to_string())
        .execute(&mut *tx)
        .await?;
        occurrence = None;
    }
    if occurrence.is_none() {
        let mut target = projection_slot_at(&schedule, at)?;
        let boundary = slot_values(&schedule, target)?.boundary_at;
        if boundary >= paused_at && boundary < resumed_at {
            target = next_slot_after(&series.rule, series.start_on, target)
                .context("recurrence schedule has no representable resumed slot")?;
        }
        let labels = load_series_labels(&mut tx, &workspace.id, series_id).await?;
        occurrence =
            Some(materialize_occurrence(&mut tx, workspace, &series, &labels, target).await?);
    }
    let series = load_series(&mut tx, &workspace.id, series_id).await?;
    tx.commit().await?;
    Ok(RecurrenceStateOutcome { series, occurrence })
}

pub async fn stop_recurrence_series(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    series_id: &RecurrenceSeriesId,
    skip_current: bool,
    stopped_at: &str,
) -> Result<RecurrenceStateOutcome> {
    let mut stopped_at = stopped_at.to_string();
    let mut tx = begin_immediate(conn).await?;
    let series = load_series(&mut tx, &workspace.id, series_id).await?;
    ensure!(
        !matches!(series.state, RecurrenceSeriesState::Stopped),
        CoreError::validation("error recurrence-already-stopped")
    );
    ensure_no_series_conflict(&mut tx, &workspace.id, series_id, "state").await?;
    let open_pause = if matches!(series.state, RecurrenceSeriesState::Paused) {
        let interval = sqlx::query(
            "SELECT id, paused_at FROM recurrence_pause_intervals
             WHERE workspace_id = ? AND series_id = ? AND resumed_at = ''",
        )
        .bind(&workspace.id)
        .bind(series_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| CoreError::validation("error recurrence-open-pause-missing"))?;
        let interval_id: String = interval.get("id");
        let paused_at: String = interval.get("paused_at");
        stopped_at = timestamp_strictly_after(&paused_at, &stopped_at)?;
        Some((interval_id, paused_at))
    } else {
        None
    };
    let stopped_at_utc = DateTime::parse_from_rfc3339(&stopped_at)
        .context("invalid recurrence stop time")?
        .with_timezone(&Utc);
    reconcile_recurrence_series_in_transaction(&mut tx, workspace, series_id, stopped_at_utc)
        .await?;
    if let Some((interval_id, paused_at)) = open_pause {
        let close_change_id = append_change(
            &mut tx,
            ChangeEntity::RecurrenceSeries,
            series_id.as_str(),
            Some("pause"),
            op_type::CLOSE_RECURRENCE_PAUSE,
            ChangePayload::workspace(workspace)
                .set("interval_id", &interval_id)
                .set("paused_at", &paused_at)
                .set("resumed_at", &stopped_at),
        )
        .await?;
        sqlx::query(
            "UPDATE recurrence_pause_intervals SET resumed_at = ?, resolved_by_change_id = ?
             WHERE workspace_id = ? AND id = ? AND resumed_at = ''",
        )
        .bind(&stopped_at)
        .bind(&close_change_id)
        .bind(&workspace.id)
        .bind(&interval_id)
        .execute(&mut *tx)
        .await?;
    }
    set_series_state(
        &mut tx,
        workspace,
        series_id,
        RecurrenceSeriesState::Stopped,
        Some(&stopped_at),
        &stopped_at,
        op_type::STOP_RECURRENCE_SERIES,
    )
    .await?;
    let projected = load_projected_occurrence(&mut tx, &workspace.id, series_id).await?;
    let occurrence = if skip_current {
        let projected = projected
            .ok_or_else(|| CoreError::validation("error recurrence-current-occurrence-missing"))?;
        let task_id = projected
            .task_id
            .as_ref()
            .expect("projected occurrence has a task");
        Some(
            resolve_recurrence_occurrence_in_transaction(
                &mut tx,
                workspace,
                task_id,
                RecurrenceOutcome::Skipped,
                &stopped_at,
            )
            .await?
            .resolved,
        )
    } else {
        projected
    };
    let series = load_series(&mut tx, &workspace.id, series_id).await?;
    tx.commit().await?;
    Ok(RecurrenceStateOutcome { series, occurrence })
}

pub(crate) async fn route_recurrence_task_field(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &TaskId,
    field: &str,
    value: &str,
) -> Result<Option<bool>> {
    let Some(mut occurrence) = load_occurrence_for_task(conn, &workspace.id, task_id).await? else {
        return Ok(None);
    };
    if matches!(
        occurrence.projection_state,
        RecurrenceProjectionState::Projected
    ) {
        reconcile_recurrence_series_in_transaction(
            conn,
            workspace,
            &occurrence.series_id,
            Utc::now(),
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
    if field == "status" {
        let task = get_task_in_workspace(conn, workspace, task_id).await?;
        let target = TaskStatus::parse(value)?;
        if task.status == target {
            return Ok(Some(false));
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
            resolve_recurrence_occurrence_in_transaction(conn, workspace, task_id, outcome, &now())
                .await?;
            return Ok(Some(true));
        }
        return Ok(None);
    }
    if field == "deleted"
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
    Ok(None)
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
            ensure_successor_untouched(conn, workspace_id, &series, &successor).await?;
            remove_materialized_occurrence(conn, workspace_id, &series, &successor).await?;
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

async fn materialize_occurrence(
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
async fn verify_materialized_occurrence(
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

async fn set_series_state(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    series_id: &RecurrenceSeriesId,
    state: RecurrenceSeriesState,
    stopped_at: Option<&str>,
    changed_at: &str,
    operation: &'static str,
) -> Result<String> {
    let base = entity_field_version(
        conn,
        &workspace.id,
        MutableEntityType::RecurrenceSeries,
        series_id.as_str(),
        "state",
    )
    .await?;
    sqlx::query(
        "UPDATE recurrence_series SET state = ?, stopped_at = ?, updated_at = ?
         WHERE workspace_id = ? AND id = ?",
    )
    .bind(state.as_str())
    .bind(stopped_at.unwrap_or(""))
    .bind(changed_at)
    .bind(&workspace.id)
    .bind(series_id)
    .execute(&mut *conn)
    .await?;
    let change_id = insert_change(
        conn,
        "recurrence_series",
        series_id.as_str(),
        Some("state"),
        operation,
        ChangePayload::workspace(workspace)
            .set("state", state.as_str())
            .set("stopped_at", stopped_at.unwrap_or(""))
            .set("changed_at", changed_at)
            .into_value(),
        base.as_deref(),
    )
    .await?;
    set_entity_field_version(
        conn,
        &workspace.id,
        MutableEntityType::RecurrenceSeries,
        series_id.as_str(),
        "state",
        &change_id,
    )
    .await?;
    if matches!(state, RecurrenceSeriesState::Stopped) {
        set_entity_field_version(
            conn,
            &workspace.id,
            MutableEntityType::RecurrenceSeries,
            series_id.as_str(),
            "stopped_at",
            &change_id,
        )
        .await?;
    }
    Ok(change_id)
}

async fn update_series_template_scalar(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series_id: &RecurrenceSeriesId,
    field: &str,
    value: &str,
    updated_at: &str,
) -> Result<()> {
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
) -> Result<()> {
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
    let workspace = crate::workspaces::workspace_for_id(conn, workspace_id).await?;
    let labels = load_series_labels(conn, workspace_id, &series.id).await?;
    let metadata = load_series_metadata(conn, workspace_id, &series.id).await?;
    let slot = slot_values(&series.schedule(), occurrence.slot_on)?;
    verify_materialized_occurrence(
        conn, &workspace, series, &labels, &metadata, &slot, &identity, occurrence,
    )
    .await
    .map_err(|_| anyhow::anyhow!("error recurrence-undo-successor-touched"))?;
    ensure_change_unsynced(conn, &identity.task_change_id).await?;
    ensure_change_unsynced(conn, &identity.occurrence_change_id).await?;
    let extra_changes: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM changes
         WHERE entity_type = 'task' AND entity_id = ? AND change_id != ?",
    )
    .bind(task_id)
    .bind(&identity.task_change_id)
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
    Ok(())
}

async fn remove_materialized_occurrence(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series: &RecurrenceSeries,
    occurrence: &RecurrenceOccurrence,
) -> Result<()> {
    let task_id = occurrence
        .task_id
        .as_ref()
        .expect("verified successor has task");
    let identity = derive_occurrence_identity(
        workspace_id,
        &series.id,
        &series.schedule(),
        occurrence.slot_on,
    )?;
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
        .bind(&identity.task_change_id)
        .bind(&identity.occurrence_change_id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

fn retryable_reconcile_error(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<sqlx::Error>()
        .and_then(sqlx::Error::as_database_error)
        .and_then(|error| error.code())
        .is_some_and(|code| matches!(code.as_ref(), "5" | "6" | "19"))
}

pub(crate) fn timestamp_strictly_after(earlier: &str, candidate: &str) -> Result<String> {
    if candidate > earlier {
        return Ok(candidate.to_string());
    }
    let earlier = DateTime::parse_from_rfc3339(earlier)?.with_timezone(&Utc);
    let adjusted = earlier
        .checked_add_signed(chrono::Duration::seconds(1))
        .context("recurrence lifecycle timestamp is out of range")?;
    Ok(format_utc(adjusted))
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
