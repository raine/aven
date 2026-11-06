-- The background host of this database: the process holding its host lock, or
-- the last one that did. One row; round generations carry over between hosts
-- so that they only grow.
CREATE TABLE local_host_status (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    pid INTEGER NOT NULL,
    role TEXT NOT NULL,
    version TEXT NOT NULL,
    capabilities TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    heartbeat_at INTEGER NOT NULL,
    round_started INTEGER NOT NULL DEFAULT 0,
    round_finished INTEGER NOT NULL DEFAULT 0
);
