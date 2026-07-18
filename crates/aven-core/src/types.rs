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

#[derive(Debug, Clone)]
pub struct TaskAttachment {
    pub workspace_id: WorkspaceId,
    pub attachment_id: String,
    pub task_id: TaskId,
    pub sha256: String,
    pub byte_size: i64,
    pub media_type: String,
    pub filename: Option<String>,
    pub alt_text: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub created_at: String,
    pub created_by_change_id: Option<String>,
    pub deleted: bool,
    pub deleted_at: Option<String>,
    pub deleted_by_change_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BlobInventoryRow {
    pub sha256: String,
    pub byte_size: i64,
    pub media_type: String,
    pub available: bool,
    pub first_seen_at: String,
    pub last_verified_at: Option<String>,
}
