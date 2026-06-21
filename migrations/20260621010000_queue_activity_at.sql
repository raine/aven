ALTER TABLE tasks ADD COLUMN queue_activity_at TEXT NOT NULL DEFAULT '';

UPDATE tasks
SET queue_activity_at = updated_at
WHERE queue_activity_at = '';
