-- Bootstrap descriptor version 2 commits every catalog slice, so staged slices
-- are never quarantined and there is no catalog failure state. Stores holding
-- bootstrap state from an earlier development format are refused unchanged:
-- their frozen bytes cannot be reinterpreted or rewritten here.
CREATE TEMP TABLE bootstrap_format_guard (checked INTEGER);
CREATE TEMP TRIGGER bootstrap_format_guard_refusal
BEFORE INSERT ON bootstrap_format_guard
WHEN EXISTS (SELECT 1 FROM main.local_shared_capture_publication)
  OR EXISTS (SELECT 1 FROM main.local_shared_capture_journal
             WHERE frozen_descriptor_commitment IS NOT NULL)
  OR EXISTS (SELECT 1 FROM main.local_seed_publication_intent)
  OR EXISTS (SELECT 1 FROM main.server_bootstrap_candidates)
  OR EXISTS (SELECT 1 FROM main.server_bootstrap_chunks)
  OR EXISTS (SELECT 1 FROM main.server_bootstrap_publication)
BEGIN
    SELECT RAISE(ABORT, 'error bootstrap-development-format-unsupported: this database holds encrypted sync setup state from an earlier development build and was left unchanged; open it with that build, or start encrypted sync again with a new database');
END;
INSERT INTO bootstrap_format_guard VALUES (1);
DROP TABLE bootstrap_format_guard;

-- The tables below are empty, so they are recreated without copying rows.
DROP TABLE local_shared_capture_package_records;
DROP TABLE local_shared_capture_publication;
DROP TABLE server_bootstrap_publication;
DROP TABLE server_bootstrap_chunks;
DROP TABLE server_bootstrap_candidates;

-- The frozen package is its exact upload components: the descriptor and three
-- catalogs, plus encrypted records keyed by component, image object and index.
-- Absence on a frozen capture requires explicit cancellation and recapture.
CREATE TABLE local_shared_capture_publication (
    candidate_id TEXT PRIMARY KEY,
    descriptor BLOB NOT NULL CHECK (length(descriptor) <= 1978),
    data_catalog BLOB NOT NULL CHECK (length(data_catalog) <= 16777216),
    prefix_catalog BLOB NOT NULL CHECK (length(prefix_catalog) <= 16777216),
    image_catalog BLOB NOT NULL CHECK (length(image_catalog) <= 16777216),
    FOREIGN KEY (candidate_id) REFERENCES local_shared_capture_journal(candidate_id)
        ON DELETE CASCADE
);

CREATE TABLE local_shared_capture_package_records (
    candidate_id TEXT NOT NULL,
    component TEXT NOT NULL CHECK (component IN ('state', 'manifest', 'image')),
    object_id BLOB NOT NULL,
    chunk_index INTEGER NOT NULL CHECK (chunk_index >= 0),
    record BLOB NOT NULL,
    PRIMARY KEY (candidate_id, component, object_id, chunk_index),
    CHECK ((component = 'image') = (length(object_id) = 32)),
    CHECK (component = 'image' OR length(object_id) = 0),
    FOREIGN KEY (candidate_id) REFERENCES local_shared_capture_publication(candidate_id)
        ON DELETE CASCADE
);

-- Terminal identities remain even after their bounded artifact storage is reclaimed.
CREATE TABLE server_bootstrap_candidates (
    bootstrap BLOB PRIMARY KEY CHECK (length(bootstrap) = 32),
    descriptor BLOB CHECK (descriptor IS NULL OR length(descriptor) <= 1978),
    canceled INTEGER NOT NULL CHECK (canceled IN (0, 1)),
    epoch INTEGER NOT NULL CHECK (epoch > 0),
    expires_at INTEGER NOT NULL,
    byte_budget INTEGER NOT NULL CHECK (byte_budget BETWEEN 0 AND 629145600),
    chunk_budget INTEGER NOT NULL CHECK (chunk_budget BETWEEN 0 AND 4096),
    CHECK (canceled = 1 OR descriptor IS NOT NULL)
);
CREATE UNIQUE INDEX server_bootstrap_one_active
    ON server_bootstrap_candidates(canceled) WHERE canceled = 0;

-- Component keys are fixed typed discriminators, never caller-provided text.
-- Only slices already checked against their descriptor slot are stored.
CREATE TABLE server_bootstrap_chunks (
    bootstrap BLOB NOT NULL REFERENCES server_bootstrap_candidates(bootstrap),
    component BLOB NOT NULL CHECK (length(component) IN (1, 33)),
    chunk_index INTEGER NOT NULL CHECK (chunk_index >= 0),
    bytes BLOB NOT NULL CHECK (length(bytes) BETWEEN 1 AND 1048798),
    PRIMARY KEY (bootstrap, component, chunk_index)
);

CREATE TABLE server_bootstrap_publication (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    bootstrap BLOB NOT NULL UNIQUE REFERENCES server_bootstrap_candidates(bootstrap),
    descriptor BLOB NOT NULL CHECK (length(descriptor) <= 1978),
    signed_record BLOB NOT NULL CHECK (length(signed_record) = 805),
    published_at INTEGER NOT NULL
);
