CREATE INDEX idx_task_attachments_sha256_deleted_workspace_task
ON task_attachments(sha256, deleted, workspace_id, task_id);

CREATE INDEX idx_server_blob_references_sha256_deleted_workspace_task
ON server_blob_references(sha256, deleted, workspace_id, task_id);

CREATE INDEX idx_changes_pending_attachment_add_sha256
ON changes(json_extract(payload, '$.sha256'))
WHERE server_seq IS NULL AND op_type = 'attachment_add';
