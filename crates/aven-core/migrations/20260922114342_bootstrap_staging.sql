-- Successor membership must retire genesis-only admission in its own transaction.
ALTER TABLE server_seed_claim ADD COLUMN genesis_only INTEGER NOT NULL DEFAULT 1
    CHECK (genesis_only IN (0, 1));

-- Terminal identities remain even after their bounded artifact storage is reclaimed.
CREATE TABLE server_bootstrap_candidates (
    bootstrap BLOB PRIMARY KEY CHECK (length(bootstrap) = 32),
    descriptor BLOB CHECK (descriptor IS NULL OR length(descriptor) <= 1024),
    canceled INTEGER NOT NULL CHECK (canceled IN (0, 1)),
    epoch INTEGER NOT NULL CHECK (epoch > 0),
    expires_at INTEGER NOT NULL,
    byte_budget INTEGER NOT NULL CHECK (byte_budget BETWEEN 0 AND 629145600),
    chunk_budget INTEGER NOT NULL CHECK (chunk_budget BETWEEN 0 AND 4096),
    catalog_failure INTEGER CHECK (catalog_failure BETWEEN 0 AND 2),
    failure_reason INTEGER CHECK (failure_reason IN (0, 1)),
    CHECK ((catalog_failure IS NULL) = (failure_reason IS NULL)),
    CHECK (canceled = 1 OR descriptor IS NOT NULL)
);
CREATE UNIQUE INDEX server_bootstrap_one_active
    ON server_bootstrap_candidates(canceled) WHERE canceled = 0;

-- Component keys are fixed typed discriminators, never caller-provided text.
CREATE TABLE server_bootstrap_chunks (
    bootstrap BLOB NOT NULL REFERENCES server_bootstrap_candidates(bootstrap),
    component BLOB NOT NULL CHECK (length(component) IN (1, 33)),
    chunk_index INTEGER NOT NULL CHECK (chunk_index >= 0),
    verified INTEGER NOT NULL CHECK (verified IN (0, 1)),
    bytes BLOB NOT NULL CHECK (length(bytes) BETWEEN 1 AND 1048798),
    PRIMARY KEY (bootstrap, component, chunk_index)
);
