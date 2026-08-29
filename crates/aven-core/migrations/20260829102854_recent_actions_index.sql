CREATE INDEX idx_changes_workspace_recent_actions
ON changes(CASE WHEN json_valid(payload) THEN json_extract(payload, '$.workspace_id') END,
           created_at DESC, local_seq DESC);
