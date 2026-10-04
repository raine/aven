use super::test_support::*;
use super::*;
use crate::operations::TaskUpdate;

#[tokio::test]
async fn encrypted_package_round_trips_and_retries_exact_bytes() {
    let (source_dir, source, task_id) = source_with_history().await;
    let durable = source
        .capture_local_shared_state_never_dispatched()
        .await
        .unwrap();
    let original_snapshot = sorted_tables(durable.shared_state());
    let key = package_key();
    let first = source
        .package_local_shared_state_never_dispatched(
            source_dir.path(),
            package_context(),
            &key,
            [0x64; 32],
        )
        .await
        .unwrap();
    let mut conn = source.acquire_writer().await.unwrap();
    let records: Vec<Vec<u8>> = sqlx::query_scalar(
        "SELECT record FROM local_shared_capture_package_records
         ORDER BY component, object_id, chunk_index",
    )
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    let snapshot: Vec<u8> =
        sqlx::query_scalar("SELECT snapshot FROM local_shared_capture_documents")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    let snapshot_json = crate::sync::shared_state::unpack_document(&snapshot).unwrap();
    drop(conn);
    assert!(records.iter().all(|record| {
        !record
            .windows(32)
            .any(|window| window == key.protected_storage_bytes())
    }));
    assert!(
        !snapshot_json
            .as_bytes()
            .windows(32)
            .any(|window| window == key.protected_storage_bytes())
    );
    assert!(records.iter().all(|record| {
        !record
            .windows("captured title".len())
            .any(|window| window == b"captured title")
    }));

    let mut conn = source.acquire_writer().await.unwrap();
    let workspace = crate::workspaces::ensure_default_workspace(&mut conn)
        .await
        .unwrap();
    drop(conn);
    source
        .update_task(
            &workspace,
            &task_id.parse().unwrap(),
            TaskUpdate {
                title: Some("after package".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let source_path = source_dir.path().join("source.sqlite");
    drop(source);
    let source = Database::open(&source_path).await.unwrap();
    let retry = source
        .package_local_shared_state_never_dispatched(
            source_dir.path(),
            package_context(),
            &key,
            [0x64; 32],
        )
        .await
        .unwrap();
    assert_eq!(first, retry);
    let decrypted = publication::decrypt_capture(&retry.upload_package(), &key).unwrap();
    assert_eq!(sorted_tables(&decrypted), original_snapshot);
    let mut different_context = package_context();
    different_context.generation_id[0] ^= 1;
    assert!(
        source
            .package_local_shared_state_never_dispatched(
                source_dir.path(),
                different_context,
                &key,
                [0x64; 32]
            )
            .await
            .is_err()
    );
    assert!(
        source
            .package_local_shared_state_never_dispatched(
                source_dir.path(),
                package_context(),
                &LocalSharedStatePackageKey::new([0x99; 32]),
                [0x64; 32]
            )
            .await
            .is_err()
    );
    assert_eq!(
        first,
        source
            .package_local_shared_state_never_dispatched(
                source_dir.path(),
                package_context(),
                &key,
                [0x64; 32]
            )
            .await
            .unwrap()
    );

    let target_dir = tempfile::tempdir().unwrap();
    let target = Database::open(&target_dir.path().join("target.sqlite"))
        .await
        .unwrap();
    let report = target
        .decrypt_and_install_local_shared_state_package(&retry, &key)
        .await
        .unwrap();
    let installed = target
        .export_data("2026-09-22T00:00:01Z".into())
        .await
        .unwrap();
    assert_eq!(report.prefix_count as usize, installed.tables.changes.len());
    assert_eq!(
        installed
            .tables
            .tasks
            .iter()
            .find(|task| task.id.to_string() == task_id)
            .unwrap()
            .title,
        "captured title"
    );
    assert_eq!(
        installed.tables.shared_history_provenance.len(),
        installed.tables.changes.len()
    );
    assert!(installed.tables.changes.len() >= 2);

    assert!(
        source
            .cancel_local_shared_state_never_dispatched(retry.candidate_id())
            .await
            .unwrap()
    );
    let mut conn = source.acquire_writer().await.unwrap();
    let remaining: (i64, i64, i64) = sqlx::query_as(
        "SELECT
             (SELECT count(*) FROM local_shared_capture_journal),
             (SELECT count(*) FROM local_shared_capture_publication),
             (SELECT count(*) FROM local_shared_capture_package_records)",
    )
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(remaining, (0, 0, 0));
}

#[tokio::test]
async fn selected_images_round_trip_and_retry_exact_persisted_ciphertext_after_reopen() {
    let (source_dir, source, task_id) = source_with_history().await;
    let (current, extra, extra_hash) =
        add_selected_images(source_dir.path(), &source, &task_id).await;
    source
        .capture_local_shared_state_never_dispatched()
        .await
        .unwrap();
    let key = package_key();
    let first = source
        .package_local_shared_state_never_dispatched(
            source_dir.path(),
            package_context(),
            &key,
            [0x64; 32],
        )
        .await
        .unwrap();
    let upload = first.upload_package();
    assert_eq!(upload.images.len(), 2);
    assert_ne!(upload.images[0].object_id, upload.images[1].object_id);
    let current_hash = crate::attachments::storage::sha256_hex(&current);
    let recovered = publication::decrypt_images(&upload, &key)
        .unwrap()
        .into_iter()
        .map(|(sha256, bytes)| (sha256, bytes.to_vec()))
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(
        recovered,
        std::collections::HashMap::from([
            (current_hash.clone(), current),
            (extra_hash.clone(), extra)
        ])
    );

    let mut conn = source.acquire_reader().await.unwrap();
    let frozen: Vec<(Vec<u8>, i64, Vec<u8>)> = sqlx::query_as(
        "SELECT object_id, chunk_index, record
         FROM local_shared_capture_package_records WHERE component = 'image'
         ORDER BY object_id, chunk_index",
    )
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    drop(conn);
    // Private plaintext hashes never appear in clear frozen bytes.
    let clear = [&upload.descriptor]
        .into_iter()
        .chain(&upload.catalogs)
        .chain(frozen.iter().map(|(_, _, record)| record));
    for bytes in clear {
        for hash in [&current_hash, &extra_hash] {
            assert!(
                !bytes
                    .windows(hash.len())
                    .any(|window| window == hash.as_bytes())
            );
        }
    }

    let path = source_dir.path().join("source.sqlite");
    drop(source);
    let reopened = Database::open(&path).await.unwrap();
    let retry = reopened
        .package_local_shared_state_never_dispatched(
            source_dir.path(),
            package_context(),
            &key,
            [0x64; 32],
        )
        .await
        .unwrap();
    assert_eq!(retry, first);
    let mut conn = reopened.acquire_reader().await.unwrap();
    let retried: Vec<(Vec<u8>, i64, Vec<u8>)> = sqlx::query_as(
        "SELECT object_id, chunk_index, record
         FROM local_shared_capture_package_records WHERE component = 'image'
         ORDER BY object_id, chunk_index",
    )
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    assert_eq!(retried, frozen);
    drop(conn);
    assert!(
        reopened
            .cancel_local_shared_state_never_dispatched(retry.candidate_id())
            .await
            .unwrap()
    );
    let mut conn = reopened.acquire_reader().await.unwrap();
    let remaining: (i64, i64, i64) = sqlx::query_as(
        "SELECT
             (SELECT count(*) FROM local_shared_capture_publication),
             (SELECT count(*) FROM local_shared_capture_package_records),
             (SELECT count(*) FROM local_shared_capture_pins)",
    )
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(remaining, (0, 0, 0));
}

#[tokio::test]
async fn incomplete_image_package_rejects_selected_capture_without_rewrite() {
    let (source_dir, source, task_id) = source_with_history().await;
    add_selected_images(source_dir.path(), &source, &task_id).await;
    source
        .capture_local_shared_state_never_dispatched()
        .await
        .unwrap();
    let key = package_key();
    source
        .package_local_shared_state_never_dispatched(
            source_dir.path(),
            package_context(),
            &key,
            [0x64; 32],
        )
        .await
        .unwrap();
    let mut conn = source.acquire_writer().await.unwrap();
    sqlx::query(
        "DELETE FROM local_shared_capture_package_records
         WHERE component = 'image' AND object_id = (
             SELECT min(object_id) FROM local_shared_capture_package_records
             WHERE component = 'image'
         )",
    )
    .execute(&mut *conn)
    .await
    .unwrap();
    let frozen: Vec<(String, Vec<u8>, i64, Vec<u8>)> = sqlx::query_as(
        "SELECT component, object_id, chunk_index, record
         FROM local_shared_capture_package_records
         ORDER BY component, object_id, chunk_index",
    )
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    drop(conn);

    for _ in 0..2 {
        let error = source
            .package_local_shared_state_never_dispatched(
                source_dir.path(),
                package_context(),
                &key,
                [0x64; 32],
            )
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("frozen-records-invalid"),
            "{error:#}"
        );
    }

    let mut conn = source.acquire_reader().await.unwrap();
    let after: Vec<(String, Vec<u8>, i64, Vec<u8>)> = sqlx::query_as(
        "SELECT component, object_id, chunk_index, record
         FROM local_shared_capture_package_records
         ORDER BY component, object_id, chunk_index",
    )
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    let counts: (i64, i64) = sqlx::query_as(
        "SELECT
             (SELECT count(*) FROM local_shared_capture_publication),
             (SELECT count(*) FROM local_shared_capture_pins)",
    )
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(after, frozen);
    assert_eq!(counts, (1, 2));
}

#[tokio::test]
async fn corrupt_selected_source_preserves_pins_and_allows_cancellation_cleanup() {
    for corrupt in [true] {
        let (source_dir, source, task_id) = source_with_history().await;
        let (_, _, extra_hash) = add_selected_images(source_dir.path(), &source, &task_id).await;
        let capture = source
            .capture_local_shared_state_never_dispatched()
            .await
            .unwrap();
        let path =
            crate::attachments::storage::object_path(source_dir.path(), &extra_hash).unwrap();
        if corrupt {
            std::fs::write(&path, b"corrupt").unwrap();
        }
        let error = source
            .package_local_shared_state_never_dispatched(
                source_dir.path(),
                package_context(),
                &package_key(),
                [0x64; 32],
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("size-mismatch"));
        let mut conn = source.acquire_reader().await.unwrap();
        let counts: (i64, i64) = sqlx::query_as(
            "SELECT
                 (SELECT count(*) FROM local_shared_capture_publication),
                 (SELECT count(*) FROM local_shared_capture_pins)",
        )
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(counts, (0, 2));
        drop(conn);
        assert!(
            source
                .cancel_local_shared_state_never_dispatched(capture.candidate_id())
                .await
                .unwrap()
        );
        let mut conn = source.acquire_reader().await.unwrap();
        let pins: i64 = sqlx::query_scalar("SELECT count(*) FROM local_shared_capture_pins")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(pins, 0);
    }
}

async fn freeze_and_pin_counts(database: &Database) -> (i64, i64, i64) {
    let mut conn = database.acquire_reader().await.unwrap();
    sqlx::query_as(
        "SELECT
             (SELECT count(*) FROM local_shared_capture_journal
              WHERE frozen_descriptor_commitment IS NOT NULL),
             (SELECT count(*) FROM local_shared_capture_publication),
             (SELECT count(*) FROM local_shared_capture_pins)",
    )
    .fetch_one(&mut *conn)
    .await
    .unwrap()
}

#[tokio::test]
async fn setup_capture_classifies_a_missing_file_as_unavailable() {
    let (dir, database, task_id) = source_with_history().await;
    let (current, _, _) = add_selected_images(dir.path(), &database, &task_id).await;
    let current_hash = crate::attachments::storage::sha256_hex(&current);
    let current_path = crate::attachments::storage::object_path(dir.path(), &current_hash).unwrap();
    std::fs::remove_file(current_path).unwrap();

    let capture = database
        .capture_local_shared_state_for_setup(dir.path())
        .await
        .unwrap();
    let classification: String = sqlx::query_scalar(
        "SELECT classification FROM local_shared_capture_images
         WHERE candidate_id = ? AND sha256 = ?",
    )
    .bind(capture.candidate_id())
    .bind(current_hash)
    .fetch_one(&mut *database.acquire_reader().await.unwrap())
    .await
    .unwrap();
    assert_eq!(classification, "unavailable");
}

#[tokio::test]
async fn capture_pins_unverified_selected_bytes_and_packaging_is_the_byte_gate() {
    for case in ["missing-current", "corrupt-extra", "dimension-metadata"] {
        let (dir, database, task_id) = source_with_history().await;
        let (current, extra, extra_hash) =
            add_selected_images(dir.path(), &database, &task_id).await;
        let current_hash = crate::attachments::storage::sha256_hex(&current);
        let current_path =
            crate::attachments::storage::object_path(dir.path(), &current_hash).unwrap();
        let extra_path = crate::attachments::storage::object_path(dir.path(), &extra_hash).unwrap();
        let expected_error = match case {
            "missing-current" => {
                std::fs::remove_file(&current_path).unwrap();
                ""
            }
            "corrupt-extra" => {
                let mut corrupt = extra.clone();
                *corrupt.last_mut().unwrap() ^= 1;
                std::fs::write(&extra_path, corrupt).unwrap();
                "selected-image-hash-mismatch"
            }
            _ => {
                let mut conn = database.acquire_writer().await.unwrap();
                sqlx::query("UPDATE task_attachments SET width = 3 WHERE sha256 = ?")
                    .bind(&current_hash)
                    .execute(&mut *conn)
                    .await
                    .unwrap();
                "selected-image-metadata-mismatch"
            }
        };

        // The general capture pins inventory-selected bytes. Packaging may
        // still downgrade a file that vanished before the package froze.
        let capture = database
            .capture_local_shared_state_never_dispatched()
            .await
            .unwrap();
        let mut conn = database.acquire_reader().await.unwrap();
        let classes: Vec<(String, String)> = sqlx::query_as(
            "SELECT sha256, classification FROM local_shared_capture_images ORDER BY sha256",
        )
        .fetch_all(&mut *conn)
        .await
        .unwrap();
        drop(conn);
        let mut expected_classes = vec![
            (current_hash.clone(), "current_selected".to_string()),
            (extra_hash.clone(), "extra_selected".to_string()),
            ("cd".repeat(32), "unavailable".to_string()),
        ];
        expected_classes.sort();
        assert_eq!(classes, expected_classes, "{case}");
        assert_eq!(freeze_and_pin_counts(&database).await, (0, 0, 2), "{case}");

        if case == "missing-current" {
            let frozen = database
                .package_local_shared_state_never_dispatched(
                    dir.path(),
                    package_context(),
                    &package_key(),
                    [0x64; 32],
                )
                .await
                .unwrap();
            assert_eq!(freeze_and_pin_counts(&database).await, (1, 1, 1), "{case}");
            std::fs::remove_file(&extra_path).unwrap();
            let retry = database
                .package_local_shared_state_never_dispatched(
                    dir.path(),
                    package_context(),
                    &package_key(),
                    [0x64; 32],
                )
                .await
                .unwrap();
            assert!(retry == frozen);
            continue;
        }

        for _ in 0..2 {
            let error = database
                .package_local_shared_state_never_dispatched(
                    dir.path(),
                    package_context(),
                    &package_key(),
                    [0x64; 32],
                )
                .await
                .unwrap_err();
            assert!(
                error.to_string().contains(expected_error),
                "{case}: {error:#}"
            );
            assert_eq!(freeze_and_pin_counts(&database).await, (0, 0, 2), "{case}");
        }

        let candidate = if case == "dimension-metadata" {
            // Captured metadata cannot be repaired in place: cancel before any
            // intent exists, fix the source, and recapture a new candidate.
            assert!(
                database
                    .cancel_local_shared_state_never_dispatched(capture.candidate_id())
                    .await
                    .unwrap()
            );
            assert_eq!(freeze_and_pin_counts(&database).await, (0, 0, 0), "{case}");
            let mut conn = database.acquire_writer().await.unwrap();
            sqlx::query("UPDATE task_attachments SET width = 2 WHERE sha256 = ?")
                .bind(&current_hash)
                .execute(&mut *conn)
                .await
                .unwrap();
            drop(conn);
            let recapture = database
                .capture_local_shared_state_never_dispatched()
                .await
                .unwrap();
            assert_ne!(recapture.candidate_id(), capture.candidate_id());
            recapture.candidate_id().to_string()
        } else {
            // Restoring the exact source bytes resumes the same candidate.
            std::fs::write(&current_path, &current).unwrap();
            std::fs::write(&extra_path, &extra).unwrap();
            capture.candidate_id().to_string()
        };
        let frozen = database
            .package_local_shared_state_never_dispatched(
                dir.path(),
                package_context(),
                &package_key(),
                [0x64; 32],
            )
            .await
            .unwrap();
        assert_eq!(frozen.candidate_id(), candidate, "{case}");
        assert_eq!(freeze_and_pin_counts(&database).await, (1, 1, 2), "{case}");
        std::fs::remove_file(&extra_path).unwrap();
        let retry = database
            .package_local_shared_state_never_dispatched(
                dir.path(),
                package_context(),
                &package_key(),
                [0x64; 32],
            )
            .await
            .unwrap();
        assert!(retry == frozen, "{case}");
    }
}

#[tokio::test]
async fn wrong_key_and_interrupted_persistence_leave_no_install_or_partial_package() {
    let (source_dir, source, task_id) = source_with_history().await;
    add_selected_images(source_dir.path(), &source, &task_id).await;
    source
        .capture_local_shared_state_never_dispatched()
        .await
        .unwrap();
    let mut conn = source.acquire_writer().await.unwrap();
    sqlx::query(
        "CREATE TRIGGER fail_image_chunk BEFORE INSERT ON local_shared_capture_package_records
         WHEN NEW.component = 'image'
         BEGIN SELECT RAISE(ABORT, 'injected package failure'); END",
    )
    .execute(&mut *conn)
    .await
    .unwrap();
    drop(conn);

    assert!(
        source
            .package_local_shared_state_never_dispatched(
                source_dir.path(),
                package_context(),
                &package_key(),
                [0x64; 32]
            )
            .await
            .is_err()
    );
    let mut conn = source.acquire_writer().await.unwrap();
    let package_counts: (i64, i64, i64) = sqlx::query_as(
        "SELECT
             (SELECT count(*) FROM local_shared_capture_publication),
             (SELECT count(*) FROM local_shared_capture_package_records),
             (SELECT count(*) FROM local_shared_capture_journal
              WHERE frozen_descriptor_commitment IS NOT NULL)",
    )
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(package_counts, (0, 0, 0));
    sqlx::query("DROP TRIGGER fail_image_chunk")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);

    let package = source
        .package_local_shared_state_never_dispatched(
            source_dir.path(),
            package_context(),
            &package_key(),
            [0x64; 32],
        )
        .await
        .unwrap();
    let target_dir = tempfile::tempdir().unwrap();
    let target = Database::open(&target_dir.path().join("target.sqlite"))
        .await
        .unwrap();
    let wrong_key = LocalSharedStatePackageKey::new([0x99; 32]);
    assert!(
        target
            .decrypt_and_install_local_shared_state_package(&package, &wrong_key)
            .await
            .is_err()
    );
    let empty = target
        .export_data("2026-09-22T00:00:02Z".into())
        .await
        .unwrap();
    assert!(empty.tables.tasks.is_empty());
    assert!(empty.tables.changes.is_empty());
}

#[test]
fn chunk_authentication_rejects_tampering_truncation_and_reordering() {
    let context = package_context();
    let stream = [0x64; 32];
    let object_id = [0x75; 32];
    let key = package_key();
    let derived = derive_image_key(&key, context, object_id).unwrap();
    let mut other_generation = context;
    other_generation.generation_id[0] ^= 1;
    assert_ne!(
        derived.as_ref(),
        derive_image_key(&key, other_generation, object_id)
            .unwrap()
            .as_ref()
    );
    assert_ne!(
        derived.as_ref(),
        derive_image_key(&key, context, [0x76; 32])
            .unwrap()
            .as_ref()
    );
    let plaintext = vec![0x86; CHUNK_PLAINTEXT_BYTES + 17];
    let artifact = encrypt_artifact(
        &plaintext,
        context,
        stream,
        object_id,
        IMAGE_FAMILY,
        IMAGE_CLASS,
        &derived,
    )
    .unwrap();
    assert_eq!(artifact.chunks.len(), 2);
    assert_eq!(
        decrypt_artifact(
            &artifact,
            context,
            stream,
            object_id,
            IMAGE_FAMILY,
            IMAGE_CLASS,
            &derived,
            plaintext.len(),
        )
        .unwrap(),
        plaintext
    );

    let mut wrong_context = context;
    wrong_context.generation_id[0] ^= 1;
    assert!(
        decrypt_artifact(
            &artifact,
            wrong_context,
            stream,
            object_id,
            IMAGE_FAMILY,
            IMAGE_CLASS,
            &derived,
            plaintext.len(),
        )
        .is_err()
    );
    assert!(
        decrypt_artifact(
            &artifact,
            context,
            stream,
            [0x76; 32],
            IMAGE_FAMILY,
            IMAGE_CLASS,
            &derived,
            plaintext.len(),
        )
        .is_err()
    );

    let mut tampered = artifact.clone();
    tampered.chunks[0].record.to_mut()[CHUNK_RECORD_OVERHEAD] ^= 1;
    tampered.chunks[0].record_commitment = crate::sync::codec::hash(&tampered.chunks[0].record);
    tampered.aggregate_commitment = aggregate_commitment(&tampered.chunks);
    assert!(
        decrypt_artifact(
            &tampered,
            context,
            stream,
            object_id,
            IMAGE_FAMILY,
            IMAGE_CLASS,
            &derived,
            plaintext.len(),
        )
        .is_err()
    );

    let mut truncated = artifact.clone();
    truncated.chunks[1].record.to_mut().pop();
    truncated.chunks[1].record_commitment = crate::sync::codec::hash(&truncated.chunks[1].record);
    truncated.aggregate_commitment = aggregate_commitment(&truncated.chunks);
    assert!(
        decrypt_artifact(
            &truncated,
            context,
            stream,
            object_id,
            IMAGE_FAMILY,
            IMAGE_CLASS,
            &derived,
            plaintext.len(),
        )
        .is_err()
    );

    let mut reordered = artifact.clone();
    reordered.chunks.swap(0, 1);
    reordered.aggregate_commitment = aggregate_commitment(&reordered.chunks);
    assert!(
        decrypt_artifact(
            &reordered,
            context,
            stream,
            object_id,
            IMAGE_FAMILY,
            IMAGE_CLASS,
            &derived,
            plaintext.len(),
        )
        .is_err()
    );
}

#[tokio::test]
async fn persisted_ciphertext_corruption_blocks_resume_without_repackaging() {
    let (source_dir, source, task_id) = source_with_history().await;
    add_selected_images(source_dir.path(), &source, &task_id).await;
    source
        .capture_local_shared_state_never_dispatched()
        .await
        .unwrap();
    source
        .package_local_shared_state_never_dispatched(
            source_dir.path(),
            package_context(),
            &package_key(),
            [0x64; 32],
        )
        .await
        .unwrap();
    let mut conn = source.acquire_writer().await.unwrap();
    sqlx::query(
        "UPDATE local_shared_capture_package_records
         SET record = substr(record, 1, length(record) - 1)
         WHERE component = 'image' AND chunk_index = 0",
    )
    .execute(&mut *conn)
    .await
    .unwrap();
    drop(conn);
    for _ in 0..2 {
        assert!(
            source
                .package_local_shared_state_never_dispatched(
                    source_dir.path(),
                    package_context(),
                    &package_key(),
                    [0x64; 32]
                )
                .await
                .is_err()
        );
    }
    assert_eq!(freeze_and_pin_counts(&source).await, (1, 1, 2));
}

fn sorted_tables(capture: &super::super::SharedStateCapture) -> serde_json::Value {
    let mut tables = serde_json::to_value(&capture.snapshot.tables).unwrap();
    for rows in tables.as_object_mut().unwrap().values_mut() {
        rows.as_array_mut()
            .unwrap()
            .sort_by_key(|row| row.to_string());
    }
    tables
}

#[test]
fn artifact_writer_encrypts_pieces_across_chunk_boundaries() {
    let context = package_context();
    let (stream, artifact_id, key) = ([0x31; 32], [0x32; 32], [0x33; 32]);
    let writer = || ArtifactWriter::new(context, stream, artifact_id, 2, 1, &key).unwrap();
    for total in [
        0,
        1,
        CHUNK_PLAINTEXT_BYTES,
        CHUNK_PLAINTEXT_BYTES + 1,
        2 * CHUNK_PLAINTEXT_BYTES + 5,
    ] {
        let plaintext = (0..total).map(|i| (i % 251) as u8).collect::<Vec<_>>();
        let mut streamed = writer();
        streamed.begin(total).unwrap();
        for piece in plaintext.chunks(65_537) {
            streamed.write(piece).unwrap();
        }
        let artifact = streamed.finish().unwrap();
        assert_eq!(
            artifact.chunks.len(),
            total.div_ceil(CHUNK_PLAINTEXT_BYTES).max(1)
        );
        let decrypted =
            decrypt_artifact(&artifact, context, stream, artifact_id, 2, 1, &key, total).unwrap();
        assert_eq!(decrypted, plaintext, "{total}");
    }
    let mut short = writer();
    short.begin(10).unwrap();
    short.write(&[0; 9]).unwrap();
    assert!(short.finish().is_err());
    let mut long = writer();
    long.begin(10).unwrap();
    assert!(long.write(&[0; 11]).is_err());
    let mut undeclared = writer();
    assert!(undeclared.write(&[0]).is_err());
}

#[test]
fn artifact_reader_authenticates_one_chunk_at_a_time() {
    let context = package_context();
    let (stream, artifact_id, key) = ([0x41; 32], [0x42; 32], [0x43; 32]);
    let total = 2 * CHUNK_PLAINTEXT_BYTES + 5;
    let plaintext = (0..total).map(|i| (i % 253) as u8).collect::<Vec<_>>();
    let artifact = encrypt_artifact(&plaintext, context, stream, artifact_id, 2, 1, &key).unwrap();
    let reader = |artifact| {
        ArtifactReader::new(artifact, context, stream, artifact_id, 2, 1, &key, total).unwrap()
    };

    let mut whole = reader(&artifact);
    let mut read = Vec::new();
    loop {
        assert_eq!(whole.remaining(), (total - read.len()) as u64);
        let piece = whole.take(100_003).unwrap();
        if piece.is_empty() {
            break;
        }
        assert!(piece.len() <= 100_003);
        read.extend_from_slice(piece);
    }
    assert_eq!(read, plaintext);
    assert!(whole.finish().is_ok());

    let mut partial = reader(&artifact);
    partial.take(usize::MAX).unwrap();
    assert!(partial.finish().is_err());

    // A forged middle chunk with consistent commitments is only refused when
    // it is reached, after the first chunk's plaintext was handed out.
    let mut forged = artifact.clone();
    let record = forged.chunks[1].record.to_mut();
    let last = record.len() - 1;
    record[last] ^= 1;
    forged.chunks[1].record_commitment = codec::hash(&forged.chunks[1].record);
    forged.aggregate_commitment = aggregate_commitment(&forged.chunks);
    let mut tampered = reader(&forged);
    assert_eq!(
        tampered.take(usize::MAX).unwrap(),
        &plaintext[..CHUNK_PLAINTEXT_BYTES]
    );
    assert!(tampered.take(usize::MAX).is_err());
    assert!(
        ArtifactReader::new(&forged, context, stream, artifact_id, 2, 1, &key, total - 1).is_err()
    );
}
