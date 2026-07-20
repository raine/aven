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
use crate::ids::{TaskId, WorkspaceId, new_id, now};
use crate::labels::resolve_labels_in_workspace;
use crate::mutation::apply_field_value_in_workspace;
use crate::projects::resolve_or_create_project_in_workspace;
use crate::recurrence::{
    RecurrenceDuePolicy, RecurrenceOutcome, RecurrenceProjectionState, RecurrenceRule,
    RecurrenceSchedule, RecurrenceSeriesId, RecurrenceSeriesState, TimeZoneId,
    derive_occurrence_identity, is_slot, live_slot_on, next_slot_after, slot_cutoff, slot_values,
};
use crate::refs::get_task_in_workspace;
use crate::task_fields::TaskField;
use crate::types::{MutableEntityType, RecurrenceOccurrence, RecurrenceSeries, Task};
use crate::workspaces::Workspace;

#[cfg(test)]
#[path = "recurrence_tests.rs"]
mod tests;

const RECONCILE_ATTEMPTS: usize = 3;
const SERIES_REF_PREFIX: &str = "RCR";
const DISPLAY_SUFFIX_FLOOR: usize = 4;
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
    pub schedule: RecurrenceSchedule,
}

#[derive(Debug, Clone, Default)]
pub struct RecurrenceTemplateUpdate {
    pub title: Option<String>,
    pub description: Option<String>,
    pub project: Option<String>,
    pub priority: Option<String>,
    pub initial_status: Option<String>,
    pub labels: Option<Vec<String>>,
    pub available_local_time: Option<Option<NaiveTime>>,
    pub due_policy: Option<RecurrenceDuePolicy>,
    pub rule: Option<RecurrenceRule>,
    pub start_on: Option<NaiveDate>,
    pub timezone: Option<TimeZoneId>,
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

#[derive(Debug, Clone)]
pub struct RecurrenceRecordOutcome {
    pub series: RecurrenceSeries,
    pub occurrence: RecurrenceOccurrence,
}

impl Database {
    pub async fn create_recurrence_series(
        &self,
        workspace: &Workspace,
        draft: RecurrenceSeriesDraft,
    ) -> Result<RecurrenceCreateOutcome> {
        self.create_recurrence_series_at(workspace, draft, utc_now()?)
            .await
    }

    pub async fn create_recurrence_series_at(
        &self,
        workspace: &Workspace,
        draft: RecurrenceSeriesDraft,
        at: DateTime<Utc>,
    ) -> Result<RecurrenceCreateOutcome> {
        let mut conn = self.acquire().await?;
        create_recurrence_series(&mut conn, workspace, draft, at).await
    }

    pub async fn update_recurrence_template(
        &self,
        workspace: &Workspace,
        series_id: &RecurrenceSeriesId,
        update: RecurrenceTemplateUpdate,
    ) -> Result<RecurrenceTemplateUpdateOutcome> {
        let mut conn = self.acquire().await?;
        update_recurrence_template(&mut conn, workspace, series_id, update).await
    }

    pub async fn reconcile_recurrence_series(
        &self,
        workspace: &Workspace,
        series_id: &RecurrenceSeriesId,
        at: DateTime<Utc>,
    ) -> Result<RecurrenceReconcileOutcome> {
        let mut conn = self.acquire().await?;
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
        let mut conn = self.acquire().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let result = resolve_recurrence_occurrence_in_transaction(
            &mut tx,
            workspace,
            task_id,
            outcome,
            &now(),
        )
        .await?;
        tx.commit().await?;
        Ok(result)
    }

    pub async fn record_recurrence_outcome(
        &self,
        workspace: &Workspace,
        series_id: &RecurrenceSeriesId,
        slot_on: NaiveDate,
        outcome: RecurrenceOutcome,
        resolved_at: String,
        at: DateTime<Utc>,
    ) -> Result<RecurrenceRecordOutcome> {
        let mut conn = self.acquire().await?;
        record_recurrence_outcome(
            &mut conn,
            workspace,
            series_id,
            slot_on,
            outcome,
            resolved_at,
            at,
        )
        .await
    }

    pub async fn pause_recurrence_series(
        &self,
        workspace: &Workspace,
        series_id: &RecurrenceSeriesId,
    ) -> Result<RecurrenceStateOutcome> {
        let mut conn = self.acquire().await?;
        pause_recurrence_series(&mut conn, workspace, series_id, &now()).await
    }

