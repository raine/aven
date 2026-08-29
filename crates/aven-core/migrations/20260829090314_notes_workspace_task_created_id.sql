CREATE INDEX idx_notes_workspace_task_created_id
ON notes(workspace_id, task_id, created_at DESC, id DESC);
