-- A mirror detects loss and never establishes membership or key authority.
CREATE TABLE local_membership_checkpoint (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    identity BLOB NOT NULL CHECK (length(identity) = 32),
    sequence INTEGER NOT NULL CHECK (sequence BETWEEN 1 AND 32),
    head BLOB NOT NULL CHECK (length(head) = 32),
    evidence BLOB NOT NULL CHECK (length(evidence) = 32)
);
