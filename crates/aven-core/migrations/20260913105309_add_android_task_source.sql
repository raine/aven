ALTER TABLE tasks
ADD COLUMN source_with_android TEXT NOT NULL DEFAULT 'unknown'
CHECK (source_with_android IN ('cli', 'tui', 'api', 'ios', 'android', 'unknown'));

UPDATE tasks SET source_with_android = source;
ALTER TABLE tasks DROP COLUMN source;
ALTER TABLE tasks RENAME COLUMN source_with_android TO source;
