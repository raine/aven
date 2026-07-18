use anyhow::Result;

use crate::db::Database;
use crate::ids::{TaskId, WorkspaceId};
use crate::refs::DisplayRefContext;
use crate::types::Task;

mod database_stats;
mod dependencies;
mod details;
mod doctor;
pub mod fragments;
mod hydration;
mod projects;
mod recent_actions;
mod search;
mod sidebar;
mod sorting;
mod sync_history;
mod tasks;
#[cfg(test)]
mod test_support;
mod types;

pub use database_stats::{DatabaseStats, DatabaseStatsPriorityCounts, DatabaseStatsStatusCounts};
pub use dependencies::{TaskDependencyItem, TaskDependencySummary};
pub(crate) use dependencies::{task_dependency_summary, task_dependency_summary_with_display_refs};
pub use details::{TaskDetail, TaskDetailConflict};
pub(crate) use details::{conflict_display_value, task_detail, task_detail_with_display_refs};
pub use doctor::WorkspaceTaskCounts;
pub(crate) use doctor::{unresolved_conflict_count, workspace_task_counts};
pub(crate) use projects::list_project_items_in_workspace;
pub(crate) use recent_actions::list_recent_actions_in_workspace;
pub use search::{
    SearchMatchedField, TaskSearchPreviewResultSet, TaskSearchQuery, TaskSearchResult,
};
pub(crate) use search::{search_task_items_in_workspace, search_task_preview_set_in_workspace};
pub(crate) use sidebar::sidebar_counts_for_scope_in_workspace;
pub use sync_history::SyncHistoryStats;
pub(crate) use sync_history::sync_history_stats;
pub(crate) use tasks::{list_task_items_in_workspace, list_task_items_with_display_refs};
pub use types::RecentActionTarget;
pub use types::{
    ProjectListItem, RecentActionItem, SidebarCounts, SortDirection, TaskAvailabilityFilter,
    TaskDependencyLink, TaskFilters, TaskListItem, TaskNote, TaskQueryMode, TaskSort,
};

impl Database {
    pub async fn list_project_items(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<Vec<ProjectListItem>> {
        let mut conn = self.acquire().await?;
        list_project_items_in_workspace(&mut conn, workspace_id).await
    }

    pub async fn list_task_items(
        &self,
        workspace_id: &WorkspaceId,
        filters: TaskFilters,
        mode: TaskQueryMode,
        sort: TaskSort,
        direction: SortDirection,
    ) -> Result<Vec<TaskListItem>> {
        let mut conn = self.acquire().await?;
        list_task_items_in_workspace(&mut conn, workspace_id, filters, mode, sort, direction).await
    }

    pub async fn list_task_items_with_display_refs(
        &self,
        workspace_id: &WorkspaceId,
        filters: TaskFilters,
        mode: TaskQueryMode,
        sort: TaskSort,
        direction: SortDirection,
        display_refs: &DisplayRefContext,
    ) -> Result<Vec<TaskListItem>> {
        let mut conn = self.acquire().await?;
        list_task_items_with_display_refs(
            &mut conn,
            workspace_id,
            filters,
            mode,
            sort,
            direction,
            display_refs,
        )
        .await
    }

    pub async fn sidebar_counts_for_scope(
        &self,
        workspace_id: &WorkspaceId,
        project_key: Option<&str>,
    ) -> Result<SidebarCounts> {
        let mut conn = self.acquire().await?;
        sidebar_counts_for_scope_in_workspace(&mut conn, workspace_id, project_key).await
    }

    pub async fn list_recent_actions(
        &self,
        workspace_id: &WorkspaceId,
        project_scope: Option<&str>,
    ) -> Result<Vec<RecentActionItem>> {
        let mut conn = self.acquire().await?;
        list_recent_actions_in_workspace(&mut conn, workspace_id, project_scope).await
    }

    pub async fn search_task_items(
        &self,
        workspace_id: &WorkspaceId,
        query: TaskSearchQuery,
    ) -> Result<Vec<TaskSearchResult>> {
        let mut conn = self.acquire().await?;
        search_task_items_in_workspace(&mut conn, workspace_id, query).await
    }

    pub async fn search_task_preview_set(
        &self,
        workspace_id: &WorkspaceId,
        query: TaskSearchQuery,
    ) -> Result<TaskSearchPreviewResultSet> {
        let mut conn = self.acquire().await?;
        search_task_preview_set_in_workspace(&mut conn, workspace_id, query).await
    }

    pub async fn task_detail(&self, task: &Task) -> Result<TaskDetail> {
        let mut conn = self.acquire().await?;
        task_detail(&mut conn, task).await
    }

    pub async fn task_detail_with_display_refs(
        &self,
        task: &Task,
        display_refs: &DisplayRefContext,
    ) -> Result<TaskDetail> {
        let mut conn = self.acquire().await?;
        task_detail_with_display_refs(&mut conn, task, display_refs).await
    }

    pub async fn conflict_display_value(
        &self,
        workspace_id: &WorkspaceId,
        field: &str,
        value: &str,
    ) -> Result<String> {
        let mut conn = self.acquire().await?;
        conflict_display_value(&mut conn, workspace_id, field, value).await
    }

    pub async fn task_dependency_summary(
        &self,
        workspace_id: &WorkspaceId,
        task_id: &TaskId,
    ) -> Result<TaskDependencySummary> {
        let mut conn = self.acquire().await?;
        task_dependency_summary(&mut conn, workspace_id, task_id).await
    }

    pub async fn workspace_task_counts(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<WorkspaceTaskCounts> {
        let mut conn = self.acquire().await?;
        workspace_task_counts(&mut conn, workspace_id).await
    }

    pub async fn unresolved_conflict_count(&self) -> Result<i64> {
        let mut conn = self.acquire().await?;
        unresolved_conflict_count(&mut conn).await
    }

    pub async fn sync_history_stats(&self) -> Result<SyncHistoryStats> {
        let mut conn = self.acquire().await?;
        sync_history_stats(&mut conn).await
    }

    pub async fn database_stats(
        &self,
        workspace: &crate::workspaces::Workspace,
    ) -> Result<DatabaseStats> {
        let mut conn = self.acquire().await?;
        database_stats::database_stats(&mut conn, workspace).await
    }
}
