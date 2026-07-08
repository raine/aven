use crate::ids::{ProjectId, TaskId, WorkspaceId};

use crate::choices::{TaskPriority, TaskStatus};

#[derive(Debug, Clone)]
pub(crate) struct Task {
    pub(crate) id: TaskId,
    pub(crate) workspace_id: WorkspaceId,
    pub(crate) title: String,
    pub(crate) description: String,
    pub(crate) project_id: ProjectId,
    pub(crate) project_key: String,
    pub(crate) project_prefix: String,
    pub(crate) status: TaskStatus,
    pub(crate) priority: TaskPriority,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
    pub(crate) queue_activity_at: String,
    pub(crate) available_at: Option<String>,
    pub(crate) due_on: Option<String>,
    pub(crate) deleted: bool,
    pub(crate) is_epic: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct Project {
    pub(crate) id: ProjectId,
    pub(crate) workspace_id: WorkspaceId,
    pub(crate) key: String,
    pub(crate) name: String,
    pub(crate) prefix: String,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct TaskAttachment {
    pub(crate) workspace_id: String,
    pub(crate) attachment_id: String,
    pub(crate) task_id: String,
    pub(crate) sha256: String,
    pub(crate) byte_size: i64,
    pub(crate) media_type: String,
    pub(crate) filename: Option<String>,
    pub(crate) alt_text: Option<String>,
    pub(crate) width: Option<i64>,
    pub(crate) height: Option<i64>,
    pub(crate) created_at: String,
    pub(crate) created_by_change_id: Option<String>,
    pub(crate) deleted: bool,
    pub(crate) deleted_at: Option<String>,
    pub(crate) deleted_by_change_id: Option<String>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct BlobInventoryRow {
    pub(crate) sha256: String,
    pub(crate) byte_size: i64,
    pub(crate) media_type: String,
    pub(crate) available: bool,
    pub(crate) first_seen_at: String,
    pub(crate) last_verified_at: Option<String>,
}