    pub async fn resume_recurrence_series(
        &self,
        workspace: &Workspace,
        series_id: &RecurrenceSeriesId,
        at: DateTime<Utc>,
    ) -> Result<RecurrenceStateOutcome> {
        let mut conn = self.acquire().await?;
        resume_recurrence_series(&mut conn, workspace, series_id, at).await
    }

    pub async fn stop_recurrence_series(
        &self,
        workspace: &Workspace,
        series_id: &RecurrenceSeriesId,
        skip_current: bool,
    ) -> Result<RecurrenceStateOutcome> {
        let mut conn = self.acquire().await?;
        stop_recurrence_series(&mut conn, workspace, series_id, skip_current, &now()).await
    }

    pub async fn resolve_recurrence_ref(
        &self,
        workspace: &Workspace,
        input: &str,
    ) -> Result<RecurrenceSeries> {
        let mut conn = self.acquire().await?;
        resolve_recurrence_ref(&mut conn, workspace, input).await
    }

    pub async fn recurrence_series_ref(
        &self,
        workspace_id: &WorkspaceId,
        series_id: &RecurrenceSeriesId,
    ) -> Result<String> {
        let mut conn = self.acquire().await?;
        recurrence_series_ref(&mut conn, workspace_id, series_id).await
    }
}

pub async fn create_recurrence_series(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    draft: RecurrenceSeriesDraft,
    at: DateTime<Utc>,
) -> Result<RecurrenceCreateOutcome> {
    let priority = TaskPriority::parse(&draft.priority)?;
    let initial_status = TaskStatus::parse(&draft.initial_status)?;
    ensure!(
        initial_status.is_open(),
        "error recurrence-initial-status-terminal status={}",
        initial_status.as_str()
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
    let labels = resolve_labels_in_workspace(&mut tx, &workspace.id, &draft.labels).await?;
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
            .set("created_at", &created_at),
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
    update: RecurrenceTemplateUpdate,
) -> Result<RecurrenceTemplateUpdateOutcome> {
    if update.rule.is_some() || update.start_on.is_some() || update.timezone.is_some() {
        bail!(
            "error recurrence-schedule-immutable hint=\"stop this series and create a replacement\""
        );
    }
    if let Some(priority) = update.priority.as_deref() {
        TaskPriority::parse(priority)?;
    }
    if let Some(status) = update.initial_status.as_deref() {
        let status = TaskStatus::parse(status)?;
        ensure!(
            status.is_open(),
            "error recurrence-initial-status-terminal status={}",
            status.as_str()
        );
    }

    let mut tx = begin_immediate(conn).await?;
    let current = load_series(&mut tx, &workspace.id, series_id).await?;
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
        resolve_labels_in_workspace(&mut tx, &workspace.id, &labels).await?
    } else {
        current_labels.clone()
    };
    let labels_changed = current_labels != target_labels;
    if values.is_empty() && !labels_changed {
        tx.commit().await?;
        return Ok(RecurrenceTemplateUpdateOutcome {
            series: current,
            changed: false,
        });
    }

    for (field, _) in &values {
        ensure_no_series_conflict(&mut tx, &workspace.id, series_id, field).await?;
    }
    if labels_changed {
        ensure_no_series_conflict(&mut tx, &workspace.id, series_id, "labels").await?;
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

async fn reconcile_recurrence_series_in_transaction(
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

    let schedule = schedule_for_series(&series);
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
    let labels = load_series_labels(conn, &workspace.id, series_id).await?;
    let occurrence = materialize_occurrence(conn, workspace, &series, &labels, target).await?;
    append_change(
        conn,
        ChangeEntity::RecurrenceSeries,
        series_id.as_str(),
        None,
        op_type::PROJECT_RECURRENCE_OCCURRENCE,
        ChangePayload::workspace(workspace)
            .set("slot_on", target.format("%Y-%m-%d").to_string())
            .set("reconciled_at", &changed_at),
    )
    .await?;
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
    let occurrence = load_occurrence_for_task(conn, &workspace.id, task_id)
        .await?
        .context("error recurrence-occurrence-not-found")?;
    ensure!(
        matches!(
            occurrence.projection_state,
            RecurrenceProjectionState::Projected
        ),
        "error recurrence-occurrence-not-current task_id={task_id}"
    );
    let task = get_task_in_workspace(conn, workspace, task_id).await?;
    ensure!(
        task.status.is_open(),
        "error recurrence-occurrence-terminal task_id={task_id}"
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
            derive_occurrence_identity(
                &workspace.id,
                &series.id,
                &schedule_for_series(&series),
                slot,
            )
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
            .set("resolved_at", resolved_at)
            .set("task_status_change_id", &status_change_id)
            .set(
                "successor_task_id",
                successor_task_id.as_ref().map(TaskId::as_str).unwrap_or(""),
            ),
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

pub async fn record_recurrence_outcome(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    series_id: &RecurrenceSeriesId,
    slot_on: NaiveDate,
    outcome: RecurrenceOutcome,
    resolved_at: String,
    at: DateTime<Utc>,
) -> Result<RecurrenceRecordOutcome> {
    DateTime::parse_from_rfc3339(&resolved_at).context("invalid recurrence resolution time")?;
    let mut tx = begin_immediate(conn).await?;
    let series = load_series(&mut tx, &workspace.id, series_id).await?;
    let schedule = schedule_for_series(&series);
    ensure!(
        is_slot(&series.rule, series.start_on, slot_on),
        "error recurrence-slot-off-lattice slot={slot_on}"
    );
    let current = projection_slot_at(&schedule, at)?;
    ensure!(
        slot_on < current,
        "error recurrence-correction-not-past slot={slot_on}"
    );
    let created_date = DateTime::parse_from_rfc3339(&series.created_at)?
        .with_timezone(&series.timezone.timezone())
        .date_naive();
    ensure!(
        slot_on >= created_date && slot_on >= series.start_on,
        "error recurrence-slot-outside-lifetime slot={slot_on}"
    );
    if let Some(stopped_at) = series.stopped_at.as_deref() {
        let stop = DateTime::parse_from_rfc3339(stopped_at)?.with_timezone(&Utc);
        ensure!(
            slot_values(&schedule, slot_on)?.boundary_at < format_utc(stop),
            "error recurrence-slot-outside-lifetime slot={slot_on}"
        );
    }
    ensure!(
        !slot_is_paused(&mut tx, &workspace.id, series_id, slot_on).await?,
        "error recurrence-slot-paused slot={slot_on}"
    );
    ensure!(
        load_occurrence(&mut tx, &workspace.id, series_id, slot_on)
            .await?
            .is_none(),
        "error recurrence-outcome-exists slot={slot_on}"
    );
    let change_id = append_change(
        &mut tx,
        ChangeEntity::RecurrenceSeries,
        series_id.as_str(),
        Some("outcome"),
        op_type::RECORD_RECURRENCE_OUTCOME,
        ChangePayload::workspace(workspace)
            .set("slot_on", slot_on.format("%Y-%m-%d").to_string())
            .set("outcome", outcome.as_str())
            .set("resolved_at", &resolved_at),
    )
    .await?;
    sqlx::query(
        "INSERT INTO recurrence_occurrences(
            workspace_id, series_id, slot_on, task_id, outcome, resolved_at,
            outcome_change_id, projection_state, archived_at
         ) VALUES (?, ?, ?, '', ?, ?, ?, 'corrected', '')",
    )
    .bind(&workspace.id)
    .bind(series_id)
    .bind(slot_on.format("%Y-%m-%d").to_string())
    .bind(outcome.as_str())
    .bind(&resolved_at)
    .bind(&change_id)
    .execute(&mut *tx)
    .await?;
    let occurrence = load_occurrence(&mut tx, &workspace.id, series_id, slot_on)
        .await?
        .expect("corrected occurrence was inserted");
    tx.commit().await?;
    Ok(RecurrenceRecordOutcome { series, occurrence })
}

pub async fn pause_recurrence_series(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    series_id: &RecurrenceSeriesId,
    paused_at: &str,
) -> Result<RecurrenceStateOutcome> {
    DateTime::parse_from_rfc3339(paused_at).context("invalid recurrence pause time")?;
    let mut tx = begin_immediate(conn).await?;
    let series = load_series(&mut tx, &workspace.id, series_id).await?;
    ensure!(
        matches!(series.state, RecurrenceSeriesState::Active),
        "error recurrence-pause-invalid-state state={}",
        series.state.as_str()
    );
    ensure_no_series_conflict(&mut tx, &workspace.id, series_id, "state").await?;
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
        "error recurrence-resume-invalid-state state={}",
        series.state.as_str()
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
    .context("error recurrence-open-pause-missing")?;
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

    let schedule = schedule_for_series(&series);
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
    DateTime::parse_from_rfc3339(stopped_at).context("invalid recurrence stop time")?;
    let mut tx = begin_immediate(conn).await?;
    let series = load_series(&mut tx, &workspace.id, series_id).await?;
    ensure!(
        !matches!(series.state, RecurrenceSeriesState::Stopped),
        "error recurrence-already-stopped"
    );
    ensure_no_series_conflict(&mut tx, &workspace.id, series_id, "state").await?;
    if matches!(series.state, RecurrenceSeriesState::Paused) {
        let close_change_id = append_change(
            &mut tx,
            ChangeEntity::RecurrenceSeries,
            series_id.as_str(),
            Some("pause"),
            op_type::CLOSE_RECURRENCE_PAUSE,
            ChangePayload::workspace(workspace).set("resumed_at", stopped_at),
        )
        .await?;
        sqlx::query(
            "UPDATE recurrence_pause_intervals SET resumed_at = ?, resolved_by_change_id = ?
             WHERE workspace_id = ? AND series_id = ? AND resumed_at = ''",
        )
        .bind(stopped_at)
        .bind(&close_change_id)
        .bind(&workspace.id)
        .bind(series_id)
        .execute(&mut *tx)
        .await?;
    }
    set_series_state(
        &mut tx,
        workspace,
        series_id,
        RecurrenceSeriesState::Stopped,
        Some(stopped_at),
        stopped_at,
        op_type::STOP_RECURRENCE_SERIES,
    )
    .await?;
    let projected = load_projected_occurrence(&mut tx, &workspace.id, series_id).await?;
    let occurrence = if skip_current {
        let projected = projected.context("error recurrence-current-occurrence-missing")?;
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
                stopped_at,
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
    let Some(occurrence) = load_occurrence_for_task(conn, &workspace.id, task_id).await? else {
        return Ok(None);
    };
    if matches!(
        occurrence.projection_state,
        RecurrenceProjectionState::Archived
    ) {
        bail!("error recurrence-occurrence-archived task_id={task_id}");
    }
    if field == "status" {
        let task = get_task_in_workspace(conn, workspace, task_id).await?;
        let target = TaskStatus::parse(value)?;
        if task.status == target {
            return Ok(Some(false));
        }
        if task.status.is_terminal() {
            if target.is_open() {
                bail!(
                    "error recurrence-terminal-reopen task_id={task_id} hint=\"use immediate undo or record a historical correction\""
                );
            }
            bail!("error recurrence-outcome-correction-required task_id={task_id}");
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
        bail!("error recurrence-current-delete task_id={task_id} hint=\"{guidance}\"");
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
    let schedule = schedule_for_series(series);
    let slot = slot_values(&schedule, slot_on)?;
    let identity = derive_occurrence_identity(&workspace.id, &series.id, &schedule, slot_on)?;
    if let Some(existing) = load_occurrence(conn, &workspace.id, &series.id, slot_on).await? {
        verify_materialized_occurrence(
            conn, workspace, series, labels, &slot, &identity, &existing,
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

    let payload = deterministic_task_payload(workspace, series, labels, &slot, &identity.task_id);
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
    load_occurrence(conn, &workspace.id, &series.id, slot_on)
        .await?
        .context("materialized recurrence occurrence missing")
}

async fn verify_materialized_occurrence(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    series: &RecurrenceSeries,
    labels: &[String],
    slot: &crate::recurrence::RecurrenceSlot,
    identity: &crate::recurrence::RecurrenceOccurrenceIdentity,
    occurrence: &RecurrenceOccurrence,
) -> Result<()> {
    ensure!(
        occurrence.task_id.as_ref() == Some(&identity.task_id),
        "error recurrence-generation-conflict slot={} field=task_id",
        occurrence.slot_on
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
        "error recurrence-generation-conflict slot={} field=task",
        occurrence.slot_on
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
        "error recurrence-generation-conflict slot={} field=labels",
        occurrence.slot_on
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
            "error recurrence-generation-conflict slot={} field=field_versions",
            occurrence.slot_on
        );
    }
    let payload = deterministic_task_payload(workspace, series, labels, slot, &identity.task_id);
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
    slot: &crate::recurrence::RecurrenceSlot,
    task_id: &TaskId,
) -> Value {
    ChangePayload::workspace(workspace)
        .set("task_id", task_id.as_str())
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
        .set("created_at", &slot.boundary_at)
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
    .with_context(|| format!("error recurrence-series-not-found series_id={series_id}"))?;
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

fn schedule_for_series(series: &RecurrenceSeries) -> RecurrenceSchedule {
    RecurrenceSchedule::new(
        series.rule,
        series.timezone.clone(),
        series.start_on,
        series.available_local_time,
        series.due_policy,
    )
}

fn projection_slot_at(schedule: &RecurrenceSchedule, at: DateTime<Utc>) -> Result<NaiveDate> {
    if let Some(slot) = live_slot_on(&schedule.rule, schedule.start_on, at, &schedule.timezone) {
        return Ok(slot);
    }
    schedule
        .slots_on_or_after(schedule.start_on)
        .next()
        .context("recurrence schedule has no representable projection")
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
        "error recurrence-conflicted-field series_id={series_id} field={field}"
    );
    Ok(())
}

async fn slot_is_paused(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    series_id: &RecurrenceSeriesId,
    slot_on: NaiveDate,
) -> Result<bool> {
    let boundaries = sqlx::query(
        "SELECT paused_at, resumed_at FROM recurrence_pause_intervals
         WHERE workspace_id = ? AND series_id = ?",
    )
    .bind(workspace_id)
    .bind(series_id)
    .fetch_all(&mut *conn)
    .await?;
    let series = load_series(conn, workspace_id, series_id).await?;
    let boundary = slot_values(&schedule_for_series(&series), slot_on)?.boundary_at;
    Ok(boundaries.into_iter().any(|row| {
        let paused_at: String = row.get("paused_at");
        let resumed_at: String = row.get("resumed_at");
        boundary >= paused_at && (resumed_at.is_empty() || boundary < resumed_at)
    }))
}

pub async fn resolve_recurrence_ref(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    input: &str,
) -> Result<RecurrenceSeries> {
    let (hint, suffix) = split_ref(input);
    if suffix.len() < 3 {
        bail!("error recurrence-ref-too-short input={input} minimum=3");
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
        bail!("error ambiguous-recurrence-ref input={input}");
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
    let ids: Vec<String> =
        sqlx::query_scalar("SELECT id FROM recurrence_series WHERE workspace_id = ? ORDER BY id")
            .bind(workspace_id)
            .fetch_all(&mut *conn)
            .await?;
    let id = series_id.as_str();
    ensure!(
        ids.iter().any(|candidate| candidate == id),
        "error recurrence-series-not-found series_id={series_id}"
    );
    let shared = ids
        .iter()
        .filter(|candidate| candidate.as_str() != id)
        .map(|candidate| common_prefix_len(id, candidate))
        .max()
        .unwrap_or(0);
    let length = DISPLAY_SUFFIX_FLOOR
        .max(shared.saturating_add(1))
        .min(id.len());
    Ok(format!("{SERIES_REF_PREFIX}-{}", &id[..length]))
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

fn common_prefix_len(left: &str, right: &str) -> usize {
    left.bytes()
        .zip(right.bytes())
        .take_while(|(left, right)| left == right)
        .count()
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
        &schedule_for_series(series),
        occurrence.slot_on,
    )?;
    ensure!(
        task_id == &identity.task_id,
        "error recurrence-undo-successor-changed"
    );
    let workspace = crate::workspaces::workspace_for_id(conn, workspace_id).await?;
    let labels = load_series_labels(conn, workspace_id, &series.id).await?;
    let slot = slot_values(&schedule_for_series(series), occurrence.slot_on)?;
    verify_materialized_occurrence(
        conn, &workspace, series, &labels, &slot, &identity, occurrence,
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
    ensure!(
        extra_changes == 0 && notes == 0 && attachments == 0 && dependencies == 0 && epics == 0,
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
        &schedule_for_series(series),
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

fn timestamp_strictly_after(earlier: &str, candidate: &str) -> Result<String> {
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
    Ok(DateTime::parse_from_rfc3339(&now())?.with_timezone(&Utc))
}
