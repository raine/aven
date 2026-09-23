CREATE TABLE local_shared_capture_package_images (
    candidate_id TEXT NOT NULL,
    source_sha256 TEXT NOT NULL CHECK (length(source_sha256) = 64),
    classification TEXT NOT NULL CHECK (classification IN ('current_selected', 'extra_selected')),
    object_id BLOB NOT NULL CHECK (length(object_id) = 32),
    total_plaintext_bytes INTEGER NOT NULL CHECK (total_plaintext_bytes > 0),
    chunk_count INTEGER NOT NULL CHECK (chunk_count > 0 AND chunk_count <= 25),
    aggregate_commitment BLOB NOT NULL CHECK (length(aggregate_commitment) = 32),
    PRIMARY KEY (candidate_id, source_sha256),
    UNIQUE (candidate_id, object_id),
    FOREIGN KEY (candidate_id) REFERENCES local_shared_capture_packages(candidate_id)
        ON DELETE CASCADE
);

CREATE TABLE local_shared_capture_package_image_chunks (
    candidate_id TEXT NOT NULL,
    object_id BLOB NOT NULL CHECK (length(object_id) = 32),
    chunk_index INTEGER NOT NULL CHECK (chunk_index >= 0),
    record_length INTEGER NOT NULL CHECK (record_length > 0),
    record_commitment BLOB NOT NULL CHECK (length(record_commitment) = 32),
    record BLOB NOT NULL,
    PRIMARY KEY (candidate_id, object_id, chunk_index),
    CHECK (record_length = length(record)),
    FOREIGN KEY (candidate_id, object_id)
        REFERENCES local_shared_capture_package_images(candidate_id, object_id)
        ON DELETE CASCADE
);
