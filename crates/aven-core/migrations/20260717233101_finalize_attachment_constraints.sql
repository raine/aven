CREATE TRIGGER task_attachments_validate_insert
BEFORE INSERT ON task_attachments
BEGIN
    SELECT CASE WHEN
        length(NEW.attachment_id) != 16
        OR NEW.attachment_id GLOB '*[^0123456789ABCDEFGHJKMNPQRSTVWXYZ]*'
        OR length(NEW.sha256) != 64
        OR NEW.sha256 GLOB '*[^0-9a-f]*'
        OR NEW.byte_size <= 0
        OR NEW.byte_size > 26214400
        OR NEW.media_type NOT IN ('image/png', 'image/jpeg', 'image/gif', 'image/webp')
        OR NEW.width IS NULL
        OR NEW.height IS NULL
        OR NEW.width <= 0
        OR NEW.height <= 0
        OR NEW.width > 16384
        OR NEW.height > 16384
        OR NEW.width * NEW.height > 40000000
    THEN RAISE(ABORT, 'invalid task attachment metadata') END;
END;

CREATE TRIGGER task_attachments_validate_update
BEFORE UPDATE ON task_attachments
BEGIN
    SELECT CASE WHEN
        length(NEW.attachment_id) != 16
        OR NEW.attachment_id GLOB '*[^0123456789ABCDEFGHJKMNPQRSTVWXYZ]*'
        OR length(NEW.sha256) != 64
        OR NEW.sha256 GLOB '*[^0-9a-f]*'
        OR NEW.byte_size <= 0
        OR NEW.byte_size > 26214400
        OR NEW.media_type NOT IN ('image/png', 'image/jpeg', 'image/gif', 'image/webp')
        OR NEW.width IS NULL
        OR NEW.height IS NULL
        OR NEW.width <= 0
        OR NEW.height <= 0
        OR NEW.width > 16384
        OR NEW.height > 16384
        OR NEW.width * NEW.height > 40000000
    THEN RAISE(ABORT, 'invalid task attachment metadata') END;
END;

CREATE TRIGGER blob_inventory_validate_insert
BEFORE INSERT ON blob_inventory
BEGIN
    SELECT CASE WHEN
        length(NEW.sha256) != 64
        OR NEW.sha256 GLOB '*[^0-9a-f]*'
        OR NEW.byte_size <= 0
        OR NEW.byte_size > 26214400
        OR NEW.media_type NOT IN ('image/png', 'image/jpeg', 'image/gif', 'image/webp')
        OR NEW.available NOT IN (0, 1)
    THEN RAISE(ABORT, 'invalid blob inventory metadata') END;
END;

CREATE TRIGGER blob_inventory_validate_update
BEFORE UPDATE ON blob_inventory
BEGIN
    SELECT CASE WHEN
        length(NEW.sha256) != 64
        OR NEW.sha256 GLOB '*[^0-9a-f]*'
        OR NEW.byte_size <= 0
        OR NEW.byte_size > 26214400
        OR NEW.media_type NOT IN ('image/png', 'image/jpeg', 'image/gif', 'image/webp')
        OR NEW.available NOT IN (0, 1)
    THEN RAISE(ABORT, 'invalid blob inventory metadata') END;
END;
