use crate::config::AppConfig;
mod attachments;
mod config;
mod conflicts;
mod domain;
mod epics;
mod launch;
mod onboarding;
mod pickers;
mod recurrence;
mod sidebar;
mod sort;
mod stats;
mod task_commands;
mod task_creation;
mod types;
mod undo;
mod view;
mod workspaces;

#[cfg(test)]
mod tests;

use std::cell::OnceCell;
use std::ops::Deref;
#[cfg(test)]
use std::ops::DerefMut;
use std::time::Instant;

use anyhow::Result;
use aven_core::db::Database;

pub(crate) use crate::query::RecentActionItem;
pub(crate) use attachments::AttachmentWorkerContext;
pub(crate) use epics::{AddEpicChildContext, EpicChildTarget, EpicContext};
pub(crate) use launch::{TuiLaunch, TuiStartup};
pub(crate) use onboarding::OnboardingStatus;
pub(crate) use pickers::{
    ADD_TASK_STATUS_AUTO_VALUE, CREATE_PROJECT_PICKER_VALUE_PREFIX,
    blocker_navigation_picker_items, create_project_picker_name, deleted_picker_items,
    epic_picker_items, related_picker_items,
};
pub(crate) use recurrence::recurrence_draft;
pub(crate) use task_commands::{PriorityMutation, TaskDateField, TaskTextField};
pub(crate) use task_creation::task_creation_committed;
#[cfg(test)]
pub(crate) use types::SyncStatusCheck;
pub(crate) use types::{
    ClosedTaskVisibility, ConflictTarget, DetailRevision, MainRowAnchor, MainRowIdentity,
    MainRowPosition, MainRowSelection, MutationMessage, RecurringSeriesViewState, SearchPreview,
    SelectionRestore, SidebarEntry, SidebarEntryTarget, SidebarSection, TaskFilterModifiers,
    TaskLayout, TaskListRenderMode, TaskOrder, TaskProjection, TaskProjectionOrigin, TaskQuery,
    TaskScope, TaskScopeTarget, TaskViewState, TuiDatabaseStats, TuiSyncStatus, UndoPresentation,
    mutation_committed,
};
#[cfg(test)]
pub(crate) use types::{DatabaseStatsPriorityCounts, DatabaseStatsStatusCounts};

use crate::query::{
    ProjectListItem, RecurrenceSeriesDetail, RecurrenceSeriesListItem, SidebarCounts,
    TaskItemHydration, TaskListItem,
};
use crate::tui::columns::ColumnBoard;
use crate::tui::ui::TaskListView;
use crate::workspaces::Workspace;

// The summary row owns task identity, visibility, ordering, conflict, and recurrence fields.
// Detail hydration fills only collections omitted from the summary projection.
fn absorb_task_detail(summary: &mut TaskListItem, detail: TaskListItem) {
    debug_assert_eq!(summary.task.id, detail.task.id);
    summary.notes = detail.notes;
    summary.has_notes = detail.has_notes;
    summary.attachments = detail.attachments;
    summary.metadata = detail.metadata;
    summary.activity = detail.activity;
    summary.related = detail.related;
    summary.epic_child_dependencies = detail.epic_child_dependencies;
    summary.hydration = TaskItemHydration::Detail;
}

pub(crate) struct TuiStore {
    database: Database,
    app_config: AppConfig,
    projection: TuiProjection,
    task_columns: Vec<crate::config::TaskColumnConfig>,
    derived: DerivedTaskProjections,
    pub(crate) columns_preview_visible: bool,
    pub(crate) db_stats: TuiDatabaseStats,
    /// A manual sync or a Sync dialog operation is running.
    pub(crate) sync_busy: bool,
    refresh_health: RefreshHealth,
    /// Name of an active workspace that a refresh found deleted and replaced
    /// with the default workspace, until the app reports it.
    deleted_active_workspace: Option<String>,
    #[cfg(test)]
    fail_next_refresh: Option<RefreshFailureStage>,
    #[cfg(test)]
    pub(crate) _test_database_dir: Option<std::sync::Arc<tempfile::TempDir>>,
}

