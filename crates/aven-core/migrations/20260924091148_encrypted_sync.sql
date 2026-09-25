-- Shared history provenance and the durable never-dispatched capture.

CREATE TABLE shared_history_provenance (
    change_id TEXT PRIMARY KEY NOT NULL,
    source_server_seq INTEGER,
    source_pending_rank INTEGER,
    FOREIGN KEY (change_id) REFERENCES changes(change_id) ON DELETE CASCADE,
    CHECK (source_server_seq IS NULL OR source_server_seq > 0),
    CHECK (source_pending_rank IS NULL OR source_pending_rank > 0),
    CHECK ((source_server_seq IS NULL) != (source_pending_rank IS NULL))
);

CREATE TABLE local_shared_capture_journal (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    candidate_id TEXT NOT NULL UNIQUE,
    stream_id TEXT NOT NULL UNIQUE,
    state TEXT NOT NULL CHECK (state = 'never_dispatched'),
    internal_format TEXT NOT NULL,
    internal_version INTEGER NOT NULL,
    snapshot_json TEXT NOT NULL,
    local_seq_floor INTEGER NOT NULL CHECK (local_seq_floor >= 0),
    sync_generation INTEGER NOT NULL CHECK (sync_generation > 0),
    created_at TEXT NOT NULL,
    frozen_descriptor_commitment BLOB
        CHECK (frozen_descriptor_commitment IS NULL OR length(frozen_descriptor_commitment) = 32),
    source_authority BLOB,
    source_history TEXT,
    source_provenance TEXT,
    publication_owned INTEGER NOT NULL DEFAULT 0 CHECK (publication_owned IN (0, 1))
);

CREATE TABLE local_shared_capture_changes (
    candidate_id TEXT NOT NULL,
    change_id TEXT NOT NULL,
    prefix_rank INTEGER NOT NULL CHECK (prefix_rank > 0),
    source_server_seq INTEGER,
    source_pending_rank INTEGER,
    PRIMARY KEY (candidate_id, change_id),
    UNIQUE (candidate_id, prefix_rank),
    CHECK (
        (source_server_seq IS NOT NULL AND source_server_seq > 0 AND source_pending_rank IS NULL)
        OR
        (source_server_seq IS NULL AND source_pending_rank IS NOT NULL AND source_pending_rank > 0)
    ),
    FOREIGN KEY (candidate_id) REFERENCES local_shared_capture_journal(candidate_id)
        ON DELETE CASCADE
);

CREATE INDEX idx_local_shared_capture_changes_change
    ON local_shared_capture_changes(change_id);

CREATE TABLE local_shared_capture_images (
    candidate_id TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    classification TEXT NOT NULL CHECK (
        classification IN ('current_selected', 'extra_selected', 'unavailable')
    ),
    PRIMARY KEY (candidate_id, sha256),
    FOREIGN KEY (candidate_id) REFERENCES local_shared_capture_journal(candidate_id)
        ON DELETE CASCADE
);

CREATE TABLE local_shared_capture_pins (
    candidate_id TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    PRIMARY KEY (candidate_id, sha256),
    FOREIGN KEY (candidate_id, sha256)
        REFERENCES local_shared_capture_images(candidate_id, sha256) ON DELETE CASCADE,
    FOREIGN KEY (sha256) REFERENCES blob_inventory(sha256) ON DELETE RESTRICT
);

CREATE INDEX idx_local_shared_capture_pins_sha256
    ON local_shared_capture_pins(sha256);

-- The frozen package is its exact upload components: the descriptor and three
-- catalogs, plus encrypted records keyed by component, image object and index.
-- Absence on a frozen capture requires explicit cancellation and recapture.
CREATE TABLE local_shared_capture_publication (
    candidate_id TEXT PRIMARY KEY,
    descriptor BLOB NOT NULL CHECK (length(descriptor) <= 1978),
    data_catalog BLOB NOT NULL CHECK (length(data_catalog) <= 16777216),
    prefix_catalog BLOB NOT NULL CHECK (length(prefix_catalog) <= 16777216),
    image_catalog BLOB NOT NULL CHECK (length(image_catalog) <= 16777216),
    FOREIGN KEY (candidate_id) REFERENCES local_shared_capture_journal(candidate_id)
        ON DELETE CASCADE
);

