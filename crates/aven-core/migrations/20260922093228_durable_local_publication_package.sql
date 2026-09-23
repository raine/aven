ALTER TABLE local_shared_capture_journal
    ADD COLUMN frozen_descriptor_commitment BLOB
    CHECK (frozen_descriptor_commitment IS NULL OR length(frozen_descriptor_commitment) = 32);

-- Absence on an existing frozen package requires explicit cancellation/recapture.
CREATE TABLE local_shared_capture_publication (
    candidate_id TEXT PRIMARY KEY,
    descriptor BLOB NOT NULL CHECK (length(descriptor) <= 1024),
    data_catalog BLOB NOT NULL CHECK (length(data_catalog) <= 16777216),
    prefix_catalog BLOB NOT NULL CHECK (length(prefix_catalog) <= 16777216),
    image_catalog BLOB NOT NULL CHECK (length(image_catalog) <= 16777216),
    FOREIGN KEY (candidate_id) REFERENCES local_shared_capture_packages(candidate_id)
        ON DELETE CASCADE
);
