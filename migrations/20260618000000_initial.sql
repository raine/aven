CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS projects (
    key TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    prefix TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    deleted INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS project_paths (
    project_key TEXT NOT NULL,
    path TEXT NOT NULL UNIQUE,
    PRIMARY KEY (project_key, path)
);

CREATE TABLE IF NOT EXISTS labels (
    name TEXT PRIMARY KEY,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS tasks (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    project_key TEXT NOT NULL,
    status TEXT NOT NULL,
    priority TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    deleted INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS task_labels (
    task_id TEXT NOT NULL,
    label TEXT NOT NULL,
    PRIMARY KEY (task_id, label)
);

CREATE TABLE IF NOT EXISTS notes (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    body TEXT NOT NULL,
    created_at TEXT NOT NULL,
    change_id TEXT NOT NULL UNIQUE
);

CREATE TABLE IF NOT EXISTS changes (
    change_id TEXT PRIMARY KEY,
    client_id TEXT NOT NULL,
    local_seq INTEGER NOT NULL,
    entity_type TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    field TEXT,
    op_type TEXT NOT NULL,
    payload TEXT NOT NULL,
    base_version TEXT,
    created_at TEXT NOT NULL,
    server_seq INTEGER
);

CREATE TABLE IF NOT EXISTS field_versions (
    entity_id TEXT NOT NULL,
    field TEXT NOT NULL,
    version TEXT NOT NULL,
    PRIMARY KEY (entity_id, field)
);

CREATE TABLE IF NOT EXISTS conflicts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
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
    UNIQUE (task_id, field, remote_change_id)
);

CREATE INDEX IF NOT EXISTS idx_changes_server_seq ON changes(server_seq);
CREATE INDEX IF NOT EXISTS idx_tasks_project ON tasks(project_key);