#[derive(Default)]
struct DerivedTaskProjections {
    task_list: OnceCell<TaskListView>,
    columns: OnceCell<ColumnBoard>,
}

#[derive(Debug)]
pub(crate) enum TaskListViewRef<'a> {
    Stable(&'a TaskListView),
    Owned(TaskListView),
}

impl Deref for TaskListViewRef<'_> {
    type Target = TaskListView;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Stable(view) => view,
            Self::Owned(view) => view,
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct TaskDetailHydration {
    ready: std::collections::BTreeSet<crate::ids::TaskId>,
    missing: std::collections::BTreeSet<crate::ids::TaskId>,
    unresolved: std::collections::BTreeSet<crate::ids::TaskId>,
}

impl TaskDetailHydration {
    pub(crate) fn is_ready(&self, task_id: &crate::ids::TaskId) -> bool {
        self.ready.contains(task_id)
    }

    pub(crate) fn is_complete(&self) -> bool {
        self.missing.is_empty() && self.unresolved.is_empty()
    }

    pub(crate) fn stale_ids(&self) -> impl Iterator<Item = &crate::ids::TaskId> {
        self.missing.iter().chain(&self.unresolved)
    }
}

#[cfg_attr(test, derive(Clone))]
pub(crate) struct TuiProjection {
    pub(crate) tasks: TaskProjection,
    pub(crate) recurrence_series: Vec<RecurrenceSeriesListItem>,
    pub(crate) recurrence_detail: Option<RecurrenceSeriesDetail>,
    pub(crate) recent_actions: Vec<RecentActionItem>,
    pub(crate) projects: Vec<ProjectListItem>,
    pub(crate) labels: Vec<String>,
    pub(crate) workspaces: Vec<Workspace>,
    pub(crate) workspace_open_counts: std::collections::HashMap<crate::ids::WorkspaceId, i64>,
    pub(crate) active_workspace: Workspace,
    pub(crate) counts: SidebarCounts,
    pub(crate) sidebar_entries: Vec<SidebarEntry>,
    pub(crate) view_state: TaskViewState,
    pub(crate) sync_status: TuiSyncStatus,
    pub(crate) latest_undo: Option<UndoPresentation>,
    pub(crate) new_undo_entry_id: Option<String>,
    pub(crate) last_refresh: Instant,
    #[cfg(test)]
    clone_sentinel: ProjectionCloneSentinel,
}

#[cfg(test)]
#[derive(Default)]
struct ProjectionCloneSentinel {
    clone_count: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(test)]
impl Clone for ProjectionCloneSentinel {
    fn clone(&self) -> Self {
        self.clone_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self {
            clone_count: self.clone_count.clone(),
        }
    }
}

impl TuiStore {
    pub(crate) fn database_path(&self) -> &std::path::Path {
        self.database.path()
    }

    pub(crate) fn database_file_identity(&self) -> Option<&std::path::Path> {
        self.database.file_identity()
    }
}

impl Deref for TuiStore {
    type Target = TuiProjection;

    fn deref(&self) -> &Self::Target {
        &self.projection
    }
}

