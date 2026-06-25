CREATE TABLE projects_new (
    id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL DEFAULT '0000000000000000',
    key TEXT NOT NULL,
    name TEXT NOT NULL,
    prefix TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    deleted INTEGER NOT NULL DEFAULT 0,
    UNIQUE (workspace_id, key),
    UNIQUE (workspace_id, prefix)
);

CREATE TEMP TABLE project_id_map AS
WITH RECURSIVE referenced_projects(workspace_id, key) AS (
    SELECT workspace_id, key FROM projects
    UNION
    SELECT workspace_id, project_key FROM tasks
    UNION
    SELECT
        COALESCE(json_extract(payload, '$.workspace_id'), '0000000000000000'),
        entity_id
    FROM changes
    WHERE op_type = 'create_project' AND entity_type = 'project'
    UNION
    SELECT
        COALESCE(json_extract(payload, '$.workspace_id'), '0000000000000000'),
        json_extract(payload, '$.project_key')
    FROM changes
    WHERE op_type = 'create_task'
      AND entity_type = 'task'
      AND json_extract(payload, '$.project_key') IS NOT NULL
    UNION
    SELECT
        COALESCE(json_extract(payload, '$.workspace_id'), '0000000000000000'),
        COALESCE(json_extract(payload, '$.project_key'), json_extract(payload, '$.value'))
    FROM changes
    WHERE op_type IN ('set_field', 'resolve_field')
      AND entity_type = 'task'
      AND field = 'project'
      AND COALESCE(json_extract(payload, '$.project_key'), json_extract(payload, '$.value')) IS NOT NULL
),
hashes(workspace_id, key, input, pos, h1, h2) AS (
    SELECT
        workspace_id,
        key,
        'aven/project/v1/' || workspace_id || '/' || key,
        1,
        1,
        7
    FROM referenced_projects
    UNION ALL
    SELECT
        workspace_id,
        key,
        input,
        pos + 1,
        ((h1 * 131) + unicode(substr(input, pos, 1))) % 2147483647,
        ((h2 * 137) + unicode(substr(input, pos, 1))) % 2147483647
    FROM hashes
    WHERE pos <= length(input)
)
SELECT
    workspace_id,
    key,
    printf('%08X%08X', h1, h2) AS id
FROM hashes
WHERE pos > length(input);

INSERT INTO projects_new(id, workspace_id, key, name, prefix, created_at, updated_at, deleted)
SELECT
    m.id,
    m.workspace_id,
    m.key,
    COALESCE(p.name, (
        SELECT json_extract(c.payload, '$.name')
        FROM changes c
        WHERE c.op_type = 'create_project'
          AND c.entity_type = 'project'
          AND c.entity_id = m.key
          AND COALESCE(json_extract(c.payload, '$.workspace_id'), '0000000000000000') = m.workspace_id
        ORDER BY c.local_seq LIMIT 1
    ), m.key),
    COALESCE(p.prefix, (
        SELECT json_extract(c.payload, '$.prefix')
        FROM changes c
        WHERE c.op_type = 'create_project'
          AND c.entity_type = 'project'
          AND c.entity_id = m.key
          AND COALESCE(json_extract(c.payload, '$.workspace_id'), '0000000000000000') = m.workspace_id
        ORDER BY c.local_seq LIMIT 1
    ), m.id),
    COALESCE(p.created_at, (
        SELECT json_extract(c.payload, '$.created_at')
        FROM changes c
        WHERE c.op_type = 'create_project'
          AND c.entity_type = 'project'
          AND c.entity_id = m.key
          AND COALESCE(json_extract(c.payload, '$.workspace_id'), '0000000000000000') = m.workspace_id
        ORDER BY c.local_seq LIMIT 1
    ), '1970-01-01T00:00:00Z'),
    COALESCE(p.updated_at, (
        SELECT json_extract(c.payload, '$.created_at')
        FROM changes c
        WHERE c.op_type = 'create_project'
          AND c.entity_type = 'project'
          AND c.entity_id = m.key
          AND COALESCE(json_extract(c.payload, '$.workspace_id'), '0000000000000000') = m.workspace_id
        ORDER BY c.local_seq LIMIT 1
    ), '1970-01-01T00:00:00Z'),
    COALESCE(p.deleted, 1)
