CREATE TABLE task_attachments (
    workspace_id TEXT NOT NULL DEFAULT '0000000000000000',
    attachment_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    byte_size INTEGER NOT NULL,
    media_type TEXT NOT NULL,
    filename TEXT,
    alt_text TEXT,
    width INTEGER,
    height INTEGER,
    created_at TEXT NOT NULL,
    created_by_change_id TEXT,
    deleted INTEGER NOT NULL DEFAULT 0,
    deleted_at TEXT,
    deleted_by_change_id TEXT,
    PRIMARY KEY (workspace_id, attachment_id),
    CHECK (length(attachment_id) = 16),
    CHECK (length(sha256) = 64),
    CHECK (byte_size > 0),
    CHECK (media_type IN ('image/png', 'image/jpeg', 'image/gif', 'image/webp')),
    CHECK (filename IS NULL OR length(filename) BETWEEN 1 AND 255),
    CHECK (alt_text IS NULL OR length(alt_text) <= 500),
    CHECK (width IS NULL OR width > 0),
    CHECK (height IS NULL OR height > 0),
    CHECK (deleted IN (0, 1)),
    CHECK (deleted = 0 OR deleted_at IS NOT NULL)
);

CREATE INDEX idx_task_attachments_workspace_task
ON task_attachments(workspace_id, task_id, deleted, created_at);

CREATE INDEX idx_task_attachments_workspace_sha256
ON task_attachments(workspace_id, sha256);

CREATE TABLE blob_inventory (
    sha256 TEXT PRIMARY KEY,
    byte_size INTEGER NOT NULL,
    media_type TEXT NOT NULL,
    available INTEGER NOT NULL DEFAULT 0,
    first_seen_at TEXT NOT NULL,
    last_verified_at TEXT,
    CHECK (length(sha256) = 64),
    CHECK (byte_size > 0),
    CHECK (media_type IN ('image/png', 'image/jpeg', 'image/gif', 'image/webp')),
    CHECK (available IN (0, 1))
);

CREATE INDEX idx_blob_inventory_available
ON blob_inventory(available, sha256);
