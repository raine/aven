CREATE INDEX idx_recurrence_pause_intervals_history
ON recurrence_pause_intervals(workspace_id, series_id, paused_at DESC, id DESC);