// Test fixtures seed projection fields directly. Production code mutates the
// projection through named transitions that state their index invalidation.
#[cfg(test)]
impl DerefMut for TuiStore {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.derived = DerivedTaskProjections::default();
        &mut self.projection
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum RefreshHealth {
    #[default]
    Healthy,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RefreshFailureStage {
    Projects,
    Tasks,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScopeRefreshResult {
    pub(crate) selected: Option<usize>,
    pub(crate) fallback_scope: Option<String>,
}

impl TuiStore {
    #[cfg(test)]
    pub(crate) async fn new(database: Database, workspace: Workspace) -> Result<Self> {
        Self::new_with_view_state(database, workspace, TaskViewState::default()).await
    }

    #[cfg(test)]
    pub(crate) async fn new_with_view_state(
        database: Database,
        workspace: Workspace,
        view_state: TaskViewState,
    ) -> Result<Self> {
        Self::new_with_view_state_and_config(database, workspace, view_state, AppConfig::default())
            .await
    }

    pub(crate) async fn new_with_view_state_and_config(
        database: Database,
        workspace: Workspace,
        view_state: TaskViewState,
        app_config: AppConfig,
    ) -> Result<Self> {
        let task_columns = app_config.tui.columns.clone();
        let mut store = Self {
            database,
            app_config,
            projection: TuiProjection::new(workspace, view_state),
            task_columns,
            derived: DerivedTaskProjections::default(),
            columns_preview_visible: true,
            db_stats: TuiDatabaseStats::default(),
            sync_busy: false,
            refresh_health: RefreshHealth::Healthy,
            deleted_active_workspace: None,
            #[cfg(test)]
            fail_next_refresh: None,
            #[cfg(test)]
            _test_database_dir: None,
        };
        store.database.clear_pending_tui_undo_entries().await?;
        store.refresh(None).await?;
        Ok(store)
    }

    pub(crate) fn config(&self) -> &AppConfig {
        &self.app_config
    }

    pub(crate) fn set_config(&mut self, config: AppConfig) {
        let task_columns = config.tui.columns.clone();
        self.app_config = config;
        self.set_task_columns(task_columns);
        self.projection.rebuild_sidebar(&self.app_config);
    }

    pub(crate) fn task_columns(&self) -> &[crate::config::TaskColumnConfig] {
        &self.task_columns
    }

    pub(crate) fn set_task_columns(&mut self, columns: Vec<crate::config::TaskColumnConfig>) {
        self.task_columns = columns;
        self.derived.columns = OnceCell::new();
    }

    pub(crate) fn task_list_view(&self) -> TaskListViewRef<'_> {
        self.task_list_view_at(crate::queue::now_seconds())
    }

    fn task_list_view_at(&self, now_seconds: i64) -> TaskListViewRef<'_> {
        let render_mode = self.view_state.render_mode();
        if render_mode != TaskListRenderMode::Upcoming {
            return TaskListViewRef::Stable(self.derived.task_list.get_or_init(|| {
                TaskListView::from_tasks(
                    render_mode,
                    &self.tasks,
                    &self.view_state.expanded_epic_ids,
                )
            }));
        }

        TaskListViewRef::Owned(TaskListView::from_tasks_at(
            render_mode,
            &self.tasks,
            &self.view_state.expanded_epic_ids,
            now_seconds,
        ))
    }

    pub(crate) fn column_board(&self) -> &ColumnBoard {
        self.derived
            .columns
            .get_or_init(|| ColumnBoard::new(&self.task_columns, &self.tasks))
    }

    pub(crate) fn database(&self) -> Database {
        self.database.clone()
    }

    pub(super) fn wake_after_mutation(&self) {
        crate::daemon::wake_if_enabled(&self.app_config);
    }

    #[cfg(test)]
    pub(crate) fn fail_next_refresh(&mut self) {
        self.fail_next_refresh_at(RefreshFailureStage::Projects);
    }

    #[cfg(test)]
    pub(crate) fn fail_next_refresh_at(&mut self, stage: RefreshFailureStage) {
        self.fail_next_refresh = Some(stage);
    }

    #[cfg(test)]
    fn projection_clone_count(&self) -> std::sync::Arc<std::sync::atomic::AtomicUsize> {
        self.projection.clone_sentinel.clone_count.clone()
    }

    pub(crate) async fn load_task_item(
        &self,
        task_id: &crate::ids::TaskId,
    ) -> Result<Option<TaskListItem>> {
        Ok(self
            .database
            .list_task_items(
                &self.active_workspace.id,
                crate::query::TaskFilters {
                    include_deleted: true,
                    task_ids: crate::query::TaskIdFilter::Only(vec![task_id.clone()]),
                    ..crate::query::TaskFilters::default()
                },
                crate::query::TaskQueryMode::Flat,
                crate::query::TaskSort::Created,
                crate::query::SortDirection::Asc,
            )
            .await?
            .into_iter()
            .next())
    }

    pub(crate) async fn load_task_items(
        &self,
        task_ids: &[crate::ids::TaskId],
    ) -> Result<Vec<TaskListItem>> {
        let requested = task_ids.iter().collect::<std::collections::BTreeSet<_>>();
        let resident = self
            .tasks
            .iter()
            .filter(|item| {
                requested.contains(&item.task.id) && item.hydration == TaskItemHydration::Detail
            })
            .map(|item| (item.task.id.clone(), item.clone()))
            .collect::<std::collections::BTreeMap<_, _>>();
        let missing = task_ids
            .iter()
            .filter(|task_id| !resident.contains_key(*task_id))
            .cloned()
            .collect::<Vec<_>>();
        let hydrated = if missing.is_empty() {
            Vec::new()
        } else {
            self.database
                .list_task_items(
                    &self.active_workspace.id,
                    crate::query::TaskFilters {
                        include_deleted: true,
                        task_ids: crate::query::TaskIdFilter::Only(missing),
                        ..crate::query::TaskFilters::default()
                    },
                    crate::query::TaskQueryMode::Flat,
                    crate::query::TaskSort::Created,
                    crate::query::SortDirection::Asc,
                )
                .await?
        };
        let hydrated = hydrated
            .into_iter()
            .map(|item| (item.task.id.clone(), item))
            .collect::<std::collections::BTreeMap<_, _>>();
        Ok(task_ids
            .iter()
            .filter_map(|task_id| {
                resident
                    .get(task_id)
                    .or_else(|| hydrated.get(task_id))
                    .cloned()
            })
            .collect())
    }

    pub(crate) async fn task_full_report(
        &self,
        blob_dir: &std::path::Path,
        task_id: &crate::ids::TaskId,
    ) -> Result<Option<crate::task_render::TaskFullReport>> {
        let Some(item) = self.load_task_item(task_id).await? else {
            return Ok(None);
        };
        let detail = self.database.task_detail(&item.task).await?;
        crate::task_render::build_full_task_report(
            &self.database,
            &self.active_workspace,
            blob_dir,
            detail,
        )
        .await
        .map(Some)
    }

    pub(crate) fn show_exact_task(&mut self, item: TaskListItem) {
        self.projection.view_state = TaskViewState::for_exact_task(item.task.id.clone());
        *self.tasks_mut() = vec![item].into();
    }

    pub(crate) fn selected_task(&self, selected: Option<usize>) -> Option<&TaskListItem> {
        selected.and_then(|index| self.tasks.get(index))
    }

    pub(crate) fn selected_recent_action(
        &self,
        selected: Option<usize>,
    ) -> Option<&RecentActionItem> {
        selected.and_then(|index| self.recent_actions.get(index))
    }

    pub(crate) fn selected_recurrence_series(
        &self,
        selected: Option<usize>,
    ) -> Option<&RecurrenceSeriesListItem> {
        selected.and_then(|index| self.recurrence_series.get(index))
    }

    pub(crate) fn main_row_count(&self) -> usize {
        match self.view_state.query {
            TaskQuery::RecentActions => self.recent_actions.len(),
            TaskQuery::Recurring => self.recurrence_series.len(),
            _ => self.tasks.len(),
        }
    }

    pub(crate) async fn refresh(
        &mut self,
        selected_id: Option<&crate::ids::TaskId>,
    ) -> Result<Option<usize>> {
        let selected = selected_id.cloned().map(MainRowSelection::Task);
        Ok(self
            .refresh_with_scope_fallback(selected.as_ref())
            .await?
            .selected)
    }

    pub(crate) async fn refresh_with_scope_fallback(
        &mut self,
        selected: Option<&MainRowSelection>,
    ) -> Result<ScopeRefreshResult> {
        let restore = selected
            .cloned()
            .map(SelectionRestore::Identity)
            .unwrap_or(SelectionRestore::Default);
        self.refresh_replacement(&restore, None, None).await
    }

    pub(crate) async fn refresh_preserving_visible_deleted(
        &mut self,
        selected: Option<&MainRowSelection>,
    ) -> Result<ScopeRefreshResult> {
        let visible_deleted = self
            .tasks
            .iter()
            .enumerate()
            .filter(|(_, item)| item.task.deleted)
            .map(|(index, item)| (index, item.clone()))
            .collect::<Vec<_>>();
        let workspace_id = self.active_workspace.id.clone();
        let mut result = self.refresh_with_scope_fallback(selected).await?;
        if self.active_workspace.id != workspace_id {
            return Ok(result);
        }
        for (index, item) in visible_deleted {
            if self
                .tasks
                .iter()
                .all(|candidate| candidate.task.id != item.task.id)
            {
                self.insert_task(index.min(self.tasks.len()), item);
            }
        }
        if let Some(MainRowSelection::Task(task_id)) = selected {
            result.selected = self
                .tasks
                .iter()
                .position(|item| &item.task.id == task_id)
                .or(result.selected);
        }
        Ok(result)
    }

    pub(super) async fn refresh_with_view_state(
        &mut self,
        view_state: TaskViewState,
        selected_id: Option<&crate::ids::TaskId>,
    ) -> Result<ScopeRefreshResult> {
        let selected = selected_id.cloned().map(MainRowSelection::Task);
        let restore = selected
            .map(SelectionRestore::Identity)
            .unwrap_or(SelectionRestore::Default);
        self.refresh_replacement(&restore, Some(view_state), None)
            .await
    }

    pub(super) async fn refresh_with_workspace_and_view_state(
        &mut self,
        active_workspace: Workspace,
        view_state: TaskViewState,
    ) -> Result<ScopeRefreshResult> {
        self.refresh_replacement(
            &SelectionRestore::Default,
            Some(view_state),
            Some(active_workspace),
        )
        .await
    }

    pub(super) async fn refresh_replacement(
        &mut self,
        restore: &SelectionRestore,
        view_state: Option<TaskViewState>,
        active_workspace: Option<Workspace>,
    ) -> Result<ScopeRefreshResult> {
        let active_workspace = active_workspace.unwrap_or_else(|| self.active_workspace.clone());
        let view_state = view_state.unwrap_or_else(|| self.view_state.clone());
        let recurrence_detail_id = self
            .recurrence_detail
            .as_ref()
            .map(|detail| detail.series.id.clone());
        #[cfg(test)]
        let fail_at = self.fail_next_refresh.take();
        #[cfg(not(test))]
        let fail_at = None;
        let requested_workspace = active_workspace.clone();
        let mut candidate = TuiProjection::new(active_workspace, view_state);
        // Parts of the sync status may be unreadable mid-operation; the refresh
        // keeps these last-known values until they can be read again.
        candidate.sync_status = self.sync_status.clone();
        // Every TUI refresh path awaits the projection load; boxing it keeps the
        // refresh futures nested inside each action small.
        let fallback_scope = match Box::pin(candidate.load(
            &self.database,
            &self.app_config,
            restore,
            recurrence_detail_id.as_ref(),
            fail_at,
        ))
        .await
        {
            Ok(fallback_scope) => fallback_scope,
            Err(error) => {
                self.refresh_health = RefreshHealth::Failed;
                return Err(error);
            }
        };
        let previous_undo_entry_id = self.latest_undo.as_ref().map(|undo| undo.entry_id.clone());
        candidate.new_undo_entry_id = candidate
            .latest_undo
            .as_ref()
            .map(|undo| undo.entry_id.clone())
            .filter(|id| Some(id) != previous_undo_entry_id.as_ref());
        if candidate.active_workspace.id != requested_workspace.id {
            self.deleted_active_workspace = Some(requested_workspace.name);
        }
        self.projection = candidate;
        self.derived = DerivedTaskProjections::default();
        self.refresh_health = RefreshHealth::Healthy;
        Ok(ScopeRefreshResult {
            selected: self.restored_main_selection(restore),
            fallback_scope,
        })
    }

    pub(crate) async fn ensure_task_details(
        &mut self,
        task_ids: &[crate::ids::TaskId],
    ) -> Result<TaskDetailHydration> {
        // Detail hydration fills only fields that the derived row and lane indexes do not read.
        self.projection
            .ensure_task_details(&self.database, task_ids)
            .await
    }

    pub(crate) fn refresh_health(&self) -> RefreshHealth {
        self.refresh_health
    }

    pub(crate) fn available_undo(&self) -> Option<&UndoPresentation> {
        (self.refresh_health == RefreshHealth::Healthy)
            .then_some(self.latest_undo.as_ref())
            .flatten()
    }

    pub(crate) fn take_new_undo_entry_id(&mut self) -> Option<String> {
        self.projection.new_undo_entry_id.take()
    }

    pub(crate) fn clear_recurrence_detail(&mut self) {
        self.projection.recurrence_detail = None;
    }

    pub(crate) fn set_layout(&mut self, layout: TaskLayout) -> Result<(), &'static str> {
        // Layout selects which derived index is consumed; neither index depends on it.
        self.projection.view_state.set_layout(layout)
    }

