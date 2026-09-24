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
        "SELECT record FROM local_shared_capture_package_chunks ORDER BY class, chunk_index",
    )
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    let snapshot_json: String = sqlx::query_scalar(
        "SELECT snapshot_json FROM local_shared_capture_journal WHERE singleton = 1",
    )
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    drop(conn);
    assert!(
        records
            .iter()
            .all(|record| !record.windows(32).any(|window| window == key.expose()))
    );
    assert!(
        !snapshot_json
            .as_bytes()
            .windows(32)
            .any(|window| window == key.expose())
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
    let decrypted = decrypt_package(&retry, &key).unwrap();
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
             (SELECT count(*) FROM local_shared_capture_packages),
             (SELECT count(*) FROM local_shared_capture_package_chunks)",
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
    assert_eq!(first.images().len(), 2);
    assert_ne!(first.images()[0].object_id(), first.images()[1].object_id());
    let decrypted = decrypt_local_shared_state_package_images(&first, &key).unwrap();
    let recovered = decrypted
        .iter()
        .map(|image| (image.source_sha256().to_string(), image.bytes().to_vec()))
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(
        recovered[&crate::attachments::storage::sha256_hex(&current)],
        current
    );
    assert_eq!(recovered[&extra_hash], extra);
    assert!(
        decrypted
            .iter()
            .any(|image| image.classification() == "current_selected")
    );
    assert!(
        decrypted
            .iter()
            .any(|image| image.classification() == "extra_selected")
    );

    let mut conn = source.acquire_reader().await.unwrap();
    let frozen: Vec<(Vec<u8>, i64, Vec<u8>)> = sqlx::query_as(
        "SELECT object_id, chunk_index, record
         FROM local_shared_capture_package_image_chunks
         ORDER BY object_id, chunk_index",
    )
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    let persisted_hashes: Vec<String> = sqlx::query_scalar(
        "SELECT source_sha256 FROM local_shared_capture_package_images ORDER BY source_sha256",
    )
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    drop(conn);
    assert_eq!(persisted_hashes.len(), 2);
    assert!(!persisted_hashes.contains(&"cd".repeat(32)));
    for image in first.images() {
        for (_, record) in image.chunks() {
            assert!(!persisted_hashes.iter().any(|hash| {
                record
                    .windows(hash.len())
                    .any(|window| window == hash.as_bytes())
            }));
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
         FROM local_shared_capture_package_image_chunks
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
             (SELECT count(*) FROM local_shared_capture_package_images),
             (SELECT count(*) FROM local_shared_capture_package_image_chunks),
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
    sqlx::query("DELETE FROM local_shared_capture_package_images")
        .execute(&mut *conn)
        .await
        .unwrap();
    let frozen: Vec<(i64, i64, Vec<u8>)> = sqlx::query_as(
        "SELECT class, chunk_index, record
         FROM local_shared_capture_package_chunks ORDER BY class, chunk_index",
    )
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    drop(conn);

    let error = source
        .package_local_shared_state_never_dispatched(
            source_dir.path(),
            package_context(),
            &key,
            [0x64; 32],
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("image-coverage-mismatch"));

    let mut conn = source.acquire_reader().await.unwrap();
    let after: Vec<(i64, i64, Vec<u8>)> = sqlx::query_as(
        "SELECT class, chunk_index, record
         FROM local_shared_capture_package_chunks ORDER BY class, chunk_index",
    )
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    let counts: (i64, i64, i64) = sqlx::query_as(
        "SELECT
             (SELECT count(*) FROM local_shared_capture_packages),
             (SELECT count(*) FROM local_shared_capture_package_images),
             (SELECT count(*) FROM local_shared_capture_pins)",
    )
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(after, frozen);
    assert_eq!(counts, (1, 0, 2));
}

#[tokio::test]
async fn selected_source_failure_preserves_pins_and_allows_cancellation_cleanup() {
    for corrupt in [false, true] {
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
        } else {
            std::fs::remove_file(&path).unwrap();
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
        assert!(error.to_string().contains(if corrupt {
            "size-mismatch"
        } else {
            "selected-image-missing"
        }));
        let mut conn = source.acquire_reader().await.unwrap();
        let counts: (i64, i64) = sqlx::query_as(
            "SELECT
                 (SELECT count(*) FROM local_shared_capture_packages),
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
             (SELECT count(*) FROM local_shared_capture_packages),
             (SELECT count(*) FROM local_shared_capture_pins)",
    )
    .fetch_one(&mut *conn)
    .await
    .unwrap()
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
                "selected-image-missing"
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

        // Capture classifies metadata only; unverified selected bytes stay
        // selected and pinned rather than being downgraded to unavailable.
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
        "CREATE TRIGGER fail_image_chunk BEFORE INSERT ON local_shared_capture_package_image_chunks
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
    let package_counts: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
             (SELECT count(*) FROM local_shared_capture_packages),
             (SELECT count(*) FROM local_shared_capture_package_chunks),
             (SELECT count(*) FROM local_shared_capture_package_images),
             (SELECT count(*) FROM local_shared_capture_package_image_chunks)",
    )
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(package_counts, (0, 0, 0, 0));
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
    tampered.chunks[0].record[CHUNK_RECORD_OVERHEAD] ^= 1;
    tampered.chunks[0].record_commitment = sha256(&tampered.chunks[0].record);
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
    truncated.chunks[1].record.pop();
    truncated.chunks[1].record_commitment = sha256(&truncated.chunks[1].record);
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
        "UPDATE local_shared_capture_package_image_chunks
         SET record = substr(record, 1, length(record) - 1)
         WHERE chunk_index = 0",
    )
    .execute(&mut *conn)
    .await
    .unwrap_err();
    sqlx::query(
        "UPDATE local_shared_capture_package_image_chunks
         SET record_commitment = zeroblob(32)
         WHERE chunk_index = 0",
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
}

fn sorted_tables(capture: &SharedStateCapture) -> serde_json::Value {
    let mut tables = serde_json::to_value(&capture.snapshot.tables).unwrap();
    for rows in tables.as_object_mut().unwrap().values_mut() {
        rows.as_array_mut()
            .unwrap()
            .sort_by_key(|row| row.to_string());
    }
    tables
}
