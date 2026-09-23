CREATE TABLE local_shared_capture_packages (
    candidate_id TEXT PRIMARY KEY,
    format_version INTEGER NOT NULL CHECK (format_version = 1),
    suite INTEGER NOT NULL CHECK (suite = 1),
    vault_id BLOB NOT NULL CHECK (length(vault_id) = 32),
    generation_id BLOB NOT NULL CHECK (length(generation_id) = 32),
    state_total_plaintext_bytes INTEGER NOT NULL CHECK (state_total_plaintext_bytes >= 0),
    state_chunk_count INTEGER NOT NULL CHECK (state_chunk_count > 0),
    state_aggregate_commitment BLOB NOT NULL CHECK (length(state_aggregate_commitment) = 32),
    manifest_total_plaintext_bytes INTEGER NOT NULL CHECK (manifest_total_plaintext_bytes > 0),
    manifest_chunk_count INTEGER NOT NULL CHECK (manifest_chunk_count > 0),
    manifest_aggregate_commitment BLOB NOT NULL CHECK (length(manifest_aggregate_commitment) = 32),
    created_at TEXT NOT NULL,
    FOREIGN KEY (candidate_id) REFERENCES local_shared_capture_journal(candidate_id)
        ON DELETE CASCADE
);

CREATE TABLE local_shared_capture_package_chunks (
    candidate_id TEXT NOT NULL,
    class INTEGER NOT NULL CHECK (class IN (1, 2)),
    chunk_index INTEGER NOT NULL CHECK (chunk_index >= 0),
    record_length INTEGER NOT NULL CHECK (record_length > 0),
    record_commitment BLOB NOT NULL CHECK (length(record_commitment) = 32),
    record BLOB NOT NULL,
    PRIMARY KEY (candidate_id, class, chunk_index),
    CHECK (record_length = length(record)),
    FOREIGN KEY (candidate_id) REFERENCES local_shared_capture_packages(candidate_id)
        ON DELETE CASCADE
);
