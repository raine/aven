use crate::ids::{ProjectId, TaskId, WorkspaceId};

use crate::choices::{TaskPriority, TaskStatus};

#[derive(Debug, Clone)]
pub struct Task {
    pub id: TaskId,
    pub workspace_id: WorkspaceId,
    pub title: String,
    pub description: String,
    pub project_id: ProjectId,
    pub project_key: String,
    pub project_prefix: String,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    pub created_at: String,
    pub updated_at: String,
    pub queue_activity_at: String,
    pub available_at: Option<String>,
    pub due_on: Option<String>,
    pub deleted: bool,
    pub is_epic: bool,
}

#[derive(Debug, Clone)]
pub struct Project {
    pub id: ProjectId,
    pub workspace_id: WorkspaceId,
    pub key: String,
    pub name: String,
    pub prefix: String,
}