CREATE TABLE local_shared_capture_package_records (
    candidate_id TEXT NOT NULL,
    component TEXT NOT NULL CHECK (component IN ('state', 'manifest', 'image')),
    object_id BLOB NOT NULL,
    chunk_index INTEGER NOT NULL CHECK (chunk_index >= 0),
    record BLOB NOT NULL,
    PRIMARY KEY (candidate_id, component, object_id, chunk_index),
    CHECK ((component = 'image') = (length(object_id) = 32)),
    CHECK (component = 'image' OR length(object_id) = 0),
    FOREIGN KEY (candidate_id) REFERENCES local_shared_capture_publication(candidate_id)
        ON DELETE CASCADE
);

-- Seed claim and adoption. Public bindings only; private seed authority belongs
-- to host protected storage.

CREATE TABLE local_seed_genesis_pin (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    commitment BLOB NOT NULL CHECK (length(commitment) = 32)
);

-- The immutable signed record is the authority, not a mutable device projection.
-- Successor membership must retire genesis-only admission in its own transaction.
CREATE TABLE server_seed_claim (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    genesis BLOB NOT NULL CHECK (length(genesis) = 902),
    genesis_only INTEGER NOT NULL DEFAULT 1 CHECK (genesis_only IN (0, 1))
);

CREATE TABLE local_seed_source (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    authority BLOB NOT NULL,
    client_id TEXT NOT NULL
);

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

-- Bootstrap staging and publication.

-- Terminal identities remain even after their bounded artifact storage is reclaimed.
CREATE TABLE server_bootstrap_candidates (
    bootstrap BLOB PRIMARY KEY CHECK (length(bootstrap) = 32),
    descriptor BLOB CHECK (descriptor IS NULL OR length(descriptor) <= 1978),
    canceled INTEGER NOT NULL CHECK (canceled IN (0, 1)),
    expires_at INTEGER NOT NULL,
    byte_budget INTEGER NOT NULL CHECK (byte_budget BETWEEN 0 AND 629145600),
    chunk_budget INTEGER NOT NULL CHECK (chunk_budget BETWEEN 0 AND 4096),
    CHECK (canceled = 1 OR descriptor IS NOT NULL)
);
CREATE UNIQUE INDEX server_bootstrap_one_active
    ON server_bootstrap_candidates(canceled) WHERE canceled = 0;

-- Component keys are fixed typed discriminators, never caller-provided text.
-- Only slices already checked against their descriptor slot are stored.
CREATE TABLE server_bootstrap_chunks (
    bootstrap BLOB NOT NULL REFERENCES server_bootstrap_candidates(bootstrap),
    component BLOB NOT NULL CHECK (length(component) IN (1, 33)),
    chunk_index INTEGER NOT NULL CHECK (chunk_index >= 0),
    bytes BLOB NOT NULL CHECK (length(bytes) BETWEEN 1 AND 1048798),
    PRIMARY KEY (bootstrap, component, chunk_index)
);

-- Current authority is separate from immutable historical publication outcomes.
CREATE TABLE server_e2ee_membership_head (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    sequence INTEGER NOT NULL CHECK (sequence > 0),
    commitment BLOB NOT NULL CHECK (length(commitment) = 32)
);

CREATE TABLE server_bootstrap_publication (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    bootstrap BLOB NOT NULL UNIQUE REFERENCES server_bootstrap_candidates(bootstrap),
    descriptor BLOB NOT NULL CHECK (length(descriptor) <= 1978),
    signed_record BLOB NOT NULL CHECK (length(signed_record) = 805),
    published_at INTEGER NOT NULL
);

CREATE TABLE server_e2ee_allocator (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    stream BLOB NOT NULL CHECK (length(stream) = 32),
    prefix_count INTEGER NOT NULL CHECK (prefix_count >= 0),
    high_water INTEGER NOT NULL CHECK (high_water >= prefix_count)
);

CREATE TABLE server_bootstrap_prefix (
    operation_id TEXT PRIMARY KEY,
    rank INTEGER NOT NULL UNIQUE CHECK (rank > 0)
);

-- Membership: signed transition replay owns authority; invitation rows are
-- outcome indexes.

CREATE TABLE server_membership_invitations (
    handle BLOB PRIMARY KEY CHECK (length(handle) = 32),
    inviter BLOB NOT NULL CHECK (length(inviter) = 32),
    declaration BLOB NOT NULL CHECK (length(declaration) = 280),
    request BLOB CHECK (request IS NULL OR length(request) = 314),
    expiry INTEGER NOT NULL CHECK (expiry >= 0),
    expired INTEGER NOT NULL DEFAULT 0 CHECK (expired IN (0, 1)),
    admitted_sequence INTEGER UNIQUE CHECK (admitted_sequence >= 2),
    revoked INTEGER NOT NULL DEFAULT 0 CHECK (revoked IN (0, 1))
);
CREATE UNIQUE INDEX server_membership_unfinished_inviter
    ON server_membership_invitations(inviter)
    WHERE expired = 0 AND admitted_sequence IS NULL;

