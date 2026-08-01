use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use aven_core::api as core_api;
use aven_core::choices::{TaskPriority as CoreTaskPriority, TaskStatus as CoreTaskStatus};
use aven_core::ids::{TaskId, WorkspaceId};
use aven_core::recurrence::RecurrenceSeriesId;
use aven_core::sync as core_sync;
use tokio::runtime::{Builder, Runtime};

static RUNTIME: OnceLock<Result<Runtime, String>> = OnceLock::new();

fn runtime() -> Result<&'static Runtime, AvenError> {
    match RUNTIME.get_or_init(|| {
        Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|error| error.to_string())
    }) {
        Ok(runtime) => Ok(runtime),
        Err(message) => Err(AvenError::internal(message.clone())),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ErrorCode {
    Validation,
    NotFound,
    OpenConflict,
    GenerationConflict,
    Database,
    Internal,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum AvenError {
    #[error("{message}")]
    Failure { code: ErrorCode, message: String },
}

impl AvenError {
    fn validation(message: impl Into<String>) -> Self {
        Self::Failure {
            code: ErrorCode::Validation,
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::Failure {
            code: ErrorCode::Internal,
            message: message.into(),
        }
    }
}

impl From<core_api::Error> for AvenError {
    fn from(error: core_api::Error) -> Self {
        Self::Failure {
            code: match error.code {
                core_api::ErrorCode::Validation => ErrorCode::Validation,
                core_api::ErrorCode::NotFound => ErrorCode::NotFound,
                core_api::ErrorCode::OpenConflict => ErrorCode::OpenConflict,
                core_api::ErrorCode::GenerationConflict => ErrorCode::GenerationConflict,
                core_api::ErrorCode::Database => ErrorCode::Database,
                core_api::ErrorCode::Internal => ErrorCode::Internal,
            },
            message: error.message,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum TaskStatus {
    Inbox,
    Backlog,
    Todo,
    Active,
    Done,
    Canceled,
}

impl From<TaskStatus> for CoreTaskStatus {
    fn from(value: TaskStatus) -> Self {
        match value {
            TaskStatus::Inbox => Self::Inbox,
            TaskStatus::Backlog => Self::Backlog,
            TaskStatus::Todo => Self::Todo,
            TaskStatus::Active => Self::Active,
            TaskStatus::Done => Self::Done,
            TaskStatus::Canceled => Self::Canceled,
        }
    }
}

impl From<CoreTaskStatus> for TaskStatus {
    fn from(value: CoreTaskStatus) -> Self {
        match value {
            CoreTaskStatus::Inbox => Self::Inbox,
            CoreTaskStatus::Backlog => Self::Backlog,
            CoreTaskStatus::Todo => Self::Todo,
            CoreTaskStatus::Active => Self::Active,
            CoreTaskStatus::Done => Self::Done,
            CoreTaskStatus::Canceled => Self::Canceled,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum TaskPriority {
    None,
    Low,
    Medium,
    High,
    Urgent,
}

impl From<TaskPriority> for CoreTaskPriority {
    fn from(value: TaskPriority) -> Self {
        match value {
            TaskPriority::None => Self::None,
            TaskPriority::Low => Self::Low,
            TaskPriority::Medium => Self::Medium,
            TaskPriority::High => Self::High,
            TaskPriority::Urgent => Self::Urgent,
        }
    }
}

impl From<CoreTaskPriority> for TaskPriority {
    fn from(value: CoreTaskPriority) -> Self {
        match value {
            CoreTaskPriority::None => Self::None,
            CoreTaskPriority::Low => Self::Low,
            CoreTaskPriority::Medium => Self::Medium,
            CoreTaskPriority::High => Self::High,
            CoreTaskPriority::Urgent => Self::Urgent,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum OptionalDateUpdate {
    Unchanged,
    Set { value: String },
    Clear,
}

impl From<OptionalDateUpdate> for core_api::OptionalDateUpdate {
    fn from(value: OptionalDateUpdate) -> Self {
        match value {
            OptionalDateUpdate::Unchanged => Self::Unchanged,
            OptionalDateUpdate::Set { value } => Self::Set(value),
            OptionalDateUpdate::Clear => Self::Clear,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum RecurrenceFrequency {
    Daily,
    Weekly,
    Monthly,
}

impl From<RecurrenceFrequency> for core_api::RecurrenceFrequency {
    fn from(value: RecurrenceFrequency) -> Self {
        match value {
            RecurrenceFrequency::Daily => Self::Daily,
            RecurrenceFrequency::Weekly => Self::Weekly,
            RecurrenceFrequency::Monthly => Self::Monthly,
        }
    }
}

impl From<core_api::RecurrenceFrequency> for RecurrenceFrequency {
    fn from(value: core_api::RecurrenceFrequency) -> Self {
        match value {
            core_api::RecurrenceFrequency::Daily => Self::Daily,
            core_api::RecurrenceFrequency::Weekly => Self::Weekly,
            core_api::RecurrenceFrequency::Monthly => Self::Monthly,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum RecurrenceWeekday {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

impl From<RecurrenceWeekday> for core_api::RecurrenceWeekday {
    fn from(value: RecurrenceWeekday) -> Self {
        match value {
            RecurrenceWeekday::Monday => Self::Monday,
            RecurrenceWeekday::Tuesday => Self::Tuesday,
            RecurrenceWeekday::Wednesday => Self::Wednesday,
            RecurrenceWeekday::Thursday => Self::Thursday,
            RecurrenceWeekday::Friday => Self::Friday,
            RecurrenceWeekday::Saturday => Self::Saturday,
            RecurrenceWeekday::Sunday => Self::Sunday,
        }
    }
}

impl From<core_api::RecurrenceWeekday> for RecurrenceWeekday {
    fn from(value: core_api::RecurrenceWeekday) -> Self {
        match value {
            core_api::RecurrenceWeekday::Monday => Self::Monday,
            core_api::RecurrenceWeekday::Tuesday => Self::Tuesday,
            core_api::RecurrenceWeekday::Wednesday => Self::Wednesday,
            core_api::RecurrenceWeekday::Thursday => Self::Thursday,
            core_api::RecurrenceWeekday::Friday => Self::Friday,
            core_api::RecurrenceWeekday::Saturday => Self::Saturday,
            core_api::RecurrenceWeekday::Sunday => Self::Sunday,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum RecurrenceDuePolicy {
    SameDay,
    None,
}

impl From<RecurrenceDuePolicy> for core_api::RecurrenceDuePolicy {
    fn from(value: RecurrenceDuePolicy) -> Self {
        match value {
            RecurrenceDuePolicy::SameDay => Self::SameDay,
            RecurrenceDuePolicy::None => Self::None,
        }
    }
}

impl From<core_api::RecurrenceDuePolicy> for RecurrenceDuePolicy {
    fn from(value: core_api::RecurrenceDuePolicy) -> Self {
        match value {
            core_api::RecurrenceDuePolicy::SameDay => Self::SameDay,
            core_api::RecurrenceDuePolicy::None => Self::None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum RecurrenceSeriesState {
    Active,
    Paused,
    Stopped,
}

impl From<core_api::RecurrenceSeriesState> for RecurrenceSeriesState {
    fn from(value: core_api::RecurrenceSeriesState) -> Self {
        match value {
            core_api::RecurrenceSeriesState::Active => Self::Active,
            core_api::RecurrenceSeriesState::Paused => Self::Paused,
            core_api::RecurrenceSeriesState::Stopped => Self::Stopped,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum RecurrenceOutcome {
    Completed,
    Skipped,
}

impl From<RecurrenceOutcome> for core_api::RecurrenceOutcome {
    fn from(value: RecurrenceOutcome) -> Self {
        match value {
            RecurrenceOutcome::Completed => Self::Completed,
            RecurrenceOutcome::Skipped => Self::Skipped,
        }
    }
}

impl From<core_api::RecurrenceOutcome> for RecurrenceOutcome {
    fn from(value: core_api::RecurrenceOutcome) -> Self {
        match value {
            core_api::RecurrenceOutcome::Completed => Self::Completed,
            core_api::RecurrenceOutcome::Skipped => Self::Skipped,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum RecurrenceProjectionState {
    Projected,
    Resolved,
    Archived,
}

impl From<core_api::RecurrenceProjectionState> for RecurrenceProjectionState {
    fn from(value: core_api::RecurrenceProjectionState) -> Self {
        match value {
            core_api::RecurrenceProjectionState::Projected => Self::Projected,
            core_api::RecurrenceProjectionState::Resolved => Self::Resolved,
            core_api::RecurrenceProjectionState::Archived => Self::Archived,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RecurrenceRule {
    pub frequency: RecurrenceFrequency,
    pub interval: u32,
    pub weekdays: Vec<RecurrenceWeekday>,
}

impl From<RecurrenceRule> for core_api::RecurrenceRule {
    fn from(value: RecurrenceRule) -> Self {
        Self {
            frequency: value.frequency.into(),
            interval: value.interval,
            weekdays: value.weekdays.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<core_api::RecurrenceRule> for RecurrenceRule {
    fn from(value: core_api::RecurrenceRule) -> Self {
        Self {
            frequency: value.frequency.into(),
            interval: value.interval,
            weekdays: value.weekdays.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RecurrenceScheduleInput {
    pub rule: RecurrenceRule,
    pub timezone: String,
    pub start_on: String,
    pub available_local_time: Option<String>,
    pub due_policy: RecurrenceDuePolicy,
}

impl From<RecurrenceScheduleInput> for core_api::RecurrenceScheduleInput {
    fn from(value: RecurrenceScheduleInput) -> Self {
        Self {
            rule: value.rule.into(),
            timezone: value.timezone,
            start_on: value.start_on,
            available_local_time: value.available_local_time,
            due_policy: value.due_policy.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct CreateRecurrenceSeries {
    pub title: String,
    pub description: String,
    pub project: String,
    pub priority: TaskPriority,
    pub initial_status: TaskStatus,
    pub labels: Vec<String>,
    pub schedule: RecurrenceScheduleInput,
}

impl From<CreateRecurrenceSeries> for core_api::CreateRecurrenceSeries {
    fn from(value: CreateRecurrenceSeries) -> Self {
        Self {
            title: value.title,
            description: value.description,
            project: value.project,
            priority: value.priority.into(),
            initial_status: value.initial_status.into(),
            labels: value.labels,
            schedule: value.schedule.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum OptionalLocalTimeUpdate {
    Unchanged,
    Set { value: String },
    Clear,
}

impl From<OptionalLocalTimeUpdate> for core_api::OptionalLocalTimeUpdate {
    fn from(value: OptionalLocalTimeUpdate) -> Self {
        match value {
            OptionalLocalTimeUpdate::Unchanged => Self::Unchanged,
            OptionalLocalTimeUpdate::Set { value } => Self::Set(value),
            OptionalLocalTimeUpdate::Clear => Self::Clear,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
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

impl From<UpdateRecurrenceTemplate> for core_api::UpdateRecurrenceTemplate {
    fn from(value: UpdateRecurrenceTemplate) -> Self {
        Self {
            title: value.title,
            description: value.description,
            project: value.project,
            priority: value.priority.map(Into::into),
            initial_status: value.initial_status.map(Into::into),
            labels: value.labels,
            available_local_time: value.available_local_time.into(),
            due_policy: value.due_policy.map(Into::into),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct StorageLayout {
    pub root: String,
    pub objects: String,
    pub staging: String,
    pub trash: String,
    pub previews: String,
}

impl From<core_api::StorageLayout> for StorageLayout {
    fn from(value: core_api::StorageLayout) -> Self {
        Self {
            root: value.root.to_string_lossy().into_owned(),
            objects: value.objects.to_string_lossy().into_owned(),
            staging: value.staging.to_string_lossy().into_owned(),
            trash: value.trash.to_string_lossy().into_owned(),
            previews: value.previews.to_string_lossy().into_owned(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct WorkspaceRecord {
    pub id: String,
    pub key: String,
    pub name: String,
}

impl From<core_api::WorkspaceRecord> for WorkspaceRecord {
    fn from(value: core_api::WorkspaceRecord) -> Self {
        Self {
            id: value.id.to_string(),
            key: value.key,
            name: value.name,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct CreateTask {
    pub title: String,
    pub description: String,
    pub project: String,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    pub available_at: Option<String>,
    pub due_on: Option<String>,
}

impl From<CreateTask> for core_api::CreateTask {
    fn from(value: CreateTask) -> Self {
        Self {
            title: value.title,
            description: value.description,
            project: value.project,
            status: value.status.into(),
            priority: value.priority.into(),
            available_at: value.available_at,
            due_on: value.due_on,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct UpdateTask {
    pub title: Option<String>,
    pub description: Option<String>,
    pub project: Option<String>,
    pub status: Option<TaskStatus>,
    pub priority: Option<TaskPriority>,
    pub available_at: OptionalDateUpdate,
    pub due_on: OptionalDateUpdate,
}

impl From<UpdateTask> for core_api::UpdateTask {
    fn from(value: UpdateTask) -> Self {
        Self {
            title: value.title,
            description: value.description,
            project: value.project,
            status: value.status.map(Into::into),
            priority: value.priority.map(Into::into),
            available_at: value.available_at.into(),
            due_on: value.due_on.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TaskRecord {
    pub id: String,
    pub workspace_id: String,
    pub title: String,
    pub description: String,
    pub project_id: String,
    pub project_key: String,
    pub project_prefix: String,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    pub created_at: String,
    pub updated_at: String,
    pub available_at: Option<String>,
    pub due_on: Option<String>,
}

impl From<core_api::TaskRecord> for TaskRecord {
    fn from(value: core_api::TaskRecord) -> Self {
        Self {
            id: value.id.to_string(),
            workspace_id: value.workspace_id.to_string(),
            title: value.title,
            description: value.description,
            project_id: value.project_id.to_string(),
            project_key: value.project_key,
            project_prefix: value.project_prefix,
            status: value.status.into(),
            priority: value.priority.into(),
            created_at: value.created_at,
            updated_at: value.updated_at,
            available_at: value.available_at,
            due_on: value.due_on,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TaskUpdateResult {
    pub task: TaskRecord,
    pub changed: bool,
}

impl From<core_api::TaskUpdateResult> for TaskUpdateResult {
    fn from(value: core_api::TaskUpdateResult) -> Self {
        Self {
            task: value.task.into(),
            changed: value.changed,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RecurrenceSeriesRecord {
    pub workspace_id: String,
    pub id: String,
    pub title: String,
    pub description: String,
    pub project_id: String,
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

impl From<core_api::RecurrenceSeriesRecord> for RecurrenceSeriesRecord {
    fn from(value: core_api::RecurrenceSeriesRecord) -> Self {
        Self {
            workspace_id: value.workspace_id.to_string(),
            id: value.id.to_string(),
            title: value.title,
            description: value.description,
            project_id: value.project_id.to_string(),
            priority: value.priority.into(),
            initial_status: value.initial_status.into(),
            rule: value.rule.into(),
            timezone: value.timezone,
            start_on: value.start_on,
            available_local_time: value.available_local_time,
            due_policy: value.due_policy.into(),
            state: value.state.into(),
            stopped_at: value.stopped_at,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RecurrenceOccurrenceRecord {
    pub series_id: String,
    pub slot_on: String,
    pub task_id: Option<String>,
    pub outcome: Option<RecurrenceOutcome>,
    pub resolved_at: Option<String>,
    pub projection_state: RecurrenceProjectionState,
    pub archived_at: Option<String>,
}

impl From<core_api::RecurrenceOccurrenceRecord> for RecurrenceOccurrenceRecord {
    fn from(value: core_api::RecurrenceOccurrenceRecord) -> Self {
        Self {
            series_id: value.series_id.to_string(),
            slot_on: value.slot_on,
            task_id: value.task_id.map(|id| id.to_string()),
            outcome: value.outcome.map(Into::into),
            resolved_at: value.resolved_at,
            projection_state: value.projection_state.into(),
            archived_at: value.archived_at,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RecurrenceCreateResult {
    pub series: RecurrenceSeriesRecord,
    pub series_ref: String,
    pub occurrence: RecurrenceOccurrenceRecord,
    pub task: TaskRecord,
}

impl From<core_api::RecurrenceCreateResult> for RecurrenceCreateResult {
    fn from(value: core_api::RecurrenceCreateResult) -> Self {
        Self {
            series: value.series.into(),
            series_ref: value.series_ref,
            occurrence: value.occurrence.into(),
            task: value.task.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RecurrenceTemplateUpdateResult {
    pub series: RecurrenceSeriesRecord,
    pub changed: bool,
}

impl From<core_api::RecurrenceTemplateUpdateResult> for RecurrenceTemplateUpdateResult {
    fn from(value: core_api::RecurrenceTemplateUpdateResult) -> Self {
        Self {
            series: value.series.into(),
            changed: value.changed,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RecurrenceResolveResult {
    pub series: RecurrenceSeriesRecord,
    pub occurrence: RecurrenceOccurrenceRecord,
    pub task: TaskRecord,
    pub successor: Option<TaskRecord>,
}

impl From<core_api::RecurrenceResolveResult> for RecurrenceResolveResult {
    fn from(value: core_api::RecurrenceResolveResult) -> Self {
        Self {
            series: value.series.into(),
            occurrence: value.occurrence.into(),
            task: value.task.into(),
            successor: value.successor.map(Into::into),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RecurrenceStateResult {
    pub series: RecurrenceSeriesRecord,
    pub occurrence: Option<RecurrenceOccurrenceRecord>,
}

impl From<core_api::RecurrenceStateResult> for RecurrenceStateResult {
    fn from(value: core_api::RecurrenceStateResult) -> Self {
        Self {
            series: value.series.into(),
            occurrence: value.occurrence.map(Into::into),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RecurrenceRefResolution {
    pub series_id: String,
    pub series_ref: String,
}

impl From<core_api::RecurrenceRefResolution> for RecurrenceRefResolution {
    fn from(value: core_api::RecurrenceRefResolution) -> Self {
        Self {
            series_id: value.series_id.to_string(),
            series_ref: value.series_ref,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RecurrenceCounts {
    pub completed: u64,
    pub skipped: u64,
    pub missed: u64,
    pub pause_intervals: u64,
    pub latest_slot_on: Option<String>,
    pub latest_outcome: Option<RecurrenceOutcome>,
}

impl From<core_api::RecurrenceCounts> for RecurrenceCounts {
    fn from(value: core_api::RecurrenceCounts) -> Self {
        Self {
            completed: value.completed,
            skipped: value.skipped,
            missed: value.missed,
            pause_intervals: value.pause_intervals,
            latest_slot_on: value.latest_slot_on,
            latest_outcome: value.latest_outcome.map(Into::into),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RecurrenceSeriesSummary {
    pub series: RecurrenceSeriesRecord,
    pub series_ref: String,
    pub rule_label: String,
    pub current_slot_on: Option<String>,
    pub current_task_ref: Option<String>,
    pub counts: RecurrenceCounts,
}

impl From<core_api::RecurrenceSeriesSummary> for RecurrenceSeriesSummary {
    fn from(value: core_api::RecurrenceSeriesSummary) -> Self {
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

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RecurrenceSeriesConflict {
    pub field: String,
    pub variant_a: String,
    pub local_value: String,
    pub variant_b: String,
    pub remote_value: String,
}

impl From<core_api::RecurrenceSeriesConflict> for RecurrenceSeriesConflict {
    fn from(value: core_api::RecurrenceSeriesConflict) -> Self {
        Self {
            field: value.field,
            variant_a: value.variant_a,
            local_value: value.local_value,
            variant_b: value.variant_b,
            remote_value: value.remote_value,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RecurrenceSeriesDetail {
    pub series: RecurrenceSeriesRecord,
    pub labels: Vec<String>,
    pub summary: RecurrenceSeriesSummary,
    pub current_occurrence: Option<RecurrenceOccurrenceRecord>,
    pub lifecycle_conflicts: Vec<RecurrenceSeriesConflict>,
}

impl From<core_api::RecurrenceSeriesDetail> for RecurrenceSeriesDetail {
    fn from(value: core_api::RecurrenceSeriesDetail) -> Self {
        Self {
            series: value.series.into(),
            labels: value.labels,
            summary: value.summary.into(),
            current_occurrence: value.current_occurrence.map(Into::into),
            lifecycle_conflicts: value
                .lifecycle_conflicts
                .into_iter()
                .map(Into::into)
                .collect(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum RecurrenceHistoryKind {
    Completed,
    Skipped,
    Missed,
    Paused,
}

impl From<core_api::RecurrenceHistoryKind> for RecurrenceHistoryKind {
    fn from(value: core_api::RecurrenceHistoryKind) -> Self {
        match value {
            core_api::RecurrenceHistoryKind::Completed => Self::Completed,
            core_api::RecurrenceHistoryKind::Skipped => Self::Skipped,
            core_api::RecurrenceHistoryKind::Missed => Self::Missed,
            core_api::RecurrenceHistoryKind::Paused => Self::Paused,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RecurrenceHistoryRow {
    pub kind: RecurrenceHistoryKind,
    pub slot_on: Option<String>,
    pub interval_started_at: Option<String>,
    pub interval_ended_at: Option<String>,
    pub task_id: Option<String>,
    pub task_ref: Option<String>,
    pub openable: bool,
    pub archived_projection: bool,
    pub resolved_at: Option<String>,
}

impl From<core_api::RecurrenceHistoryRow> for RecurrenceHistoryRow {
    fn from(value: core_api::RecurrenceHistoryRow) -> Self {
        Self {
            kind: value.kind.into(),
            slot_on: value.slot_on,
            interval_started_at: value.interval_started_at,
            interval_ended_at: value.interval_ended_at,
            task_id: value.task_id.map(|id| id.to_string()),
            task_ref: value.task_ref,
            openable: value.openable,
            archived_projection: value.archived_projection,
            resolved_at: value.resolved_at,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RecurrenceHistoryPage {
    pub series_ref: String,
    pub items: Vec<RecurrenceHistoryRow>,
    pub offset: u64,
    pub limit: u64,
    pub total: u64,
    pub has_more: bool,
}

impl TryFrom<core_api::RecurrenceHistoryPage> for RecurrenceHistoryPage {
    type Error = AvenError;

    fn try_from(value: core_api::RecurrenceHistoryPage) -> Result<Self, Self::Error> {
        Ok(Self {
            series_ref: value.series_ref,
            items: value.items.into_iter().map(Into::into).collect(),
            offset: to_u64(value.offset, "history offset")?,
            limit: to_u64(value.limit, "history limit")?,
            total: to_u64(value.total, "history total")?,
            has_more: value.has_more,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TaskRecurrenceSummary {
    pub series_id: String,
    pub series_ref: String,
    pub slot_on: String,
    pub rule_label: String,
    pub timezone: String,
    pub lifecycle: RecurrenceSeriesState,
    pub outcome: Option<RecurrenceOutcome>,
    pub projection_state: RecurrenceProjectionState,
}

impl From<core_api::TaskRecurrenceSummary> for TaskRecurrenceSummary {
    fn from(value: core_api::TaskRecurrenceSummary) -> Self {
        Self {
            series_id: value.series_id.to_string(),
            series_ref: value.series_ref,
            slot_on: value.slot_on,
            rule_label: value.rule_label,
            timezone: value.timezone,
            lifecycle: value.lifecycle.into(),
            outcome: value.outcome.map(Into::into),
            projection_state: value.projection_state.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RecurrenceTaskGroup {
    pub series_id: String,
    pub series_ref: String,
    pub counts: RecurrenceCounts,
}

impl From<core_api::RecurrenceTaskGroup> for RecurrenceTaskGroup {
    fn from(value: core_api::RecurrenceTaskGroup) -> Self {
        Self {
            series_id: value.series_id.to_string(),
            series_ref: value.series_ref,
            counts: value.counts.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TaskSummary {
    pub task: TaskRecord,
    pub display_ref: String,
    pub recurrence: Option<TaskRecurrenceSummary>,
    pub recurrence_group: Option<RecurrenceTaskGroup>,
}

impl From<core_api::TaskSummary> for TaskSummary {
    fn from(value: core_api::TaskSummary) -> Self {
        Self {
            task: value.task.into(),
            display_ref: value.display_ref,
            recurrence: value.recurrence.map(Into::into),
            recurrence_group: value.recurrence_group.map(Into::into),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
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

impl From<ConflictField> for core_api::ConflictField {
    fn from(value: ConflictField) -> Self {
        match value {
            ConflictField::Title => Self::Title,
            ConflictField::Description => Self::Description,
            ConflictField::Project => Self::Project,
            ConflictField::Status => Self::Status,
            ConflictField::Priority => Self::Priority,
            ConflictField::AvailableAt => Self::AvailableAt,
            ConflictField::DueOn => Self::DueOn,
            ConflictField::Deleted => Self::Deleted,
            ConflictField::IsEpic => Self::IsEpic,
        }
    }
}

impl From<core_api::ConflictField> for ConflictField {
    fn from(value: core_api::ConflictField) -> Self {
        match value {
            core_api::ConflictField::Title => Self::Title,
            core_api::ConflictField::Description => Self::Description,
            core_api::ConflictField::Project => Self::Project,
            core_api::ConflictField::Status => Self::Status,
            core_api::ConflictField::Priority => Self::Priority,
            core_api::ConflictField::AvailableAt => Self::AvailableAt,
            core_api::ConflictField::DueOn => Self::DueOn,
            core_api::ConflictField::Deleted => Self::Deleted,
            core_api::ConflictField::IsEpic => Self::IsEpic,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ConflictSummary {
    pub task_id: String,
    pub task_title: String,
    pub project_key: String,
    pub project_prefix: String,
    pub field: ConflictField,
}

impl From<core_api::ConflictSummary> for ConflictSummary {
    fn from(value: core_api::ConflictSummary) -> Self {
        Self {
            task_id: value.task_id.to_string(),
            task_title: value.task_title,
            project_key: value.project_key,
            project_prefix: value.project_prefix,
            field: value.field.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct Conflict {
    pub task_id: String,
    pub field: ConflictField,
    pub local_value: String,
    pub remote_value: String,
}

impl From<core_api::Conflict> for Conflict {
    fn from(value: core_api::Conflict) -> Self {
        Self {
            task_id: value.task_id.to_string(),
            field: value.field.into(),
            local_value: value.local_value,
            remote_value: value.remote_value,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ConflictChoice {
    Local,
    Remote,
}

impl From<ConflictChoice> for core_api::ConflictChoice {
    fn from(value: ConflictChoice) -> Self {
        match value {
            ConflictChoice::Local => Self::Local,
            ConflictChoice::Remote => Self::Remote,
        }
    }
}

#[derive(uniffi::Object)]
pub struct AvenClient {
    store: core_api::Store,
}

#[uniffi::export]
impl AvenClient {
    #[uniffi::constructor]
    pub fn open(path: String) -> Result<Arc<Self>, AvenError> {
        let store = runtime()?.block_on(core_api::Store::open(path))?;
        Ok(Arc::new(Self { store }))
    }

    pub fn list_workspaces(&self) -> Result<Vec<WorkspaceRecord>, AvenError> {
        runtime()?
            .block_on(self.store.list_workspaces())
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(Into::into)
    }

    pub fn initialize_storage(&self) -> Result<StorageLayout, AvenError> {
        self.store
            .initialize_storage()
            .map(Into::into)
            .map_err(Into::into)
    }

    pub fn resolve_workspace(&self, name_or_key: String) -> Result<WorkspaceRecord, AvenError> {
        runtime()?
            .block_on(self.store.resolve_workspace(&name_or_key))
            .map(Into::into)
            .map_err(Into::into)
    }

    pub fn create_task(
        &self,
        workspace_id: String,
        input: CreateTask,
    ) -> Result<TaskRecord, AvenError> {
        let workspace_id = parse_workspace_id(&workspace_id)?;
        runtime()?
            .block_on(self.store.create_task(&workspace_id, input.into()))
            .map(Into::into)
            .map_err(Into::into)
    }

    pub fn update_task(
        &self,
        workspace_id: String,
        task_id: String,
        input: UpdateTask,
    ) -> Result<TaskUpdateResult, AvenError> {
        let workspace_id = parse_workspace_id(&workspace_id)?;
        let task_id = parse_task_id(&task_id)?;
        runtime()?
            .block_on(
                self.store
                    .update_task(&workspace_id, &task_id, input.into()),
            )
            .map(Into::into)
            .map_err(Into::into)
    }

    pub fn list_tasks(&self, workspace_id: String) -> Result<Vec<TaskRecord>, AvenError> {
        let workspace_id = parse_workspace_id(&workspace_id)?;
        runtime()?
            .block_on(self.store.list_tasks(&workspace_id))
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(Into::into)
    }

    pub fn fetch_task(
        &self,
        workspace_id: String,
        task_id: String,
    ) -> Result<TaskRecord, AvenError> {
        let workspace_id = parse_workspace_id(&workspace_id)?;
        let task_id = parse_task_id(&task_id)?;
        runtime()?
            .block_on(self.store.fetch_task(&workspace_id, &task_id))
            .map(Into::into)
            .map_err(Into::into)
    }

    pub fn create_recurrence_series(
        &self,
        workspace_id: String,
        input: CreateRecurrenceSeries,
    ) -> Result<RecurrenceCreateResult, AvenError> {
        let workspace_id = parse_workspace_id(&workspace_id)?;
        runtime()?
            .block_on(
                self.store
                    .create_recurrence_series(&workspace_id, input.into()),
            )
            .map(Into::into)
            .map_err(Into::into)
    }

    pub fn update_recurrence_template(
        &self,
        workspace_id: String,
        series_id: String,
        input: UpdateRecurrenceTemplate,
    ) -> Result<RecurrenceTemplateUpdateResult, AvenError> {
        let workspace_id = parse_workspace_id(&workspace_id)?;
        let series_id = parse_recurrence_series_id(&series_id)?;
        runtime()?
            .block_on(self.store.update_recurrence_template(
                &workspace_id,
                &series_id,
                input.into(),
            ))
            .map(Into::into)
            .map_err(Into::into)
    }

    pub fn resolve_recurrence_ref(
        &self,
        workspace_id: String,
        input: String,
    ) -> Result<RecurrenceRefResolution, AvenError> {
        let workspace_id = parse_workspace_id(&workspace_id)?;
        runtime()?
            .block_on(self.store.resolve_recurrence_ref(&workspace_id, &input))
            .map(Into::into)
            .map_err(Into::into)
    }

    pub fn list_recurrence_series(
        &self,
        workspace_id: String,
    ) -> Result<Vec<RecurrenceSeriesSummary>, AvenError> {
        let workspace_id = parse_workspace_id(&workspace_id)?;
        runtime()?
            .block_on(self.store.list_recurrence_series(&workspace_id))
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(Into::into)
    }

    pub fn show_recurrence_series(
        &self,
        workspace_id: String,
        input: String,
    ) -> Result<RecurrenceSeriesDetail, AvenError> {
        let workspace_id = parse_workspace_id(&workspace_id)?;
        runtime()?
            .block_on(self.store.show_recurrence_series(&workspace_id, &input))
            .map(Into::into)
            .map_err(Into::into)
    }

    pub fn recurrence_history(
        &self,
        workspace_id: String,
        input: String,
        offset: u64,
        limit: u64,
    ) -> Result<RecurrenceHistoryPage, AvenError> {
        let workspace_id = parse_workspace_id(&workspace_id)?;
        let offset = usize::try_from(offset)
            .map_err(|_| AvenError::validation("history offset exceeds the host limit"))?;
        let limit = usize::try_from(limit)
            .map_err(|_| AvenError::validation("history limit exceeds the host limit"))?;
        let page = runtime()?.block_on(self.store.recurrence_history(
            &workspace_id,
            &input,
            offset,
            limit,
        ))?;
        page.try_into()
    }

    pub fn complete_recurrence_occurrence(
        &self,
        workspace_id: String,
        task_id: String,
    ) -> Result<RecurrenceResolveResult, AvenError> {
        let workspace_id = parse_workspace_id(&workspace_id)?;
        let task_id = parse_task_id(&task_id)?;
        runtime()?
            .block_on(
                self.store
                    .complete_recurrence_occurrence(&workspace_id, &task_id),
            )
            .map(Into::into)
            .map_err(Into::into)
    }

    pub fn skip_recurrence_occurrence(
        &self,
        workspace_id: String,
        task_id: String,
    ) -> Result<RecurrenceResolveResult, AvenError> {
        let workspace_id = parse_workspace_id(&workspace_id)?;
        let task_id = parse_task_id(&task_id)?;
        runtime()?
            .block_on(
                self.store
                    .skip_recurrence_occurrence(&workspace_id, &task_id),
            )
            .map(Into::into)
            .map_err(Into::into)
    }

    pub fn pause_recurrence_series(
        &self,
        workspace_id: String,
        series_id: String,
    ) -> Result<RecurrenceStateResult, AvenError> {
        let workspace_id = parse_workspace_id(&workspace_id)?;
        let series_id = parse_recurrence_series_id(&series_id)?;
        runtime()?
            .block_on(
                self.store
                    .pause_recurrence_series(&workspace_id, &series_id),
            )
            .map(Into::into)
            .map_err(Into::into)
    }

    pub fn resume_recurrence_series(
        &self,
        workspace_id: String,
        series_id: String,
    ) -> Result<RecurrenceStateResult, AvenError> {
        let workspace_id = parse_workspace_id(&workspace_id)?;
        let series_id = parse_recurrence_series_id(&series_id)?;
        runtime()?
            .block_on(
                self.store
                    .resume_recurrence_series(&workspace_id, &series_id),
            )
            .map(Into::into)
            .map_err(Into::into)
    }

    pub fn stop_recurrence_series(
        &self,
        workspace_id: String,
        series_id: String,
        skip_current: bool,
    ) -> Result<RecurrenceStateResult, AvenError> {
        let workspace_id = parse_workspace_id(&workspace_id)?;
        let series_id = parse_recurrence_series_id(&series_id)?;
        runtime()?
            .block_on(
                self.store
                    .stop_recurrence_series(&workspace_id, &series_id, skip_current),
            )
            .map(Into::into)
            .map_err(Into::into)
    }

    pub fn recurrence_task_report(
        &self,
        workspace_id: String,
        expand_recurring: bool,
    ) -> Result<Vec<TaskSummary>, AvenError> {
        let workspace_id = parse_workspace_id(&workspace_id)?;
        runtime()?
            .block_on(
                self.store
                    .recurrence_task_report(&workspace_id, expand_recurring),
            )
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(Into::into)
    }

    pub fn list_conflicts(&self, workspace_id: String) -> Result<Vec<ConflictSummary>, AvenError> {
        let workspace_id = parse_workspace_id(&workspace_id)?;
        runtime()?
            .block_on(self.store.list_conflicts(&workspace_id))
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(Into::into)
    }

    pub fn inspect_conflicts(
        &self,
        workspace_id: String,
        task_id: String,
    ) -> Result<Vec<Conflict>, AvenError> {
        let workspace_id = parse_workspace_id(&workspace_id)?;
        let task_id = parse_task_id(&task_id)?;
        runtime()?
            .block_on(self.store.inspect_conflicts(&workspace_id, &task_id))
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(Into::into)
    }

    pub fn resolve_conflict(
        &self,
        workspace_id: String,
        task_id: String,
        field: ConflictField,
        choice: ConflictChoice,
    ) -> Result<TaskRecord, AvenError> {
        let workspace_id = parse_workspace_id(&workspace_id)?;
        let task_id = parse_task_id(&task_id)?;
        runtime()?
            .block_on(self.store.resolve_conflict(
                &workspace_id,
                &task_id,
                field.into(),
                choice.into(),
            ))
            .map(Into::into)
            .map_err(Into::into)
    }

    pub fn start_sync_session(
        &self,
        server: String,
        auth_token: Option<String>,
        page_budget: Option<u64>,
    ) -> Result<Arc<AvenSyncSession>, AvenError> {
        let page_budget = page_budget
            .map(|value| {
                usize::try_from(value)
                    .map_err(|_| AvenError::validation("page budget exceeds the host limit"))
            })
            .transpose()?;
        let session = runtime()?.block_on(self.store.start_sync_session(
            server,
            auth_token,
            page_budget,
        ))?;
        Ok(Arc::new(AvenSyncSession {
            session: Mutex::new(session),
        }))
    }
}

fn parse_workspace_id(value: &str) -> Result<WorkspaceId, AvenError> {
    value
        .parse()
        .map_err(|error| AvenError::validation(format!("invalid workspace id: {error}")))
}

fn parse_task_id(value: &str) -> Result<TaskId, AvenError> {
    value
        .parse()
        .map_err(|error| AvenError::validation(format!("invalid task id: {error}")))
}

fn parse_recurrence_series_id(value: &str) -> Result<RecurrenceSeriesId, AvenError> {
    value
        .parse()
        .map_err(|error| AvenError::validation(format!("invalid recurrence series id: {error}")))
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SyncHttpHeader {
    pub name: String,
    pub value: String,
}

impl From<SyncHttpHeader> for core_sync::SyncHttpHeader {
    fn from(value: SyncHttpHeader) -> Self {
        Self {
            name: value.name,
            value: value.value,
        }
    }
}

impl From<core_sync::SyncHttpHeader> for SyncHttpHeader {
    fn from(value: core_sync::SyncHttpHeader) -> Self {
        Self {
            name: value.name,
            value: value.value,
        }
    }
}

#[derive(uniffi::Object)]
pub struct SyncRequestContext {
    context: core_sync::SyncRequestContext,
}

#[derive(Clone, uniffi::Record)]
pub struct PreparedSyncRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<SyncHttpHeader>,
    pub body: Vec<u8>,
    pub context: Arc<SyncRequestContext>,
}

impl From<core_sync::PreparedSyncRequest> for PreparedSyncRequest {
    fn from(value: core_sync::PreparedSyncRequest) -> Self {
        Self {
            method: value.method,
            url: value.url,
            headers: value.headers.into_iter().map(Into::into).collect(),
            body: value.body,
            context: Arc::new(SyncRequestContext {
                context: value.context,
            }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SyncHttpResponse {
    pub status: u16,
    pub headers: Vec<SyncHttpHeader>,
    pub body: Vec<u8>,
}

impl From<SyncHttpResponse> for core_sync::SyncHttpResponse {
    fn from(value: SyncHttpResponse) -> Self {
        Self {
            status: value.status,
            headers: value.headers.into_iter().map(Into::into).collect(),
            body: value.body,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SyncPageOutcome {
    pub page: u64,
    pub pushed: u64,
    pub pulled: u64,
    pub blob_uploaded: u64,
    pub blob_uploaded_bytes: u64,
    pub blob_downloaded: u64,
    pub blob_downloaded_bytes: u64,
    pub cursor: i64,
    pub complete: bool,
    pub has_more: bool,
    pub local_more: bool,
    pub request_bytes: u64,
    pub request_wire_bytes: u64,
    pub response_decoded_bytes: u64,
    pub response_compression: String,
    pub apply_ms: u64,
}

impl TryFrom<core_sync::SyncPageOutcome> for SyncPageOutcome {
    type Error = AvenError;

    fn try_from(value: core_sync::SyncPageOutcome) -> Result<Self, Self::Error> {
        Ok(Self {
            page: to_u64(value.page, "page")?,
            pushed: to_u64(value.pushed, "pushed")?,
            pulled: to_u64(value.pulled, "pulled")?,
            blob_uploaded: to_u64(value.blob_uploaded, "blob uploaded")?,
            blob_uploaded_bytes: value.blob_uploaded_bytes,
            blob_downloaded: to_u64(value.blob_downloaded, "blob downloaded")?,
            blob_downloaded_bytes: value.blob_downloaded_bytes,
            cursor: value.cursor,
            complete: value.complete,
            has_more: value.has_more,
            local_more: value.local_more,
            request_bytes: to_u64(value.request_bytes, "request bytes")?,
            request_wire_bytes: to_u64(value.request_wire_bytes, "request wire bytes")?,
            response_decoded_bytes: to_u64(value.response_decoded_bytes, "response decoded bytes")?,
            response_compression: value.response_compression,
            apply_ms: u64::try_from(value.apply_ms)
                .map_err(|_| AvenError::internal("apply duration exceeds the facade limit"))?,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SyncSessionSummary {
    pub pushed: i64,
    pub pulled: u64,
    pub blob_uploaded: u64,
    pub blob_uploaded_bytes: u64,
    pub blob_downloaded: u64,
    pub blob_downloaded_bytes: u64,
    pub blob_upload_remaining: u64,
    pub blob_upload_remaining_bytes: u64,
    pub blob_download_remaining: u64,
    pub blob_download_remaining_bytes: u64,
    pub cursor: i64,
    pub complete: bool,
    pub pages: u64,
    pub request_bytes: u64,
    pub request_wire_bytes: u64,
    pub response_decoded_bytes: u64,
    pub response_compression: String,
    pub apply_ms: u64,
}

impl TryFrom<core_sync::SyncSessionSummary> for SyncSessionSummary {
    type Error = AvenError;

    fn try_from(value: core_sync::SyncSessionSummary) -> Result<Self, Self::Error> {
        Ok(Self {
            pushed: value.pushed,
            pulled: to_u64(value.pulled, "pulled")?,
            blob_uploaded: to_u64(value.blob_uploaded, "blob uploaded")?,
            blob_uploaded_bytes: value.blob_uploaded_bytes,
            blob_downloaded: to_u64(value.blob_downloaded, "blob downloaded")?,
            blob_downloaded_bytes: value.blob_downloaded_bytes,
            blob_upload_remaining: to_u64(value.blob_upload_remaining, "blob upload remaining")?,
            blob_upload_remaining_bytes: value.blob_upload_remaining_bytes,
            blob_download_remaining: to_u64(
                value.blob_download_remaining,
                "blob download remaining",
            )?,
            blob_download_remaining_bytes: value.blob_download_remaining_bytes,
            cursor: value.cursor,
            complete: value.complete,
            pages: to_u64(value.pages, "pages")?,
            request_bytes: to_u64(value.request_bytes, "request bytes")?,
            request_wire_bytes: to_u64(value.request_wire_bytes, "request wire bytes")?,
            response_decoded_bytes: to_u64(value.response_decoded_bytes, "response decoded bytes")?,
            response_compression: value.response_compression,
            apply_ms: u64::try_from(value.apply_ms)
                .map_err(|_| AvenError::internal("apply duration exceeds the facade limit"))?,
        })
    }
}

fn to_u64(value: usize, name: &str) -> Result<u64, AvenError> {
    u64::try_from(value)
        .map_err(|_| AvenError::internal(format!("{name} exceeds the facade limit")))
}

#[derive(uniffi::Object)]
pub struct AvenSyncSession {
    session: Mutex<core_sync::SyncSession>,
}

#[uniffi::export]
impl AvenSyncSession {
    pub fn prepare_request(&self) -> Result<Option<PreparedSyncRequest>, AvenError> {
        let mut session = self.lock()?;
        runtime()?
            .block_on(session.prepare_request())
            .map(|request| request.map(Into::into))
            .map_err(|error| AvenError::internal(error.to_string()))
    }

    pub fn accept_response(
        &self,
        context: Arc<SyncRequestContext>,
        response: SyncHttpResponse,
    ) -> Result<SyncPageOutcome, AvenError> {
        let mut session = self.lock()?;
        let outcome = runtime()?
            .block_on(session.accept_response(&context.context, response.into()))
            .map_err(|error| AvenError::internal(error.to_string()))?;
        outcome.try_into()
    }

    pub fn fail_request(
        &self,
        context: Arc<SyncRequestContext>,
        message: String,
    ) -> Result<(), AvenError> {
        let mut session = self.lock()?;
        runtime()?
            .block_on(session.fail_request(&context.context, message))
            .map_err(|error| AvenError::internal(error.to_string()))
    }

    pub fn summary(&self) -> Result<SyncSessionSummary, AvenError> {
        self.lock()?.summary().try_into()
    }
}

impl AvenSyncSession {
    fn lock(&self) -> Result<MutexGuard<'_, core_sync::SyncSession>, AvenError> {
        self.session
            .lock()
            .map_err(|_| AvenError::internal("sync session lock is poisoned"))
    }
}

uniffi::setup_scaffolding!();
