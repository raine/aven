use anyhow::Result;

use super::TuiStore;
use super::types::{DatabaseStatsPriorityCounts, DatabaseStatsStatusCounts, TuiDatabaseStats};

impl TuiStore {
    pub(crate) async fn load_database_stats(&mut self) -> Result<()> {
        let stats = self.database.database_stats(&self.active_workspace).await?;
        self.db_stats = TuiDatabaseStats {
            workspace_name: stats.workspace_name,
            workspace_key: stats.workspace_key,
            total_tasks: stats.total_tasks,
            open_tasks: stats.open_tasks,
            deleted_tasks: stats.deleted_tasks,
            statuses: DatabaseStatsStatusCounts {
                inbox: stats.statuses.inbox,
                backlog: stats.statuses.backlog,
                todo: stats.statuses.todo,
                active: stats.statuses.active,
                done: stats.statuses.done,
                canceled: stats.statuses.canceled,
            },
            priorities: DatabaseStatsPriorityCounts {
                none: stats.priorities.none,
                low: stats.priorities.low,
                medium: stats.priorities.medium,
                high: stats.priorities.high,
                urgent: stats.priorities.urgent,
            },
            projects: stats.projects,
            labels: stats.labels,
            notes: stats.notes,
            task_labels: stats.task_labels,
            sync_history: stats.sync_history,
            conflicts: stats.conflicts,
            sqlite_page_size: stats.sqlite_page_size,
            sqlite_page_count: stats.sqlite_page_count,
            sqlite_freelist_count: stats.sqlite_freelist_count,
            latest_created_at: stats.latest_created_at,
            latest_updated_at: stats.latest_updated_at,
        };
        Ok(())
    }
}
