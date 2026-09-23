-- The fixed first-peer profile extends the single membership chain.
CREATE TABLE server_peer_invitation (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    declaration BLOB NOT NULL CHECK (length(declaration) = 280),
    request BLOB CHECK (request IS NULL OR length(request) = 314),
    admission BLOB CHECK (admission IS NULL OR length(admission) = 1698),
    clock_high_water INTEGER NOT NULL CHECK (clock_high_water >= 0),
    expired INTEGER NOT NULL DEFAULT 0 CHECK (expired IN (0, 1))
);

-- Mirrors detect loss; protected host artifacts alone supply local authority.
CREATE TABLE local_peer_enrollment (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    identity BLOB NOT NULL CHECK (length(identity) = 32),
    client_id TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('inviter', 'peer'))
);
CREATE TABLE local_peer_enrollment_artifacts (
    kind TEXT PRIMARY KEY,
    commitment BLOB NOT NULL CHECK (length(commitment) = 32),
    owner INTEGER NOT NULL DEFAULT 1 REFERENCES local_peer_enrollment(singleton)
);
