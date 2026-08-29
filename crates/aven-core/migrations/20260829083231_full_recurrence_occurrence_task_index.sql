DROP INDEX idx_recurrence_occurrences_task;

CREATE UNIQUE INDEX idx_recurrence_occurrences_task
ON recurrence_occurrences(workspace_id, task_id);
