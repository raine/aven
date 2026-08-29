CREATE INDEX idx_task_attachments_live_sha256
ON task_attachments(sha256)
WHERE deleted = 0;
