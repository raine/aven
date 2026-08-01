use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::Error as InternalError;
use chrono::{NaiveDate, NaiveTime, Weekday};

use crate::choices::{TaskPriority, TaskSource, TaskStatus};
use crate::db::Database;
use crate::ids::{ProjectId, TaskId, WorkspaceId};
use crate::operations::{
    CreateRecurrenceSeriesParams, RecurrenceSeriesDraft,
    RecurrenceTemplateUpdate as InternalRecurrenceTemplateUpdate, TaskDraft,
    TaskUpdate as InternalTaskUpdate, UpdateRecurrenceTemplateParams,
};
pub use crate::query::RecurrenceHistoryKind;
use crate::query::{
    MAX_RECURRENCE_HISTORY_LIMIT, RecurrenceCounts as InternalRecurrenceCounts,
    RecurrenceHistoryEntry as InternalRecurrenceHistoryEntry,
    RecurrenceSeriesDetail as InternalRecurrenceSeriesDetail,
    RecurrenceSeriesSummary as InternalRecurrenceSeriesSummary, SortDirection, TaskFilters,
    TaskListItem, TaskQueryMode, TaskSort,
};
pub use crate::recurrence::{
    RecurrenceDuePolicy, RecurrenceFrequency, RecurrenceOutcome, RecurrenceProjectionState,
    RecurrenceSeriesState,
};
use crate::recurrence::{
    RecurrenceRule as InternalRecurrenceRule, RecurrenceSchedule, RecurrenceSeriesId, TimeZoneId,
    WeekdaySet,
};
use crate::sync::SyncSession;
use crate::task_fields::TaskField;
use crate::types::{RecurrenceOccurrence, RecurrenceSeries, Task};
use crate::workspaces::Workspace;

#[derive(Clone)]
pub struct Store {
    database: Database,
}

