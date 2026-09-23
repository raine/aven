use std::io::Cursor;
use std::path::Path;

use image::{DynamicImage, ImageFormat, RgbaImage};

use crate::choices::TaskSource;
use crate::operations::{TaskDraft, TaskUpdate};

use super::*;

fn package_context() -> LocalSharedStatePackageContext {
    LocalSharedStatePackageContext {
        vault_id: [0x31; 32],
        generation_id: [0x42; 32],
    }
}

fn package_key() -> LocalSharedStatePackageKey {
    LocalSharedStatePackageKey::new([0x53; 32])
}

fn png_bytes(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(RgbaImage::new(width, height))
        .write_to(&mut bytes, ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

async fn source_with_history() -> (tempfile::TempDir, Database, String) {
    let dir = tempfile::tempdir().unwrap();
    let database = Database::open(&dir.path().join("source.sqlite"))
        .await
        .unwrap();
    let mut conn = database.acquire_writer().await.unwrap();
    let workspace = crate::workspaces::ensure_default_workspace(&mut conn)
        .await
        .unwrap();
    drop(conn);
    let task = database
        .create_task(
            &workspace,
            TaskDraft {
                title: "captured title".into(),
                description: "captured description".into(),
                project: Some("app".into()),
                status: "todo".into(),
                priority: "none".into(),
                source: TaskSource::Cli,
                labels: vec![],
                metadata: vec![],
                available_at: None,
                due_on: None,
                is_epic: false,
            },
        )
        .await
        .unwrap()
        .task;
    database
        .update_task(
            &workspace,
            &task.id,
            TaskUpdate {
                description: Some("history retained".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    (dir, database, task.id.to_string())
}

async fn add_selected_images(
    dir: &Path,
    database: &Database,
    task_id: &str,
) -> (Vec<u8>, Vec<u8>, String) {
    let current = png_bytes(2, 1);
    let extra = png_bytes(1, 2);
    let current_hash = crate::attachments::storage::sha256_hex(&current);
    let extra_hash = crate::attachments::storage::sha256_hex(&extra);
    for (hash, bytes) in [(&current_hash, &current), (&extra_hash, &extra)] {
        let path = crate::attachments::storage::object_path(dir, hash).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    let mut conn = database.acquire_writer().await.unwrap();
    crate::attachments::storage::upsert_inventory_available(
        &mut conn,
        &current_hash,
        i64::try_from(current.len()).unwrap(),
        "image/png",
    )
    .await
    .unwrap();
    crate::attachments::storage::upsert_inventory_available(
        &mut conn,
        &extra_hash,
        i64::try_from(extra.len()).unwrap(),
        "image/png",
    )
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO blob_inventory(
             sha256, byte_size, media_type, available, first_seen_at, last_verified_at
         ) VALUES (?, 9, 'image/png', 0, '2026-09-22T01:00:00Z', NULL)",
    )
    .bind("cd".repeat(32))
    .execute(&mut *conn)
    .await
    .unwrap();
    let workspace_id: String = sqlx::query_scalar("SELECT workspace_id FROM tasks WHERE id = ?")
        .bind(task_id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO task_attachments(
             workspace_id, attachment_id, task_id, sha256, byte_size, media_type,
             filename, alt_text, width, height, created_at, created_by_change_id,
             deleted, deleted_at, deleted_by_change_id
         ) VALUES (?, ?, ?, ?, ?, 'image/png', 'capture.png', NULL, 2, 1,
             '2026-09-22T01:00:00Z', NULL, 0, NULL, NULL)",
    )
    .bind(workspace_id)
    .bind(crate::ids::new_id())
    .bind(task_id)
    .bind(&current_hash)
    .bind(i64::try_from(current.len()).unwrap())
    .execute(&mut *conn)
    .await
    .unwrap();
    (current, extra, extra_hash)
}

#[tokio::test]
async fn encrypted_package_round_trips_and_retries_exact_bytes() {
    let (source_dir, source, task_id) = source_with_history().await;
    let durable = source
        .capture_local_shared_state_never_dispatched(source_dir.path())
        .await
        .unwrap();
    let original_snapshot = serde_json::to_value(&durable.shared_state().snapshot).unwrap();
    let key = package_key();
    let first = source
        .package_local_shared_state_never_dispatched(source_dir.path(), package_context(), &key)
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
        .package_local_shared_state_never_dispatched(source_dir.path(), package_context(), &key)
        .await
        .unwrap();
    assert_eq!(first, retry);
    let decrypted = decrypt_package(&retry, &key).unwrap();
    assert_eq!(
        serde_json::to_value(&decrypted.snapshot).unwrap(),
        original_snapshot
    );
    let mut different_context = package_context();
    different_context.generation_id[0] ^= 1;
    assert!(
        source
            .package_local_shared_state_never_dispatched(source_dir.path(), different_context, &key)
            .await
            .is_err()
    );
    assert!(
        source
            .package_local_shared_state_never_dispatched(
                source_dir.path(),
                package_context(),
                &LocalSharedStatePackageKey::new([0x99; 32])
            )
            .await
            .is_err()
    );
    assert_eq!(
        first,
        source
            .package_local_shared_state_never_dispatched(source_dir.path(), package_context(), &key)
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
        .capture_local_shared_state_never_dispatched(source_dir.path())
        .await
        .unwrap();
    let key = package_key();
    let first = source
        .package_local_shared_state_never_dispatched(source_dir.path(), package_context(), &key)
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
        .package_local_shared_state_never_dispatched(source_dir.path(), package_context(), &key)
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
async fn authentic_state_only_package_rejects_selected_image_capture_without_rewrite() {
    let (source_dir, source, task_id) = source_with_history().await;
    add_selected_images(source_dir.path(), &source, &task_id).await;
    let capture = source
        .capture_local_shared_state_never_dispatched(source_dir.path())
        .await
        .unwrap();
    let key = package_key();
    let state_only = encrypt_package(
        capture.candidate_id(),
        decode_context_id(capture.stream_id(), "stream").unwrap(),
        package_context(),
        capture.shared_state(),
        &[],
        &key,
    )
    .unwrap();
    let mut conn = source.acquire_writer().await.unwrap();
    let mut tx = crate::db::begin_immediate(&mut conn).await.unwrap();
    persist_package(&mut tx, &state_only).await.unwrap();
    tx.commit().await.unwrap();
    let frozen: Vec<(i64, i64, Vec<u8>)> = sqlx::query_as(
        "SELECT class, chunk_index, record
         FROM local_shared_capture_package_chunks ORDER BY class, chunk_index",
    )
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    drop(conn);

    let error = source
        .package_local_shared_state_never_dispatched(source_dir.path(), package_context(), &key)
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
            .capture_local_shared_state_never_dispatched(source_dir.path())
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

#[tokio::test]
async fn wrong_key_and_interrupted_persistence_leave_no_install_or_partial_package() {
    let (source_dir, source, task_id) = source_with_history().await;
    add_selected_images(source_dir.path(), &source, &task_id).await;
    source
        .capture_local_shared_state_never_dispatched(source_dir.path())
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
                &package_key()
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
        .capture_local_shared_state_never_dispatched(source_dir.path())
        .await
        .unwrap();
    source
        .package_local_shared_state_never_dispatched(
            source_dir.path(),
            package_context(),
            &package_key(),
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
                &package_key()
            )
            .await
            .is_err()
    );
}
