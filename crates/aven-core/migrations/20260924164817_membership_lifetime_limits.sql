-- A vault allows 256 signed membership transitions and 64 key generations.
-- Enrolled devices hold protected key coverage framed for an earlier
-- development limit, so their databases are refused unchanged. Server
-- membership rows remain valid and are copied.
CREATE TEMP TABLE membership_limits_guard (checked INTEGER);
CREATE TEMP TRIGGER membership_limits_guard_refusal
BEFORE INSERT ON membership_limits_guard
WHEN EXISTS (SELECT 1 FROM main.local_peer_enrollment)
  OR EXISTS (SELECT 1 FROM main.local_membership_checkpoint)
BEGIN
    SELECT RAISE(ABORT, 'error membership-development-format-unsupported: this database holds encrypted sync membership state from an earlier development build and was left unchanged; open it with that build, or start encrypted sync again with a new database');
END;
INSERT INTO membership_limits_guard VALUES (1);
DROP TABLE membership_limits_guard;

CREATE TABLE server_membership_transitions_new (
    sequence INTEGER PRIMARY KEY CHECK (sequence BETWEEN 2 AND 257),
    handle BLOB UNIQUE REFERENCES server_membership_invitations(handle),
    record BLOB NOT NULL CHECK (length(record) <= 32768)
);
INSERT INTO server_membership_transitions_new (sequence, handle, record)
    SELECT sequence, handle, record FROM server_membership_transitions;
DROP TABLE server_membership_transitions;
ALTER TABLE server_membership_transitions_new RENAME TO server_membership_transitions;

-- Empty per the guard above, so recreated without copying rows.
DROP TABLE local_membership_checkpoint;
-- The public loss-detection mirror covers every signed membership head and never
-- establishes membership or key authority.
CREATE TABLE local_membership_checkpoint (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    identity BLOB NOT NULL CHECK (length(identity) = 32),
    sequence INTEGER NOT NULL CHECK (sequence BETWEEN 1 AND 257),
    head BLOB NOT NULL CHECK (length(head) = 32),
    evidence BLOB NOT NULL CHECK (length(evidence) = 32)
);
