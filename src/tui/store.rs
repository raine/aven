use crate::config::AppConfig;
use crate::ids::WorkspaceId;
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
use std::ops::{Deref, DerefMut};
use std::time::Instant;

use anyhow::Result;
use aven_core::db::Database;

pub(crate) use crate::query::RecentActionItem;
pub(crate) use attachments::AttachmentWorkerContext;
pub(crate) use epics::{AddEpicChildContext, EpicChildTarget, EpicContext};
pub(crate) use launch::{TuiLaunch, TuiStartup};
pub(crate) use onboarding::OnboardingStatus;
pub(crate) use pickers::{
    ADD_TASK_STATUS_AUTO_VALUE, CREATE_PROJECT_PICKER_VALUE_PREFIX, create_project_picker_name,
    deleted_picker_items, epic_picker_items, related_picker_items,
};
pub(crate) use recurrence::recurrence_draft;
pub(crate) use task_commands::{PriorityMutation, TaskDateField, TaskTextField};
pub(crate) use task_creation::task_creation_committed;
pub(crate) use types::{
    ClosedTaskVisibility, ConflictTarget, MainRowAnchor, MainRowIdentity, MainRowPosition,
    MainRowSelection, MutationMessage, RecurringSeriesViewState, SelectionRestore, SidebarEntry,
    SidebarEntryTarget, SyncStatusCheck, TaskFilterModifiers, TaskLayout, TaskListRenderMode,
    TaskOrder, TaskProjectionOrigin, TaskQuery, TaskScope, TaskScopeTarget, TaskViewState,
    TuiDatabaseStats, TuiSyncStatus, UndoPresentation, mutation_committed,
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
    activity_hydrated_task: Option<crate::ids::TaskId>,
    activity_failed_task: Option<crate::ids::TaskId>,
    task_columns: Vec<crate::config::TaskColumnConfig>,
    derived: DerivedTaskProjections,
    pub(crate) columns_preview_visible: bool,
    pub(crate) db_stats: TuiDatabaseStats,
    refresh_health: RefreshHealth,
    #[cfg(test)]
    fail_next_refresh: Option<RefreshFailureStage>,
    #[cfg(test)]
    _test_database_dir: Option<std::sync::Arc<tempfile::TempDir>>,
}

#[derive(Default)]
struct DerivedTaskProjections {
    task_list: OnceCell<TaskListView>,
    columns: OnceCell<ColumnBoard>,
}

struct RefreshRetainedState {
    database: Database,
    app_config: AppConfig,
    task_columns: Vec<crate::config::TaskColumnConfig>,
    columns_preview_visible: bool,
    db_stats: TuiDatabaseStats,
    refresh_health: RefreshHealth,
    #[cfg(test)]
    test_database_dir: Option<std::sync::Arc<tempfile::TempDir>>,
}

#[cfg_attr(test, derive(Clone))]
pub(crate) struct TuiProjection {
    pub(crate) tasks: Vec<TaskListItem>,
    pub(crate) recurrence_series: Vec<RecurrenceSeriesListItem>,
    pub(crate) recurrence_detail: Option<RecurrenceSeriesDetail>,
    pub(crate) recent_actions: Vec<RecentActionItem>,
    pub(crate) projects: Vec<ProjectListItem>,
    pub(crate) labels: Vec<String>,
    pub(crate) workspaces: Vec<Workspace>,
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

impl From<&TuiStore> for RefreshRetainedState {
    fn from(store: &TuiStore) -> Self {
        Self {
            database: store.database.clone(),
            app_config: store.app_config.clone(),
            task_columns: store.task_columns.clone(),
            columns_preview_visible: store.columns_preview_visible,
            db_stats: store.db_stats.clone(),
            refresh_health: store.refresh_health,
            #[cfg(test)]
            test_database_dir: store._test_database_dir.clone(),
        }
    }
}

impl RefreshRetainedState {
    fn with_projection(self, projection: TuiProjection) -> TuiStore {
        TuiStore {
            database: self.database,
            app_config: self.app_config,
            projection,
            activity_hydrated_task: None,
            activity_failed_task: None,
            task_columns: self.task_columns,
            derived: DerivedTaskProjections::default(),
            columns_preview_visible: self.columns_preview_visible,
            db_stats: self.db_stats,
            refresh_health: self.refresh_health,
            #[cfg(test)]
            fail_next_refresh: None,
            #[cfg(test)]
            _test_database_dir: self.test_database_dir,
        }
    }
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
            projection: TuiProjection {
                tasks: Vec::new(),
                recurrence_series: Vec::new(),
                recurrence_detail: None,
                recent_actions: Vec::new(),
                projects: Vec::new(),
                labels: Vec::new(),
                workspaces: Vec::new(),
                active_workspace: workspace,
                counts: SidebarCounts::default(),
                sidebar_entries: Vec::new(),
                view_state,
                sync_status: TuiSyncStatus::default(),
                latest_undo: None,
                new_undo_entry_id: None,
                last_refresh: Instant::now(),
                #[cfg(test)]
                clone_sentinel: ProjectionCloneSentinel::default(),
            },
            activity_hydrated_task: None,
            activity_failed_task: None,
            task_columns,
            derived: DerivedTaskProjections::default(),
            columns_preview_visible: true,
            db_stats: TuiDatabaseStats::default(),
            refresh_health: RefreshHealth::Healthy,
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
    }

