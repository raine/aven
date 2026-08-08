CREATE TABLE metadata_fields (
    id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    key TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (workspace_id, key)
);

CREATE TABLE metadata_field_id_aliases (
    workspace_id TEXT NOT NULL,
    remote_field_id TEXT NOT NULL,
    local_field_id TEXT NOT NULL,
    PRIMARY KEY (workspace_id, remote_field_id)
);

CREATE INDEX idx_metadata_field_id_aliases_workspace_local
ON metadata_field_id_aliases(workspace_id, local_field_id);

CREATE TABLE task_metadata (
    workspace_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    field_id TEXT NOT NULL,
    value TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, task_id, field_id)
);

CREATE INDEX idx_task_metadata_workspace_field_value_task
ON task_metadata(workspace_id, field_id, value, task_id);

CREATE TABLE recurrence_series_metadata (
    workspace_id TEXT NOT NULL,
    series_id TEXT NOT NULL,
    field_id TEXT NOT NULL,
    value TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, series_id, field_id)
);

CREATE INDEX idx_recurrence_series_metadata_workspace_field_series
ON recurrence_series_metadata(workspace_id, field_id, series_id);

CREATE TABLE field_versions_new (
    workspace_id TEXT NOT NULL,
    entity_type TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    field TEXT NOT NULL,
    version TEXT NOT NULL,
    PRIMARY KEY (workspace_id, entity_type, entity_id, field),
    CHECK (entity_type IN ('task', 'recurrence_series', 'metadata_field')),
    CHECK (
        entity_type != 'recurrence_series'
        OR field LIKE 'metadata:%'
        OR field IN (
            'title', 'description', 'project', 'priority', 'initial_status', 'labels',
            'available_local_time', 'due_policy', 'state', 'stopped_at', 'deleted'
        )
    ),
    CHECK (entity_type != 'metadata_field' OR field = 'key')
);

INSERT INTO field_versions_new(workspace_id, entity_type, entity_id, field, version)
SELECT workspace_id, entity_type, entity_id, field, version
FROM field_versions;

DROP TABLE field_versions;
ALTER TABLE field_versions_new RENAME TO field_versions;

CREATE TABLE conflicts_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace_id TEXT NOT NULL DEFAULT '0000000000000000',
    entity_type TEXT NOT NULL DEFAULT 'task',
    entity_id TEXT NOT NULL DEFAULT '',
    task_id TEXT NOT NULL DEFAULT '',
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
    UNIQUE (workspace_id, entity_type, entity_id, field, remote_change_id),
    CHECK (entity_type IN ('task', 'recurrence_series', 'metadata_field')),
    CHECK (
        (entity_type = 'task' AND (entity_id = '' OR task_id = entity_id))
        OR (entity_type != 'task' AND task_id = '')
    ),
    CHECK (resolved IN (0, 1))
);

INSERT INTO conflicts_new(
    id, workspace_id, entity_type, entity_id, task_id, field, base_version,
    local_value, remote_value, local_change_id, remote_change_id, variant_a,
    variant_b, created_at, resolved
)
SELECT
    id, workspace_id, entity_type, entity_id, task_id, field, base_version,
    local_value, remote_value, local_change_id, remote_change_id, variant_a,
    variant_b, created_at, resolved
FROM conflicts;

DROP TABLE conflicts;
ALTER TABLE conflicts_new RENAME TO conflicts;

CREATE TRIGGER conflicts_task_identity_compat
AFTER INSERT ON conflicts
WHEN NEW.entity_type = 'task' AND NEW.entity_id = ''
BEGIN
    UPDATE conflicts SET entity_id = NEW.task_id WHERE id = NEW.id;
END;

CREATE INDEX idx_conflicts_workspace_task
ON conflicts(workspace_id, task_id, resolved)
WHERE entity_type = 'task';

CREATE INDEX idx_conflicts_workspace_resolved_created_task
ON conflicts(workspace_id, resolved, created_at, task_id);

CREATE INDEX idx_conflicts_workspace_resolved_task
ON conflicts(workspace_id, resolved, task_id);

CREATE INDEX idx_conflicts_recurrence_lifecycle
ON conflicts(workspace_id, entity_type, entity_id, field, resolved)
WHERE entity_type = 'recurrence_series';

CREATE INDEX idx_conflicts_metadata_field_lifecycle
ON conflicts(workspace_id, entity_type, entity_id, field, resolved)
WHERE entity_type = 'metadata_field';
