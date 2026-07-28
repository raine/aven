use anyhow::Result;
use chrono::{DateTime, Utc};

use crate::db::Database;
use crate::ids::{TaskId, WorkspaceId};
use crate::recurrence::RecurrenceSeriesId;
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
mod recurrence;
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
pub(crate) use recurrence::task_recurrence_summaries;
pub use search::{
    SearchMatchedField, TaskSearchPreviewResultSet, TaskSearchQuery, TaskSearchResult,
};
pub(crate) use search::{
    search_task_items_in_workspace, search_task_occurrence_items_in_workspace,
    search_task_preview_set_in_workspace,
};
pub(crate) use sidebar::sidebar_counts_for_scope_in_workspace;
pub use sync_history::SyncHistoryStats;
pub(crate) use sync_history::sync_history_stats;
pub(crate) use tasks::{list_task_items_in_workspace, list_task_items_with_display_refs};
pub use types::RecentActionTarget;
pub use types::{
    AttachmentMetadata, ProjectListItem, RecentActionItem, RecurrenceCounts,
    RecurrenceHistoryEntry, RecurrenceHistoryKind, RecurrenceHistoryPage, RecurrenceOccurrenceLink,
    RecurrenceReconciliation, RecurrenceSeriesConflict, RecurrenceSeriesDetail,
    RecurrenceSeriesLifecycleFilter, RecurrenceSeriesListItem, RecurrenceSeriesListQuery,
    RecurrenceSeriesSummary, RecurrenceTaskGroup, SidebarCounts, SortDirection,
    TaskAvailabilityFilter, TaskDependencyLink, TaskFilters, TaskListItem, TaskNote, TaskQueryMode,
    TaskRecurrenceSummary, TaskSort,
};

impl Database {
    /// Reconciles recurrence projections before a report reads current state.
    ///
    /// The candidate count is bounded. Each changed series writes one atomic projection
    /// transaction that can archive one superseded task and materialize one successor.
    pub async fn reconcile_recurrence_reports_at(
        &self,
        workspace_id: &WorkspaceId,
        at: DateTime<Utc>,
    ) -> Result<RecurrenceReconciliation> {
        let (workspace, candidates, incomplete) = {
            let mut conn = self.acquire().await?;
            let workspace = crate::workspaces::workspace_for_id(&mut conn, workspace_id).await?;
            let (candidates, incomplete) =
                recurrence::recurrence_reconciliation_candidates(&mut conn, workspace_id, at)
                    .await?;
            (workspace, candidates, incomplete)
        };
        let mut report = RecurrenceReconciliation {
            workspace_id: Some(workspace_id.clone()),
            examined: candidates.len(),
            incomplete,
            ..RecurrenceReconciliation::default()
        };
        for series_id in candidates {
            let result = self
                .reconcile_recurrence_series(&workspace, &series_id, at)
                .await?;
            report.changed += usize::from(result.changed);
            report.lifecycle_blocked += usize::from(result.lifecycle_blocked);
        }
        recurrence::validate_reconciliation(&report)?;
        Ok(report)
    }

