use crate::queue::QueueMeta;
use crate::types::Task;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskQueryMode {
    Flat,
    RankedQueue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskSort {
    Created,
    Updated,
    Priority,
    Project,
    Title,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SortDirection {
    Asc,
    Desc,
}

impl SortDirection {
    pub(crate) fn toggled(self) -> Self {
        match self {
            Self::Asc => Self::Desc,
            Self::Desc => Self::Asc,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct TaskFilters {
    pub(crate) project: Option<String>,
    pub(crate) status: Option<String>,
    pub(crate) statuses: Vec<String>,
    pub(crate) priority: Option<String>,
    pub(crate) label: Option<String>,
    pub(crate) include_deleted: bool,
    pub(crate) deleted_only: bool,
    pub(crate) hide_done: bool,
    pub(crate) conflicts_only: bool,
    pub(crate) ready_only: bool,
    pub(crate) blocked_only: bool,
    pub(crate) epics_only: bool,
    pub(crate) exclude_epics: bool,
    pub(crate) search: Option<String>,
    pub(crate) task_ids: Vec<String>,
}

impl TaskFilters {
    pub(crate) fn with_project(mut self, project: Option<String>) -> Self {
        self.project = project;
        self
    }

    pub(crate) fn with_status(mut self, status: Option<String>) -> Self {
        self.status = status;
        self
    }

    pub(crate) fn with_priority(mut self, priority: Option<String>) -> Self {
        self.priority = priority;
        self
    }

    pub(crate) fn include_deleted(mut self, include_deleted: bool) -> Self {
        self.include_deleted = include_deleted;
        self
    }

    pub(crate) fn deleted_only(mut self, deleted_only: bool) -> Self {
        self.deleted_only = deleted_only;
        self
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TaskListItem {
    pub(crate) task: Task,
    pub(crate) display_ref: String,
    pub(crate) labels: Vec<String>,
    pub(crate) notes: Vec<TaskNote>,
    pub(crate) has_conflict: bool,
    pub(crate) unresolved_blocker_count: i64,
    pub(crate) dependent_count: i64,
    pub(crate) depends_on: Vec<TaskDependencyLink>,
    pub(crate) blocks: Vec<TaskDependencyLink>,
    pub(crate) epic_children: Vec<TaskDependencyLink>,
    pub(crate) epic_parent: Option<TaskDependencyLink>,
    pub(crate) queue: QueueMeta,
}

#[derive(Debug, Clone)]
pub(crate) struct TaskNote {
    pub(crate) body: String,
    pub(crate) created_at: String,
}

#[derive(Debug, Clone)]
pub(crate) struct TaskDependencyLink {
    pub(crate) task_id: String,
    pub(crate) display_ref: String,
    pub(crate) title: String,
    pub(crate) status: String,
    pub(crate) priority: String,
    pub(crate) unresolved: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct ProjectListItem {
    pub(crate) key: String,
    pub(crate) name: String,
    pub(crate) prefix: String,
    pub(crate) open_count: i64,
    pub(crate) inbox_count: i64,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SidebarCounts {
    pub(crate) open: i64,
    pub(crate) inbox: i64,
    pub(crate) active: i64,
    pub(crate) backlog: i64,
    pub(crate) todo: i64,
    pub(crate) conflicts: i64,
    pub(crate) done: i64,
    pub(crate) epics: i64,
}
