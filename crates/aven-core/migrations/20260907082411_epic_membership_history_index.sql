CREATE INDEX idx_changes_epic_membership_history
ON changes(json_extract(payload, '$.workspace_id'), entity_id)
WHERE op_type IN ('epic_link_add', 'epic_link_remove');
