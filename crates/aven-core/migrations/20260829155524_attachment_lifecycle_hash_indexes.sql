CREATE INDEX idx_server_blob_references_live_sha256
ON server_blob_references(sha256)
WHERE deleted = 0;
