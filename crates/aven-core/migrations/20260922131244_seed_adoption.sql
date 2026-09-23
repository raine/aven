CREATE TABLE local_seed_source (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    authority BLOB NOT NULL,
    client_id TEXT NOT NULL
);
ALTER TABLE local_shared_capture_journal ADD COLUMN source_authority BLOB;
ALTER TABLE local_shared_capture_journal ADD COLUMN source_history TEXT;
ALTER TABLE local_shared_capture_journal ADD COLUMN source_provenance TEXT;
ALTER TABLE local_shared_capture_journal ADD COLUMN publication_owned INTEGER NOT NULL DEFAULT 0 CHECK (publication_owned IN (0, 1));
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
  OR (OLD.publication_owned = 1 AND NOT EXISTS (
      SELECT 1 FROM local_seed_publication_intent
      WHERE state = 'adopted' AND candidate_id = OLD.candidate_id
  ))
BEGIN
    SELECT RAISE(ABORT, 'seed publication intent owns capture');
END;