impl Store {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        let database = Database::open(path.as_ref())
            .await
            .map_err(Error::database_open)?;
        Ok(Self { database })
    }

    pub fn initialize_storage(&self) -> Result<StorageLayout, Error> {
        let root = crate::attachments::default_blob_dir(self.database.path());
        let objects = root.join("objects").join("sha256");
        let trash = root.join("trash");
        let previews = root.join("cache").join("previews");
        for directory in [&objects, &trash, &previews] {
            std::fs::create_dir_all(directory)
                .map_err(|error| Error::from_internal(error.into()))?;
        }
        Ok(StorageLayout {
            root,
            staging: objects.clone(),
            objects,
            trash,
            previews,
        })
    }

    pub async fn start_sync_session(
        &self,
        server: String,
        auth_token: Option<String>,
        page_budget: Option<usize>,
    ) -> Result<SyncSession, Error> {
        if !crate::sync::wire::sync_server_url_is_valid(&server) {
            return Err(Error::new(
                ErrorCode::Validation,
                "invalid sync server URL".to_string(),
            ));
        }
        SyncSession::start(self.database.clone(), server, auth_token, page_budget)
            .await
            .map_err(Error::from_internal)
    }

    pub async fn list_workspaces(&self) -> Result<Vec<WorkspaceRecord>, Error> {
        self.database
            .list_workspaces()
            .await
            .map(|workspaces| workspaces.into_iter().map(WorkspaceRecord::from).collect())
            .map_err(Error::from_internal)
    }

    pub async fn resolve_workspace(&self, name_or_key: &str) -> Result<WorkspaceRecord, Error> {
        self.database
            .find_workspace(name_or_key)
            .await
            .map_err(Error::from_internal)?
            .map(WorkspaceRecord::from)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::NotFound,
                    format!("workspace not found: {name_or_key}"),
                )
            })
    }

    pub async fn create_task(
        &self,
        workspace_id: &WorkspaceId,
        input: CreateTask,
    ) -> Result<TaskRecord, Error> {
        validate_project(&input.project)?;
        validate_optional_date("available_at", input.available_at.as_deref())?;
        validate_optional_date("due_on", input.due_on.as_deref())?;
        let workspace = self.workspace(workspace_id).await?;
        self.database
            .create_task(
                &workspace,
                TaskDraft {
                    title: input.title,
                    description: input.description,
                    project: Some(input.project),
                    status: input.status.as_str().to_string(),
                    priority: input.priority.as_str().to_string(),
                    source: TaskSource::Api,
                    labels: Vec::new(),
                    available_at: input.available_at,
                    due_on: input.due_on,
                    is_epic: false,
                },
            )
            .await
            .map(|outcome| TaskRecord::from(outcome.task))
            .map_err(Error::from_internal)
    }

    pub async fn update_task(
        &self,
        workspace_id: &WorkspaceId,
        task_id: &TaskId,
        input: UpdateTask,
    ) -> Result<TaskUpdateResult, Error> {
        if let Some(project) = input.project.as_deref() {
            validate_project(project)?;
        }
        validate_date_update("available_at", &input.available_at)?;
        validate_date_update("due_on", &input.due_on)?;
        let workspace = self.workspace(workspace_id).await?;
        self.fetch_task(workspace_id, task_id).await?;
        self.database
            .update_task(
                &workspace,
                task_id,
                InternalTaskUpdate {
                    title: input.title,
                    description: input.description,
                    project: input.project,
                    status: input.status.map(|status| status.as_str().to_string()),
                    priority: input.priority.map(|priority| priority.as_str().to_string()),
                    available_at: input.available_at.into_internal(),
                    due_on: input.due_on.into_internal(),
                    ..InternalTaskUpdate::default()
                },
            )
            .await
            .map(|outcome| TaskUpdateResult {
                task: TaskRecord::from(outcome.task),
                changed: outcome.changed,
            })
            .map_err(Error::from_internal)
    }

    pub async fn list_tasks(&self, workspace_id: &WorkspaceId) -> Result<Vec<TaskRecord>, Error> {
        self.workspace(workspace_id).await?;
        let filters = TaskFilters {
            exclude_epics: true,
            ..TaskFilters::default()
        };
        self.database
            .list_task_items(
                workspace_id,
                filters,
                TaskQueryMode::Flat,
                TaskSort::Created,
                SortDirection::Asc,
            )
            .await
            .map(|items| {
                items
                    .into_iter()
                    .map(|item| TaskRecord::from(item.task))
                    .collect()
            })
            .map_err(Error::from_internal)
    }

    pub async fn fetch_task(
        &self,
        workspace_id: &WorkspaceId,
        task_id: &TaskId,
    ) -> Result<TaskRecord, Error> {
        let workspace = self.workspace(workspace_id).await?;
        let mut connection = self
            .database
            .acquire_reader()
            .await
            .map_err(Error::from_internal)?;
        crate::refs::get_task_in_workspace(&mut connection, &workspace, task_id)
            .await
            .map(TaskRecord::from)
            .map_err(Error::from_internal)
    }

    pub async fn create_recurrence_series(
        &self,
        workspace_id: &WorkspaceId,
        input: CreateRecurrenceSeries,
    ) -> Result<RecurrenceCreateResult, Error> {
        validate_project(&input.project)?;
        let workspace = self.workspace(workspace_id).await?;
        let schedule = input.schedule.into_internal()?;
        self.database
            .create_recurrence_series(
                &workspace,
                CreateRecurrenceSeriesParams::new(RecurrenceSeriesDraft {
                    title: input.title,
                    description: input.description,
                    project: input.project,
                    priority: input.priority.as_str().to_string(),
                    initial_status: input.initial_status.as_str().to_string(),
                    labels: input.labels,
                    schedule,
                }),
            )
            .await
            .map(|outcome| RecurrenceCreateResult {
                series: RecurrenceSeriesRecord::from(outcome.series),
                series_ref: outcome.series_ref,
                occurrence: RecurrenceOccurrenceRecord::from(outcome.occurrence),
                task: TaskRecord::from(outcome.task),
            })
            .map_err(Error::from_internal)
    }

    pub async fn update_recurrence_template(
        &self,
        workspace_id: &WorkspaceId,
        series_id: &RecurrenceSeriesId,
        input: UpdateRecurrenceTemplate,
    ) -> Result<RecurrenceTemplateUpdateResult, Error> {
        if let Some(project) = input.project.as_deref() {
            validate_project(project)?;
        }
        let workspace = self.workspace(workspace_id).await?;
        self.database
            .update_recurrence_template(
                &workspace,
                series_id,
                UpdateRecurrenceTemplateParams::new(InternalRecurrenceTemplateUpdate {
                    title: input.title,
                    description: input.description,
                    project: input.project,
                    priority: input.priority.map(|value| value.as_str().to_string()),
                    initial_status: input.initial_status.map(|value| value.as_str().to_string()),
                    labels: input.labels,
                    available_local_time: input.available_local_time.into_internal()?,
                    due_policy: input.due_policy,
                }),
            )
            .await
            .map(|outcome| RecurrenceTemplateUpdateResult {
                series: RecurrenceSeriesRecord::from(outcome.series),
                changed: outcome.changed,
            })
            .map_err(Error::from_internal)
    }

    pub async fn resolve_recurrence_ref(
        &self,
        workspace_id: &WorkspaceId,
        input: &str,
    ) -> Result<RecurrenceRefResolution, Error> {
        let workspace = self.workspace(workspace_id).await?;
        let series = self
            .database
            .resolve_recurrence_ref(&workspace, input)
            .await
            .map_err(Error::from_internal)?;
        let series_ref = self
            .database
            .recurrence_series_ref(workspace_id, &series.id)
            .await
            .map_err(Error::from_internal)?;
        Ok(RecurrenceRefResolution {
            series_id: series.id,
            series_ref,
        })
    }

    pub async fn list_recurrence_series(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<Vec<RecurrenceSeriesSummary>, Error> {
        self.workspace(workspace_id).await?;
        self.database
            .list_recurrence_series(workspace_id)
            .await
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(Error::from_internal)
    }

    pub async fn show_recurrence_series(
        &self,
        workspace_id: &WorkspaceId,
        input: &str,
    ) -> Result<RecurrenceSeriesDetail, Error> {
        let resolution = self.resolve_recurrence_ref(workspace_id, input).await?;
        self.database
            .recurrence_series_detail(workspace_id, &resolution.series_id)
            .await
            .map(Into::into)
            .map_err(Error::from_internal)
    }

    pub async fn recurrence_history(
        &self,
        workspace_id: &WorkspaceId,
        input: &str,
        offset: usize,
        limit: usize,
    ) -> Result<RecurrenceHistoryPage, Error> {
        if limit == 0 || limit > MAX_RECURRENCE_HISTORY_LIMIT {
            return Err(Error::new(
                ErrorCode::Validation,
                format!(
                    "recurrence history limit must be between 1 and {MAX_RECURRENCE_HISTORY_LIMIT}"
                ),
            ));
        }
        let resolution = self.resolve_recurrence_ref(workspace_id, input).await?;
        self.database
            .recurrence_history(workspace_id, &resolution.series_id, offset, limit)
            .await
            .map(|page| RecurrenceHistoryPage {
                series_ref: page.series_ref,
                items: page.items.into_iter().map(Into::into).collect(),
                offset: page.offset,
                limit: page.limit,
                total: page.total,
                has_more: page.has_more,
            })
            .map_err(Error::from_internal)
    }

    pub async fn resolve_recurrence_occurrence(
        &self,
        workspace_id: &WorkspaceId,
        task_id: &TaskId,
        outcome: RecurrenceOutcome,
    ) -> Result<RecurrenceResolveResult, Error> {
        let workspace = self.workspace(workspace_id).await?;
        self.database
            .resolve_recurrence_occurrence(&workspace, task_id, outcome)
            .await
            .map(|value| RecurrenceResolveResult {
                series: value.series.into(),
                occurrence: value.resolved.into(),
                task: value.task.into(),
                successor: value.successor.map(Into::into),
            })
            .map_err(Error::from_internal)
    }

    pub async fn complete_recurrence_occurrence(
        &self,
        workspace_id: &WorkspaceId,
        task_id: &TaskId,
    ) -> Result<RecurrenceResolveResult, Error> {
        self.resolve_recurrence_occurrence(workspace_id, task_id, RecurrenceOutcome::Completed)
            .await
    }

    pub async fn skip_recurrence_occurrence(
        &self,
        workspace_id: &WorkspaceId,
        task_id: &TaskId,
    ) -> Result<RecurrenceResolveResult, Error> {
        self.resolve_recurrence_occurrence(workspace_id, task_id, RecurrenceOutcome::Skipped)
            .await
    }

    pub async fn pause_recurrence_series(
        &self,
        workspace_id: &WorkspaceId,
        series_id: &RecurrenceSeriesId,
    ) -> Result<RecurrenceStateResult, Error> {
        let workspace = self.workspace(workspace_id).await?;
        self.database
            .pause_recurrence_series(&workspace, series_id)
            .await
            .map(Into::into)
            .map_err(Error::from_internal)
    }

    pub async fn resume_recurrence_series(
        &self,
        workspace_id: &WorkspaceId,
        series_id: &RecurrenceSeriesId,
    ) -> Result<RecurrenceStateResult, Error> {
        let workspace = self.workspace(workspace_id).await?;
        let at = chrono::DateTime::parse_from_rfc3339(&crate::ids::now())
            .map_err(|error| Error::from_internal(error.into()))?
            .with_timezone(&chrono::Utc);
        self.database
            .resume_recurrence_series(&workspace, series_id, at)
            .await
            .map(Into::into)
            .map_err(Error::from_internal)
    }

    pub async fn stop_recurrence_series(
        &self,
        workspace_id: &WorkspaceId,
        series_id: &RecurrenceSeriesId,
        skip_current: bool,
    ) -> Result<RecurrenceStateResult, Error> {
        let workspace = self.workspace(workspace_id).await?;
        self.database
            .stop_recurrence_series(&workspace, series_id, skip_current)
            .await
            .map(Into::into)
            .map_err(Error::from_internal)
    }

    pub async fn recurrence_task_report(
        &self,
        workspace_id: &WorkspaceId,
        expand_recurring: bool,
    ) -> Result<Vec<TaskSummary>, Error> {
        self.workspace(workspace_id).await?;
        self.database
            .list_task_items(
                workspace_id,
                TaskFilters {
                    exclude_epics: true,
                    expand_recurring,
                    ..TaskFilters::default()
                },
                TaskQueryMode::Flat,
                TaskSort::Created,
                SortDirection::Asc,
            )
            .await
            .map(|items| items.into_iter().map(TaskSummary::from).collect())
            .map_err(Error::from_internal)
    }

    pub async fn list_conflicts(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<Vec<ConflictSummary>, Error> {
        let workspace = self.workspace(workspace_id).await?;
        self.database
            .list_conflicts(&workspace, None, None)
            .await
            .map_err(Error::from_internal)?
            .into_iter()
            .map(|conflict| {
                Ok(ConflictSummary {
                    task_id: conflict.task_id,
                    task_title: conflict.title,
                    project_key: conflict.project_key,
                    project_prefix: conflict.project_prefix,
                    field: ConflictField::from_task_field(TaskField::parse_or_unknown(
                        &conflict.field,
                    )?),
                })
            })
            .collect::<Result<Vec<_>, InternalError>>()
            .map_err(Error::from_internal)
    }

    pub async fn inspect_conflicts(
        &self,
        workspace_id: &WorkspaceId,
        task_id: &TaskId,
    ) -> Result<Vec<Conflict>, Error> {
        let workspace = self.workspace(workspace_id).await?;
        let details = self
            .database
            .task_conflicts(&workspace, task_id, None)
            .await
            .map_err(Error::from_internal)?;
        let mut conflicts = Vec::with_capacity(details.len());
        for detail in details {
            let field = TaskField::parse_or_unknown(&detail.field).map_err(Error::from_internal)?;
            let local_value = self
                .database
                .conflict_display_value(workspace_id, field.as_str(), &detail.local_value)
                .await
                .map_err(Error::from_internal)?;
            let remote_value = self
                .database
                .conflict_display_value(workspace_id, field.as_str(), &detail.remote_value)
                .await
                .map_err(Error::from_internal)?;
            conflicts.push(Conflict {
                task_id: task_id.clone(),
                field: ConflictField::from_task_field(field),
                local_value,
                remote_value,
            });
        }
        Ok(conflicts)
    }

    pub async fn resolve_conflict(
        &self,
        workspace_id: &WorkspaceId,
        task_id: &TaskId,
        field: ConflictField,
        choice: ConflictChoice,
    ) -> Result<TaskRecord, Error> {
        let workspace = self.workspace(workspace_id).await?;
        let field_name = field.as_str();
        let choice = match choice {
            ConflictChoice::Local => crate::operations::ConflictValueChoice::Local,
            ConflictChoice::Remote => crate::operations::ConflictValueChoice::Remote,
        };
        let mut connection = self
            .database
            .acquire_writer()
            .await
            .map_err(Error::from_internal)?;
        crate::operations::resolve_conflict_choice(
            &mut connection,
            &workspace,
            task_id,
            field_name,
            choice,
        )
        .await
        .map(|outcome| TaskRecord::from(outcome.task))
        .map_err(Error::from_internal)
    }

    async fn workspace(&self, workspace_id: &WorkspaceId) -> Result<Workspace, Error> {
        self.database
            .list_workspaces()
            .await
            .map_err(Error::from_internal)?
            .into_iter()
            .find(|workspace| workspace.id == *workspace_id)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::NotFound,
                    format!("workspace not found: {workspace_id}"),
                )
            })
    }
}

