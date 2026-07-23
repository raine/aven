DELETE FROM conflicts
WHERE entity_type = 'recurrence_series'
  AND (
      local_change_id IN (
          SELECT change_id FROM changes
          WHERE op_type = 'record_recurrence_outcome'
      )
      OR remote_change_id IN (
          SELECT change_id FROM changes
          WHERE op_type = 'record_recurrence_outcome'
      )
      OR EXISTS (
          SELECT 1
          FROM recurrence_occurrences occurrence
          WHERE occurrence.workspace_id = conflicts.workspace_id
            AND occurrence.series_id = conflicts.entity_id
            AND occurrence.projection_state = 'corrected'
            AND conflicts.field = 'outcome:' || occurrence.slot_on
      )
  );

DELETE FROM recurrence_occurrences
WHERE projection_state = 'corrected';

DELETE FROM changes
WHERE op_type = 'record_recurrence_outcome';

CREATE TABLE recurrence_occurrences_new (
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
    CHECK (projection_state IN ('projected', 'resolved', 'archived')),
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
        (outcome = '' AND outcome_change_id = '')
        OR (outcome != '' AND outcome_change_id != '')
    )
);

INSERT INTO recurrence_occurrences_new(
    workspace_id, series_id, slot_on, task_id, outcome, resolved_at,
    outcome_change_id, projection_state, archived_at
)
SELECT
    workspace_id, series_id, slot_on, task_id, outcome, resolved_at,
    outcome_change_id, projection_state, archived_at
FROM recurrence_occurrences;

DROP TABLE recurrence_occurrences;
ALTER TABLE recurrence_occurrences_new RENAME TO recurrence_occurrences;

CREATE UNIQUE INDEX idx_recurrence_occurrences_task
ON recurrence_occurrences(workspace_id, task_id)
WHERE task_id != '';

CREATE UNIQUE INDEX idx_recurrence_occurrences_active_projection
ON recurrence_occurrences(workspace_id, series_id)
WHERE projection_state = 'projected';

CREATE INDEX idx_recurrence_occurrences_history
ON recurrence_occurrences(workspace_id, series_id, slot_on DESC);
