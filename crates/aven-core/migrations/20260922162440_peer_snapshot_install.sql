-- Receipt is an installation association, never membership authority.
CREATE TABLE local_peer_snapshot_install (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    enrollment BLOB NOT NULL CHECK (length(enrollment) = 32),
    checkpoint BLOB NOT NULL CHECK (length(checkpoint) = 32),
    descriptor BLOB NOT NULL CHECK (length(descriptor) = 32),
    stream BLOB NOT NULL CHECK (length(stream) = 32),
    prefix_count INTEGER NOT NULL CHECK (prefix_count >= 0),
    client_id TEXT NOT NULL,
    association TEXT NOT NULL,
    sync_generation INTEGER NOT NULL,
    attachment_count INTEGER NOT NULL CHECK (attachment_count >= 0)
);
