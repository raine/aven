CREATE INDEX idx_changes_task_activity
ON changes(entity_id, created_at DESC, local_seq DESC)
WHERE entity_type = 'task';