    pub async fn reconcile_recurrence_reports(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<RecurrenceReconciliation> {
        let at = DateTime::parse_from_rfc3339(&crate::ids::now())?.with_timezone(&Utc);
        self.reconcile_recurrence_reports_at(workspace_id, at).await
    }

    pub async fn list_recurrence_series_view_at(
        &self,
        workspace_id: &WorkspaceId,
        at: DateTime<Utc>,
        query: RecurrenceSeriesListQuery,
    ) -> Result<Vec<RecurrenceSeriesListItem>> {
        self.reconcile_recurrence_reports_at(workspace_id, at)
            .await?;
        let mut conn = self.acquire().await?;
        recurrence::list_recurrence_series_view(&mut conn, workspace_id, &query).await
    }

    pub async fn list_recurrence_series_view(
        &self,
        workspace_id: &WorkspaceId,
        query: RecurrenceSeriesListQuery,
    ) -> Result<Vec<RecurrenceSeriesListItem>> {
        let at = DateTime::parse_from_rfc3339(&crate::ids::now())?.with_timezone(&Utc);
        self.list_recurrence_series_view_at(workspace_id, at, query)
            .await
    }

    pub async fn list_recurrence_series_at(
        &self,
        workspace_id: &WorkspaceId,
        at: DateTime<Utc>,
    ) -> Result<Vec<RecurrenceSeriesSummary>> {
        self.reconcile_recurrence_reports_at(workspace_id, at)
            .await?;
        let mut conn = self.acquire().await?;
        recurrence::list_recurrence_series(&mut conn, workspace_id, at).await
    }

    pub async fn recurrence_series_detail_at(
        &self,
        workspace_id: &WorkspaceId,
        series_id: &RecurrenceSeriesId,
        at: DateTime<Utc>,
    ) -> Result<RecurrenceSeriesDetail> {
        self.reconcile_recurrence_reports_at(workspace_id, at)
            .await?;
        let mut conn = self.acquire().await?;
        recurrence::recurrence_series_detail(&mut conn, workspace_id, series_id, at).await
    }

    pub async fn recurrence_history_at(
        &self,
        workspace_id: &WorkspaceId,
        series_id: &RecurrenceSeriesId,
        at: DateTime<Utc>,
        offset: usize,
        limit: usize,
    ) -> Result<RecurrenceHistoryPage> {
        self.reconcile_recurrence_reports_at(workspace_id, at)
            .await?;
        let mut conn = self.acquire().await?;
        recurrence::recurrence_history(&mut conn, workspace_id, series_id, at, offset, limit).await
    }

    pub async fn list_recurrence_series(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<Vec<RecurrenceSeriesSummary>> {
        let at = DateTime::parse_from_rfc3339(&crate::ids::now())?.with_timezone(&Utc);
        self.list_recurrence_series_at(workspace_id, at).await
    }

    pub async fn recurrence_series_detail(
        &self,
        workspace_id: &WorkspaceId,
        series_id: &RecurrenceSeriesId,
    ) -> Result<RecurrenceSeriesDetail> {
        let at = DateTime::parse_from_rfc3339(&crate::ids::now())?.with_timezone(&Utc);
        self.recurrence_series_detail_at(workspace_id, series_id, at)
            .await
    }

    pub async fn recurrence_history(
        &self,
        workspace_id: &WorkspaceId,
        series_id: &RecurrenceSeriesId,
        offset: usize,
        limit: usize,
    ) -> Result<RecurrenceHistoryPage> {
        let at = DateTime::parse_from_rfc3339(&crate::ids::now())?.with_timezone(&Utc);
        self.recurrence_history_at(workspace_id, series_id, at, offset, limit)
            .await
    }

    pub async fn list_project_items(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<Vec<ProjectListItem>> {
        self.reconcile_recurrence_reports(workspace_id).await?;
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
        self.reconcile_recurrence_reports(workspace_id).await?;
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
        _display_refs: &DisplayRefContext,
    ) -> Result<Vec<TaskListItem>> {
        self.reconcile_recurrence_reports(workspace_id).await?;
        let mut conn = self.acquire().await?;
        let refreshed_display_refs =
            DisplayRefContext::for_workspace(&mut conn, workspace_id).await?;
        list_task_items_with_display_refs(
            &mut conn,
            workspace_id,
            filters,
            mode,
            sort,
            direction,
            &refreshed_display_refs,
        )
        .await
    }

    pub async fn sidebar_counts_for_scope(
        &self,
        workspace_id: &WorkspaceId,
        project_key: Option<&str>,
    ) -> Result<SidebarCounts> {
        self.reconcile_recurrence_reports(workspace_id).await?;
        let mut conn = self.acquire().await?;
        sidebar_counts_for_scope_in_workspace(&mut conn, workspace_id, project_key).await
    }

    pub async fn list_recent_actions(
        &self,
        workspace_id: &WorkspaceId,
        project_scope: Option<&str>,
    ) -> Result<Vec<RecentActionItem>> {
        self.reconcile_recurrence_reports(workspace_id).await?;
        let mut conn = self.acquire().await?;
        list_recent_actions_in_workspace(&mut conn, workspace_id, project_scope).await
    }

    pub async fn search_task_items(
        &self,
        workspace_id: &WorkspaceId,
        query: TaskSearchQuery,
    ) -> Result<Vec<TaskSearchResult>> {
        self.reconcile_recurrence_reports(workspace_id).await?;
        let mut conn = self.acquire().await?;
        search_task_items_in_workspace(&mut conn, workspace_id, query).await
    }

    pub async fn search_task_occurrence_items(
        &self,
        workspace_id: &WorkspaceId,
        query: TaskSearchQuery,
    ) -> Result<Vec<TaskSearchResult>> {
        self.reconcile_recurrence_reports(workspace_id).await?;
        let mut conn = self.acquire().await?;
        search_task_occurrence_items_in_workspace(&mut conn, workspace_id, query).await
    }

    pub async fn search_task_preview_set(
        &self,
        workspace_id: &WorkspaceId,
        query: TaskSearchQuery,
    ) -> Result<TaskSearchPreviewResultSet> {
        self.reconcile_recurrence_reports(workspace_id).await?;
        let mut conn = self.acquire().await?;
        search_task_preview_set_in_workspace(&mut conn, workspace_id, query).await
    }

    pub async fn task_detail(&self, task: &Task) -> Result<TaskDetail> {
        self.reconcile_recurrence_reports(&task.workspace_id)
            .await?;
        let mut conn = self.acquire().await?;
        task_detail(&mut conn, task).await
    }

    pub async fn task_detail_with_display_refs(
        &self,
        task: &Task,
        display_refs: &DisplayRefContext,
    ) -> Result<TaskDetail> {
        self.reconcile_recurrence_reports(&task.workspace_id)
            .await?;
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
