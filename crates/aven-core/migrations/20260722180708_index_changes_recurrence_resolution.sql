CREATE INDEX idx_changes_recurrence_resolution
ON changes(change_id)
WHERE entity_type = 'recurrence_series'
  AND op_type = 'resolve_recurrence_occurrence';