    pub(crate) fn task_columns(&self) -> &[crate::config::TaskColumnConfig] {
        &self.task_columns
    }

    pub(crate) fn set_task_columns(&mut self, columns: Vec<crate::config::TaskColumnConfig>) {
        self.task_columns = columns;
        self.derived = DerivedTaskProjections::default();
    }

    pub(crate) fn task_list_view(&self) -> &TaskListView {
        self.derived.task_list.get_or_init(|| {
            TaskListView::from_tasks(
                self.view_state.render_mode(),
                &self.tasks,
                &self.view_state.expanded_epic_ids,
            )
        })
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

    pub(crate) async fn hydrate_task_activity(
        &mut self,
        task_id: &crate::ids::TaskId,
    ) -> Result<()> {
        if self.activity_hydrated_task.as_ref() == Some(task_id)
            || self.activity_failed_task.as_ref() == Some(task_id)
        {
            return Ok(());
        }
        let Some(index) = self.tasks.iter().position(|item| &item.task.id == task_id) else {
            return Ok(());
        };
        let activity = match self
            .database
            .task_activity_for_task(&self.active_workspace.id, task_id)
            .await
        {
            Ok(activity) => activity,
            Err(error) => {
                self.activity_failed_task = Some(task_id.clone());
                return Err(error);
            }
        };
        self.activity_hydrated_task = Some(task_id.clone());
        if let Some(item) = self.tasks.get_mut(index) {
            item.activity = activity;
        }
        Ok(())
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
        task_id: &crate::ids::TaskId,
    ) -> Result<Option<crate::task_render::TaskFullReport>> {
        let Some(item) = self.load_task_item(task_id).await? else {
            return Ok(None);
        };
        let detail = self.database.task_detail(&item.task).await?;
        crate::task_render::build_full_task_report(&self.database, &self.active_workspace, detail)
            .await
            .map(Some)
    }

    pub(crate) fn show_exact_task(&mut self, item: TaskListItem) {
        self.activity_hydrated_task = Some(item.task.id.clone());
        self.activity_failed_task = None;
        self.view_state = TaskViewState::for_exact_task(item.task.id.clone());
        self.tasks = vec![item];
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
        let mut result = self.refresh_with_scope_fallback(selected).await?;
        for (index, item) in visible_deleted {
            if self
                .tasks
                .iter()
                .all(|candidate| candidate.task.id != item.task.id)
            {
                let insertion_index = index.min(self.tasks.len());
                self.tasks.insert(insertion_index, item);
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
        let fail_next_refresh = self.fail_next_refresh.take();
        let previous_undo_entry_id = self.latest_undo.as_ref().map(|undo| undo.entry_id.clone());
        let retained = RefreshRetainedState::from(&*self);
        let mut replacement =
            retained.with_projection(Self::fresh_projection(active_workspace, view_state));
        #[cfg(test)]
        {
            replacement.fail_next_refresh = fail_next_refresh;
        }
        let mut result = match replacement
            .refresh_in_place(restore, recurrence_detail_id.as_ref())
            .await
        {
            Ok(result) => result,
            Err(error) => {
                self.refresh_health = RefreshHealth::Failed;
                return Err(error);
            }
        };
        replacement.new_undo_entry_id = replacement
            .latest_undo
            .as_ref()
            .map(|undo| undo.entry_id.clone())
            .filter(|id| Some(id) != previous_undo_entry_id.as_ref());
        replacement.refresh_health = RefreshHealth::Healthy;
        result.selected = replacement.restored_main_selection(restore);
        #[cfg(test)]
        {
            replacement.fail_next_refresh = None;
        }
        *self = replacement;
        Ok(result)
    }

    fn fresh_projection(active_workspace: Workspace, view_state: TaskViewState) -> TuiProjection {
        TuiProjection {
            tasks: Vec::new(),
            recurrence_series: Vec::new(),
            recurrence_detail: None,
            recent_actions: Vec::new(),
            projects: Vec::new(),
            labels: Vec::new(),
            workspaces: Vec::new(),
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

    async fn refresh_in_place(
        &mut self,
        restore: &SelectionRestore,
        recurrence_detail_id: Option<&aven_core::recurrence::RecurrenceSeriesId>,
    ) -> Result<ScopeRefreshResult> {
        let workspace_id = self.active_workspace.id.clone();
        self.workspaces = self.database.list_workspaces().await?;
        let reconciliation = self
            .database
            .reconcile_recurrence_reports(&workspace_id)
            .await?;
        if reconciliation.changed > 0 {
            self.wake_after_mutation();
        }
        self.inject_refresh_failure(RefreshFailureStage::Projects)?;
        self.projects = self
            .database
            .list_project_items_from_current_projection(&workspace_id)
            .await?;
        self.labels = self.database.list_labels(&workspace_id, None).await?;
        let fallback_scope = self.ensure_valid_scope();
        let project_scope = self.scope_project().map(str::to_string);
        self.counts = self
            .database
            .sidebar_counts_for_scope_from_current_projection(
                &workspace_id,
                project_scope.as_deref(),
            )
            .await?;
        self.recent_actions = self
            .database
            .list_recent_actions_from_current_projection(&workspace_id, project_scope.as_deref())
            .await?;
        self.inject_refresh_failure(RefreshFailureStage::Tasks)?;
        if self.view_state.query == TaskQuery::RecentActions {
            self.tasks.clear();
            self.recurrence_series.clear();
            self.recurrence_detail = None;
        } else if self.view_state.query == TaskQuery::Recurring {
            self.tasks.clear();
            self.recurrence_series = self
                .database
                .list_recurrence_series_view_from_current_projection(
                    &workspace_id,
                    self.view_state.recurrence_query(),
                )
                .await?;
            self.recurrence_detail = None;
            if let Some(series_id) = recurrence_detail_id
                && self
                    .recurrence_series
                    .iter()
                    .any(|item| &item.series.id == series_id)
            {
                self.load_recurrence_series_detail(series_id).await?;
            }
        } else {
            self.recurrence_series.clear();
            self.recurrence_detail = None;
            let filters = self.view_state.filters();
            self.tasks = self
                .database
                .list_task_summary_items_from_current_projection(
                    &workspace_id,
                    filters,
                    self.view_state.query_mode(),
                    self.view_state.sort(),
                    self.view_state.sort_direction(),
                    None,
                )
                .await?;
            if self.view_state.query == TaskQuery::Conflicts {
                self.append_recurrence_conflict_tasks().await?;
            }
        }
        self.load_epic_child_tasks(&workspace_id).await?;
        self.prune_expanded_epic_ids();
        self.ensure_restored_task_detail(restore).await?;
        self.sync_status = self.load_sync_status().await?;
        self.latest_undo = self.load_latest_undo_presentation().await?;
        self.rebuild_sidebar();
        self.last_refresh = Instant::now();
        Ok(ScopeRefreshResult {
            selected: None,
            fallback_scope,
        })
    }

    async fn ensure_restored_task_detail(&mut self, restore: &SelectionRestore) -> Result<()> {
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
            self.ensure_task_details(std::slice::from_ref(task_id))
                .await?;
        }
        Ok(())
    }

    pub(crate) async fn ensure_task_details(
        &mut self,
        task_ids: &[crate::ids::TaskId],
    ) -> Result<()> {
        let missing = task_ids
            .iter()
            .filter(|task_id| {
                self.tasks.iter().any(|item| {
                    &item.task.id == *task_id && item.hydration == TaskItemHydration::Summary
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        if missing.is_empty() {
            return Ok(());
        }
        let hydrated = self
            .database
            .list_task_items_from_current_projection(
                &self.active_workspace.id,
                crate::query::TaskFilters {
                    include_deleted: true,
                    expand_recurring: true,
                    task_ids: crate::query::TaskIdFilter::Only(missing),
                    ..crate::query::TaskFilters::default()
                },
                crate::query::TaskQueryMode::Flat,
                crate::query::TaskSort::Created,
                crate::query::SortDirection::Asc,
            )
            .await?;
        for detail in hydrated {
            if let Some(summary) = self
                .tasks
                .iter_mut()
                .find(|item| item.task.id == detail.task.id)
            {
                absorb_task_detail(summary, detail);
            }
        }
        Ok(())
    }

    #[cfg(test)]
    fn inject_refresh_failure(&mut self, stage: RefreshFailureStage) -> Result<()> {
        if self.fail_next_refresh == Some(stage) {
            self.fail_next_refresh = None;
            anyhow::bail!("injected refresh failure at {stage:?}");
        }
        Ok(())
    }

    #[cfg(not(test))]
    fn inject_refresh_failure(&mut self, _stage: RefreshFailureStage) -> Result<()> {
        Ok(())
    }

    pub(crate) fn refresh_health(&self) -> RefreshHealth {
        self.refresh_health
    }

    pub(crate) fn available_undo(&self) -> Option<&UndoPresentation> {
        (self.refresh_health == RefreshHealth::Healthy)
            .then_some(self.latest_undo.as_ref())
            .flatten()
    }

    pub(crate) fn scope_project(&self) -> Option<&str> {
        match &self.view_state.scope {
            TaskScope::Workspace => None,
            TaskScope::Project(project) => Some(project.as_str()),
        }
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

    async fn load_epic_child_tasks(&mut self, workspace_id: &WorkspaceId) -> Result<()> {
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
        let children = self
            .database
            .list_task_summary_items_from_current_projection(
                workspace_id,
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
