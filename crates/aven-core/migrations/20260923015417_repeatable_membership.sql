-- Invitation indexes are projections; signed predecessor replay owns authority.
CREATE TABLE server_membership_invitations (
    handle BLOB PRIMARY KEY CHECK (length(handle) = 32),
    inviter BLOB NOT NULL CHECK (length(inviter) = 32),
    declaration BLOB NOT NULL CHECK (length(declaration) = 280),
    request BLOB CHECK (request IS NULL OR length(request) = 314),
    expiry INTEGER NOT NULL CHECK (expiry >= 0),
    expired INTEGER NOT NULL DEFAULT 0 CHECK (expired IN (0, 1)),
    admitted_sequence INTEGER UNIQUE CHECK (admitted_sequence >= 2)
);
CREATE UNIQUE INDEX server_membership_unfinished_inviter
    ON server_membership_invitations(inviter)
    WHERE expired = 0 AND admitted_sequence IS NULL;

CREATE TABLE server_membership_admissions (
    sequence INTEGER PRIMARY KEY CHECK (sequence BETWEEN 2 AND 32),
    handle BLOB NOT NULL UNIQUE REFERENCES server_membership_invitations(handle),
    record BLOB NOT NULL CHECK (length(record) <= 8192)
);

-- A new invitation cannot reset a previously observed vault clock.
CREATE TABLE server_membership_clock (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    high_water INTEGER NOT NULL CHECK (high_water >= 0)
);
