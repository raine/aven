//! Task tools, independent of any MCP transport.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use aven_core::choices::{TaskPriority, TaskSource, TaskStatus};
use aven_core::db::Database;
use aven_core::operations::{
    CreateRecurrenceSeriesParams, RecurrenceSeriesDraft, RecurrenceTemplateUpdate,
    TaskCreationOptions, TaskCreationUndo, TaskDraft, TaskUpdate, UpdateRecurrenceTemplateParams,
};
use aven_core::query::{
    SortDirection, TaskAvailabilityFilter, TaskFilters, TaskListItem, TaskQueryMode,
    TaskSearchQuery, TaskSort,
};
use aven_core::types::Task;
use chrono::NaiveDate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::commands::skip_current_occurrence;
use crate::config::AppConfig;
use crate::daemon;
use crate::recurrence_input::{self, RecurrenceClock};
use crate::task_render::{build_full_task_report, task_full_json};
use crate::workspaces::{Workspace, resolve_default_workspace};

const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 200;
const DEFAULT_PROJECT: &str = "default";

/// A task error reported to the assistant so it can correct itself.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub(super) struct ToolError {
    error: String,
    message: String,
    hint: String,
}

impl ToolError {
    pub(super) fn invalid_arguments(message: impl Into<String>) -> Self {
        Self {
            error: "invalid-arguments".to_string(),
            message: message.into(),
            hint: "Fix the arguments to match the tool's input schema and retry.".to_string(),
        }
    }

    pub(super) fn from_anyhow(error: &anyhow::Error) -> Self {
        let message = format!("{error:#}");
        let code = message
            .strip_prefix("error ")
            .and_then(|rest| rest.split_whitespace().next())
            .unwrap_or("task-error")
            .to_string();
        let hint = retry_hint(&code)
            .map(str::to_string)
            .or_else(|| embedded_hint(&message))
            .unwrap_or_else(|| "Check the arguments and retry.".to_string());
        Self {
            error: code,
            message,
            hint,
        }
    }
}

fn retry_hint(code: &str) -> Option<&'static str> {
    match code {
        "unknown-ref" => Some(
            "Call search_tasks to find the task, or pass workspace if it is in another workspace.",
        ),
        "ambiguous-ref" => Some("Retry with one of the listed refs."),
        "unknown-project" => Some("Call list_projects and retry with an existing project key."),
        "unknown-workspace" | "workspace-required" => {
            Some("Call list_workspaces and retry with a workspace key.")
        }
        "unknown-recurrence-ref" => Some("Call list_recurring and retry with a series ref."),
        _ => None,
    }
}

