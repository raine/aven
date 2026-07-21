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

use std::time::Instant;

use anyhow::Result;
use aven_core::db::Database;

pub(crate) use crate::query::RecentActionItem;
pub(crate) use attachments::AttachmentWorkerContext;
pub(crate) use epics::EpicContext;
pub(crate) use launch::{TuiLaunch, TuiStartup};
pub(crate) use onboarding::OnboardingStatus;
pub(crate) use pickers::deleted_picker_items;
pub(crate) use recurrence::{recurrence_draft, recurrence_history_lines};
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

#[derive(Clone)]
pub(crate) struct TuiStore {
    database: Database,
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
    pub(crate) task_columns: Vec<crate::config::TaskColumnConfig>,
    pub(crate) columns_preview_visible: bool,
    pub(crate) sync_status: TuiSyncStatus,
    pub(crate) db_stats: TuiDatabaseStats,
    pub(crate) last_refresh: Instant,
    #[cfg(test)]
    fail_next_refresh: Option<RefreshFailureStage>,
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

    pub(crate) async fn new_with_view_state(
        database: Database,
        workspace: Workspace,
        view_state: TaskViewState,
    ) -> Result<Self> {
        let mut store = Self {
            database,
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
            task_columns: crate::config::TuiConfig::default().columns,
            columns_preview_visible: true,
            sync_status: TuiSyncStatus::default(),
            db_stats: TuiDatabaseStats::default(),
            last_refresh: Instant::now(),
            #[cfg(test)]
            fail_next_refresh: None,
        };
        store.database.clear_pending_tui_undo_entries().await?;
        store.refresh(None).await?;
        Ok(store)
    }

    #[cfg(test)]
    pub(crate) fn fail_next_refresh(&mut self) {
        self.fail_next_refresh_at(RefreshFailureStage::Projects);
    }

    #[cfg(test)]
    pub(crate) fn fail_next_refresh_at(&mut self, stage: RefreshFailureStage) {
        self.fail_next_refresh = Some(stage);
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
        selected_id: Option<&crate::ids::TaskId>,
    ) -> Result<ScopeRefreshResult> {
        let visible_deleted = self
            .tasks
            .iter()
            .enumerate()
            .filter(|(_, item)| item.task.deleted)
            .map(|(index, item)| (index, item.clone()))
            .collect::<Vec<_>>();
        let mut result = self.refresh_with_scope_fallback(selected_id).await?;
        for (index, item) in visible_deleted {
            if self
                .tasks
                .iter()
                .all(|candidate| candidate.task.id != item.task.id)
            {
                self.tasks.insert(index.min(self.tasks.len()), item);
            }
        }
        result.selected = selected_id
            .and_then(|task_id| self.tasks.iter().position(|item| &item.task.id == task_id))
            .or(result.selected);
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
        let mut replacement = self.clone();
        if let Some(active_workspace) = active_workspace {
            replacement.active_workspace = active_workspace;
        }
        if let Some(view_state) = view_state {
            replacement.view_state = view_state;
        }
        #[cfg(test)]
        {
            replacement.fail_next_refresh = self.fail_next_refresh.take();
        }
        let result = replacement.refresh_in_place(selected).await?;
        #[cfg(test)]
        {
            replacement.fail_next_refresh = None;
        }
        *self = replacement;
        Ok(result)
    }

    async fn refresh_in_place(
        &mut self,
        selected: Option<&MainRowSelection>,
    ) -> Result<ScopeRefreshResult> {
        let workspace_id = self.active_workspace.id.clone();
        self.workspaces = self.database.list_workspaces().await?;
        self.inject_refresh_failure(RefreshFailureStage::Projects)?;
        self.projects = self.database.list_project_items(&workspace_id).await?;
        self.labels = self.database.list_labels(&workspace_id, None).await?;
        let fallback_scope = self.ensure_valid_scope();
        let project_scope = self.scope_project().map(str::to_string);
        self.counts = self
            .database
            .sidebar_counts_for_scope(&workspace_id, project_scope.as_deref())
            .await?;
        self.recent_actions = self
            .database
            .list_recent_actions(&workspace_id, project_scope.as_deref())
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
                .list_recurrence_series_view(&workspace_id, self.view_state.recurrence_query())
                .await?;
        } else {
            self.recurrence_series.clear();
            self.recurrence_detail = None;
            let filters = self.view_state.filters();
            self.tasks = self
                .database
                .list_task_items(
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
        self.expand_visible_epics_by_default();
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
            .list_task_items(
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

    fn expand_visible_epics_by_default(&mut self) {
        if self.view_state.view != TaskView::Epics {
            return;
        }
        for item in &self.tasks {
            if item.task.is_epic && !self.view_state.collapsed_epic_ids.contains(&item.task.id) {
                self.view_state
                    .expanded_epic_ids
                    .insert(item.task.id.clone());
            }
        }
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