fn validate_project(project: &str) -> Result<(), Error> {
    if crate::projects::normalize_key(project).is_empty() {
        return Err(Error::new(
            ErrorCode::Validation,
            "project must contain at least one letter or number".to_string(),
        ));
    }
    Ok(())
}

fn validate_optional_date(field: &str, value: Option<&str>) -> Result<(), Error> {
    let Some(value) = value else {
        return Ok(());
    };
    if value.is_empty() {
        return Err(Error::new(
            ErrorCode::Validation,
            format!("{field} must be absent or contain a date"),
        ));
    }
    let result = match field {
        "available_at" => crate::time_validation::validate_available_at_value(value),
        "due_on" => crate::time_validation::validate_due_on_value(value),
        _ => unreachable!("consumer API validates only task dates"),
    };
    result.map_err(|error| Error::new(ErrorCode::Validation, error.to_string()))
}

fn validate_date_update(field: &str, update: &OptionalDateUpdate) -> Result<(), Error> {
    match update {
        OptionalDateUpdate::Unchanged | OptionalDateUpdate::Clear => Ok(()),
        OptionalDateUpdate::Set(value) => validate_optional_date(field, Some(value)),
    }
}

fn parse_date(field: &str, value: &str) -> Result<NaiveDate, Error> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| {
        Error::new(
            ErrorCode::Validation,
            format!("{field} must use YYYY-MM-DD"),
        )
    })
}

