CREATE INDEX idx_notes_workspace_task_created
ON notes(workspace_id, task_id, created_at DESC, id DESC);
