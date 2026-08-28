#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UndoPresentation {
    pub(crate) entry_id: String,
    pub(crate) phrase: String,
}

impl UndoPresentation {
    pub(crate) fn undo_label(&self) -> String {
        format!("undo {}", self.phrase)
    }
}

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
pub(crate) enum TaskQuery {
    Queue,
    All,
    Open,
    Inbox,
    Active,
    Backlog,
    Todo,
    Done,
    Ready,
    Blocked,
    Overdue,
    Upcoming,
    Conflicts,
    Search,
    Epics,
    Recurring,
    RecentActions,
}

impl TaskQuery {
    pub(crate) fn supports_closed_filter(self) -> bool {
        matches!(self, Self::Queue | Self::Open | Self::Epics)
    }

    pub(crate) fn supports_layout(self, layout: TaskLayout) -> bool {
        layout == TaskLayout::List
            || matches!(
                self,
                Self::All
                    | Self::Open
                    | Self::Inbox
                    | Self::Active
                    | Self::Backlog
                    | Self::Todo
                    | Self::Done
                    | Self::Ready
                    | Self::Blocked
                    | Self::Overdue
                    | Self::Conflicts
                    | Self::Search
            )
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum TaskLayout {
    #[default]
    List,
    Columns,
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
pub(crate) enum MainRowIdentity {
    Task(crate::ids::TaskId),
    RecurrenceSeries(aven_core::recurrence::RecurrenceSeriesId),
    RecentAction(String),
}

pub(crate) type MainRowSelection = MainRowIdentity;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MainRowPosition {
    Flat(usize),
    Column { column: usize, row: usize },
    EpicVisualRow(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MainRowAnchor {
    pub(crate) identity: MainRowIdentity,
    pub(crate) position: MainRowPosition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SelectionRestore {
    Default,
    Identity(MainRowIdentity),
    Anchor(MainRowAnchor),
    Index(usize),
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
    pub(crate) fn task_id_filter(&self) -> TaskIdFilter {
        match self {
            Self::NamedView => TaskIdFilter::Unrestricted,
            Self::SearchPrompt => TaskIdFilter::Only(Vec::new()),
            Self::Search { task_ids, .. } | Self::ExactTasks(task_ids) => {
                TaskIdFilter::Only(task_ids.clone())
            }
        }
    }

    pub(crate) fn match_count(&self) -> Option<usize> {
        match self {
            Self::NamedView | Self::SearchPrompt => None,
            Self::Search { task_ids, .. } | Self::ExactTasks(task_ids) => Some(task_ids.len()),
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
    Upcoming,
    Epics,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskViewState {
    pub(crate) scope: TaskScope,
    pub(crate) query: TaskQuery,
    pub(crate) layout: TaskLayout,
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
            query: TaskQuery::Queue,
            layout: TaskLayout::List,
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
    pub(crate) fn is_columns(&self) -> bool {
        self.layout == TaskLayout::Columns
    }

    pub(crate) fn set_layout(&mut self, layout: TaskLayout) -> Result<(), &'static str> {
        if !self.query.supports_layout(layout) {
            return Err("active query does not support columns layout");
        }
        self.layout = layout;
        Ok(())
    }

    pub(crate) fn set_query(&mut self, query: TaskQuery) {
        self.query = query;
        if !query.supports_layout(self.layout) {
            self.layout = TaskLayout::List;
        }
    }

    pub(crate) fn reset_projection_origin(&mut self) {
        self.projection_origin = if self.query == TaskQuery::Search {
            TaskProjectionOrigin::SearchPrompt
        } else {
            TaskProjectionOrigin::NamedView
        };
    }

    pub(crate) fn for_exact_task(task_id: crate::ids::TaskId) -> Self {
        Self {
            query: TaskQuery::Search,
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
            task_ids: self.projection_origin.task_id_filter(),
            ..TaskFilters::default()
        };
        if let TaskScope::Project(project) = &self.scope {
            filters.project = Some(project.clone());
        }
        match self.query {
            TaskQuery::Queue => {
                filters.hide_done = true;
                filters.availability = TaskAvailabilityFilter::Available;
            }
            TaskQuery::All => filters.availability = TaskAvailabilityFilter::Available,
            TaskQuery::Open => {
                filters.hide_done = true;
                filters.availability = TaskAvailabilityFilter::Available;
            }
            TaskQuery::Inbox => {
                filters.status = Some("inbox".to_string());
                filters.availability = TaskAvailabilityFilter::Available;
            }
            TaskQuery::Active => {
                filters.status = Some("active".to_string());
                filters.availability = TaskAvailabilityFilter::Available;
            }
            TaskQuery::Backlog => {
                filters.status = Some("backlog".to_string());
                filters.availability = TaskAvailabilityFilter::Available;
            }
            TaskQuery::Todo => {
                filters.status = Some("todo".to_string());
                filters.availability = TaskAvailabilityFilter::Available;
            }
            TaskQuery::Done => filters.statuses = vec!["done".to_string(), "canceled".to_string()],
            TaskQuery::Ready => {
                filters.ready_only = true;
                filters.exclude_epics = true;
                filters.availability = TaskAvailabilityFilter::Available;
            }
            TaskQuery::Blocked => {
                filters.blocked_only = true;
                filters.availability = TaskAvailabilityFilter::Available;
            }
            TaskQuery::Overdue => {
                filters.overdue_only = true;
                filters.availability = TaskAvailabilityFilter::Available;
            }
            TaskQuery::Upcoming => filters.availability = TaskAvailabilityFilter::Upcoming,
            TaskQuery::Conflicts => filters.conflicts_only = true,
            TaskQuery::Epics => {
                filters.epics_only = true;
                filters.hide_done = true;
                filters.availability = TaskAvailabilityFilter::Available;
            }
            TaskQuery::Search => {
                filters.include_deleted = true;
            }
            TaskQuery::Recurring | TaskQuery::RecentActions => {}
        }
        if self.query.supports_closed_filter() {
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
        match self.query {
            TaskQuery::Queue => TaskQueryMode::RankedQueue,
            TaskQuery::Recurring | TaskQuery::RecentActions => TaskQueryMode::Flat,
            _ => TaskQueryMode::Flat,
        }
    }

    pub(crate) fn sort(&self) -> TaskSort {
        match self.query {
            TaskQuery::Upcoming => TaskSort::AvailableAt,
            TaskQuery::Overdue => TaskSort::DueOn,
            _ => self.order.into(),
        }
    }

    pub(crate) fn sort_direction(&self) -> SortDirection {
        if matches!(self.query, TaskQuery::Upcoming | TaskQuery::Overdue) {
            SortDirection::Asc
        } else {
            self.direction
        }
    }

    pub(crate) fn render_mode(&self) -> TaskListRenderMode {
        match self.query {
            TaskQuery::Queue => TaskListRenderMode::Queue,
            TaskQuery::Upcoming => TaskListRenderMode::Upcoming,
            TaskQuery::Epics => TaskListRenderMode::Epics,
            TaskQuery::Recurring | TaskQuery::RecentActions => TaskListRenderMode::Flat,
            _ => TaskListRenderMode::Flat,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_compatibility_is_explicit() {
        for query in [
            TaskQuery::All,
            TaskQuery::Open,
            TaskQuery::Inbox,
            TaskQuery::Active,
            TaskQuery::Backlog,
            TaskQuery::Todo,
            TaskQuery::Done,
            TaskQuery::Ready,
            TaskQuery::Blocked,
            TaskQuery::Overdue,
            TaskQuery::Conflicts,
            TaskQuery::Search,
        ] {
            assert!(query.supports_layout(TaskLayout::Columns), "{query:?}");
        }
        for query in [
            TaskQuery::Queue,
            TaskQuery::Upcoming,
            TaskQuery::Epics,
            TaskQuery::Recurring,
            TaskQuery::RecentActions,
        ] {
            assert!(!query.supports_layout(TaskLayout::Columns), "{query:?}");
            assert!(query.supports_layout(TaskLayout::List), "{query:?}");
        }
    }

    #[test]
    fn layout_does_not_change_query_semantics() {
        let mut state = TaskViewState {
            query: TaskQuery::Todo,
            ..TaskViewState::default()
        };
        let list_filters = state.filters();
        let list_mode = state.query_mode();
        let list_sort = state.sort();

        state.set_layout(TaskLayout::Columns).unwrap();

        assert_eq!(
            format!("{:?}", state.filters()),
            format!("{list_filters:?}")
        );
        assert_eq!(state.query_mode(), list_mode);
        assert_eq!(state.sort(), list_sort);
    }

    #[test]
    fn incompatible_query_coerces_columns_to_list() {
        let mut state = TaskViewState {
            query: TaskQuery::All,
            layout: TaskLayout::Columns,
            ..TaskViewState::default()
        };

        state.set_query(TaskQuery::Queue);

        assert_eq!(state.query, TaskQuery::Queue);
        assert_eq!(state.layout, TaskLayout::List);
    }

    #[test]
    fn all_query_includes_every_available_status() {
        let state = TaskViewState {
            query: TaskQuery::All,
            ..TaskViewState::default()
        };
        let filters = state.filters();

        assert_eq!(filters.availability, TaskAvailabilityFilter::Available);
        assert!(!filters.hide_done);
        assert!(filters.status.is_none());
        assert!(filters.statuses.is_empty());
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
    View(TaskQuery),
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
    pub(crate) runtime_allowed: bool,
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
            runtime_allowed: true,
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
    pub(crate) fn last_error_value(&self) -> Option<&str> {
        self.last_error.as_deref().filter(|error| !error.is_empty())
    }
}
