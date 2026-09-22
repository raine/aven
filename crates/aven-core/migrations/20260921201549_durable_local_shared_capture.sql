CREATE TABLE local_shared_capture_journal (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    candidate_id TEXT NOT NULL UNIQUE,
    stream_id TEXT NOT NULL UNIQUE,
    state TEXT NOT NULL CHECK (state = 'never_dispatched'),
    internal_format TEXT NOT NULL,
    internal_version INTEGER NOT NULL,
    snapshot_json TEXT NOT NULL,
    local_seq_floor INTEGER NOT NULL CHECK (local_seq_floor >= 0),
    sync_generation INTEGER NOT NULL CHECK (sync_generation > 0),
    created_at TEXT NOT NULL
);

CREATE TABLE local_shared_capture_changes (
    candidate_id TEXT NOT NULL,
    change_id TEXT NOT NULL,
    prefix_rank INTEGER NOT NULL CHECK (prefix_rank > 0),
    source_server_seq INTEGER,
    source_pending_rank INTEGER,
    PRIMARY KEY (candidate_id, change_id),
    UNIQUE (candidate_id, prefix_rank),
    CHECK (
        (source_server_seq IS NOT NULL AND source_server_seq > 0 AND source_pending_rank IS NULL)
        OR
        (source_server_seq IS NULL AND source_pending_rank IS NOT NULL AND source_pending_rank > 0)
    ),
    FOREIGN KEY (candidate_id) REFERENCES local_shared_capture_journal(candidate_id)
        ON DELETE CASCADE
);

CREATE INDEX idx_local_shared_capture_changes_change
    ON local_shared_capture_changes(change_id);

CREATE TABLE local_shared_capture_images (
    candidate_id TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    classification TEXT NOT NULL CHECK (
        classification IN ('current_selected', 'extra_selected', 'unavailable')
    ),
    PRIMARY KEY (candidate_id, sha256),
    FOREIGN KEY (candidate_id) REFERENCES local_shared_capture_journal(candidate_id)
        ON DELETE CASCADE
);

CREATE TABLE local_shared_capture_pins (
    candidate_id TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    PRIMARY KEY (candidate_id, sha256),
    FOREIGN KEY (candidate_id, sha256)
        REFERENCES local_shared_capture_images(candidate_id, sha256) ON DELETE CASCADE,
    FOREIGN KEY (sha256) REFERENCES blob_inventory(sha256) ON DELETE RESTRICT
);

CREATE INDEX idx_local_shared_capture_pins_sha256
    ON local_shared_capture_pins(sha256);