    pub(crate) fn insert_task(&mut self, index: usize, item: TaskListItem) {
        self.tasks_mut().insert(index, item);
    }

    pub(crate) fn remove_tasks(&mut self, task_ids: &[crate::ids::TaskId]) {
        self.tasks_mut()
            .retain(|item| !task_ids.contains(&item.task.id));
    }

    fn tasks_mut(&mut self) -> &mut TaskProjection {
        self.derived = DerivedTaskProjections::default();
        &mut self.projection.tasks
    }
}

impl TuiProjection {
    fn new(active_workspace: Workspace, view_state: TaskViewState) -> Self {
        Self {
            tasks: TaskProjection::default(),
            recurrence_series: Vec::new(),
            recurrence_detail: None,
            recent_actions: Vec::new(),
            projects: Vec::new(),
            labels: Vec::new(),
            workspaces: Vec::new(),
            workspace_open_counts: Default::default(),
            active_workspace,
            counts: SidebarCounts::default(),
            sidebar_entries: Vec::new(),
            view_state,
            sync_status: TuiSyncStatus::default(),
            latest_undo: None,
            new_undo_entry_id: None,
            last_refresh: Instant::now(),
            #[cfg(test)]
            clone_sentinel: ProjectionCloneSentinel::default(),
        }
    }

