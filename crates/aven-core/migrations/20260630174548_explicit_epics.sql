ALTER TABLE tasks
ADD COLUMN is_epic INTEGER NOT NULL DEFAULT 0
CHECK (is_epic IN (0, 1));

CREATE TABLE task_epic_links (
    workspace_id TEXT NOT NULL,
    epic_task_id TEXT NOT NULL,
    child_task_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, child_task_id),
    CHECK (epic_task_id != child_task_id)
);

CREATE INDEX idx_task_epic_links_workspace_epic
ON task_epic_links(workspace_id, epic_task_id, child_task_id);
