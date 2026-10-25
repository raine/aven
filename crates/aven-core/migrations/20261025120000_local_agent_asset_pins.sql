-- Images staged by an assistant session before they are attached to tasks.
-- A pin keeps its blob alive and records the facts an attachment row needs,
-- so applying a change set never reads the object file.
CREATE TABLE local_agent_asset_pins (
    pin_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    byte_size INTEGER NOT NULL,
    media_type TEXT NOT NULL,
    width INTEGER NOT NULL,
    height INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    FOREIGN KEY (sha256) REFERENCES blob_inventory(sha256) ON DELETE RESTRICT
);

CREATE INDEX idx_local_agent_asset_pins_sha256 ON local_agent_asset_pins(sha256);
CREATE INDEX idx_local_agent_asset_pins_created_at ON local_agent_asset_pins(created_at);
