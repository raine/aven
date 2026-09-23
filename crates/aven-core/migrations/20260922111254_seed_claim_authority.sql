-- Public bindings only. Private seed authority belongs to host protected storage.
CREATE TABLE local_seed_genesis_pin (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    commitment BLOB NOT NULL CHECK (length(commitment) = 32)
);

-- The immutable signed record is the authority, not a mutable device projection.
CREATE TABLE server_seed_claim (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    genesis BLOB NOT NULL CHECK (length(genesis) = 902)
);
