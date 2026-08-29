ALTER TABLE changes ADD COLUMN recurrence_task_status_change_id TEXT
    GENERATED ALWAYS AS (
        CASE WHEN json_valid(payload) THEN
            CASE
                WHEN entity_type = 'recurrence_series'
                 AND op_type = 'resolve_recurrence_occurrence'
                THEN json_extract(payload, '$.task_status_change_id')
            END
        END
    ) VIRTUAL;

ALTER TABLE changes ADD COLUMN recurrence_successor_task_id TEXT
    GENERATED ALWAYS AS (
        CASE WHEN json_valid(payload) THEN
            CASE
                WHEN entity_type = 'recurrence_series'
                 AND op_type = 'resolve_recurrence_occurrence'
                THEN json_extract(payload, '$.successor_task_id')
            END
        END
    ) VIRTUAL;

CREATE INDEX idx_changes_task_activity
ON changes(entity_id, created_at DESC, local_seq DESC)
WHERE entity_type = 'task';

CREATE INDEX idx_changes_recurrence_task_status_change
ON changes(recurrence_task_status_change_id)
WHERE recurrence_task_status_change_id IS NOT NULL;

CREATE INDEX idx_changes_recurrence_successor_task
ON changes(recurrence_successor_task_id)
WHERE recurrence_successor_task_id IS NOT NULL;
