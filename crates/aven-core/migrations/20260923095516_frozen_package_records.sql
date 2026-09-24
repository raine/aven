-- The frozen package is its exact upload components: the descriptor and three
-- catalogs, plus encrypted records keyed by component, image object and index.
-- Every other former package column is derivable from those committed bytes or
-- held in the encrypted domain. Valid frozen bytes are copied verbatim.

ALTER TABLE local_shared_capture_publication RENAME TO local_shared_capture_publication_old;

CREATE TABLE local_shared_capture_publication (
    candidate_id TEXT PRIMARY KEY,
    descriptor BLOB NOT NULL CHECK (length(descriptor) <= 1024),
    data_catalog BLOB NOT NULL CHECK (length(data_catalog) <= 16777216),
    prefix_catalog BLOB NOT NULL CHECK (length(prefix_catalog) <= 16777216),
    image_catalog BLOB NOT NULL CHECK (length(image_catalog) <= 16777216),
    FOREIGN KEY (candidate_id) REFERENCES local_shared_capture_journal(candidate_id)
        ON DELETE CASCADE
);

INSERT INTO local_shared_capture_publication
    (candidate_id, descriptor, data_catalog, prefix_catalog, image_catalog)
SELECT candidate_id, descriptor, data_catalog, prefix_catalog, image_catalog
FROM local_shared_capture_publication_old;

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

INSERT INTO local_shared_capture_package_records
    (candidate_id, component, object_id, chunk_index, record)
SELECT candidate_id, CASE class WHEN 1 THEN 'state' WHEN 2 THEN 'manifest' END, x'',
       chunk_index, record
FROM local_shared_capture_package_chunks
WHERE candidate_id IN (SELECT candidate_id FROM local_shared_capture_publication);

INSERT INTO local_shared_capture_package_records
    (candidate_id, component, object_id, chunk_index, record)
SELECT candidate_id, 'image', object_id, chunk_index, record
FROM local_shared_capture_package_image_chunks
WHERE candidate_id IN (SELECT candidate_id FROM local_shared_capture_publication);

-- A package without a descriptor commitment predates the publication format and
-- must never be re-encrypted under the same candidate. The impossible commitment
-- keeps it fail-closed until explicit cancellation and recapture.
UPDATE local_shared_capture_journal
SET frozen_descriptor_commitment = zeroblob(32)
WHERE frozen_descriptor_commitment IS NULL
  AND candidate_id IN (SELECT candidate_id FROM local_shared_capture_packages);

DROP TABLE local_shared_capture_publication_old;
DROP TABLE local_shared_capture_package_image_chunks;
DROP TABLE local_shared_capture_package_images;
DROP TABLE local_shared_capture_package_chunks;
DROP TABLE local_shared_capture_packages;
