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

use std::ops::{Deref, DerefMut};
use std::time::Instant;

use anyhow::Result;
use aven_core::db::Database;

pub(crate) use crate::query::RecentActionItem;
pub(crate) use attachments::AttachmentWorkerContext;
pub(crate) use epics::{AddEpicChildContext, EpicContext};
pub(crate) use launch::{TuiLaunch, TuiStartup};
pub(crate) use onboarding::OnboardingStatus;
pub(crate) use pickers::{
    CREATE_PROJECT_PICKER_VALUE_PREFIX, create_project_picker_name, deleted_picker_items,
    epic_picker_items,
};
pub(crate) use recurrence::recurrence_draft;
pub(crate) use task_commands::{PriorityMutation, TaskDateField, TaskTextField};
pub(crate) use task_creation::task_creation_committed;
pub(crate) use types::{
    ClosedTaskVisibility, ConflictTarget, MainRowSelection, MutationMessage,
    RecurringSeriesViewState, SidebarEntry, SidebarEntryTarget, SyncStatusCheck,
    TaskFilterModifiers, TaskListRenderMode, TaskOrder, TaskScope, TaskScopeTarget, TaskView,
    TaskViewState, TuiDatabaseStats, TuiSyncStatus, mutation_committed,
};
#[cfg(test)]
pub(crate) use types::{DatabaseStatsPriorityCounts, DatabaseStatsStatusCounts};

use crate::query::{
    ProjectListItem, RecurrenceSeriesDetail, RecurrenceSeriesListItem, SidebarCounts, TaskListItem,
};
use crate::workspaces::Workspace;

pub(crate) struct TuiStore {
    database: Database,
    app_config: AppConfig,
    projection: TuiProjection,
    pub(crate) task_columns: Vec<crate::config::TaskColumnConfig>,
    pub(crate) columns_preview_visible: bool,
    pub(crate) db_stats: TuiDatabaseStats,
    #[cfg(test)]
    fail_next_refresh: Option<RefreshFailureStage>,
    #[cfg(test)]
    _test_database_dir: Option<std::sync::Arc<tempfile::TempDir>>,
}

struct RefreshRetainedState {
    database: Database,
    app_config: AppConfig,
    task_columns: Vec<crate::config::TaskColumnConfig>,
    columns_preview_visible: bool,
    db_stats: TuiDatabaseStats,
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

impl Deref for TuiStore {
    type Target = TuiProjection;

    fn deref(&self) -> &Self::Target {
        &self.projection
    }
}

impl DerefMut for TuiStore {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.projection
    }
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
            task_columns: self.task_columns,
            columns_preview_visible: self.columns_preview_visible,
            db_stats: self.db_stats,
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
                last_refresh: Instant::now(),
                #[cfg(test)]
                clone_sentinel: ProjectionCloneSentinel::default(),
            },
            task_columns,
            columns_preview_visible: true,
            db_stats: TuiDatabaseStats::default(),
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
        self.app_config = config;
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
                    task_ids: vec![task_id.clone()],
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

    pub(crate) fn show_exact_task(&mut self, item: TaskListItem) {
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
        match self.view_state.view {
            TaskView::RecentActions => self.recent_actions.len(),
            TaskView::Recurring => self.recurrence_series.len(),
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
        self.refresh_replacement(selected, None, None).await
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
        self.refresh_replacement(selected.as_ref(), Some(view_state), None)
            .await
    }

    pub(super) async fn refresh_with_workspace_and_view_state(
        &mut self,
        active_workspace: Workspace,
        view_state: TaskViewState,
    ) -> Result<ScopeRefreshResult> {
        self.refresh_replacement(None, Some(view_state), Some(active_workspace))
            .await
    }

    pub(super) async fn refresh_replacement(
        &mut self,
        selected: Option<&MainRowSelection>,
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
        let retained = RefreshRetainedState::from(&*self);
        let mut replacement =
            retained.with_projection(Self::fresh_projection(active_workspace, view_state));
        #[cfg(test)]
        {
            replacement.fail_next_refresh = fail_next_refresh;
        }
        let result = replacement
            .refresh_in_place(selected, recurrence_detail_id.as_ref())
            .await?;
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
            last_refresh: Instant::now(),
            #[cfg(test)]
            clone_sentinel: ProjectionCloneSentinel::default(),
        }
    }

    async fn refresh_in_place(
        &mut self,
        selected: Option<&MainRowSelection>,
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
        if self.view_state.view == TaskView::RecentActions {
            self.tasks.clear();
            self.recurrence_series.clear();
            self.recurrence_detail = None;
        } else if self.view_state.view == TaskView::Recurring {
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
                .list_task_items_from_current_projection(
                    &workspace_id,
                    filters,
                    self.view_state.query_mode(),
                    self.view_state.sort(),
                    self.view_state.sort_direction(),
                )
                .await?;
            if self.view_state.view == TaskView::Conflicts {
                self.append_recurrence_conflict_tasks().await?;
            }
        }
        self.load_epic_child_tasks(&workspace_id).await?;
        self.prune_expanded_epic_ids();
        self.sync_status = self.load_sync_status().await?;
        self.rebuild_sidebar();
        self.last_refresh = Instant::now();
        Ok(ScopeRefreshResult {
            selected: self.restored_main_selection(selected),
            fallback_scope,
        })
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
            .list_task_items_from_current_projection(
                workspace_id,
                crate::query::TaskFilters {
                    task_ids: child_ids,
                    ..crate::query::TaskFilters::default()
                },
                crate::query::TaskQueryMode::Flat,
                crate::query::TaskSort::Created,
                crate::query::SortDirection::Asc,
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
