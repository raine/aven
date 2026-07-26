CREATE TABLE recurrence_series_new (
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
    CHECK (frequency IN ('daily', 'weekly', 'monthly')),
    CHECK (interval > 0),
    CHECK (
        (frequency = 'daily' AND interval = 1 AND weekdays = '')
        OR (frequency = 'weekly' AND weekdays != '')
        OR (frequency = 'monthly' AND interval = 1 AND weekdays = '')
    ),
    CHECK (due_policy IN ('same_day', 'none')),
    CHECK (state IN ('active', 'paused', 'stopped')),
    CHECK (
        (state = 'stopped' AND stopped_at != '')
        OR (state != 'stopped' AND stopped_at = '')
    ),
    CHECK (deleted IN (0, 1))
);

INSERT INTO recurrence_series_new (
    workspace_id, id, title, description, project_id, priority, initial_status,
    frequency, interval, weekdays, timezone, start_on, available_local_time,
    due_policy, state, stopped_at, created_at, updated_at, deleted
)
SELECT
    workspace_id, id, title, description, project_id, priority, initial_status,
    frequency, interval, weekdays, timezone, start_on, available_local_time,
    due_policy, state, stopped_at, created_at, updated_at, deleted
FROM recurrence_series;

DROP TABLE recurrence_series;
ALTER TABLE recurrence_series_new RENAME TO recurrence_series;

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

CREATE INDEX idx_recurrence_series_lifecycle
ON recurrence_series(workspace_id, state, deleted);

CREATE INDEX idx_recurrence_series_project
ON recurrence_series(workspace_id, project_id, deleted);
