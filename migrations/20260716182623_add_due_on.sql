ALTER TABLE tasks ADD COLUMN due_on TEXT NOT NULL DEFAULT '';

CREATE INDEX IF NOT EXISTS idx_tasks_workspace_due_on
ON tasks(workspace_id, due_on)
WHERE due_on != '';