CREATE TABLE server_membership_transitions (
    sequence INTEGER PRIMARY KEY CHECK (sequence BETWEEN 2 AND 257),
    handle BLOB UNIQUE REFERENCES server_membership_invitations(handle),
    record BLOB NOT NULL CHECK (length(record) <= 32768)
);

-- A new invitation cannot reset a previously observed vault clock.
CREATE TABLE server_membership_clock (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    high_water INTEGER NOT NULL CHECK (high_water >= 0)
);

-- Mirrors detect loss; protected host artifacts alone supply local authority.
CREATE TABLE local_peer_enrollment (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    identity BLOB NOT NULL CHECK (length(identity) = 32),
    client_id TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('inviter', 'peer'))
);
CREATE TABLE local_peer_enrollment_artifacts (
    kind TEXT PRIMARY KEY,
    commitment BLOB NOT NULL CHECK (length(commitment) = 32),
    owner INTEGER NOT NULL DEFAULT 1 REFERENCES local_peer_enrollment(singleton)
);

-- The public loss-detection mirror covers every signed membership head and never
-- establishes membership or key authority.
CREATE TABLE local_membership_checkpoint (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    identity BLOB NOT NULL CHECK (length(identity) = 32),
    sequence INTEGER NOT NULL CHECK (sequence BETWEEN 1 AND 257),
    head BLOB NOT NULL CHECK (length(head) = 32),
    evidence BLOB NOT NULL CHECK (length(evidence) = 32)
);

-- Receipt is an installation association, never membership authority.
CREATE TABLE local_peer_snapshot_install (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    enrollment BLOB NOT NULL CHECK (length(enrollment) = 32),
    checkpoint BLOB NOT NULL CHECK (length(checkpoint) = 32),
    descriptor BLOB NOT NULL CHECK (length(descriptor) = 32),
    stream BLOB NOT NULL CHECK (length(stream) = 32),
    prefix_count INTEGER NOT NULL CHECK (prefix_count >= 0),
    client_id TEXT NOT NULL,
    association TEXT NOT NULL,
    sync_generation INTEGER NOT NULL,
    attachment_count INTEGER NOT NULL CHECK (attachment_count >= 0)
);

-- Encrypted task tail.

CREATE TABLE server_e2ee_tail (
    operation_id TEXT PRIMARY KEY,
    sequence INTEGER NOT NULL UNIQUE CHECK(sequence > 0),
    commitment BLOB NOT NULL CHECK(length(commitment) = 32),
    record BLOB NOT NULL CHECK(length(record) <= 132328)
);
CREATE TABLE local_e2ee_outbox (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    operation_id TEXT NOT NULL UNIQUE REFERENCES changes(change_id),
    association TEXT NOT NULL,
    sync_generation INTEGER NOT NULL,
    record BLOB NOT NULL CHECK(length(record) <= 132328),
    observed_sequence INTEGER CHECK(observed_sequence IS NULL OR observed_sequence > 0),
    observed_commitment BLOB CHECK(observed_commitment IS NULL OR length(observed_commitment) = 32),
    blocked INTEGER NOT NULL DEFAULT 0 CHECK(blocked IN (0,1)),
    CHECK((observed_sequence IS NULL) = (observed_commitment IS NULL))
);
CREATE TABLE local_e2ee_accepted (
    operation_id TEXT PRIMARY KEY REFERENCES changes(change_id),
    sequence INTEGER NOT NULL UNIQUE CHECK(sequence > 0),
    commitment BLOB NOT NULL CHECK(length(commitment) = 32),
    record BLOB NOT NULL CHECK(length(record) <= 132328)
);
-- Frozen and verified comparison input must survive undo and cleanup.
CREATE TRIGGER e2ee_history_delete BEFORE DELETE ON changes
WHEN EXISTS(SELECT 1 FROM local_e2ee_outbox WHERE operation_id=OLD.change_id)
  OR EXISTS(SELECT 1 FROM local_e2ee_accepted WHERE operation_id=OLD.change_id)