    /// Loads a complete projection for this candidate's workspace and view state.
    /// Returns the project scope that was replaced by workspace scope, if any.
    async fn load(
        &mut self,
        database: &Database,
        app_config: &AppConfig,
        restore: &SelectionRestore,
        recurrence_detail_id: Option<&aven_core::recurrence::RecurrenceSeriesId>,
        fail_at: Option<RefreshFailureStage>,
    ) -> Result<Option<String>> {
        self.workspaces = database.list_workspaces().await?;
        self.fall_back_from_deleted_workspace();
        let workspace_id = self.active_workspace.id.clone();
        let reconciliation = database.reconcile_recurrence_reports(&workspace_id).await?;
        if reconciliation.changed > 0 {
            crate::daemon::wake_if_enabled(app_config);
        }
        inject_refresh_failure(fail_at, RefreshFailureStage::Projects)?;
        self.projects = database
            .list_project_items_from_current_projection(&workspace_id)
            .await?;
        self.workspace_open_counts = database
            .workspace_open_counts_from_current_projection()
            .await?;
        self.labels = database.list_labels(&workspace_id, None).await?;
        let fallback_scope = self.ensure_valid_scope();
        let project_scope = self.scope_project().map(str::to_string);
        self.counts = database
            .sidebar_counts_for_scope_from_current_projection(
                &workspace_id,
                project_scope.as_deref(),
            )
            .await?;
        self.recent_actions = database
            .list_recent_actions_from_current_projection(&workspace_id, project_scope.as_deref())
            .await?;
        inject_refresh_failure(fail_at, RefreshFailureStage::Tasks)?;
        if self.view_state.query == TaskQuery::Recurring {
            self.recurrence_series = database
                .list_recurrence_series_view_from_current_projection(
                    &workspace_id,
                    self.view_state.recurrence_query(),
                )
                .await?;
            if let Some(series_id) = recurrence_detail_id
                && self
                    .recurrence_series
                    .iter()
                    .any(|item| &item.series.id == series_id)
            {
                let detail = database
                    .recurrence_series_detail_from_current_projection(&workspace_id, series_id)
                    .await?;
                self.set_recurrence_detail(series_id, detail)?;
            }
        } else if self.view_state.query != TaskQuery::RecentActions {
            self.tasks = database
                .list_task_summary_items_from_current_projection(
                    &workspace_id,
                    self.view_state.filters(),
                    self.view_state.query_mode(),
                    self.view_state.sort(),
                    self.view_state.sort_direction(),
                    None,
                )
                .await?
                .into();
            if self.view_state.query == TaskQuery::Conflicts {
                self.append_recurrence_conflict_tasks(database).await?;
            }
        }
        self.load_epic_child_tasks(database).await?;
        self.prune_expanded_epic_ids();
        self.ensure_restored_task_detail(database, restore).await?;
        self.sync_status =
            config::load_sync_status(database, app_config, &self.sync_status).await?;
        self.latest_undo = undo::latest_undo_presentation(database, &workspace_id).await?;
        self.rebuild_sidebar(app_config);
        self.last_refresh = Instant::now();
        Ok(fallback_scope)
    }