FROM project_id_map m
LEFT JOIN projects p ON p.workspace_id = m.workspace_id AND p.key = m.key;

CREATE TABLE tasks_new (
    workspace_id TEXT NOT NULL DEFAULT '0000000000000000',
    id TEXT NOT NULL PRIMARY KEY,
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    project_id TEXT NOT NULL,
    status TEXT NOT NULL,
    priority TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    queue_activity_at TEXT NOT NULL DEFAULT '1970-01-01T00:00:00Z',
    deleted INTEGER NOT NULL DEFAULT 0
);

INSERT INTO tasks_new(
    workspace_id, id, title, description, project_id, status, priority,
    created_at, updated_at, queue_activity_at, deleted
)
SELECT
    t.workspace_id, t.id, t.title, t.description, p.id, t.status, t.priority,
    t.created_at, t.updated_at, t.queue_activity_at, t.deleted
FROM tasks t
JOIN projects_new p ON p.workspace_id = t.workspace_id AND p.key = t.project_key;

CREATE TABLE project_paths_new (
    workspace_id TEXT NOT NULL DEFAULT '0000000000000000',
    project_id TEXT NOT NULL,
    path TEXT NOT NULL,
    PRIMARY KEY (workspace_id, project_id, path),
    UNIQUE (workspace_id, path)
);

INSERT INTO project_paths_new(workspace_id, project_id, path)
SELECT pp.workspace_id, p.id, pp.path
FROM project_paths pp
JOIN projects_new p ON p.workspace_id = pp.workspace_id AND p.key = pp.project_key
WHERE p.deleted = 0;

UPDATE changes
SET entity_id = (
    SELECT p.id
    FROM projects_new p
    WHERE p.workspace_id = COALESCE(json_extract(changes.payload, '$.workspace_id'), '0000000000000000')
      AND p.key = changes.entity_id
)
WHERE op_type = 'create_project'
  AND entity_type = 'project'
  AND EXISTS (
      SELECT 1
      FROM projects_new p
      WHERE p.workspace_id = COALESCE(json_extract(changes.payload, '$.workspace_id'), '0000000000000000')
        AND p.key = changes.entity_id
  );

UPDATE changes
SET payload = json_set(
    payload,
    '$.project_id', (
        SELECT p.id
        FROM projects_new p
        WHERE p.workspace_id = COALESCE(json_extract(changes.payload, '$.workspace_id'), '0000000000000000')
          AND p.key = json_extract(changes.payload, '$.project_key')
    ),
    '$.project_name', COALESCE(json_extract(payload, '$.project_name'), (
        SELECT p.name
        FROM projects_new p
        WHERE p.workspace_id = COALESCE(json_extract(changes.payload, '$.workspace_id'), '0000000000000000')
          AND p.key = json_extract(changes.payload, '$.project_key')
    )),
    '$.project_prefix', COALESCE(json_extract(payload, '$.project_prefix'), (
        SELECT p.prefix
        FROM projects_new p
        WHERE p.workspace_id = COALESCE(json_extract(changes.payload, '$.workspace_id'), '0000000000000000')
          AND p.key = json_extract(changes.payload, '$.project_key')
    ))
)
WHERE op_type = 'create_task'
  AND entity_type = 'task'
  AND json_extract(payload, '$.project_key') IS NOT NULL
  AND EXISTS (
      SELECT 1
      FROM projects_new p
      WHERE p.workspace_id = COALESCE(json_extract(changes.payload, '$.workspace_id'), '0000000000000000')
        AND p.key = json_extract(changes.payload, '$.project_key')
  );