BEGIN SELECT RAISE(ABORT, 'error encrypted-history-owned'); END;
CREATE TRIGGER e2ee_history_update BEFORE UPDATE ON changes
WHEN (EXISTS(SELECT 1 FROM local_e2ee_outbox WHERE operation_id=OLD.change_id)
  OR EXISTS(SELECT 1 FROM local_e2ee_accepted WHERE operation_id=OLD.change_id))
 AND (NEW.change_id IS NOT OLD.change_id OR NEW.client_id IS NOT OLD.client_id
  OR NEW.local_seq IS NOT OLD.local_seq OR NEW.entity_type IS NOT OLD.entity_type
  OR NEW.entity_id IS NOT OLD.entity_id OR NEW.field IS NOT OLD.field
  OR NEW.op_type IS NOT OLD.op_type OR NEW.payload IS NOT OLD.payload
  OR NEW.base_version IS NOT OLD.base_version OR NEW.created_at IS NOT OLD.created_at)
BEGIN SELECT RAISE(ABORT, 'error encrypted-history-owned'); END;

-- Local association state, excluded from portable snapshots and exports.
CREATE TABLE local_e2ee_dependency_baseline (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    association TEXT NOT NULL,
    sync_generation INTEGER NOT NULL,
    prefix_count INTEGER NOT NULL CHECK (prefix_count >= 0)
);

CREATE TABLE local_e2ee_dependency_edges (
    workspace_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    depends_on_task_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, task_id, depends_on_task_id)
);

-- Encrypted images. These rows project the committed image catalog and accepted
-- references, not plaintext domain state.

CREATE TABLE server_e2ee_image_parents (
    workspace TEXT NOT NULL,
    parent TEXT NOT NULL,
    deleted INTEGER NOT NULL CHECK (deleted IN (0, 1)),
    protected INTEGER NOT NULL CHECK (protected IN (0, 1)),
    version TEXT,
    PRIMARY KEY (workspace, parent)
);

-- Descriptors retain immutable recipes; image bytes have ordinary lifecycle
-- ownership and can be pruned without deleting those recipes.
CREATE TABLE server_e2ee_images (
    object BLOB PRIMARY KEY CHECK(length(object) = 32),
    bootstrap BLOB REFERENCES server_bootstrap_publication(bootstrap),
    byte_size INTEGER NOT NULL CHECK(byte_size > 0),
    unreferenced_at INTEGER,
    descriptor BLOB NOT NULL CHECK(length(descriptor) <= 1984),
    origin TEXT,
    complete INTEGER NOT NULL DEFAULT 0 CHECK(complete IN (0,1))
);
CREATE TABLE server_e2ee_image_references (
    workspace TEXT NOT NULL,
    reference TEXT NOT NULL,
    parent TEXT NOT NULL,
    deleted INTEGER NOT NULL CHECK(deleted IN (0,1)),
    object BLOB REFERENCES server_e2ee_images(object),
    PRIMARY KEY(workspace, reference),
    FOREIGN KEY(workspace, parent) REFERENCES server_e2ee_image_parents(workspace, parent)
);
CREATE TABLE server_e2ee_image_chunks (
    object BLOB NOT NULL REFERENCES server_e2ee_images(object),
    chunk_index INTEGER NOT NULL CHECK(chunk_index >= 0),
    bytes BLOB NOT NULL,
    PRIMARY KEY(object, chunk_index)
);
CREATE TABLE server_e2ee_image_scopes (
    object BLOB NOT NULL REFERENCES server_e2ee_images(object),
    workspace TEXT NOT NULL,
    PRIMARY KEY(object, workspace)
);
CREATE TABLE server_e2ee_image_tickets (
    reservation BLOB PRIMARY KEY CHECK(length(reservation) = 32),
    object BLOB NOT NULL REFERENCES server_e2ee_images(object),
    workspace TEXT NOT NULL,
    device BLOB NOT NULL CHECK(length(device) = 32),
    expires_at INTEGER NOT NULL,
    UNIQUE(object, workspace, device)
);
CREATE TABLE local_e2ee_image_initialization (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    association TEXT NOT NULL,
    sync_generation INTEGER NOT NULL,
    prefix_count INTEGER NOT NULL,
    descriptor BLOB NOT NULL CHECK(length(descriptor) = 32)
);
CREATE TABLE local_e2ee_image_objects (
    object BLOB PRIMARY KEY CHECK(length(object) = 32),
    descriptor BLOB NOT NULL CHECK(length(descriptor) <= 1984),
    sha256 TEXT NOT NULL,
    verified INTEGER NOT NULL DEFAULT 0 CHECK(verified IN (0,1)),
    origin TEXT NOT NULL
);
CREATE TABLE local_e2ee_image_references (
    workspace TEXT NOT NULL,
    reference TEXT NOT NULL,
    parent TEXT NOT NULL,
    object BLOB REFERENCES local_e2ee_image_objects(object),
    origin TEXT NOT NULL,
    deleted INTEGER NOT NULL CHECK(deleted IN (0,1)),
    PRIMARY KEY(workspace, reference)
);
CREATE TABLE local_e2ee_image_preparation (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    operation_id TEXT NOT NULL UNIQUE,
    descriptor BLOB NOT NULL CHECK(length(descriptor) <= 1984),
    sha256 TEXT NOT NULL
);
CREATE TABLE local_e2ee_image_staging (
    operation_id TEXT NOT NULL REFERENCES local_e2ee_image_preparation(operation_id) ON DELETE CASCADE,
    chunk_index INTEGER NOT NULL,
    bytes BLOB NOT NULL,
    PRIMARY KEY(operation_id, chunk_index)
);
CREATE TRIGGER local_e2ee_image_history_delete BEFORE DELETE ON changes
WHEN EXISTS(SELECT 1 FROM local_e2ee_image_preparation WHERE operation_id=OLD.change_id)
BEGIN SELECT RAISE(ABORT, 'encrypted image preparation owns history'); END;
CREATE TRIGGER local_e2ee_image_history_update BEFORE UPDATE ON changes
WHEN EXISTS(SELECT 1 FROM local_e2ee_image_preparation WHERE operation_id=OLD.change_id)
AND (NEW.change_id IS NOT OLD.change_id OR NEW.payload IS NOT OLD.payload
 OR NEW.entity_id IS NOT OLD.entity_id OR NEW.entity_type IS NOT OLD.entity_type
 OR NEW.field IS NOT OLD.field OR NEW.op_type IS NOT OLD.op_type
 OR NEW.base_version IS NOT OLD.base_version OR NEW.created_at IS NOT OLD.created_at)