    async fn ensure_restored_task_detail(
        &mut self,
        database: &Database,
        restore: &SelectionRestore,
    ) -> Result<()> {
        let task_id = match restore {
            SelectionRestore::Identity(MainRowIdentity::Task(task_id))
            | SelectionRestore::Anchor(MainRowAnchor {
                identity: MainRowIdentity::Task(task_id),
                ..
            }) => Some(task_id),
            SelectionRestore::Default
            | SelectionRestore::Identity(_)
            | SelectionRestore::Anchor(_)
            | SelectionRestore::Index(_) => None,
        };
        if let Some(task_id) = task_id {
            let hydration = self
                .ensure_task_details(database, std::slice::from_ref(task_id))
                .await?;
            let stale = hydration.stale_ids().cloned().collect::<Vec<_>>();
            self.tasks.retain(|item| !stale.contains(&item.task.id));
        }
        Ok(())
    }

    async fn ensure_task_details(
        &mut self,
        database: &Database,
        task_ids: &[crate::ids::TaskId],
    ) -> Result<TaskDetailHydration> {
        let requested = task_ids
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        let resident = self
            .tasks
            .iter()
            .filter(|item| requested.contains(&item.task.id))
            .map(|item| item.task.id.clone())
            .collect::<std::collections::BTreeSet<_>>();
        let missing = requested
            .difference(&resident)
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        let summaries = self
            .tasks
            .iter()
            .filter(|item| {
                requested.contains(&item.task.id) && item.hydration == TaskItemHydration::Summary
            })
            .map(|item| item.task.id.clone())
            .collect::<Vec<_>>();
        let hydrated = if summaries.is_empty() {
            Vec::new()
        } else {
            database
                .list_task_items_from_current_projection(
                    &self.active_workspace.id,
                    crate::query::TaskFilters {
                        include_deleted: true,
                        expand_recurring: true,
                        task_ids: crate::query::TaskIdFilter::Only(summaries),
                        ..crate::query::TaskFilters::default()
                    },
                    crate::query::TaskQueryMode::Flat,
                    crate::query::TaskSort::Created,
                    crate::query::SortDirection::Asc,
                )
                .await?
        };
        for detail in hydrated {
            if detail.hydration != TaskItemHydration::Detail {
                continue;
            }
            if let Some(summary) = self.tasks.iter_mut().find(|item| {
                requested.contains(&item.task.id)
                    && item.task.id == detail.task.id
                    && item.hydration == TaskItemHydration::Summary
            }) {
                absorb_task_detail(summary, detail);
            }
        }
        let ready = self
            .tasks
            .iter()
            .filter(|item| {
                requested.contains(&item.task.id) && item.hydration == TaskItemHydration::Detail
            })
            .map(|item| item.task.id.clone())
            .collect::<std::collections::BTreeSet<_>>();
        let unresolved = resident
            .difference(&ready)
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        Ok(TaskDetailHydration {
            ready,
            missing,
            unresolved,
        })
    }

