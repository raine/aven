CREATE TABLE blob_lifecycle (
    sha256 TEXT PRIMARY KEY,
    unreferenced_at TEXT,
    CHECK (length(sha256) = 64)
);

CREATE INDEX idx_blob_lifecycle_unreferenced
ON blob_lifecycle(unreferenced_at, sha256);

CREATE TABLE blob_leases (
    lease_id TEXT PRIMARY KEY,
    sha256 TEXT NOT NULL,
    kind TEXT NOT NULL,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    CHECK (length(lease_id) = 16),
    CHECK (length(sha256) = 64),
    CHECK (kind IN ('staging', 'read', 'backup', 'transfer'))
);

CREATE INDEX idx_blob_leases_sha256_expiry
ON blob_leases(sha256, expires_at);

CREATE TABLE blob_upload_reservations (
    reservation_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    byte_size INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    CHECK (length(reservation_id) = 16),
    CHECK (length(sha256) = 64),
    CHECK (byte_size > 0),
    UNIQUE (workspace_id, sha256)
);

CREATE INDEX idx_blob_upload_reservations_expiry
ON blob_upload_reservations(expires_at);

CREATE TABLE server_blob_references (
    workspace_id TEXT NOT NULL,
    attachment_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    byte_size INTEGER NOT NULL,
    deleted INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (workspace_id, attachment_id),
    CHECK (length(sha256) = 64),
    CHECK (byte_size > 0),
    CHECK (deleted IN (0, 1))
);

CREATE INDEX idx_server_blob_references_workspace_hash
ON server_blob_references(workspace_id, sha256, deleted);

CREATE TABLE server_task_tombstones (
    workspace_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    deleted INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (workspace_id, task_id),
    CHECK (deleted IN (0, 1))
);

INSERT OR REPLACE INTO server_blob_references(
    workspace_id, attachment_id, task_id, sha256, byte_size, deleted
)
SELECT
    json_extract(payload, '$.workspace_id'),
    json_extract(payload, '$.attachment_id'),
    entity_id,
    json_extract(payload, '$.sha256'),
    json_extract(payload, '$.byte_size'),
    0
FROM changes
WHERE op_type = 'attachment_add'
  AND json_type(payload, '$.workspace_id') = 'text'
  AND json_type(payload, '$.attachment_id') = 'text'
  AND json_type(payload, '$.sha256') = 'text'
  AND json_type(payload, '$.byte_size') = 'integer';

UPDATE server_blob_references
SET deleted = 1
WHERE EXISTS (
    SELECT 1 FROM changes c
    WHERE c.op_type = 'attachment_delete'
      AND json_extract(c.payload, '$.workspace_id') = server_blob_references.workspace_id
      AND json_extract(c.payload, '$.attachment_id') = server_blob_references.attachment_id
);

INSERT OR REPLACE INTO server_task_tombstones(workspace_id, task_id, deleted)
SELECT
    json_extract(payload, '$.workspace_id'),
    entity_id,
    CASE json_extract(payload, '$.value') WHEN '1' THEN 1 ELSE 0 END
FROM changes
WHERE op_type = 'set_field' AND field = 'deleted'
  AND json_type(payload, '$.workspace_id') = 'text'
ORDER BY COALESCE(server_seq, local_seq);
