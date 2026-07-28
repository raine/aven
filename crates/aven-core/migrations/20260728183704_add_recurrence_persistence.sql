CREATE TABLE recurrence_series (
    workspace_id TEXT NOT NULL,
    id TEXT NOT NULL,
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    project_id TEXT NOT NULL,
    priority TEXT NOT NULL,
    initial_status TEXT NOT NULL,
    frequency TEXT NOT NULL,
    interval INTEGER NOT NULL,
    weekdays TEXT NOT NULL,
    timezone TEXT NOT NULL,
    start_on TEXT NOT NULL,
    available_local_time TEXT NOT NULL DEFAULT '',
    due_policy TEXT NOT NULL,
    state TEXT NOT NULL,
    stopped_at TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    deleted INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (workspace_id, id),
    CHECK (priority IN ('none', 'low', 'medium', 'high', 'urgent')),
    CHECK (initial_status IN ('inbox', 'backlog', 'todo', 'active')),
    CHECK (frequency IN ('daily', 'weekly')),
    CHECK (interval > 0),
    CHECK (
        (frequency = 'daily' AND interval = 1 AND weekdays = '')
        OR (frequency = 'weekly' AND weekdays != '')
    ),
    CHECK (due_policy IN ('same_day', 'none')),
    CHECK (state IN ('active', 'paused', 'stopped')),
    CHECK (
        (state = 'stopped' AND stopped_at != '')
        OR (state != 'stopped' AND stopped_at = '')
    ),
    CHECK (deleted IN (0, 1))
);

CREATE TRIGGER recurrence_series_schedule_immutable
BEFORE UPDATE OF frequency, interval, weekdays, timezone, start_on
ON recurrence_series
WHEN NEW.frequency != OLD.frequency
    OR NEW.interval != OLD.interval
    OR NEW.weekdays != OLD.weekdays
    OR NEW.timezone != OLD.timezone
    OR NEW.start_on != OLD.start_on
BEGIN
    SELECT RAISE(ABORT, 'recurrence schedule is immutable');
END;

CREATE TABLE recurrence_series_labels (
    workspace_id TEXT NOT NULL,
    series_id TEXT NOT NULL,
    label TEXT NOT NULL,
    PRIMARY KEY (workspace_id, series_id, label)
);

CREATE TABLE recurrence_occurrences (
    workspace_id TEXT NOT NULL,
    series_id TEXT NOT NULL,
    slot_on TEXT NOT NULL,
    task_id TEXT NOT NULL DEFAULT '',
    outcome TEXT NOT NULL DEFAULT '',
    resolved_at TEXT NOT NULL DEFAULT '',
    outcome_change_id TEXT NOT NULL DEFAULT '',
    projection_state TEXT NOT NULL,
    archived_at TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (workspace_id, series_id, slot_on),
    CHECK (outcome IN ('', 'completed', 'skipped')),
    CHECK (projection_state IN ('projected', 'resolved', 'archived', 'corrected')),
    CHECK (
        (projection_state = 'projected' AND task_id != '' AND outcome = '' AND resolved_at = '' AND archived_at = '')
        OR projection_state != 'projected'
    ),
    CHECK (
        (projection_state = 'resolved' AND task_id != '' AND outcome != '' AND resolved_at != '' AND archived_at = '')
        OR projection_state != 'resolved'
    ),
    CHECK (
        (projection_state = 'archived' AND task_id != '' AND outcome = '' AND resolved_at = '' AND outcome_change_id = '' AND archived_at != '')
        OR projection_state != 'archived'
    ),
    CHECK (
        (projection_state = 'corrected' AND task_id = '' AND outcome != '' AND resolved_at != '' AND archived_at = '')
        OR projection_state != 'corrected'
    ),
    CHECK (
        (outcome = '' AND outcome_change_id = '')
        OR (outcome != '' AND outcome_change_id != '')
    )
);

CREATE UNIQUE INDEX idx_recurrence_occurrences_task
ON recurrence_occurrences(workspace_id, task_id)
WHERE task_id != '';

CREATE UNIQUE INDEX idx_recurrence_occurrences_active_projection
ON recurrence_occurrences(workspace_id, series_id)
WHERE projection_state = 'projected';

CREATE INDEX idx_recurrence_occurrences_history
ON recurrence_occurrences(workspace_id, series_id, slot_on DESC);

CREATE TABLE recurrence_pause_intervals (
    workspace_id TEXT NOT NULL,
    id TEXT NOT NULL,
    series_id TEXT NOT NULL,
    paused_at TEXT NOT NULL,
    resumed_at TEXT NOT NULL DEFAULT '',
    suspended_slot_on TEXT NOT NULL DEFAULT '',
    suspended_task_id TEXT NOT NULL DEFAULT '',
    created_by_change_id TEXT NOT NULL,
    resolved_by_change_id TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (workspace_id, id),
    CHECK (resumed_at = '' OR resumed_at > paused_at),
    CHECK (
        (resumed_at = '' AND resolved_by_change_id = '')
        OR (resumed_at != '' AND resolved_by_change_id != '')
    ),
    CHECK (
        (suspended_slot_on = '' AND suspended_task_id = '')
        OR (suspended_slot_on != '' AND suspended_task_id != '')
    )
);

CREATE UNIQUE INDEX idx_recurrence_pause_intervals_open
ON recurrence_pause_intervals(workspace_id, series_id)
WHERE resumed_at = '';

CREATE INDEX idx_recurrence_series_lifecycle
ON recurrence_series(workspace_id, state, deleted);

CREATE INDEX idx_recurrence_series_project
ON recurrence_series(workspace_id, project_id, deleted);

CREATE INDEX idx_recurrence_series_labels_series
ON recurrence_series_labels(workspace_id, series_id);

CREATE TABLE field_versions_new (
    workspace_id TEXT NOT NULL,
    entity_type TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    field TEXT NOT NULL,
    version TEXT NOT NULL,
    PRIMARY KEY (workspace_id, entity_type, entity_id, field),
    CHECK (entity_type IN ('task', 'recurrence_series')),
    CHECK (
        entity_type != 'recurrence_series'
        OR field IN (
            'title', 'description', 'project', 'priority', 'initial_status', 'labels',
            'available_local_time', 'due_policy', 'state', 'stopped_at', 'deleted'
        )
    )
);

INSERT INTO field_versions_new(workspace_id, entity_type, entity_id, field, version)
SELECT
    COALESCE(
        (SELECT t.workspace_id FROM tasks t WHERE t.id = field_versions.entity_id LIMIT 1),
        '0000000000000000'
    ),
    'task',
    entity_id,
    field,
    version
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
    CHECK (entity_type IN ('task', 'recurrence_series')),
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
    id, workspace_id, 'task', task_id, task_id, field, base_version,
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
