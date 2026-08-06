use crate::attachments::AttachmentBytesState;
use crate::ids::{TaskId, WorkspaceId};
use crate::queue::QueueMeta;
use crate::recurrence::{
    RecurrenceOutcome, RecurrenceProjectionState, RecurrenceSeriesId, RecurrenceSeriesState,
};
use crate::types::{RecurrenceOccurrence, RecurrenceSeries, Task};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskQueryMode {
    Flat,
    RankedQueue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskSort {
    Created,
    Updated,
    Priority,
    Project,
    Title,
    AvailableAt,
    DueOn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TaskAvailabilityFilter {
    #[default]
    All,
    Available,
    Upcoming,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    Asc,
    Desc,
}

impl SortDirection {
    pub fn toggled(self) -> Self {
        match self {
            Self::Asc => Self::Desc,
            Self::Desc => Self::Asc,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum TaskIdFilter {
    #[default]
    Unrestricted,
    Only(Vec<TaskId>),
}

#[derive(Debug, Clone, Default)]
pub struct TaskFilters {
    pub project: Option<String>,
    pub status: Option<String>,
    pub statuses: Vec<String>,
    pub priority: Option<String>,
    pub label: Option<String>,
    pub include_deleted: bool,
    pub deleted_only: bool,
    pub hide_done: bool,
    pub conflicts_only: bool,
    pub ready_only: bool,
    pub blocked_only: bool,
    pub epics_only: bool,
    pub exclude_epics: bool,
    pub availability: TaskAvailabilityFilter,
    pub overdue_only: bool,
    pub search: Option<String>,
    pub task_ids: TaskIdFilter,
    pub expand_recurring: bool,
}

impl TaskFilters {
    pub fn with_project(mut self, project: Option<String>) -> Self {
        self.project = project;
        self
    }

    pub fn with_status(mut self, status: Option<String>) -> Self {
        self.status = status;
        self
    }

    pub fn with_priority(mut self, priority: Option<String>) -> Self {
        self.priority = priority;
        self
    }

    pub fn include_deleted(mut self, include_deleted: bool) -> Self {
        self.include_deleted = include_deleted;
        self
    }

    pub fn deleted_only(mut self, deleted_only: bool) -> Self {
        self.deleted_only = deleted_only;
        self
    }
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RecurrenceSeriesLifecycleFilter {
    #[default]
    ActiveOrPaused,
    Active,
    Paused,
    Stopped,
    All,
}

impl RecurrenceSeriesLifecycleFilter {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ActiveOrPaused => "active-or-paused",
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Stopped => "stopped",
            Self::All => "all",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecurrenceSeriesListQuery {
    pub lifecycle: RecurrenceSeriesLifecycleFilter,
    pub project: Option<String>,
    pub search: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceOccurrenceLink {
    pub slot_on: String,
    pub task_id: TaskId,
    pub task_ref: String,
}

#[derive(Debug, Clone)]
pub struct RecurrenceSeriesListItem {
    pub series: RecurrenceSeries,
    pub series_ref: String,
    pub project_key: String,
    pub rule_label: String,
    pub current_occurrence: Option<RecurrenceOccurrenceLink>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecurrenceCounts {
    pub series_ref: String,
    pub completed: usize,
    pub skipped: usize,
    pub missed: usize,
    pub pause_intervals: usize,
    pub latest_slot_on: Option<String>,
    pub latest_outcome: Option<RecurrenceOutcome>,
}

#[derive(Debug, Clone)]
pub struct RecurrenceSeriesSummary {
    pub series: RecurrenceSeries,
    pub series_ref: String,
    pub rule_label: String,
    pub current_slot_on: Option<String>,
    pub current_task_ref: Option<String>,
    pub counts: RecurrenceCounts,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceSeriesConflict {
    pub field: String,
    pub variant_a: String,
    pub local_value: String,
    pub variant_b: String,
    pub remote_value: String,
}

#[derive(Debug, Clone)]
pub struct RecurrenceSeriesDetail {
    pub series: RecurrenceSeries,
    pub labels: Vec<String>,
    pub summary: RecurrenceSeriesSummary,
    pub current_occurrence: Option<RecurrenceOccurrence>,
    pub lifecycle_conflicts: Vec<RecurrenceSeriesConflict>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecurrenceHistoryKind {
    Completed,
    Skipped,
    Missed,
    Paused,
}

impl RecurrenceHistoryKind {
    pub(crate) const fn order(self) -> u8 {
        match self {
            Self::Completed => 0,
            Self::Skipped => 1,
            Self::Missed => 2,
            Self::Paused => 3,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceHistoryEntry {
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

impl RecurrenceHistoryEntry {
    pub(crate) fn sort_key(&self) -> &str {
        self.slot_on
            .as_deref()
            .or(self.interval_started_at.as_deref())
            .unwrap_or("")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceHistoryPage {
    pub series_ref: String,
    pub items: Vec<RecurrenceHistoryEntry>,
    pub offset: usize,
    pub limit: usize,
    pub total: usize,
    pub has_more: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecurrenceReconciliation {
    pub workspace_id: Option<WorkspaceId>,
    pub examined: usize,
    pub changed: usize,
    pub lifecycle_blocked: usize,
    pub incomplete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceTaskGroup {
    pub series_id: RecurrenceSeriesId,
    pub series_ref: String,
    pub counts: RecurrenceCounts,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskListItem {
    pub task: Task,
    pub display_ref: String,
    pub labels: Vec<String>,
    pub notes: Vec<TaskNote>,
    pub attachments: Vec<AttachmentMetadata>,
    pub has_conflict: bool,
    pub unresolved_blocker_count: i64,
    pub dependent_count: i64,
    pub depends_on: Vec<TaskDependencyLink>,
    pub blocks: Vec<TaskDependencyLink>,
    pub epic_children: Vec<TaskDependencyLink>,
    pub epic_parent: Option<TaskDependencyLink>,
    pub queue: QueueMeta,
    pub recurrence: Option<TaskRecurrenceSummary>,
    pub recurrence_group: Option<RecurrenceTaskGroup>,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct AttachmentMetadata {
    pub attachment_id: String,
    pub task_id: String,
    #[serde(skip)]
    pub sha256: String,
    pub media_type: String,
    pub byte_size: i64,
    pub filename: Option<String>,
    pub alt_text: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub created_at: String,
    pub deleted: bool,
    pub deleted_at: Option<String>,
    #[serde(skip)]
    pub bytes_state: AttachmentBytesState,
    pub has_blob: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskNote {
    pub id: String,
    pub body: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskDependencyLink {
    pub task_id: TaskId,
    pub display_ref: String,
    pub title: String,
    pub status: String,
    pub priority: String,
    pub unresolved: bool,
}

#[derive(Debug, Clone)]
pub struct RecentActionItem {
    pub change_id: String,
    pub entity_type: String,
    pub entity_id: String,
    pub op_type: String,
    pub field: Option<String>,
    pub created_at: String,
    pub synced: bool,
    pub target: RecentActionTarget,
    pub verb: String,
    pub summary: String,
    pub detail: Option<String>,
    pub accent: String,
    pub grouped_change_count: usize,
}

#[derive(Debug, Clone)]
pub struct RecentActionTarget {
    pub display_ref: Option<String>,
    pub title: Option<String>,
    pub project_key: Option<String>,
    pub status: Option<String>,
    pub deleted: bool,
}

#[derive(Debug, Clone)]
pub struct ProjectListItem {
    pub key: String,
    pub name: String,
    pub prefix: String,
    pub open_count: i64,
    pub inbox_count: i64,
}

#[derive(Debug, Clone, Default)]
pub struct SidebarCounts {
    pub open: i64,
    pub inbox: i64,
    pub active: i64,
    pub backlog: i64,
    pub todo: i64,
    pub conflicts: i64,
    pub done: i64,
    pub epics: i64,
    pub recurring: i64,
    pub upcoming: i64,
}
