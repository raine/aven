CREATE INDEX idx_changes_task_activity
ON changes(entity_id, created_at DESC, local_seq DESC)
WHERE entity_type = 'task';

CREATE INDEX idx_changes_recurrence_task_status_change
ON changes(json_extract(payload, '$.task_status_change_id'))
WHERE entity_type = 'recurrence_series'
  AND op_type = 'resolve_recurrence_occurrence';

CREATE INDEX idx_changes_recurrence_successor_task
ON changes(json_extract(payload, '$.successor_task_id'))
WHERE entity_type = 'recurrence_series'
  AND op_type = 'resolve_recurrence_occurrence';
