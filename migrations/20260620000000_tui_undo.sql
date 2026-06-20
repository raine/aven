CREATE TABLE IF NOT EXISTS tui_undo_entries (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    summary TEXT NOT NULL,
    payload_version INTEGER NOT NULL DEFAULT 1,
    payload TEXT NOT NULL,
    seq INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    undone_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_tui_undo_entries_latest_unconsumed
ON tui_undo_entries(workspace_id, seq DESC)
WHERE undone_at IS NULL;