/// Reads the `hint="..."` or trailing `hint "..."` of an Aven error message.
fn embedded_hint(message: &str) -> Option<String> {
    let start = message
        .find("hint=\"")
        .map(|index| index + "hint=\"".len())
        .or_else(|| message.find("hint \"").map(|index| index + "hint \"".len()))?;
    let hint = &message[start..];
    Some(hint[..hint.find('"')?].to_string())
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub(super) enum TaskView {
    Open,
    Ready,
    Blocked,
    Overdue,
    Upcoming,
    All,
    Deleted,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct TaskFilterArgs {
    /// Workspace name or key. Omit for the default workspace.
    workspace: Option<String>,
    /// Project key. Omit for all projects in the workspace.
    project: Option<String>,
    /// open (default without status): open tasks available now; ready: unblocked; blocked; overdue (needs today); upcoming: scheduled for later; all: including done; deleted.
    view: Option<TaskView>,
    /// inbox, backlog, todo, active, done, or canceled.
    status: Option<String>,
    /// none, low, medium, high, or urgent.
    priority: Option<String>,
    /// Label name.
    label: Option<String>,
    /// The user's current date, YYYY-MM-DD. Required for the overdue view.
    today: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct ListTasksArgs {
    #[serde(flatten)]
    filters: TaskFilterArgs,
    /// Maximum rows, 1 to 200. Defaults to 50.
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct SearchTasksArgs {
    /// Search text, matched against refs, titles, labels, projects, descriptions, and notes.
    query: String,
    /// Workspace name or key. Omit for the default workspace.
    workspace: Option<String>,
    /// Project key. Omit to search all projects in the workspace.
    project: Option<String>,
    /// Include deleted tasks.
    include_deleted: Option<bool>,
    /// Maximum rows, 1 to 200. Defaults to 50.
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct TaskRefArgs {
    /// Task ref, such as APP-7KQ9.
    r#ref: String,
    /// Workspace name or key. Omit for the default workspace.
    workspace: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct CreateTaskArgs {
    title: String,
    description: Option<String>,
    /// Existing project key. Omit for the default project.
    project: Option<String>,
    /// inbox, backlog, todo, or active. done and canceled also work without repeat.
    status: Option<String>,
    /// none, low, medium, high, or urgent.
    priority: Option<String>,
    /// Labels, created when missing.
    labels: Option<Vec<String>>,
    /// Due date, YYYY-MM-DD. Not valid with repeat.
    due: Option<String>,
    /// When the task becomes available, YYYY-MM-DDTHH:MM:SSZ. Not valid with repeat.
    available_at: Option<String>,
    /// Makes the task recur: daily, every 3 days, weekdays, every Friday, monthly, or yearly.
    repeat: Option<String>,
    /// The user's IANA time zone, such as Europe/Stockholm. Required with repeat.
    time_zone: Option<String>,
    /// Local time each occurrence becomes available, HH:MM.
    repeat_at: Option<String>,
    /// First date of the series, YYYY-MM-DD. Defaults to today in time_zone.
    repeat_start_on: Option<String>,
    /// Workspace name or key. Omit for the default workspace.
    workspace: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct UpdateTaskArgs {
    /// Task ref, such as APP-7KQ9.
    r#ref: String,
    title: Option<String>,
    /// Replaces the description.
    description: Option<String>,
    /// Existing project key to move the task to.
    project: Option<String>,
    /// inbox, backlog, todo, active, done, or canceled. done completes a recurring occurrence.
    status: Option<String>,
    /// none, low, medium, high, or urgent.
    priority: Option<String>,
    /// Due date, YYYY-MM-DD, or none to clear it.
    due: Option<String>,
    /// Availability, YYYY-MM-DDTHH:MM:SSZ, or none to clear it.
    available_at: Option<String>,
    /// Labels to add, created when missing.
    labels_add: Option<Vec<String>>,
    /// Labels to remove.
    labels_remove: Option<Vec<String>>,
    /// Workspace name or key. Omit for the default workspace.
    workspace: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct AddNoteArgs {
    /// Task ref, such as APP-7KQ9.
    r#ref: String,
    /// Note text.
    body: String,
    /// Workspace name or key. Omit for the default workspace.
    workspace: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct WorkspaceArgs {
    /// Workspace name or key. Omit for the default workspace.
    workspace: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct ListRecurringArgs {
    /// Workspace name or key. Omit for the default workspace.
    workspace: Option<String>,
    /// Maximum rows, 1 to 200. Defaults to 50.
    limit: Option<usize>,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub(super) enum RecurringAction {
    Skip,
    Pause,
    Resume,
    Stop,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct EditRecurringArgs {
    /// Series ref, or the ref of one of its occurrences.
    r#ref: String,
    /// skip the current occurrence, or pause, resume, or stop the series. Cannot combine with template fields.
    action: Option<RecurringAction>,
    /// With stop, also skip the current occurrence.
    skip_current: Option<bool>,
    title: Option<String>,
    description: Option<String>,
    /// Existing project key.
    project: Option<String>,
    /// none, low, medium, high, or urgent.
    priority: Option<String>,
    /// Status of future occurrences: inbox, backlog, todo, or active.
    initial_status: Option<String>,
    /// Replaces the labels of future occurrences.
    labels: Option<Vec<String>>,
    /// Local availability time, HH:MM, or none.
    repeat_at: Option<String>,
    /// same-day or none.
    repeat_due: Option<String>,
    /// Workspace name or key. Omit for the default workspace.
    workspace: Option<String>,
}

impl EditRecurringArgs {
    fn has_template_fields(&self) -> bool {
        self.title.is_some()
            || self.description.is_some()
            || self.project.is_some()
            || self.priority.is_some()
            || self.initial_status.is_some()
            || self.labels.is_some()
            || self.repeat_at.is_some()
            || self.repeat_due.is_some()
    }
}

#[derive(Serialize)]
struct TaskRow {
    r#ref: String,
    title: String,
    status: String,
    priority: String,
    project: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    labels: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    due_on: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    available_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    series: Option<String>,
    #[serde(skip_serializing_if = "is_zero")]
    blocked_by: i64,
    #[serde(skip_serializing_if = "is_false")]
    epic: bool,
    #[serde(skip_serializing_if = "is_false")]
    conflict: bool,
    #[serde(skip_serializing_if = "is_false")]
    deleted: bool,
}

fn is_zero(value: &i64) -> bool {
    *value == 0
}

fn is_false(value: &bool) -> bool {
    !value
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value.filter(|value| !value.is_empty()).map(str::to_string)
}

impl TaskRow {
    fn from_task(task: &Task, display_ref: String) -> Self {
        Self {
            r#ref: display_ref,
            title: task.title.clone(),
            status: task.status.to_string(),
            priority: task.priority.to_string(),
            project: task.project_key.clone(),
            labels: Vec::new(),
            due_on: non_empty(task.due_on.as_deref()),
            available_at: non_empty(task.available_at.as_deref()),
            series: None,
            blocked_by: 0,
            epic: task.is_epic,
            conflict: false,
            deleted: task.deleted,
        }
    }

    fn from_item(item: &TaskListItem) -> Self {
        Self {
            labels: item.labels.clone(),
            series: item
                .recurrence
                .as_ref()
                .map(|recurrence| recurrence.series_ref.clone()),
            blocked_by: item.unresolved_blocker_count,
            conflict: item.has_conflict,
            ..Self::from_task(&item.task, item.display_ref.clone())
        }
    }
}

fn limit_or_default(limit: Option<usize>) -> Result<usize> {
    let limit = limit.unwrap_or(DEFAULT_LIMIT);
    if limit == 0 || limit > MAX_LIMIT {
        bail!("error invalid-limit value={limit} hint=\"use a limit from 1 to {MAX_LIMIT}\"");
    }
    Ok(limit)
}

/// Trims a `limit + 1` read to `limit` rows and reports whether any were cut.
fn truncate<T>(rows: &mut Vec<T>, limit: usize) -> bool {
    let truncated = rows.len() > limit;
    rows.truncate(limit);
    truncated
}

fn validate_status(status: Option<&str>) -> Result<()> {
    if let Some(status) = status {
        TaskStatus::parse(status)?;
    }
    Ok(())
}

fn validate_priority(priority: Option<&str>) -> Result<()> {
    if let Some(priority) = priority {
        TaskPriority::parse(priority)?;
    }
    Ok(())
}

/// A date field update: `none` clears the value and anything else sets it.
fn date_field_update(value: Option<String>, field: &str) -> Result<Option<Option<String>>> {
    match value.as_deref() {
        None => Ok(None),
        Some("none") => Ok(Some(None)),
        Some("") => bail!("error invalid-{field} hint=\"pass a value, or none to clear it\""),
        Some(_) => Ok(Some(value)),
    }
}

fn task_filters(args: &TaskFilterArgs) -> Result<(TaskFilters, TaskSort, SortDirection)> {
    validate_status(args.status.as_deref())?;
    validate_priority(args.priority.as_deref())?;
    let view = args.view.unwrap_or(if args.status.is_some() {
        TaskView::All
    } else {
        TaskView::Open
    });
    let today = args
        .today
        .as_deref()
        .map(|today| {
            today.parse::<NaiveDate>().with_context(|| {
                format!("error invalid-today value={today:?} hint=\"use YYYY-MM-DD\"")
            })
        })
        .transpose()?;
    if matches!(view, TaskView::Overdue) && today.is_none() {
        bail!("error today-required hint=\"pass today as the user's current date, YYYY-MM-DD\"");
    }
    let availability = match view {
        TaskView::Upcoming => TaskAvailabilityFilter::Upcoming,
        TaskView::All | TaskView::Deleted => TaskAvailabilityFilter::All,
        _ => TaskAvailabilityFilter::Available,
    };
    let filters = TaskFilters {
        ready_only: matches!(view, TaskView::Ready),
        exclude_epics: matches!(view, TaskView::Ready),
        blocked_only: matches!(view, TaskView::Blocked),
        overdue_only: matches!(view, TaskView::Overdue),
        hide_done: matches!(view, TaskView::Open),
        today,
        availability,
        expand_recurring: true,
        label: args.label.clone(),
        ..TaskFilters::default()
            .with_status(args.status.clone())
            .with_priority(args.priority.clone())
            .include_deleted(matches!(view, TaskView::Deleted))
            .deleted_only(matches!(view, TaskView::Deleted))
    };
    let (sort, direction) = match view {
        TaskView::Upcoming => (TaskSort::AvailableAt, SortDirection::Asc),
        TaskView::Overdue => (TaskSort::DueOn, SortDirection::Asc),
        _ => (TaskSort::Updated, SortDirection::Desc),
    };
    Ok((filters, sort, direction))
}

pub(super) struct Tools {
    database: Database,
    config: AppConfig,
    workspace: Option<String>,
    blob_dir: PathBuf,
}

impl Tools {
    pub(super) fn new(
        database: Database,
        config: AppConfig,
        workspace: Option<String>,
        blob_dir: PathBuf,
    ) -> Self {
        Self {
            database,
            config,
            workspace,
            blob_dir,
        }
    }

    async fn resolve_workspace(&self, requested: Option<&str>) -> Result<Workspace> {
        match requested.or(self.workspace.as_deref()) {
            Some(name) => {
                self.database
                    .resolve_required_workspace(name, "workspace")
                    .await
            }
            None => resolve_default_workspace(&self.database, &self.config).await,
        }
    }

    async fn project_key(&self, workspace: &Workspace, project: &str) -> Result<String> {
        Ok(self
            .database
            .resolve_existing_project(&workspace.id, project)
            .await?
            .key)
    }

    async fn optional_project_key(
        &self,
        workspace: &Workspace,
        project: Option<&str>,
    ) -> Result<Option<String>> {
        match project {
            Some(project) => self.project_key(workspace, project).await.map(Some),
            None => Ok(None),
        }
    }

    async fn task_row(&self, workspace: &Workspace, task: &Task) -> Result<TaskRow> {
        let display_refs = self.database.display_ref_context(&workspace.id).await?;
        Ok(TaskRow::from_task(task, display_refs.display_ref(task)))
    }

    fn wake(&self) {
        daemon::wake_if_enabled(&self.config);
    }

    async fn filtered_items(
        &self,
        args: &TaskFilterArgs,
        limit: Option<usize>,
    ) -> Result<(Workspace, Vec<TaskListItem>)> {
        let workspace = self.resolve_workspace(args.workspace.as_deref()).await?;
        let project = self
            .optional_project_key(&workspace, args.project.as_deref())
            .await?;
        let (filters, sort, direction) = task_filters(args)?;
        let items = self
            .database
            .list_task_summary_items(
                &workspace.id,
                filters.with_project(project),
                TaskQueryMode::Flat,
                sort,
                direction,
                limit,
            )
            .await?;
        Ok((workspace, items))
    }

    pub(super) async fn list_tasks(&self, args: ListTasksArgs) -> Result<Value> {
        let limit = limit_or_default(args.limit)?;
        let (workspace, mut items) = self.filtered_items(&args.filters, Some(limit + 1)).await?;
        let truncated = truncate(&mut items, limit);
        let tasks = items.iter().map(TaskRow::from_item).collect::<Vec<_>>();
        Ok(json!({ "workspace": workspace.key, "tasks": tasks, "truncated": truncated }))
    }

    pub(super) async fn count_tasks(&self, args: TaskFilterArgs) -> Result<Value> {
        let (workspace, items) = self.filtered_items(&args, None).await?;
        Ok(json!({ "workspace": workspace.key, "count": items.len() }))
    }

    pub(super) async fn search_tasks(&self, args: SearchTasksArgs) -> Result<Value> {
        if args.query.trim().is_empty() {
            bail!("error search-query-required hint=\"pass one or more search terms\"");
        }
        let limit = limit_or_default(args.limit)?;
        let workspace = self.resolve_workspace(args.workspace.as_deref()).await?;
        let project = self
            .optional_project_key(&workspace, args.project.as_deref())
            .await?;
        let mut results = self
            .database
            .search_task_occurrence_items(
                &workspace.id,
                TaskSearchQuery {
                    text: args.query,
                    project,
                    metadata: Vec::new(),
                    has_metadata: Vec::new(),
                    missing_metadata: Vec::new(),
                    include_deleted: args.include_deleted.unwrap_or(false),
                    limit: limit + 1,
                },
            )
            .await?;
        let truncated = truncate(&mut results, limit);
        let tasks = results
            .iter()
            .map(|result| {
                #[derive(Serialize)]
                struct SearchRow {
                    #[serde(flatten)]
                    task: TaskRow,
                    matched_field: &'static str,
                    #[serde(skip_serializing_if = "Option::is_none")]
                    snippet: Option<String>,
                }
                SearchRow {
                    task: TaskRow::from_item(&result.item),
                    matched_field: result.matched_field.as_str(),
                    snippet: result.snippet.clone(),
                }
            })
            .collect::<Vec<_>>();
        Ok(json!({ "workspace": workspace.key, "tasks": tasks, "truncated": truncated }))
    }

    pub(super) async fn show_task(&self, args: TaskRefArgs) -> Result<Value> {
        let workspace = self.resolve_workspace(args.workspace.as_deref()).await?;
        let task = self
            .database
            .resolve_task_ref(&workspace, &args.r#ref)
            .await?;
        let detail = self.database.task_detail(&task).await?;
        let report =
            build_full_task_report(&self.database, &workspace, &self.blob_dir, detail).await?;
        Ok(serde_json::to_value(task_full_json(&report))?)
    }

    pub(super) async fn create_task(&self, args: CreateTaskArgs) -> Result<Value> {
        if args.repeat.is_none()
            && (args.time_zone.is_some()
                || args.repeat_at.is_some()
                || args.repeat_start_on.is_some())
        {
            bail!(
                "error recurrence-fields-require-repeat hint=\"pass repeat with time_zone, repeat_at, or repeat_start_on\""
            );
        }
        let workspace = self.resolve_workspace(args.workspace.as_deref()).await?;
        let project = match args.project.as_deref() {
            Some(project) => self.project_key(&workspace, project).await?,
            None => DEFAULT_PROJECT.to_string(),
        };
        let priority = args.priority.unwrap_or_else(|| "none".to_string());
        let description = args.description.unwrap_or_default();
        let labels = args.labels.unwrap_or_default();
        if let Some(repeat) = args.repeat.as_deref() {
            if args.due.is_some() || args.available_at.is_some() {
                bail!(
                    "error recurrence-absolute-time-conflict hint=\"use repeat_at for local availability instead of due or available_at\""
                );
            }
            let time_zone = args.time_zone.as_deref().context(
                "error time-zone-required hint=\"pass time_zone as the user's IANA time zone\"",
            )?;
            let rule = recurrence_input::canonical_rule_input(repeat)?
                .context("error repeat-required hint=\"pass a repeat rule such as daily\"")?;
            let schedule = recurrence_input::recurrence_schedule(
                &rule,
                args.repeat_at.as_deref(),
                None,
                Some(time_zone),
                args.repeat_start_on.as_deref(),
                RecurrenceClock::system(),
            )?;
            let outcome = self
                .database
                .create_recurrence_series(
                    &workspace,
                    CreateRecurrenceSeriesParams::new(RecurrenceSeriesDraft {
                        title: args.title,
                        description,
                        project,
                        priority,
                        initial_status: args.status.unwrap_or_else(|| "todo".to_string()),
                        labels,
                        metadata: Vec::new(),
                        schedule,
                    })
                    .with_create_missing_labels(),
                )
                .await?;
            self.wake();
            let task = self.task_row(&workspace, &outcome.task).await?;
            return Ok(json!({
                "task": task,
                "series": outcome.series_ref,
                "slot": outcome.occurrence.slot_on.to_string(),
            }));
        }
        let outcome = self
            .database
            .create_task_with_options(
                &workspace,
                TaskDraft {
                    title: args.title,
                    description,
                    project: Some(project),
                    status: args.status,
                    priority,
                    source: TaskSource::Api,
                    labels,
                    metadata: Vec::new(),
                    available_at: args.available_at,
                    due_on: args.due,
                    is_epic: false,
                },
                TaskCreationOptions::standalone(TaskCreationUndo::None)
                    .with_create_missing_labels(),
            )
            .await?;
        self.wake();
        let task = self.task_row(&workspace, &outcome.task).await?;
        Ok(json!({ "task": task }))
    }

    pub(super) async fn update_task(&self, args: UpdateTaskArgs) -> Result<Value> {
        let workspace = self.resolve_workspace(args.workspace.as_deref()).await?;
        let task = self
            .database
            .resolve_task_ref(&workspace, &args.r#ref)
            .await?;
        let project = self
            .optional_project_key(&workspace, args.project.as_deref())
            .await?;
        let outcome = self
            .database
            .update_task(
                &workspace,
                &task.id,
                TaskUpdate {
                    title: args.title,
                    description: args.description,
                    project,
                    status: args.status,
                    priority: args.priority,
                    due_on: date_field_update(args.due, "due")?,
                    available_at: date_field_update(args.available_at, "available-at")?,
                    add_labels: args.labels_add.unwrap_or_default(),
                    remove_labels: args.labels_remove.unwrap_or_default(),
                    create_missing_labels: true,
                    ..TaskUpdate::default()
                },
            )
            .await?;
        self.wake();
        let row = self.task_row(&workspace, &outcome.task).await?;
        Ok(json!({ "task": row, "changed": outcome.changed }))
    }

    pub(super) async fn add_note(&self, args: AddNoteArgs) -> Result<Value> {
        if args.body.trim().is_empty() {
            bail!("error note-empty hint=\"pass the note text in body\"");
        }
        let workspace = self.resolve_workspace(args.workspace.as_deref()).await?;
        let task = self
            .database
            .resolve_task_ref(&workspace, &args.r#ref)
            .await?;
        let outcome = self
            .database
            .add_note(&workspace, &task.id, args.body)
            .await?;
        self.wake();
        let row = self.task_row(&workspace, &task).await?;
        Ok(json!({ "task": row, "note": outcome.note_id }))
    }

    pub(super) async fn delete_task(&self, args: TaskRefArgs) -> Result<Value> {
        let workspace = self.resolve_workspace(args.workspace.as_deref()).await?;
        let task = self
            .database
            .resolve_task_ref(&workspace, &args.r#ref)
            .await?;
        let outcome = self
            .database
            .set_task_deleted(&workspace, &task.id, true)
            .await?;
        self.wake();
        let row = self.task_row(&workspace, &outcome.task).await?;
        Ok(json!({ "task": row, "restore": "aven restore <ref>" }))
    }

    pub(super) async fn list_projects(&self, args: WorkspaceArgs) -> Result<Value> {
        let workspace = self.resolve_workspace(args.workspace.as_deref()).await?;
        let projects = self
            .database
            .list_projects(&workspace.id, None)
            .await?
            .into_iter()
            .map(|project| json!({ "key": project.key, "name": project.name }))
            .collect::<Vec<_>>();
        Ok(json!({ "workspace": workspace.key, "projects": projects }))
    }

    pub(super) async fn list_workspaces(&self) -> Result<Value> {
        let default = self.resolve_workspace(None).await.ok();
        let workspaces = self
            .database
            .list_workspace_open_summaries()
            .await?
            .into_iter()
            .map(|summary| {
                let is_default = default
                    .as_ref()
                    .is_some_and(|default| default.id == summary.workspace.id);
                json!({
                    "key": summary.workspace.key,
                    "name": summary.workspace.name,
                    "open_tasks": summary.open_task_count,
                    "default": is_default,
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({ "workspaces": workspaces }))
    }

    pub(super) async fn list_recurring(&self, args: ListRecurringArgs) -> Result<Value> {
        let limit = limit_or_default(args.limit)?;
        let workspace = self.resolve_workspace(args.workspace.as_deref()).await?;
        let mut series = self.database.list_recurrence_series(&workspace.id).await?;
        let truncated = truncate(&mut series, limit);
        let series = series
            .iter()
            .map(|item| {
                json!({
                    "ref": item.series_ref,
                    "title": item.series.title,
                    "state": item.series.state.as_str(),
                    "rule": item.rule_label,
                    "time_zone": item.series.timezone.as_str(),
                    "current_task": item.current_task_ref,
                    "current_slot": item.current_slot_on,
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({ "workspace": workspace.key, "series": series, "truncated": truncated }))
    }

    pub(super) async fn edit_recurring(&self, args: EditRecurringArgs) -> Result<Value> {
        let has_template_fields = args.has_template_fields();
        if args.action.is_some() == has_template_fields {
            bail!(
                "error recurring-edit-conflict hint=\"pass either action or template fields, not both or neither\""
            );
        }
        if args.skip_current.is_some() && !matches!(args.action, Some(RecurringAction::Stop)) {
            bail!("error skip-current-requires-stop hint=\"pass skip_current with action stop\"");
        }
        let workspace = self.resolve_workspace(args.workspace.as_deref()).await?;
        let series = self
            .database
            .resolve_recurrence_ref(&workspace, &args.r#ref)
            .await?;
        let database = &self.database;
        let state = match args.action {
            None => {
                let project = self
                    .optional_project_key(&workspace, args.project.as_deref())
                    .await?;
                let available_local_time = args
                    .repeat_at
                    .as_deref()
                    .map(recurrence_input::parse_repeat_time)
                    .transpose()?;
                let due_policy = args
                    .repeat_due
                    .as_deref()
                    .map(recurrence_input::parse_due_policy)
                    .transpose()?;
                let outcome = database
                    .update_recurrence_template(
                        &workspace,
                        &series.id,
                        UpdateRecurrenceTemplateParams::new(RecurrenceTemplateUpdate {
                            title: args.title,
                            description: args.description,
                            project,
                            priority: args.priority,
                            initial_status: args.initial_status,
                            labels: args.labels,
                            set_metadata: Vec::new(),
                            remove_metadata: Vec::new(),
                            available_local_time,
                            due_policy,
                        })
                        .with_create_missing_labels(),
                    )
                    .await?;
                self.wake();
                outcome.series.state
            }
            Some(RecurringAction::Skip) => {
                let outcome = skip_current_occurrence(database, &workspace, &series.id).await?;
                self.wake();
                let display_refs = database.display_ref_context(&workspace.id).await?;
                return Ok(json!({
                    "series": database.recurrence_series_ref(&workspace.id, &outcome.series.id).await?,
                    "skipped_slot": outcome.resolved.slot_on.to_string(),
                    "successor": outcome.successor.as_ref().map(|task| display_refs.display_ref(task)),
                }));
            }
            Some(RecurringAction::Pause) => {
                let outcome = database
                    .pause_recurrence_series(&workspace, &series.id)
                    .await?;
                self.wake();
                outcome.series.state
            }
            Some(RecurringAction::Resume) => {
                let outcome = database
                    .resume_recurrence_series(&workspace, &series.id, aven_core::ids::now_utc())
                    .await?;
                self.wake();
                outcome.series.state
            }
            Some(RecurringAction::Stop) => {
                let outcome = database
                    .stop_recurrence_series(
                        &workspace,
                        &series.id,
                        args.skip_current.unwrap_or(false),
                    )
                    .await?;
                self.wake();
                outcome.series.state
            }
        };
        let series_ref = database
            .recurrence_series_ref(&workspace.id, &series.id)
            .await?;
        Ok(json!({ "series": series_ref, "state": state.as_str() }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(message: &str) -> ToolError {
        ToolError::from_anyhow(&anyhow::anyhow!(message.to_string()))
    }

    #[test]
    fn tool_errors_prefer_mcp_guidance_over_cli_hints() {
        let error = parsed("error workspace-required hint=\"pass --workspace\"");
        assert_eq!(error.error, "workspace-required");
        assert!(error.hint.contains("list_workspaces"));
    }

    #[test]
    fn tool_errors_read_embedded_hints() {
        let error = parsed("error invalid-due value=x hint=\"use YYYY-MM-DD or empty\"");
        assert_eq!(error.error, "invalid-due");
        assert_eq!(error.hint, "use YYYY-MM-DD or empty");
        let error = parsed("error unknown-label input=x\nchoice y\nhint \"create it\"");
        assert_eq!(error.error, "unknown-label");
        assert_eq!(error.hint, "create it");
    }

    #[test]
    fn tool_errors_without_codes_keep_the_message() {
        let error = parsed("Try daily, or every Friday");
        assert_eq!(error.error, "task-error");
        assert_eq!(error.message, "Try daily, or every Friday");
        assert!(!error.hint.is_empty());
    }
}
