-- The public loss-detection mirror covers every signed membership head.
CREATE TABLE local_membership_checkpoint_expanded (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    identity BLOB NOT NULL CHECK (length(identity) = 32),
    sequence INTEGER NOT NULL CHECK (sequence BETWEEN 1 AND 129),
    head BLOB NOT NULL CHECK (length(head) = 32),
    evidence BLOB NOT NULL CHECK (length(evidence) = 32)
);
INSERT INTO local_membership_checkpoint_expanded
    (singleton, identity, sequence, head, evidence)
    SELECT singleton, identity, sequence, head, evidence
    FROM local_membership_checkpoint;
DROP TABLE local_membership_checkpoint;
ALTER TABLE local_membership_checkpoint_expanded
    RENAME TO local_membership_checkpoint;
