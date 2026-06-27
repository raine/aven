#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MutationMessage {
    pub(crate) message: String,
    pub(crate) selected: Option<usize>,
}

impl MutationMessage {
    pub(crate) fn new(message: impl Into<String>, selected: Option<usize>) -> Self {
        Self {
            message: message.into(),
            selected,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConflictTarget {
    pub(crate) task_id: String,
    pub(crate) display_ref: String,
    pub(crate) field: String,
    pub(crate) variant_a: String,
    pub(crate) local_value: String,
    pub(crate) variant_b: String,
    pub(crate) remote_value: String,
}

use crate::query::{SortDirection, TaskFilters, TaskQueryMode, TaskSort};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TaskScope {
    Workspace,
    Project(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskView {
    Queue,
    Open,
    Inbox,
    Active,
    Backlog,
    Todo,
    Done,
    Conflicts,
    Search,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TaskFilterModifiers {
    pub(crate) label: Option<String>,
    pub(crate) priority: Option<String>,
    pub(crate) include_deleted: bool,
    pub(crate) deleted_only: bool,
    pub(crate) search: Option<String>,
    pub(crate) task_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskOrder {
    Created,
    Updated,
    Priority,
    Project,
    Title,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskListRenderMode {
    Flat,
    Queue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskViewState {
    pub(crate) scope: TaskScope,
    pub(crate) view: TaskView,
    pub(crate) filter_modifiers: TaskFilterModifiers,
    pub(crate) order: TaskOrder,
    pub(crate) direction: SortDirection,
}

impl Default for TaskViewState {
    fn default() -> Self {
        Self {
            scope: TaskScope::Workspace,
            view: TaskView::Queue,
            filter_modifiers: TaskFilterModifiers::default(),
            order: TaskOrder::Created,
            direction: SortDirection::Asc,
        }
    }
}

impl TaskViewState {
    pub(crate) fn filters(&self) -> TaskFilters {
        let mut filters = TaskFilters {
            label: self.filter_modifiers.label.clone(),
            priority: self.filter_modifiers.priority.clone(),
            include_deleted: self.filter_modifiers.include_deleted,
            deleted_only: self.filter_modifiers.deleted_only,
            search: self.filter_modifiers.search.clone(),
            task_ids: self.filter_modifiers.task_ids.clone(),
            ..TaskFilters::default()
        };
        if let TaskScope::Project(project) = &self.scope {
            filters.project = Some(project.clone());
        }
        match self.view {
            TaskView::Queue | TaskView::Open => filters.hide_done = true,
            TaskView::Inbox => filters.status = Some("inbox".to_string()),
            TaskView::Active => filters.status = Some("active".to_string()),
            TaskView::Backlog => filters.status = Some("backlog".to_string()),
            TaskView::Todo => filters.status = Some("todo".to_string()),
            TaskView::Done => filters.statuses = vec!["done".to_string(), "canceled".to_string()],
            TaskView::Conflicts => filters.conflicts_only = true,
            TaskView::Search => {
                filters.include_deleted = true;
            }
        }
        filters
    }

    pub(crate) fn query_mode(&self) -> TaskQueryMode {
        match self.view {
            TaskView::Queue => TaskQueryMode::RankedQueue,
            _ => TaskQueryMode::Flat,
        }
    }

    pub(crate) fn sort(&self) -> TaskSort {
        self.order.into()
    }

    pub(crate) fn render_mode(&self) -> TaskListRenderMode {
        match self.query_mode() {
            TaskQueryMode::RankedQueue => TaskListRenderMode::Queue,
            TaskQueryMode::Flat => TaskListRenderMode::Flat,
        }
    }
}

impl From<TaskOrder> for TaskSort {
    fn from(order: TaskOrder) -> Self {
        match order {
            TaskOrder::Created => Self::Created,
            TaskOrder::Updated => Self::Updated,
            TaskOrder::Priority => Self::Priority,
            TaskOrder::Project => Self::Project,
            TaskOrder::Title => Self::Title,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TaskScopeTarget {
    Workspace,
    Project(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SidebarEntryTarget {
    View(TaskView),
    Scope(TaskScopeTarget),
}

#[derive(Debug, Clone)]
pub(crate) struct SidebarEntry {
    pub(crate) label: String,
    pub(crate) count: i64,
    pub(crate) target: Option<SidebarEntryTarget>,
    pub(crate) section: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SyncStatusCheck {
    pub(crate) ok: bool,
    pub(crate) value: String,
}

impl SyncStatusCheck {
    pub(crate) fn new(ok: bool, value: impl Into<String>) -> Self {
        Self {
            ok,
            value: value.into(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DatabaseStatsStatusCounts {
    pub(crate) inbox: i64,
    pub(crate) backlog: i64,
    pub(crate) todo: i64,
    pub(crate) active: i64,
    pub(crate) done: i64,
    pub(crate) canceled: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DatabaseStatsPriorityCounts {
    pub(crate) none: i64,
    pub(crate) low: i64,
    pub(crate) medium: i64,
    pub(crate) high: i64,
    pub(crate) urgent: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TuiDatabaseStats {
    pub(crate) workspace_name: String,
    pub(crate) workspace_key: String,
    pub(crate) total_tasks: i64,
    pub(crate) open_tasks: i64,
    pub(crate) deleted_tasks: i64,
    pub(crate) statuses: DatabaseStatsStatusCounts,
    pub(crate) priorities: DatabaseStatsPriorityCounts,
    pub(crate) projects: i64,
    pub(crate) labels: i64,
    pub(crate) notes: i64,
    pub(crate) task_labels: i64,
    pub(crate) pending_changes: i64,
    pub(crate) conflicts: i64,
    pub(crate) sqlite_page_size: i64,
    pub(crate) sqlite_page_count: i64,
    pub(crate) sqlite_freelist_count: i64,
    pub(crate) latest_created_at: Option<String>,
    pub(crate) latest_updated_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TuiSyncStatus {
    pub(crate) enabled: bool,
    pub(crate) config_error: Option<String>,
    pub(crate) configured_server: Option<SyncStatusCheck>,
    pub(crate) pinned_server: Option<String>,
    pub(crate) server_match: Option<SyncStatusCheck>,
    pub(crate) daemon_server: Option<SyncStatusCheck>,
    pub(crate) auth_token_configured: bool,
    pub(crate) interval_seconds: u64,
    pub(crate) daemon_wake: SyncStatusCheck,
    pub(crate) pending_changes: i64,
    pub(crate) conflicts: i64,
    pub(crate) sync_cursor: Option<String>,
    pub(crate) local_sequence: Option<String>,
    pub(crate) last_attempt: Option<String>,
    pub(crate) last_success: Option<String>,
    pub(crate) last_error: Option<String>,
    pub(crate) last_pushed: Option<String>,
    pub(crate) last_pulled: Option<String>,
    pub(crate) last_cursor: Option<String>,
}

impl Default for TuiSyncStatus {
    fn default() -> Self {
        Self {
            enabled: false,
            config_error: None,
            configured_server: None,
            pinned_server: None,
            server_match: None,
            daemon_server: None,
            auth_token_configured: false,
            interval_seconds: 30,
            daemon_wake: SyncStatusCheck::new(true, "not checked"),
            pending_changes: 0,
            conflicts: 0,
            sync_cursor: None,
            local_sequence: None,
            last_attempt: None,
            last_success: None,
            last_error: None,
            last_pushed: None,
            last_pulled: None,
            last_cursor: None,
        }
    }
}

impl TuiSyncStatus {
    pub(crate) fn has_sync_error(&self) -> bool {
        self.config_error.is_some()
            || self.last_error_value().is_some()
            || (self.enabled
                && (!self
                    .configured_server
                    .as_ref()
                    .is_some_and(|check| check.ok)
                    || self.server_match.as_ref().is_some_and(|check| !check.ok)
                    || self.daemon_server.as_ref().is_some_and(|check| !check.ok)
                    || !self.daemon_wake.ok))
    }

    pub(crate) fn last_error_value(&self) -> Option<&str> {
        self.last_error.as_deref().filter(|error| !error.is_empty())
    }
}
