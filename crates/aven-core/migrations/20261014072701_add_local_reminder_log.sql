-- Reminders this device has already handled. Digest rows use an empty task_id
-- because SQLite treats NULLs in a primary key as distinct.
CREATE TABLE local_reminder_log (
    kind TEXT NOT NULL,
    task_id TEXT NOT NULL,
    trigger TEXT NOT NULL,
    outcome TEXT NOT NULL,
    recorded_at TEXT NOT NULL,
    PRIMARY KEY (kind, task_id, trigger)
) WITHOUT ROWID;
