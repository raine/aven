CREATE TABLE local_seed_source (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    authority BLOB NOT NULL,
    client_id TEXT NOT NULL
);
ALTER TABLE local_shared_capture_journal ADD COLUMN source_authority BLOB;
ALTER TABLE local_shared_capture_journal ADD COLUMN source_history TEXT;
ALTER TABLE local_shared_capture_journal ADD COLUMN source_provenance TEXT;
CREATE TABLE local_seed_publication_intent (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    candidate_id TEXT NOT NULL,
    intent BLOB NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('preparing', 'sealed', 'adopted')),
    association_generation INTEGER,
    association TEXT
);
CREATE TRIGGER protect_seed_intent_capture
BEFORE DELETE ON local_shared_capture_journal
WHEN EXISTS (SELECT 1 FROM local_seed_publication_intent
             WHERE state != 'adopted')
BEGIN
    SELECT RAISE(ABORT, 'seed publication intent owns capture');
END;
