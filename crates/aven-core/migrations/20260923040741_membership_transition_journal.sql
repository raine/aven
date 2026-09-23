-- Signed transition replay owns authority; invitation rows are outcome indexes.
-- Admission-only histories remain intact and are refused, never converted.
CREATE TABLE server_membership_transitions (
    sequence INTEGER PRIMARY KEY CHECK (sequence BETWEEN 2 AND 129),
    handle BLOB UNIQUE REFERENCES server_membership_invitations(handle),
    record BLOB NOT NULL CHECK (length(record) <= 32768)
);
ALTER TABLE server_membership_invitations
    ADD COLUMN revoked INTEGER NOT NULL DEFAULT 0 CHECK (revoked IN (0, 1));
