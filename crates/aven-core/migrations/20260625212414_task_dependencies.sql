CREATE TABLE IF NOT EXISTS task_dependencies (
    workspace_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    depends_on_task_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, task_id, depends_on_task_id),
    CHECK (task_id != depends_on_task_id)
);

CREATE INDEX IF NOT EXISTS idx_task_dependencies_workspace_task
ON task_dependencies(workspace_id, task_id, depends_on_task_id);

CREATE INDEX IF NOT EXISTS idx_task_dependencies_workspace_depends_on
ON task_dependencies(workspace_id, depends_on_task_id, task_id);
