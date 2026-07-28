ALTER TABLE tasks
ADD COLUMN source TEXT NOT NULL DEFAULT 'unknown'
CHECK (source IN ('cli', 'tui', 'api', 'unknown'));
