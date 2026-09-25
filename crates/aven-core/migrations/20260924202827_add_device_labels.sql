CREATE INDEX idx_changes_device_label
ON changes(entity_id, server_seq, local_seq)
WHERE op_type = 'publish_device_label';