fn parse_local_time(field: &str, value: &str) -> Result<NaiveTime, Error> {
    NaiveTime::parse_from_str(value, "%H:%M")
        .or_else(|_| NaiveTime::parse_from_str(value, "%H:%M:%S"))
        .map_err(|_| {
            Error::new(
                ErrorCode::Validation,
                format!("{field} must use HH:MM or HH:MM:SS"),
            )
        })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageLayout {
    pub root: PathBuf,
    pub objects: PathBuf,
    pub staging: PathBuf,
    pub trash: PathBuf,
    pub previews: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRecord {
    pub id: WorkspaceId,
    pub key: String,
    pub name: String,
}

impl From<Workspace> for WorkspaceRecord {
    fn from(workspace: Workspace) -> Self {
        Self {
            id: workspace.id,
            key: workspace.key,
            name: workspace.name,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateTask {
    pub title: String,
    pub description: String,
    pub project: String,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    pub available_at: Option<String>,
    pub due_on: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UpdateTask {
    pub title: Option<String>,
    pub description: Option<String>,
    pub project: Option<String>,
    pub status: Option<TaskStatus>,
    pub priority: Option<TaskPriority>,
    pub available_at: OptionalDateUpdate,
    pub due_on: OptionalDateUpdate,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum OptionalDateUpdate {
    #[default]
    Unchanged,
    Set(String),
    Clear,
}

impl OptionalDateUpdate {
    fn into_internal(self) -> Option<Option<String>> {
        match self {
            Self::Unchanged => None,
            Self::Set(value) => Some(Some(value)),
            Self::Clear => Some(None),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecurrenceWeekday {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

impl From<RecurrenceWeekday> for Weekday {
    fn from(value: RecurrenceWeekday) -> Self {
        match value {
            RecurrenceWeekday::Monday => Self::Mon,
            RecurrenceWeekday::Tuesday => Self::Tue,
            RecurrenceWeekday::Wednesday => Self::Wed,
            RecurrenceWeekday::Thursday => Self::Thu,
            RecurrenceWeekday::Friday => Self::Fri,
            RecurrenceWeekday::Saturday => Self::Sat,
            RecurrenceWeekday::Sunday => Self::Sun,
        }
    }
}

impl From<Weekday> for RecurrenceWeekday {
    fn from(value: Weekday) -> Self {
        match value {
            Weekday::Mon => Self::Monday,
            Weekday::Tue => Self::Tuesday,
            Weekday::Wed => Self::Wednesday,
            Weekday::Thu => Self::Thursday,
            Weekday::Fri => Self::Friday,
            Weekday::Sat => Self::Saturday,
            Weekday::Sun => Self::Sunday,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceRule {
    pub frequency: RecurrenceFrequency,
    pub interval: u32,
    pub weekdays: Vec<RecurrenceWeekday>,
}

impl RecurrenceRule {
    fn into_internal(self) -> Result<InternalRecurrenceRule, Error> {
        InternalRecurrenceRule::new(
            self.frequency,
            self.interval,
            WeekdaySet::from_weekdays(self.weekdays.into_iter().map(Into::into)),
        )
        .map_err(|error| Error::new(ErrorCode::Validation, error.to_string()))
    }
}

impl From<InternalRecurrenceRule> for RecurrenceRule {
    fn from(value: InternalRecurrenceRule) -> Self {
        Self {
            frequency: value.frequency(),
            interval: value.interval(),
            weekdays: value.weekdays_set().iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceScheduleInput {
    pub rule: RecurrenceRule,
    pub timezone: String,
    pub start_on: String,
    pub available_local_time: Option<String>,
    pub due_policy: RecurrenceDuePolicy,
}

impl RecurrenceScheduleInput {
    fn into_internal(self) -> Result<RecurrenceSchedule, Error> {
        let timezone = self
            .timezone
            .parse::<TimeZoneId>()
            .map_err(|error| Error::new(ErrorCode::Validation, error.to_string()))?;
        let start_on = parse_date("start_on", &self.start_on)?;
        let available_local_time = self
            .available_local_time
            .map(|value| parse_local_time("available_local_time", &value))
            .transpose()?;
        Ok(RecurrenceSchedule::new(
            self.rule.into_internal()?,
            timezone,
            start_on,
            available_local_time,
            self.due_policy,
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateRecurrenceSeries {
    pub title: String,
    pub description: String,
    pub project: String,
    pub priority: TaskPriority,
    pub initial_status: TaskStatus,
    pub labels: Vec<String>,
    pub schedule: RecurrenceScheduleInput,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum OptionalLocalTimeUpdate {
    #[default]
    Unchanged,
    Set(String),
    Clear,
}

impl OptionalLocalTimeUpdate {
    fn into_internal(self) -> Result<Option<Option<NaiveTime>>, Error> {
        match self {
            Self::Unchanged => Ok(None),
            Self::Set(value) => Ok(Some(Some(parse_local_time(
                "available_local_time",
                &value,
            )?))),
            Self::Clear => Ok(Some(None)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UpdateRecurrenceTemplate {
    pub title: Option<String>,
    pub description: Option<String>,
    pub project: Option<String>,
    pub priority: Option<TaskPriority>,
    pub initial_status: Option<TaskStatus>,
    pub labels: Option<Vec<String>>,
    pub available_local_time: OptionalLocalTimeUpdate,
    pub due_policy: Option<RecurrenceDuePolicy>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceSeriesRecord {
    pub workspace_id: WorkspaceId,
    pub id: RecurrenceSeriesId,
    pub title: String,
    pub description: String,
    pub project_id: ProjectId,
    pub priority: TaskPriority,
    pub initial_status: TaskStatus,
    pub rule: RecurrenceRule,
    pub timezone: String,
    pub start_on: String,
    pub available_local_time: Option<String>,
    pub due_policy: RecurrenceDuePolicy,
    pub state: RecurrenceSeriesState,
    pub stopped_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl From<RecurrenceSeries> for RecurrenceSeriesRecord {
    fn from(value: RecurrenceSeries) -> Self {
        Self {
            workspace_id: value.workspace_id,
            id: value.id,
            title: value.title,
            description: value.description,
            project_id: value.project_id,
            priority: value.priority,
            initial_status: value.initial_status,
            rule: value.rule.into(),
            timezone: value.timezone.to_string(),
            start_on: value.start_on.to_string(),
            available_local_time: value
                .available_local_time
                .map(|time| time.format("%H:%M:%S").to_string()),
            due_policy: value.due_policy,
            state: value.state,
            stopped_at: value.stopped_at,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceOccurrenceRecord {
    pub series_id: RecurrenceSeriesId,
    pub slot_on: String,
    pub task_id: Option<TaskId>,
    pub outcome: Option<RecurrenceOutcome>,
    pub resolved_at: Option<String>,
    pub projection_state: RecurrenceProjectionState,
    pub archived_at: Option<String>,
}

impl From<RecurrenceOccurrence> for RecurrenceOccurrenceRecord {
    fn from(value: RecurrenceOccurrence) -> Self {
        Self {
            series_id: value.series_id,
            slot_on: value.slot_on.to_string(),
            task_id: value.task_id,
            outcome: value.outcome,
            resolved_at: value.resolved_at,
            projection_state: value.projection_state,
            archived_at: value.archived_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceCreateResult {
    pub series: RecurrenceSeriesRecord,
    pub series_ref: String,
    pub occurrence: RecurrenceOccurrenceRecord,
    pub task: TaskRecord,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceTemplateUpdateResult {
    pub series: RecurrenceSeriesRecord,
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceResolveResult {
    pub series: RecurrenceSeriesRecord,
    pub occurrence: RecurrenceOccurrenceRecord,
    pub task: TaskRecord,
    pub successor: Option<TaskRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceStateResult {
    pub series: RecurrenceSeriesRecord,
    pub occurrence: Option<RecurrenceOccurrenceRecord>,
}

impl From<crate::operations::RecurrenceStateOutcome> for RecurrenceStateResult {
    fn from(value: crate::operations::RecurrenceStateOutcome) -> Self {
        Self {
            series: value.series.into(),
            occurrence: value.occurrence.map(Into::into),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceRefResolution {
    pub series_id: RecurrenceSeriesId,
    pub series_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RecurrenceCounts {
    pub completed: u64,
    pub skipped: u64,
    pub missed: u64,
    pub pause_intervals: u64,
    pub latest_slot_on: Option<String>,
    pub latest_outcome: Option<RecurrenceOutcome>,
}

impl From<InternalRecurrenceCounts> for RecurrenceCounts {
    fn from(value: InternalRecurrenceCounts) -> Self {
        Self {
            completed: value.completed as u64,
            skipped: value.skipped as u64,
            missed: value.missed as u64,
            pause_intervals: value.pause_intervals as u64,
            latest_slot_on: value.latest_slot_on,
            latest_outcome: value.latest_outcome,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceSeriesSummary {
    pub series: RecurrenceSeriesRecord,
    pub series_ref: String,
    pub rule_label: String,
    pub current_slot_on: Option<String>,
    pub current_task_ref: Option<String>,
    pub counts: RecurrenceCounts,
}

impl From<InternalRecurrenceSeriesSummary> for RecurrenceSeriesSummary {
    fn from(value: InternalRecurrenceSeriesSummary) -> Self {
        Self {
            series: value.series.into(),
            series_ref: value.series_ref,
            rule_label: value.rule_label,
            current_slot_on: value.current_slot_on,
            current_task_ref: value.current_task_ref,
            counts: value.counts.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceSeriesConflict {
    pub field: String,
    pub variant_a: String,
    pub local_value: String,
    pub variant_b: String,
    pub remote_value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceSeriesDetail {
    pub series: RecurrenceSeriesRecord,
    pub labels: Vec<String>,
    pub summary: RecurrenceSeriesSummary,
    pub current_occurrence: Option<RecurrenceOccurrenceRecord>,
    pub lifecycle_conflicts: Vec<RecurrenceSeriesConflict>,
}

impl From<InternalRecurrenceSeriesDetail> for RecurrenceSeriesDetail {
    fn from(value: InternalRecurrenceSeriesDetail) -> Self {
        Self {
            series: value.series.into(),
            labels: value.labels,
            summary: value.summary.into(),
            current_occurrence: value.current_occurrence.map(Into::into),
            lifecycle_conflicts: value
                .lifecycle_conflicts
                .into_iter()
                .map(|conflict| RecurrenceSeriesConflict {
                    field: conflict.field,
                    variant_a: conflict.variant_a,
                    local_value: conflict.local_value,
                    variant_b: conflict.variant_b,
                    remote_value: conflict.remote_value,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceHistoryRow {
    pub kind: RecurrenceHistoryKind,
    pub slot_on: Option<String>,
    pub interval_started_at: Option<String>,
    pub interval_ended_at: Option<String>,
    pub task_id: Option<TaskId>,
    pub task_ref: Option<String>,
    pub openable: bool,
    pub archived_projection: bool,
    pub resolved_at: Option<String>,
}

impl From<InternalRecurrenceHistoryEntry> for RecurrenceHistoryRow {
    fn from(value: InternalRecurrenceHistoryEntry) -> Self {
        Self {
            kind: value.kind,
            slot_on: value.slot_on,
            interval_started_at: value.interval_started_at,
            interval_ended_at: value.interval_ended_at,
            task_id: value.task_id,
            task_ref: value.task_ref,
            openable: value.openable,
            archived_projection: value.archived_projection,
            resolved_at: value.resolved_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceHistoryPage {
    pub series_ref: String,
    pub items: Vec<RecurrenceHistoryRow>,
    pub offset: usize,
    pub limit: usize,
    pub total: usize,
    pub has_more: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRecurrenceSummary {
    pub series_id: RecurrenceSeriesId,
    pub series_ref: String,
    pub slot_on: String,
    pub rule_label: String,
    pub timezone: String,
    pub lifecycle: RecurrenceSeriesState,
    pub outcome: Option<RecurrenceOutcome>,
    pub projection_state: RecurrenceProjectionState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceTaskGroup {
    pub series_id: RecurrenceSeriesId,
    pub series_ref: String,
    pub counts: RecurrenceCounts,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskSummary {
    pub task: TaskRecord,
    pub display_ref: String,
    pub recurrence: Option<TaskRecurrenceSummary>,
    pub recurrence_group: Option<RecurrenceTaskGroup>,
}

impl From<TaskListItem> for TaskSummary {
    fn from(value: TaskListItem) -> Self {
        Self {
            task: value.task.into(),
            display_ref: value.display_ref,
            recurrence: value.recurrence.map(|summary| TaskRecurrenceSummary {
                series_id: summary.series_id,
                series_ref: summary.series_ref,
                slot_on: summary.slot_on,
                rule_label: summary.rule_label,
                timezone: summary.timezone,
                lifecycle: summary.lifecycle,
                outcome: summary.outcome,
                projection_state: summary.projection_state,
            }),
            recurrence_group: value.recurrence_group.map(|group| RecurrenceTaskGroup {
                series_id: group.series_id,
                series_ref: group.series_ref,
                counts: group.counts.into(),
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRecord {
    pub id: TaskId,
    pub workspace_id: WorkspaceId,
    pub title: String,
    pub description: String,
    pub project_id: ProjectId,
    pub project_key: String,
    pub project_prefix: String,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    pub created_at: String,
    pub updated_at: String,
    pub available_at: Option<String>,
    pub due_on: Option<String>,
}

impl From<Task> for TaskRecord {
    fn from(task: Task) -> Self {
        Self {
            id: task.id,
            workspace_id: task.workspace_id,
            title: task.title,
            description: task.description,
            project_id: task.project_id,
            project_key: task.project_key,
            project_prefix: task.project_prefix,
            status: task.status,
            priority: task.priority,
            created_at: task.created_at,
            updated_at: task.updated_at,
            available_at: task.available_at,
            due_on: task.due_on,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskUpdateResult {
    pub task: TaskRecord,
    pub changed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictField {
    Title,
    Description,
    Project,
    Status,
    Priority,
    AvailableAt,
    DueOn,
    Deleted,
    IsEpic,
}

impl ConflictField {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Description => "description",
            Self::Project => "project",
            Self::Status => "status",
            Self::Priority => "priority",
            Self::AvailableAt => "available_at",
            Self::DueOn => "due_on",
            Self::Deleted => "deleted",
            Self::IsEpic => "is_epic",
        }
    }

    fn from_task_field(field: TaskField) -> Self {
        match field {
            TaskField::Title => Self::Title,
            TaskField::Description => Self::Description,
            TaskField::Project => Self::Project,
            TaskField::Status => Self::Status,
            TaskField::Priority => Self::Priority,
            TaskField::AvailableAt => Self::AvailableAt,
            TaskField::DueOn => Self::DueOn,
            TaskField::Deleted => Self::Deleted,
            TaskField::IsEpic => Self::IsEpic,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictSummary {
    pub task_id: TaskId,
    pub task_title: String,
    pub project_key: String,
    pub project_prefix: String,
    pub field: ConflictField,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub task_id: TaskId,
    pub field: ConflictField,
    pub local_value: String,
    pub remote_value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictChoice {
    Local,
    Remote,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    Validation,
    NotFound,
    OpenConflict,
    GenerationConflict,
    Database,
    Internal,
}

impl ErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Validation => "validation",
            Self::NotFound => "not_found",
            Self::OpenConflict => "open_conflict",
            Self::GenerationConflict => "generation_conflict",
            Self::Database => "database",
            Self::Internal => "internal",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub code: ErrorCode,
    pub message: String,
}

impl Error {
    fn new(code: ErrorCode, message: String) -> Self {
        Self { code, message }
    }

    fn database_open(error: InternalError) -> Self {
        Self::new(ErrorCode::Database, error.to_string())
    }

    fn from_internal(error: InternalError) -> Self {
        let code = error
            .chain()
            .find_map(|cause| cause.downcast_ref::<crate::error::CoreError>())
            .map(|error| match error.kind() {
                crate::error::ErrorKind::Validation => ErrorCode::Validation,
                crate::error::ErrorKind::NotFound => ErrorCode::NotFound,
                crate::error::ErrorKind::OpenConflict => ErrorCode::OpenConflict,
                crate::error::ErrorKind::GenerationConflict => ErrorCode::GenerationConflict,
            })
            .unwrap_or_else(|| {
                if error
                    .chain()
                    .filter_map(|cause| cause.downcast_ref::<sqlx::Error>())
                    .any(|error| matches!(error, sqlx::Error::RowNotFound))
                {
                    ErrorCode::NotFound
                } else if error
                    .chain()
                    .any(|cause| cause.downcast_ref::<sqlx::Error>().is_some())
                {
                    ErrorCode::Database
                } else {
                    ErrorCode::Internal
                }
            });
        Self::new(code, error.to_string())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_are_stable_and_internal_errors_are_distinct() {
        assert_eq!(ErrorCode::Validation.as_str(), "validation");
        assert_eq!(ErrorCode::NotFound.as_str(), "not_found");
        assert_eq!(ErrorCode::OpenConflict.as_str(), "open_conflict");
        assert_eq!(
            ErrorCode::GenerationConflict.as_str(),
            "generation_conflict"
        );
        assert_eq!(ErrorCode::Database.as_str(), "database");
        assert_eq!(ErrorCode::Internal.as_str(), "internal");
        assert_eq!(
            Error::from_internal(
                crate::error::CoreError::generation_conflict(
                    "error recurrence-generation-conflict slot=2026-08-01 field=task"
                )
                .into()
            )
            .code,
            ErrorCode::GenerationConflict
        );
        assert_eq!(
            Error::from_internal(
                crate::error::CoreError::not_found("error recurrence-series-not-found").into()
            )
            .code,
            ErrorCode::NotFound
        );
        assert_eq!(
            Error::from_internal(anyhow::anyhow!("unexpected invariant")).code,
            ErrorCode::Internal
        );
        assert_eq!(
            Error::from_internal(sqlx::Error::PoolClosed.into()).code,
            ErrorCode::Database
        );
    }
}