    pub(crate) fn scope_project(&self) -> Option<&str> {
        match &self.view_state.scope {
            TaskScope::Workspace => None,
            TaskScope::Project(project) => Some(project.as_str()),
        }
    }

    /// Switches to the default workspace when the active one is no longer
    /// listed because it was deleted, here or on another device.
    fn fall_back_from_deleted_workspace(&mut self) {
        if self
            .workspaces
            .iter()
            .any(|workspace| workspace.id == self.active_workspace.id)
        {
            return;
        }
        let Some(default) = self
            .workspaces
            .iter()
            .find(|workspace| workspace.id.as_str() == aven_core::workspaces::DEFAULT_WORKSPACE_ID)
        else {
            return;
        };
        self.active_workspace = default.clone();
        self.view_state.scope = TaskScope::Workspace;
        self.view_state.filter_modifiers = TaskFilterModifiers::default();
        self.view_state.reset_projection_origin();
    }

    fn ensure_valid_scope(&mut self) -> Option<String> {
        let TaskScope::Project(project) = &self.view_state.scope else {
            return None;
        };
        if self.projects.iter().any(|item| item.key == *project) {
            return None;
        }
        let project = project.clone();
        self.view_state.scope = TaskScope::Workspace;
        Some(project)
    }

    async fn load_epic_child_tasks(&mut self, database: &Database) -> Result<()> {
        if self.view_state.render_mode() != TaskListRenderMode::Epics {
            return Ok(());
        }
        let expanded = &self.view_state.expanded_epic_ids;
        if expanded.is_empty() {
            return Ok(());
        }
        let existing_ids = self
            .tasks
            .iter()
            .map(|item| item.task.id.clone())
            .collect::<std::collections::BTreeSet<_>>();
        let child_ids = self
            .tasks
            .iter()
            .filter(|item| expanded.contains(&item.task.id))
            .flat_map(|item| item.epic_children.iter().map(|link| link.task_id.clone()))
            .filter(|task_id| !existing_ids.contains(task_id))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if child_ids.is_empty() {
            return Ok(());
        }
        let children = database
            .list_task_summary_items_from_current_projection(
                &self.active_workspace.id,
                crate::query::TaskFilters {
                    task_ids: crate::query::TaskIdFilter::Only(child_ids),
                    ..crate::query::TaskFilters::default()
                },
                crate::query::TaskQueryMode::Flat,
                crate::query::TaskSort::Created,
                crate::query::SortDirection::Asc,
                None,
            )
            .await?;
        self.tasks.extend(children);
        Ok(())
    }

    fn prune_expanded_epic_ids(&mut self) {
        let visible_parent_ids = self.visible_epic_ids();
        self.view_state
            .expanded_epic_ids
            .retain(|id| visible_parent_ids.contains(id));
        self.view_state
            .collapsed_epic_ids
            .retain(|id| visible_parent_ids.contains(id));
    }

    fn visible_epic_ids(&self) -> std::collections::BTreeSet<crate::ids::TaskId> {
        self.tasks
            .iter()
            .filter(|item| item.task.is_epic)
            .map(|item| item.task.id.clone())
            .collect()
    }
}

fn inject_refresh_failure(
    fail_at: Option<RefreshFailureStage>,
    stage: RefreshFailureStage,
) -> Result<()> {
    if fail_at == Some(stage) {
        anyhow::bail!("injected refresh failure at {stage:?}");
    }
    Ok(())
}