UPDATE changes
SET payload = json_set(
    payload,
    '$.value', (
        SELECT p.id
        FROM projects_new p
        WHERE p.workspace_id = COALESCE(json_extract(changes.payload, '$.workspace_id'), '0000000000000000')
          AND p.key = COALESCE(json_extract(changes.payload, '$.project_key'), json_extract(changes.payload, '$.value'))
    ),
    '$.project_id', (
        SELECT p.id
        FROM projects_new p
        WHERE p.workspace_id = COALESCE(json_extract(changes.payload, '$.workspace_id'), '0000000000000000')
          AND p.key = COALESCE(json_extract(changes.payload, '$.project_key'), json_extract(changes.payload, '$.value'))
    ),
    '$.project_key', COALESCE(json_extract(payload, '$.project_key'), json_extract(payload, '$.value')),
    '$.project_name', COALESCE(json_extract(payload, '$.project_name'), (
        SELECT p.name
        FROM projects_new p
        WHERE p.workspace_id = COALESCE(json_extract(changes.payload, '$.workspace_id'), '0000000000000000')
          AND p.key = COALESCE(json_extract(changes.payload, '$.project_key'), json_extract(changes.payload, '$.value'))
    )),
    '$.project_prefix', COALESCE(json_extract(payload, '$.project_prefix'), (
        SELECT p.prefix
        FROM projects_new p
        WHERE p.workspace_id = COALESCE(json_extract(changes.payload, '$.workspace_id'), '0000000000000000')
          AND p.key = COALESCE(json_extract(changes.payload, '$.project_key'), json_extract(changes.payload, '$.value'))
    ))
)
WHERE op_type IN ('set_field', 'resolve_field')
  AND entity_type = 'task'
  AND field = 'project'
  AND EXISTS (
      SELECT 1
      FROM projects_new p
      WHERE p.workspace_id = COALESCE(json_extract(changes.payload, '$.workspace_id'), '0000000000000000')
        AND p.key = COALESCE(json_extract(changes.payload, '$.project_key'), json_extract(changes.payload, '$.value'))
  );

UPDATE conflicts
SET local_value = (
    SELECT p.id
    FROM projects_new p
    WHERE p.workspace_id = conflicts.workspace_id AND p.key = conflicts.local_value
)
WHERE field = 'project'
  AND EXISTS (
      SELECT 1
      FROM projects_new p
      WHERE p.workspace_id = conflicts.workspace_id AND p.key = conflicts.local_value
  );

UPDATE conflicts
SET remote_value = (
    SELECT p.id
    FROM projects_new p
    WHERE p.workspace_id = conflicts.workspace_id AND p.key = conflicts.remote_value
)
WHERE field = 'project'
  AND EXISTS (
      SELECT 1
      FROM projects_new p
      WHERE p.workspace_id = conflicts.workspace_id AND p.key = conflicts.remote_value
  );

DROP TABLE tasks;
ALTER TABLE tasks_new RENAME TO tasks;

DROP TABLE project_paths;
ALTER TABLE project_paths_new RENAME TO project_paths;

DROP TABLE projects;
ALTER TABLE projects_new RENAME TO projects;

DROP TABLE project_id_map;

CREATE TABLE project_id_aliases (
    workspace_id TEXT NOT NULL,
    remote_project_id TEXT NOT NULL,
    local_project_id TEXT NOT NULL,
    PRIMARY KEY (workspace_id, remote_project_id)
);

CREATE INDEX idx_project_id_aliases_workspace_local
ON project_id_aliases(workspace_id, local_project_id);

CREATE INDEX IF NOT EXISTS idx_tasks_workspace_project ON tasks(workspace_id, project_id);
CREATE INDEX IF NOT EXISTS idx_task_labels_workspace_task ON task_labels(workspace_id, task_id);
CREATE INDEX IF NOT EXISTS idx_conflicts_workspace_task ON conflicts(workspace_id, task_id, resolved);
CREATE INDEX IF NOT EXISTS idx_project_paths_workspace_path ON project_paths(workspace_id, path);
CREATE INDEX IF NOT EXISTS idx_tasks_workspace_deleted_updated
ON tasks(workspace_id, deleted, updated_at DESC, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_tasks_workspace_deleted_status_updated
ON tasks(workspace_id, deleted, status, updated_at DESC, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_tasks_workspace_deleted_priority_updated
ON tasks(workspace_id, deleted, priority, updated_at DESC, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_tasks_workspace_project_deleted_updated
ON tasks(workspace_id, project_id, deleted, updated_at DESC, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_tasks_workspace_project_deleted_status
ON tasks(workspace_id, project_id, deleted, status);
CREATE INDEX IF NOT EXISTS idx_conflicts_workspace_resolved_created_task
ON conflicts(workspace_id, resolved, created_at, task_id);
CREATE INDEX IF NOT EXISTS idx_conflicts_workspace_resolved_task
ON conflicts(workspace_id, resolved, task_id);
CREATE INDEX IF NOT EXISTS idx_task_labels_workspace_label_task
ON task_labels(workspace_id, label, task_id);