BEGIN SELECT RAISE(ABORT, 'encrypted image preparation owns history'); END;

CREATE TRIGGER server_e2ee_image_identity_update BEFORE UPDATE ON server_e2ee_images
WHEN NEW.object IS NOT OLD.object OR NEW.bootstrap IS NOT OLD.bootstrap
 OR NEW.descriptor IS NOT OLD.descriptor OR NEW.byte_size IS NOT OLD.byte_size
 OR (OLD.origin IS NOT NULL AND NEW.origin IS NOT OLD.origin)
BEGIN SELECT RAISE(ABORT, 'opaque image identity is immutable'); END;
CREATE TRIGGER server_e2ee_image_identity_delete BEFORE DELETE ON server_e2ee_images
BEGIN SELECT RAISE(ABORT, 'opaque image identity survives reclamation'); END;
CREATE TRIGGER server_e2ee_image_reference_update BEFORE UPDATE ON server_e2ee_image_references
WHEN NEW.workspace IS NOT OLD.workspace OR NEW.reference IS NOT OLD.reference
 OR NEW.parent IS NOT OLD.parent OR NEW.object IS NOT OLD.object
 OR NEW.deleted < OLD.deleted
BEGIN SELECT RAISE(ABORT, 'opaque reference identity is immutable'); END;
CREATE TRIGGER server_e2ee_image_reference_delete BEFORE DELETE ON server_e2ee_image_references
BEGIN SELECT RAISE(ABORT, 'opaque reference tombstones are retained'); END;
CREATE TRIGGER local_e2ee_image_object_update BEFORE UPDATE ON local_e2ee_image_objects
WHEN NEW.object IS NOT OLD.object OR NEW.descriptor IS NOT OLD.descriptor
 OR NEW.sha256 IS NOT OLD.sha256 OR NEW.origin IS NOT OLD.origin
BEGIN SELECT RAISE(ABORT, 'authenticated image mapping is immutable'); END;
CREATE TRIGGER local_e2ee_image_reference_update BEFORE UPDATE ON local_e2ee_image_references
WHEN NEW.workspace IS NOT OLD.workspace OR NEW.reference IS NOT OLD.reference
 OR NEW.parent IS NOT OLD.parent OR NEW.object IS NOT OLD.object
 OR NEW.origin IS NOT OLD.origin OR NEW.deleted < OLD.deleted
BEGIN SELECT RAISE(ABORT, 'authenticated reference mapping is immutable'); END;
CREATE TRIGGER local_e2ee_image_preparation_update BEFORE UPDATE ON local_e2ee_image_preparation
BEGIN SELECT RAISE(ABORT, 'image preparation is immutable'); END;
CREATE TRIGGER local_e2ee_image_staging_update BEFORE UPDATE ON local_e2ee_image_staging
BEGIN SELECT RAISE(ABORT, 'frozen image records are immutable'); END;

-- Device labels.

CREATE INDEX idx_changes_device_label
ON changes(entity_id, server_seq, local_seq)
WHERE op_type = 'publish_device_label';
