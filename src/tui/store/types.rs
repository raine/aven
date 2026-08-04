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

#[derive(Debug)]
pub(crate) struct MutationCommittedError {
    source: anyhow::Error,
}

impl std::fmt::Display for MutationCommittedError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "mutation committed but TUI refresh failed: {}",
            self.source
        )
    }
}

impl std::error::Error for MutationCommittedError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

pub(crate) fn mutation_committed(error: &anyhow::Error) -> bool {
    error.downcast_ref::<MutationCommittedError>().is_some()
}

pub(super) fn committed_mutation_error(source: anyhow::Error) -> anyhow::Error {
    MutationCommittedError { source }.into()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConflictTarget {
    pub(crate) task_id: crate::ids::TaskId,
    pub(crate) recurrence_series_id: Option<aven_core::recurrence::RecurrenceSeriesId>,
    pub(crate) display_ref: String,
    pub(crate) field: String,
    pub(crate) variant_a: String,
    pub(crate) local_value: String,
    pub(crate) variant_b: String,
    pub(crate) remote_value: String,
}

use std::collections::BTreeSet;

use crate::query::{
    RecurrenceSeriesLifecycleFilter, RecurrenceSeriesListQuery, SortDirection, SyncHistoryStats,
    TaskAvailabilityFilter, TaskFilters, TaskIdFilter, TaskQueryMode, TaskSort,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TaskScope {
    Workspace,
    Project(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskView {
    Queue,
    Columns,
    Open,
    Inbox,
    Active,
    Backlog,
    Todo,
    Done,
    Upcoming,
    Conflicts,
    Search,
    Epics,
    Recurring,
    RecentActions,
}

impl TaskView {
    pub(crate) fn supports_closed_filter(self) -> bool {
        matches!(self, Self::Queue | Self::Open | Self::Epics)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum ClosedTaskVisibility {
    #[default]
    Default,
    Included,
    Only,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RecurringSeriesViewState {
    pub(crate) lifecycle: RecurrenceSeriesLifecycleFilter,
    pub(crate) search: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MainRowSelection {
    Task(crate::ids::TaskId),
    RecurrenceSeries(aven_core::recurrence::RecurrenceSeriesId),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum TaskProjectionOrigin {
    #[default]
    NamedView,
    SearchPrompt,
    Search {
        query: String,
        task_ids: Vec<crate::ids::TaskId>,
    },
    ExactTasks(Vec<crate::ids::TaskId>),
}

impl TaskProjectionOrigin {
    pub(crate) fn task_ids(&self) -> Option<&[crate::ids::TaskId]> {
        match self {
            Self::NamedView => None,
            Self::SearchPrompt => Some(&[]),
            Self::Search { task_ids, .. } | Self::ExactTasks(task_ids) => Some(task_ids),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TaskFilterModifiers {
    pub(crate) label: Option<String>,
    pub(crate) priority: Option<String>,
    pub(crate) closed: ClosedTaskVisibility,
    pub(crate) include_deleted: bool,
    pub(crate) deleted_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskOrder {
    Created,
    Updated,
    Priority,
    Project,
    Title,
    DueOn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskListRenderMode {
    Flat,
    Queue,
    Columns,
    Upcoming,
    Epics,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskViewState {
    pub(crate) scope: TaskScope,
    pub(crate) view: TaskView,
    pub(crate) projection_origin: TaskProjectionOrigin,
    pub(crate) filter_modifiers: TaskFilterModifiers,
    pub(crate) order: TaskOrder,
    pub(crate) direction: SortDirection,
    pub(crate) recurring: RecurringSeriesViewState,
    pub(crate) expanded_epic_ids: BTreeSet<crate::ids::TaskId>,
    pub(crate) collapsed_epic_ids: BTreeSet<crate::ids::TaskId>,
}

impl Default for TaskViewState {
    fn default() -> Self {
        Self {
            scope: TaskScope::Workspace,
            view: TaskView::Queue,
            projection_origin: TaskProjectionOrigin::default(),
            filter_modifiers: TaskFilterModifiers::default(),
            order: TaskOrder::Created,
            direction: SortDirection::Asc,
            recurring: RecurringSeriesViewState::default(),
            expanded_epic_ids: BTreeSet::new(),
            collapsed_epic_ids: BTreeSet::new(),
        }
    }
}

impl TaskViewState {
    pub(crate) fn for_exact_task(task_id: crate::ids::TaskId) -> Self {
        Self {
            view: TaskView::Search,
            projection_origin: TaskProjectionOrigin::ExactTasks(vec![task_id]),
            ..Self::default()
        }
    }

    pub(crate) fn filters(&self) -> TaskFilters {
        let mut filters = TaskFilters {
            label: self.filter_modifiers.label.clone(),
            priority: self.filter_modifiers.priority.clone(),
            include_deleted: self.filter_modifiers.include_deleted,
            deleted_only: self.filter_modifiers.deleted_only,
            task_ids: self
                .projection_origin
                .task_ids()
                .map(|task_ids| TaskIdFilter::Only(task_ids.to_vec()))
                .unwrap_or_default(),
            ..TaskFilters::default()
        };
        if let TaskScope::Project(project) = &self.scope {
            filters.project = Some(project.clone());
        }
        match self.view {
            TaskView::Queue => {
                filters.hide_done = true;
                filters.availability = TaskAvailabilityFilter::Available;
            }
            TaskView::Columns => filters.availability = TaskAvailabilityFilter::Available,
            TaskView::Open => {
                filters.hide_done = true;
                filters.availability = TaskAvailabilityFilter::Available;
            }
            TaskView::Inbox => {
                filters.status = Some("inbox".to_string());
                filters.availability = TaskAvailabilityFilter::Available;
            }
            TaskView::Active => {
                filters.status = Some("active".to_string());
                filters.availability = TaskAvailabilityFilter::Available;
            }
            TaskView::Backlog => {
                filters.status = Some("backlog".to_string());
                filters.availability = TaskAvailabilityFilter::Available;
            }
            TaskView::Todo => {
                filters.status = Some("todo".to_string());
                filters.availability = TaskAvailabilityFilter::Available;
            }
            TaskView::Done => filters.statuses = vec!["done".to_string(), "canceled".to_string()],
            TaskView::Upcoming => filters.availability = TaskAvailabilityFilter::Upcoming,
            TaskView::Conflicts => filters.conflicts_only = true,
            TaskView::Epics => {
                filters.epics_only = true;
                filters.hide_done = true;
                filters.availability = TaskAvailabilityFilter::Available;
            }
            TaskView::Search => {
                filters.include_deleted = true;
            }
            TaskView::Recurring | TaskView::RecentActions => {}
        }
        if self.view.supports_closed_filter() {
            match self.filter_modifiers.closed {
                ClosedTaskVisibility::Default => {}
                ClosedTaskVisibility::Included => filters.hide_done = false,
                ClosedTaskVisibility::Only => {
                    filters.hide_done = false;
                    filters.statuses = vec!["done".to_string(), "canceled".to_string()];
                }
            }
        }
        filters
    }

    pub(crate) fn recurrence_query(&self) -> RecurrenceSeriesListQuery {
        RecurrenceSeriesListQuery {
            lifecycle: self.recurring.lifecycle,
            project: match &self.scope {
                TaskScope::Workspace => None,
                TaskScope::Project(project) => Some(project.clone()),
            },
            search: self.recurring.search.clone(),
        }
    }

    pub(crate) fn query_mode(&self) -> TaskQueryMode {
        match self.view {
            TaskView::Queue => TaskQueryMode::RankedQueue,
            TaskView::Columns | TaskView::Recurring | TaskView::RecentActions => {
                TaskQueryMode::Flat
            }
            _ => TaskQueryMode::Flat,
        }
    }

    pub(crate) fn sort(&self) -> TaskSort {
        if self.view == TaskView::Upcoming {
            TaskSort::AvailableAt
        } else {
            self.order.into()
        }
    }

    pub(crate) fn sort_direction(&self) -> SortDirection {
        if self.view == TaskView::Upcoming {
            SortDirection::Asc
        } else {
            self.direction
        }
    }

    pub(crate) fn render_mode(&self) -> TaskListRenderMode {
        match self.view {
            TaskView::Queue => TaskListRenderMode::Queue,
            TaskView::Columns => TaskListRenderMode::Columns,
            TaskView::Upcoming => TaskListRenderMode::Upcoming,
            TaskView::Epics => TaskListRenderMode::Epics,
            TaskView::Recurring | TaskView::RecentActions => TaskListRenderMode::Flat,
            _ => TaskListRenderMode::Flat,
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
            TaskOrder::DueOn => Self::DueOn,
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
    pub(crate) sync_history: SyncHistoryStats,
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
