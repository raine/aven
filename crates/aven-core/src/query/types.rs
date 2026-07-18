use crate::attachments::AttachmentBytesState;
use crate::ids::TaskId;
use crate::queue::QueueMeta;
use crate::types::Task;
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
    pub task_ids: Vec<TaskId>,
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

#[derive(Debug, Clone)]
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
}

#[derive(Serialize, Debug, Clone)]
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

#[derive(Debug, Clone)]
pub struct TaskNote {
    pub body: String,
    pub created_at: String,
}

#[derive(Debug, Clone)]
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
    pub upcoming: i64,
}
