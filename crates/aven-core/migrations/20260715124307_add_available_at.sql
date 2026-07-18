ALTER TABLE tasks ADD COLUMN available_at TEXT NOT NULL DEFAULT '';

CREATE INDEX IF NOT EXISTS idx_tasks_workspace_available_at_upcoming
ON tasks(workspace_id, available_at)
WHERE available_at != '';
