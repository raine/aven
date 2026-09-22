CREATE TABLE shared_history_provenance (
    change_id TEXT PRIMARY KEY NOT NULL,
    source_server_seq INTEGER,
    source_pending_rank INTEGER,
    FOREIGN KEY (change_id) REFERENCES changes(change_id) ON DELETE CASCADE,
    CHECK (source_server_seq IS NULL OR source_server_seq > 0),
    CHECK (source_pending_rank IS NULL OR source_pending_rank > 0),
    CHECK ((source_server_seq IS NULL) != (source_pending_rank IS NULL))
);
