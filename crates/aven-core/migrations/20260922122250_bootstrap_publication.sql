-- Current authority is separate from immutable historical publication outcomes.
CREATE TABLE server_e2ee_membership_head (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    sequence INTEGER NOT NULL CHECK (sequence > 0),
    commitment BLOB NOT NULL CHECK (length(commitment) = 32)
);

CREATE TABLE server_bootstrap_publication (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    bootstrap BLOB NOT NULL UNIQUE REFERENCES server_bootstrap_candidates(bootstrap),
    descriptor BLOB NOT NULL CHECK (length(descriptor) <= 1024),
    signed_record BLOB NOT NULL CHECK (length(signed_record) = 805),
    published_at INTEGER NOT NULL
);

CREATE TABLE server_e2ee_allocator (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    stream BLOB NOT NULL CHECK (length(stream) = 32),
    prefix_count INTEGER NOT NULL CHECK (prefix_count >= 0),
    high_water INTEGER NOT NULL CHECK (high_water >= prefix_count)
);

CREATE TABLE server_bootstrap_prefix (
    operation_id TEXT PRIMARY KEY,
    rank INTEGER NOT NULL UNIQUE CHECK (rank > 0)
);

-- These rows project the committed image catalog, not plaintext domain state.
CREATE TABLE server_e2ee_image_parents (
    workspace TEXT NOT NULL,
    parent TEXT NOT NULL,
    deleted INTEGER NOT NULL CHECK (deleted IN (0, 1)),
    protected INTEGER NOT NULL CHECK (protected IN (0, 1)),
    version TEXT,
    PRIMARY KEY (workspace, parent)
);

-- Catalog roots retain immutable recipes; image bytes have ordinary lifecycle
-- ownership and can eventually be pruned without deleting those recipes.
CREATE TABLE server_e2ee_images (
    object BLOB PRIMARY KEY CHECK (length(object) = 32),
    bootstrap BLOB NOT NULL REFERENCES server_bootstrap_publication(bootstrap),
    byte_size INTEGER NOT NULL CHECK (byte_size > 0),
    unreferenced_at INTEGER
);

CREATE TABLE server_e2ee_image_references (
    workspace TEXT NOT NULL,
    reference TEXT NOT NULL,
    parent TEXT NOT NULL,
    deleted INTEGER NOT NULL CHECK (deleted IN (0, 1)),
    object BLOB REFERENCES server_e2ee_images(object),
    PRIMARY KEY (workspace, reference),
    FOREIGN KEY (workspace, parent) REFERENCES server_e2ee_image_parents(workspace, parent)
);

CREATE TABLE server_e2ee_image_chunks (
    object BLOB NOT NULL REFERENCES server_e2ee_images(object),
    chunk_index INTEGER NOT NULL CHECK (chunk_index >= 0),
    bytes BLOB NOT NULL,
    PRIMARY KEY (object, chunk_index)
);
