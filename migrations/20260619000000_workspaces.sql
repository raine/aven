CREATE TABLE IF NOT EXISTS workspaces (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    key TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    archived INTEGER NOT NULL DEFAULT 0
);

INSERT OR IGNORE INTO workspaces(id, name, key, created_at, updated_at)
VALUES ('0000000000000000', 'default', 'default', '1970-01-01T00:00:00Z', '1970-01-01T00:00:00Z');

CREATE TABLE projects_new (
    workspace_id TEXT NOT NULL DEFAULT '0000000000000000',
    key TEXT NOT NULL,
    name TEXT NOT NULL,
    prefix TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    deleted INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (workspace_id, key),
    UNIQUE (workspace_id, prefix)
);

INSERT INTO projects_new(workspace_id, key, name, prefix, created_at, updated_at, deleted)
SELECT '0000000000000000', key, name, prefix, created_at, updated_at, deleted FROM projects;

DROP TABLE projects;
ALTER TABLE projects_new RENAME TO projects;

CREATE TABLE project_paths_new (
    workspace_id TEXT NOT NULL DEFAULT '0000000000000000',
    project_key TEXT NOT NULL,
    path TEXT NOT NULL,
    PRIMARY KEY (workspace_id, project_key, path),
    UNIQUE (workspace_id, path)
);

INSERT INTO project_paths_new(workspace_id, project_key, path)
SELECT '0000000000000000', project_key, path FROM project_paths;

DROP TABLE project_paths;
ALTER TABLE project_paths_new RENAME TO project_paths;

CREATE TABLE labels_new (
    workspace_id TEXT NOT NULL DEFAULT '0000000000000000',
    name TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, name)
);

INSERT INTO labels_new(workspace_id, name, created_at)
SELECT '0000000000000000', name, created_at FROM labels;

DROP TABLE labels;
ALTER TABLE labels_new RENAME TO labels;

ALTER TABLE tasks ADD COLUMN workspace_id TEXT NOT NULL DEFAULT '0000000000000000';

CREATE TABLE task_labels_new (
    workspace_id TEXT NOT NULL DEFAULT '0000000000000000',
    task_id TEXT NOT NULL,
    label TEXT NOT NULL,
    PRIMARY KEY (workspace_id, task_id, label)
);

INSERT INTO task_labels_new(workspace_id, task_id, label)
SELECT '0000000000000000', task_id, label FROM task_labels;

DROP TABLE task_labels;
ALTER TABLE task_labels_new RENAME TO task_labels;

ALTER TABLE notes ADD COLUMN workspace_id TEXT NOT NULL DEFAULT '0000000000000000';

CREATE TABLE conflicts_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace_id TEXT NOT NULL DEFAULT '0000000000000000',
    task_id TEXT NOT NULL,
    field TEXT NOT NULL,
    base_version TEXT,
    local_value TEXT NOT NULL,
    remote_value TEXT NOT NULL,
    local_change_id TEXT,
    remote_change_id TEXT NOT NULL,
    variant_a TEXT NOT NULL,
    variant_b TEXT NOT NULL,
    created_at TEXT NOT NULL,
    resolved INTEGER NOT NULL DEFAULT 0,
    UNIQUE (workspace_id, task_id, field, remote_change_id)
);

INSERT INTO conflicts_new(
    workspace_id, task_id, field, base_version, local_value, remote_value,
    local_change_id, remote_change_id, variant_a, variant_b, created_at, resolved
)
SELECT
    '0000000000000000', task_id, field, base_version, local_value, remote_value,
    local_change_id, remote_change_id, variant_a, variant_b, created_at, resolved
FROM conflicts;

DROP TABLE conflicts;
ALTER TABLE conflicts_new RENAME TO conflicts;

CREATE INDEX IF NOT EXISTS idx_tasks_workspace_project ON tasks(workspace_id, project_key);
CREATE INDEX IF NOT EXISTS idx_task_labels_workspace_task ON task_labels(workspace_id, task_id);
CREATE INDEX IF NOT EXISTS idx_conflicts_workspace_task ON conflicts(workspace_id, task_id, resolved);
CREATE INDEX IF NOT EXISTS idx_project_paths_workspace_path ON project_paths(workspace_id, path);
