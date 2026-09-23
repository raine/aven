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
        .package_local_shared_state_never_dispatched(package_context(), &key)
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
        .package_local_shared_state_never_dispatched(package_context(), &key)
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
            .package_local_shared_state_never_dispatched(different_context, &key)
            .await
            .is_err()
    );
    assert!(
        source
            .package_local_shared_state_never_dispatched(
                package_context(),
                &LocalSharedStatePackageKey::new([0x99; 32])
            )
            .await
            .is_err()
    );
    assert_eq!(
        first,
        source
            .package_local_shared_state_never_dispatched(package_context(), &key)
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
async fn wrong_key_and_interrupted_persistence_leave_no_install_or_partial_package() {
    let (source_dir, source, _) = source_with_history().await;
    source
        .capture_local_shared_state_never_dispatched(source_dir.path())
        .await
        .unwrap();
    let mut conn = source.acquire_writer().await.unwrap();
    sqlx::query(
        "CREATE TRIGGER fail_manifest_chunk BEFORE INSERT ON local_shared_capture_package_chunks
         WHEN NEW.class = 2 BEGIN SELECT RAISE(ABORT, 'injected package failure'); END",
    )
    .execute(&mut *conn)
    .await
    .unwrap();
    drop(conn);

    assert!(
        source
            .package_local_shared_state_never_dispatched(package_context(), &package_key())
            .await
            .is_err()
    );
    let mut conn = source.acquire_writer().await.unwrap();
    let package_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM local_shared_capture_packages")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(package_count, 0);
    sqlx::query("DROP TRIGGER fail_manifest_chunk")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);

    let package = source
        .package_local_shared_state_never_dispatched(package_context(), &package_key())
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
    let candidate = [0x75; 32];
    let key = package_key();
    let derived = derive_class_key(&key, context, stream, candidate, STATE_CLASS).unwrap();
    let plaintext = vec![0x86; CHUNK_PLAINTEXT_BYTES + 17];
    let artifact = encrypt_artifact(
        &plaintext,
        context,
        stream,
        candidate,
        STATE_CLASS,
        &derived,
    )
    .unwrap();
    assert_eq!(artifact.chunks.len(), 2);
    assert_eq!(
        decrypt_artifact(
            &artifact,
            context,
            stream,
            candidate,
            STATE_CLASS,
            &derived,
            plaintext.len(),
        )
        .unwrap(),
        plaintext
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
            candidate,
            STATE_CLASS,
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
            candidate,
            STATE_CLASS,
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
            candidate,
            STATE_CLASS,
            &derived,
            plaintext.len(),
        )
        .is_err()
    );
}

#[tokio::test]
async fn persisted_ciphertext_corruption_blocks_resume_without_repackaging() {
    let (source_dir, source, _) = source_with_history().await;
    source
        .capture_local_shared_state_never_dispatched(source_dir.path())
        .await
        .unwrap();
    source
        .package_local_shared_state_never_dispatched(package_context(), &package_key())
        .await
        .unwrap();
    let mut conn = source.acquire_writer().await.unwrap();
    sqlx::query(
        "UPDATE local_shared_capture_package_chunks
         SET record = substr(record, 1, length(record) - 1)
         WHERE class = 1 AND chunk_index = 0",
    )
    .execute(&mut *conn)
    .await
    .unwrap_err();
    sqlx::query(
        "UPDATE local_shared_capture_package_chunks
         SET record_commitment = zeroblob(32)
         WHERE class = 1 AND chunk_index = 0",
    )
    .execute(&mut *conn)
    .await
    .unwrap();
    drop(conn);
    assert!(
        source
            .package_local_shared_state_never_dispatched(package_context(), &package_key())
            .await
            .is_err()
    );
}
