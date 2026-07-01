mod config;
mod conflicts;
mod domain;
mod epics;
mod pickers;
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
use sqlx::SqlitePool;

pub(crate) use pickers::deleted_picker_items;
pub(crate) use types::{
    ConflictTarget, MutationMessage, SidebarEntry, SidebarEntryTarget, SyncStatusCheck,
    TaskFilterModifiers, TaskListRenderMode, TaskOrder, TaskScope, TaskScopeTarget, TaskView,
    TaskViewState, TuiDatabaseStats, TuiSyncStatus,
};
#[cfg(test)]
pub(crate) use types::{DatabaseStatsPriorityCounts, DatabaseStatsStatusCounts};

use crate::labels::list_labels_in_workspace;
use crate::projects::{
    inferred_existing_project_key_in_workspace, resolve_existing_project_in_workspace,
};
use crate::query::{
    ProjectListItem, SidebarCounts, TaskListItem, list_project_items_in_workspace,
    list_task_items_in_workspace, sidebar_counts_for_scope_in_workspace,
};
use crate::workspaces::{Workspace, active_workspace, list_workspaces, set_active_workspace};

pub(crate) struct TuiStore {
    pool: SqlitePool,
    pub(crate) tasks: Vec<TaskListItem>,
    pub(crate) projects: Vec<ProjectListItem>,
    pub(crate) labels: Vec<String>,
    pub(crate) workspaces: Vec<Workspace>,
    pub(crate) active_workspace: Workspace,
    pub(crate) counts: SidebarCounts,
    pub(crate) sidebar_entries: Vec<SidebarEntry>,
    pub(crate) view_state: TaskViewState,
    pub(crate) sync_status: TuiSyncStatus,
    pub(crate) db_stats: TuiDatabaseStats,
    pub(crate) last_refresh: Instant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScopeRefreshResult {
    pub(crate) selected: Option<usize>,
    pub(crate) fallback_scope: Option<String>,
}

impl TuiStore {
    pub(crate) async fn new(pool: SqlitePool) -> Result<Self> {
        Self::new_with_initial_project(pool, None).await
    }

    pub(crate) async fn new_for_inferred_project(pool: SqlitePool) -> Result<Self> {
        let active_workspace = active_workspace();
        let initial_project = {
            let mut conn = pool.acquire().await?;
            inferred_existing_project_key_in_workspace(&mut conn, &active_workspace.id).await?
        };
        Self::new_with_initial_project(pool, initial_project).await
    }

    pub(crate) async fn new_for_project(pool: SqlitePool, project: &str) -> Result<Self> {
        let active_workspace = active_workspace();
        let project = {
            let mut conn = pool.acquire().await?;
            resolve_existing_project_in_workspace(&mut conn, &active_workspace.id, project).await?
        };
        Self::new_with_initial_project(pool, Some(project.key)).await
    }

    async fn new_with_initial_project(
        pool: SqlitePool,
        initial_project: Option<String>,
    ) -> Result<Self> {
        let mut view_state = TaskViewState::default();
        if let Some(project) = initial_project {
            view_state.scope = TaskScope::Project(project);
        }
        let mut store = Self {
            pool,
            tasks: Vec::new(),
            projects: Vec::new(),
            labels: Vec::new(),
            workspaces: Vec::new(),
            active_workspace: active_workspace(),
            counts: SidebarCounts::default(),
            sidebar_entries: Vec::new(),
            view_state,
            sync_status: TuiSyncStatus::default(),
            db_stats: TuiDatabaseStats::default(),
            last_refresh: Instant::now(),
        };
        {
            let mut conn = store.pool.acquire().await?;
            crate::undo::clear_pending_tui_undo_entries(&mut conn).await?;
        }
        store.refresh(None).await?;
        Ok(store)
    }

    pub(crate) fn selected_task(&self, selected: Option<usize>) -> Option<&TaskListItem> {
        selected.and_then(|index| self.tasks.get(index))
    }

    fn activate_workspace(&self) {
        set_active_workspace(self.active_workspace.clone());
    }

    pub(crate) async fn refresh(&mut self, selected_id: Option<&str>) -> Result<Option<usize>> {
        Ok(self
            .refresh_with_scope_fallback(selected_id)
            .await?
            .selected)
    }

    pub(crate) async fn refresh_with_scope_fallback(
        &mut self,
        selected_id: Option<&str>,
    ) -> Result<ScopeRefreshResult> {
        let mut conn = self.pool.acquire().await?;
        let workspace_id = self.active_workspace.id.clone();
        self.workspaces = list_workspaces(&mut conn).await?;
        self.projects = list_project_items_in_workspace(&mut conn, workspace_id.as_str()).await?;
        self.labels = list_labels_in_workspace(&mut conn, workspace_id.as_str(), None).await?;
        let fallback_scope = self.ensure_valid_scope();
        let project_scope = self.scope_project();
        self.counts =
            sidebar_counts_for_scope_in_workspace(&mut conn, workspace_id.as_str(), project_scope)
                .await?;
        let filters = self.view_state.filters();
        self.tasks = list_task_items_in_workspace(
            &mut conn,
            workspace_id.as_str(),
            filters,
            self.view_state.query_mode(),
            self.view_state.sort(),
            self.view_state.direction,
        )
        .await?;
        self.expand_visible_epics_by_default();
        self.load_epic_child_tasks(&mut conn, &workspace_id).await?;
        self.prune_expanded_epic_ids();
        self.sync_status = self.load_sync_status(&mut conn).await?;
        self.rebuild_sidebar();
        self.last_refresh = Instant::now();
        Ok(ScopeRefreshResult {
            selected: self.restored_task_selection(selected_id),
            fallback_scope,
        })
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

    async fn load_epic_child_tasks(
        &mut self,
        conn: &mut sqlx::SqliteConnection,
        workspace_id: &str,
    ) -> Result<()> {
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
            .flat_map(|item| {
                item.epic_children
                    .iter()
                    .filter(|link| link.unresolved)
                    .map(|link| link.task_id.clone())
            })
            .filter(|task_id| !existing_ids.contains(task_id))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if child_ids.is_empty() {
            return Ok(());
        }
        let children = list_task_items_in_workspace(
            conn,
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

    fn visible_epic_ids(&self) -> std::collections::BTreeSet<String> {
        self.tasks
            .iter()
            .filter(|item| item.task.is_epic)
            .map(|item| item.task.id.clone())
            .collect()
    }
}
