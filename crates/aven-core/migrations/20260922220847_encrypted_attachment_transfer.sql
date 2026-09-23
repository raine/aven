-- Preserve domain and opaque bytes without inventing missing mapping initialization.
CREATE TABLE image_references_saved AS SELECT * FROM server_e2ee_image_references;
CREATE TABLE image_chunks_saved AS SELECT * FROM server_e2ee_image_chunks;
DROP TABLE server_e2ee_image_references;
DROP TABLE server_e2ee_image_chunks;
CREATE TABLE server_e2ee_images_new (
    object BLOB PRIMARY KEY CHECK(length(object) = 32),
    bootstrap BLOB REFERENCES server_bootstrap_publication(bootstrap),
    byte_size INTEGER NOT NULL CHECK(byte_size > 0),
    unreferenced_at INTEGER,
    descriptor BLOB CHECK(length(descriptor) <= 1984),
    origin TEXT,
    epoch INTEGER NOT NULL DEFAULT 1 CHECK(epoch > 0),
    complete INTEGER NOT NULL DEFAULT 0 CHECK(complete IN (0,1))
);
INSERT INTO server_e2ee_images_new(object,bootstrap,byte_size,unreferenced_at)
SELECT object,bootstrap,byte_size,unreferenced_at FROM server_e2ee_images;
DROP TABLE server_e2ee_images;
ALTER TABLE server_e2ee_images_new RENAME TO server_e2ee_images;
CREATE TABLE server_e2ee_image_references (
    workspace TEXT NOT NULL,
    reference TEXT NOT NULL,
    parent TEXT NOT NULL,
    deleted INTEGER NOT NULL CHECK(deleted IN (0,1)),
    object BLOB REFERENCES server_e2ee_images(object),
    PRIMARY KEY(workspace, reference),
    FOREIGN KEY(workspace, parent) REFERENCES server_e2ee_image_parents(workspace, parent)
);
INSERT INTO server_e2ee_image_references SELECT * FROM image_references_saved;
DROP TABLE image_references_saved;
CREATE TABLE server_e2ee_image_chunks (
    object BLOB NOT NULL REFERENCES server_e2ee_images(object),
    chunk_index INTEGER NOT NULL CHECK(chunk_index >= 0),
    bytes BLOB NOT NULL,
    PRIMARY KEY(object, chunk_index)
);
INSERT INTO server_e2ee_image_chunks SELECT * FROM image_chunks_saved;
DROP TABLE image_chunks_saved;
CREATE TABLE server_e2ee_image_initialization (
    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
    descriptor BLOB NOT NULL CHECK(length(descriptor)=32)
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
    epoch INTEGER NOT NULL CHECK(epoch > 0),
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
