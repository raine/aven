mod config;
mod conflicts;
mod domain;
mod pickers;
mod sidebar;
mod sort;
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
pub(crate) use types::{ConflictTarget, MutationMessage, SidebarEntry, SidebarTarget};

use crate::labels::list_labels_in_workspace;
use crate::query::{
    ProjectListItem, SidebarCounts, SortDirection, TaskFilters, TaskListItem, TaskSort,
    list_project_items_in_workspace, list_task_items_in_workspace, sidebar_counts_in_workspace,
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
    pub(crate) active_view: SidebarTarget,
    pub(crate) filters: TaskFilters,
    pub(crate) sort: TaskSort,
    pub(crate) sort_direction: SortDirection,
    pub(crate) last_refresh: Instant,
}

impl TuiStore {
    pub(crate) async fn new(pool: SqlitePool) -> Result<Self> {
        let mut store = Self {
            pool,
            tasks: Vec::new(),
            projects: Vec::new(),
            labels: Vec::new(),
            workspaces: Vec::new(),
            active_workspace: active_workspace(),
            counts: SidebarCounts::default(),
            sidebar_entries: Vec::new(),
            active_view: SidebarTarget::All,
            filters: TaskFilters::default(),
            sort: TaskSort::Queue,
            sort_direction: SortDirection::Asc,
            last_refresh: Instant::now(),
        };
        store.apply_active_view_filters();
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
        let mut conn = self.pool.acquire().await?;
        let workspace_id = self.active_workspace.id.as_str();
        self.workspaces = list_workspaces(&mut conn).await?;
        self.projects = list_project_items_in_workspace(&mut conn, workspace_id).await?;
        self.labels = list_labels_in_workspace(&mut conn, workspace_id, None).await?;
        self.counts = sidebar_counts_in_workspace(&mut conn, workspace_id).await?;
        self.tasks = list_task_items_in_workspace(
            &mut conn,
            workspace_id,
            self.filters.clone(),
            self.sort,
            self.sort_direction,
        )
        .await?;
        self.rebuild_sidebar();
        self.last_refresh = Instant::now();
        Ok(self.restored_task_selection(selected_id))
    }
}
