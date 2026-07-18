DROP INDEX IF EXISTS idx_changes_server_seq;
CREATE UNIQUE INDEX IF NOT EXISTS idx_changes_server_seq_unique
    ON changes(server_seq) WHERE server_seq IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_changes_unsynced_local_seq
    ON changes(local_seq, created_at) WHERE server_seq IS NULL;
