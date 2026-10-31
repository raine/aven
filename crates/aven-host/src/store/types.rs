#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoPresentation {
    pub entry_id: String,
    pub phrase: String,
}

impl UndoPresentation {
    pub fn undo_label(&self) -> String {
        format!("undo {}", self.phrase)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutationMessage {
    pub message: String,
    pub selected: Option<usize>,
}

impl MutationMessage {
    pub fn new(message: impl Into<String>, selected: Option<usize>) -> Self {
        Self {
            message: message.into(),
            selected,
        }
    }
}

#[derive(Debug)]
pub struct MutationCommittedError {
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

pub fn mutation_committed(error: &anyhow::Error) -> bool {
    error.downcast_ref::<MutationCommittedError>().is_some()
}

pub(super) fn committed_mutation_error(source: anyhow::Error) -> anyhow::Error {
    MutationCommittedError { source }.into()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictTarget {
    pub task_id: crate::ids::TaskId,
    pub recurrence_series_id: Option<aven_core::recurrence::RecurrenceSeriesId>,
    pub display_ref: String,
    pub field: String,
    pub variant_a: String,
    pub local_value: String,
    pub variant_b: String,
    pub remote_value: String,
}

use std::collections::BTreeSet;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::query::{
    RecurrenceSeriesLifecycleFilter, RecurrenceSeriesListQuery, SortDirection, SyncHistoryStats,
    TaskAvailabilityFilter, TaskFilters, TaskIdFilter, TaskListItem, TaskQueryMode, TaskSort,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DetailRevision(u64);

impl DetailRevision {
    pub const UNCACHED: Self = Self(0);

    pub fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Debug, Clone)]
pub struct TaskProjection {
    items: Vec<TaskListItem>,
    revision: DetailRevision,
}

impl Default for TaskProjection {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            revision: DetailRevision::UNCACHED,
        }
    }
}

impl From<Vec<TaskListItem>> for TaskProjection {
    fn from(items: Vec<TaskListItem>) -> Self {
        Self {
            items,
            revision: DetailRevision::next(),
        }
    }
}

impl Deref for TaskProjection {
    type Target = Vec<TaskListItem>;

    fn deref(&self) -> &Self::Target {
        &self.items
    }
}

impl DerefMut for TaskProjection {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.revision = DetailRevision::next();
        &mut self.items
    }
}

impl TaskProjection {
    pub fn revision(&self) -> DetailRevision {
        self.revision
    }
}

pub struct SearchPreview {
    pub results: crate::query::TaskSearchPreviewResultSet,
    pub other_project_matches: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskScope {
    Workspace,
    Project(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskQuery {
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
    pub fn supports_closed_filter(self) -> bool {
        matches!(self, Self::Queue | Self::Open | Self::Epics)
    }

    pub fn supports_layout(self, layout: TaskLayout) -> bool {
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
pub enum TaskLayout {
    #[default]
    List,
    Columns,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ClosedTaskVisibility {
    #[default]
    Default,
    Included,
    Only,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecurringSeriesViewState {
    pub lifecycle: RecurrenceSeriesLifecycleFilter,
    pub search: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MainRowIdentity {
    Task(crate::ids::TaskId),
    RecurrenceSeries(aven_core::recurrence::RecurrenceSeriesId),
    RecentAction(String),
}

pub type MainRowSelection = MainRowIdentity;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MainRowPosition {
    Flat(usize),
    Column { column: usize, row: usize },
    EpicVisualRow(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MainRowAnchor {
    pub identity: MainRowIdentity,
    pub position: MainRowPosition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectionRestore {
    Default,
    Identity(MainRowIdentity),
    Anchor(MainRowAnchor),
    Index(usize),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum TaskProjectionOrigin {
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
    pub fn task_id_filter(&self) -> TaskIdFilter {
        match self {
            Self::NamedView => TaskIdFilter::Unrestricted,
            Self::SearchPrompt => TaskIdFilter::Only(Vec::new()),
            Self::Search { task_ids, .. } | Self::ExactTasks(task_ids) => {
                TaskIdFilter::Only(task_ids.clone())
            }
        }
    }

    pub fn match_count(&self) -> Option<usize> {
        match self {
            Self::NamedView | Self::SearchPrompt => None,
            Self::Search { task_ids, .. } | Self::ExactTasks(task_ids) => Some(task_ids.len()),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskFilterModifiers {
    pub label: Option<String>,
    pub priority: Option<String>,
    pub closed: ClosedTaskVisibility,
    pub include_deleted: bool,
    pub deleted_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskOrder {
    Created,
    Updated,
    Priority,
    Project,
    Title,
    DueOn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskListRenderMode {
    Flat,
    Queue,
    Upcoming,
    Epics,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskViewState {
    pub scope: TaskScope,
    pub query: TaskQuery,
    pub layout: TaskLayout,
    pub projection_origin: TaskProjectionOrigin,
    pub filter_modifiers: TaskFilterModifiers,
    pub order: TaskOrder,
    pub direction: SortDirection,
    pub recurring: RecurringSeriesViewState,
    pub expanded_epic_ids: BTreeSet<crate::ids::TaskId>,
    pub collapsed_epic_ids: BTreeSet<crate::ids::TaskId>,
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
    pub fn is_columns(&self) -> bool {
        self.layout == TaskLayout::Columns
    }

    pub fn set_layout(&mut self, layout: TaskLayout) -> Result<(), &'static str> {
        if !self.query.supports_layout(layout) {
            return Err("active query does not support columns layout");
        }
        self.layout = layout;
        Ok(())
    }

    pub fn set_query(&mut self, query: TaskQuery) {
        self.query = query;
        if !query.supports_layout(self.layout) {
            self.layout = TaskLayout::List;
        }
    }

    pub fn reset_projection_origin(&mut self) {
        self.projection_origin = if self.query == TaskQuery::Search {
            TaskProjectionOrigin::SearchPrompt
        } else {
            TaskProjectionOrigin::NamedView
        };
    }

    pub fn for_exact_task(task_id: crate::ids::TaskId) -> Self {
        Self {
            query: TaskQuery::Search,
            projection_origin: TaskProjectionOrigin::ExactTasks(vec![task_id]),
            ..Self::default()
        }
    }

    pub fn filters(&self) -> TaskFilters {
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

    pub fn recurrence_query(&self) -> RecurrenceSeriesListQuery {
        RecurrenceSeriesListQuery {
            lifecycle: self.recurring.lifecycle,
            project: match &self.scope {
                TaskScope::Workspace => None,
                TaskScope::Project(project) => Some(project.clone()),
            },
            search: self.recurring.search.clone(),
        }
    }

    pub fn query_mode(&self) -> TaskQueryMode {
        match self.query {
            TaskQuery::Queue => TaskQueryMode::RankedQueue,
            TaskQuery::Recurring | TaskQuery::RecentActions => TaskQueryMode::Flat,
            _ => TaskQueryMode::Flat,
        }
    }

    pub fn sort(&self) -> TaskSort {
        match self.query {
            TaskQuery::Upcoming => TaskSort::AvailableAt,
            TaskQuery::Overdue => TaskSort::DueOn,
            _ => self.order.into(),
        }
    }

    pub fn sort_direction(&self) -> SortDirection {
        if matches!(self.query, TaskQuery::Upcoming | TaskQuery::Overdue) {
            SortDirection::Asc
        } else {
            self.direction
        }
    }

    pub fn render_mode(&self) -> TaskListRenderMode {
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
pub enum TaskScopeTarget {
    Workspace,
    Project(String),
}

pub use aven_core::local_state::SidebarSection;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidebarEntryTarget {
    Section(SidebarSection),
    View(TaskQuery),
    Scope(TaskScopeTarget),
    Workspace(String),
}

#[derive(Debug, Clone)]
pub struct SidebarEntry {
    pub label: String,
    pub count: i64,
    pub target: Option<SidebarEntryTarget>,
    pub section: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncStatusCheck {
    pub ok: bool,
    pub value: String,
}

impl SyncStatusCheck {
    pub fn new(ok: bool, value: impl Into<String>) -> Self {
        Self {
            ok,
            value: value.into(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DatabaseStatsStatusCounts {
    pub inbox: i64,
    pub backlog: i64,
    pub todo: i64,
    pub active: i64,
    pub done: i64,
    pub canceled: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DatabaseStatsPriorityCounts {
    pub none: i64,
    pub low: i64,
    pub medium: i64,
    pub high: i64,
    pub urgent: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TuiDatabaseStats {
    pub workspace_name: String,
    pub workspace_key: String,
    pub total_tasks: i64,
    pub open_tasks: i64,
    pub deleted_tasks: i64,
    pub statuses: DatabaseStatsStatusCounts,
    pub priorities: DatabaseStatsPriorityCounts,
    pub projects: i64,
    pub labels: i64,
    pub notes: i64,
    pub task_labels: i64,
    pub sync_history: SyncHistoryStats,
    pub conflicts: i64,
    pub sqlite_page_size: i64,
    pub sqlite_page_count: i64,
    pub sqlite_freelist_count: i64,
    pub latest_created_at: Option<String>,
    pub latest_updated_at: Option<String>,
}

/// Local sync state read from the database, configuration, and protected
/// membership; contacts no server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiSyncStatus {
    pub devices: Option<usize>,
    /// Automatic sync by the daemon is configured.
    pub enabled: bool,
    pub runtime_allowed: bool,
    /// The database was set up or joined, including an unfinished attempt.
    pub set_up: bool,
    pub phase: crate::sync::encrypted::LocalPhase,
    pub interval_seconds: u64,
    pub daemon_wake: SyncStatusCheck,
    pub daemon_sync: aven_core::local_state::DaemonSyncState,
    pub daemon_service: Option<crate::status::DaemonStatusReport>,
    pub protected_storage: SyncStatusCheck,
    pub pending_changes: i64,
    pub conflicts: i64,
    pub sync_cursor: Option<String>,
    pub local_sequence: Option<String>,
    /// The associated sync server, once setup or joining has recorded it.
    pub server: Option<String>,
    pub invitation: Option<crate::sync::encrypted::InvitationStatus>,
    /// Time of the latest server authentication refusal, cleared by a successful sync.
    pub access_refused_at: Option<String>,
    /// Time the server reported this sync's vault deleted, cleared by reset or
    /// a successful `Sync now`.
    pub vault_deleted_at: Option<String>,
}

impl Default for TuiSyncStatus {
    fn default() -> Self {
        Self {
            devices: None,
            enabled: false,
            runtime_allowed: true,
            set_up: false,
            phase: crate::sync::encrypted::LocalPhase::NotSetUp,
            interval_seconds: 30,
            daemon_wake: SyncStatusCheck::new(true, "not checked"),
            daemon_sync: Default::default(),
            daemon_service: None,
            protected_storage: SyncStatusCheck::new(true, "available"),
            pending_changes: 0,
            conflicts: 0,
            sync_cursor: None,
            local_sequence: None,
            server: None,
            invitation: None,
            access_refused_at: None,
            vault_deleted_at: None,
        }
    }
}
