CREATE INDEX IF NOT EXISTS idx_tasks_workspace_deleted_updated
ON tasks(workspace_id, deleted, updated_at DESC, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_tasks_workspace_deleted_status_updated
ON tasks(workspace_id, deleted, status, updated_at DESC, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_tasks_workspace_deleted_priority_updated
ON tasks(workspace_id, deleted, priority, updated_at DESC, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_tasks_workspace_project_deleted_updated
ON tasks(workspace_id, project_key, deleted, updated_at DESC, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_tasks_workspace_project_deleted_status
ON tasks(workspace_id, project_key, deleted, status);

CREATE INDEX IF NOT EXISTS idx_conflicts_workspace_resolved_created_task
ON conflicts(workspace_id, resolved, created_at, task_id);

CREATE INDEX IF NOT EXISTS idx_conflicts_workspace_resolved_task
ON conflicts(workspace_id, resolved, task_id);
