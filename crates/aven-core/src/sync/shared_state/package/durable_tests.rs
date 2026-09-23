use super::test_support::*;
use super::*;

fn fingerprint(package: &EncryptedLocalSharedStatePackage) -> [u8; 32] {
    let upload = package.upload_package();
    let mut digest = Sha256::new();
    for bytes in std::iter::once(&upload.descriptor)
        .chain(upload.catalogs.iter())
        .chain(upload.state.iter())
        .chain(upload.manifest.iter())
        .chain(upload.images.iter().flat_map(|image| &image.records))
    {
        digest.update((bytes.len() as u64).to_be_bytes());
        digest.update(bytes);
    }
    digest.finalize().into()
}

#[tokio::test]
async fn frozen_upload_survives_edits_source_loss_and_reopen() {
    let (dir, database, task) = source_with_history().await;
    let (current, _, extra_hash) = add_selected_images(dir.path(), &database, &task).await;
    let capture = database
        .capture_local_shared_state_never_dispatched(dir.path())
        .await
        .unwrap();
    let key = package_key();
    let frozen = database
        .package_local_shared_state_never_dispatched(dir.path(), package_context(), &key, [7; 32])
        .await
        .unwrap();
    let exact = frozen.upload_package();
    assert_eq!(
        publication::validate_keyless(&exact).unwrap().image_count,
        2
    );
    publication::authenticate(
        &exact,
        &key,
        package_context(),
        *frozen.stream_id(),
        decode_context_id(capture.candidate_id(), "candidate").unwrap(),
        [7; 32],
    )
    .unwrap();
    let mut conn = database.acquire_writer().await.unwrap();
    let workspace = crate::workspaces::ensure_default_workspace(&mut conn)
        .await
        .unwrap();
    drop(conn);
    database
        .update_task(
            &workspace,
            &task.parse().unwrap(),
            crate::operations::TaskUpdate {
                title: Some("later title".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let current_hash = crate::attachments::storage::sha256_hex(&current);
    std::fs::write(
        crate::attachments::storage::object_path(dir.path(), &current_hash).unwrap(),
        b"changed after freeze",
    )
    .unwrap();
    std::fs::remove_file(
        crate::attachments::storage::object_path(dir.path(), &extra_hash).unwrap(),
    )
    .unwrap();
    drop(database);
    let database = Database::open(&dir.path().join("source.sqlite"))
        .await
        .unwrap();
    let retry = database
        .package_local_shared_state_never_dispatched(dir.path(), package_context(), &key, [7; 32])
        .await
        .unwrap();
    assert!(retry.upload_package() == exact);
    for predecessor in [[0; 32], [8; 32]] {
        assert!(
            database
                .package_local_shared_state_never_dispatched(
                    dir.path(),
                    package_context(),
                    &key,
                    predecessor,
                )
                .await
                .is_err()
        );
    }
    for (context, stream, bootstrap) in [
        (
            LocalSharedStatePackageContext {
                vault_id: [1; 32],
                ..package_context()
            },
            *retry.stream_id(),
            decode_context_id(retry.candidate_id(), "candidate").unwrap(),
        ),
        (
            package_context(),
            [2; 32],
            decode_context_id(retry.candidate_id(), "candidate").unwrap(),
        ),
        (package_context(), *retry.stream_id(), [3; 32]),
    ] {
        assert!(
            publication::authenticate(&exact, &key, context, stream, bootstrap, [7; 32]).is_err()
        );
    }
    assert!(
        publication::authenticate(
            &exact,
            &LocalSharedStatePackageKey::new([9; 32]),
            package_context(),
            *retry.stream_id(),
            decode_context_id(retry.candidate_id(), "candidate").unwrap(),
            [7; 32]
        )
        .is_err()
    );
    let decoded = decrypt_package(&retry, &key).unwrap();
    assert_eq!(decoded.snapshot.tables.tasks[0].title, "captured title");
    assert_eq!(
        decrypt_local_shared_state_package_images(&retry, &key)
            .unwrap()
            .len(),
        2
    );
    let target = Database::open(&dir.path().join("target.sqlite"))
        .await
        .unwrap();
    let installed = target
        .decrypt_and_install_local_shared_state_package(&retry, &key)
        .await
        .unwrap();
    assert_eq!(installed.attachment_count, 1);
    let exported = target
        .export_data("2026-09-22T00:00:00Z".into())
        .await
        .unwrap();
    assert_eq!(exported.tables.tasks[0].title, "captured title");
    assert_eq!(
        exported.tables.changes.len(),
        decoded.snapshot.tables.changes.len()
    );
    assert!(
        exported
            .tables
            .blob_inventory
            .iter()
            .all(|image| image.available == 0)
    );
    assert_eq!(exported.tables.task_attachments[0].sha256, current_hash);
    database
        .cancel_local_shared_state_never_dispatched(retry.candidate_id())
        .await
        .unwrap();
    let mut conn = database.acquire_reader().await.unwrap();
    let counts: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM local_shared_capture_publication),
                (SELECT count(*) FROM local_shared_capture_packages),
                (SELECT count(*) FROM local_shared_capture_package_image_chunks),
                (SELECT count(*) FROM local_shared_capture_pins)",
    )
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(counts, (0, 0, 0, 0));
}

#[tokio::test]
async fn prior_local_only_storage_requires_explicit_cancel_and_new_identity() {
    let (dir, database, task) = source_with_history().await;
    add_selected_images(dir.path(), &database, &task).await;
    let capture = database
        .capture_local_shared_state_never_dispatched(dir.path())
        .await
        .unwrap();
    let mut conn = database.acquire_writer().await.unwrap();
    // The local-only storage profile has no descriptor/catalog row or freeze
    // commitment. Refusal must happen before interpreting its encrypted bytes.
    sqlx::query(
        "INSERT INTO local_shared_capture_packages (
            candidate_id, format_version, suite, vault_id, generation_id,
            state_total_plaintext_bytes, state_chunk_count, state_aggregate_commitment,
            manifest_total_plaintext_bytes, manifest_chunk_count,
            manifest_aggregate_commitment, created_at
        ) VALUES (?, 1, 1, ?, ?, 1, 1, zeroblob(32), 1, 1, zeroblob(32), 'legacy')",
    )
    .bind(capture.candidate_id())
    .bind(package_context().vault_id.as_slice())
    .bind(package_context().generation_id.as_slice())
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO local_shared_capture_package_chunks
            (candidate_id, class, chunk_index, record_length, record_commitment, record)
         VALUES (?, 1, 0, 4, zeroblob(32), x'01020304')",
    )
    .bind(capture.candidate_id())
    .execute(&mut *conn)
    .await
    .unwrap();
    drop(conn);
    for _ in 0..2 {
        let error = database
            .package_local_shared_state_never_dispatched(
                dir.path(),
                package_context(),
                &package_key(),
                [7; 32],
            )
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("cancel-never-dispatched-capture-and-recapture")
        );
    }
    let mut conn = database.acquire_reader().await.unwrap();
    let record: Vec<u8> =
        sqlx::query_scalar("SELECT record FROM local_shared_capture_package_chunks")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(record, [1, 2, 3, 4]);
    let pins: i64 = sqlx::query_scalar("SELECT count(*) FROM local_shared_capture_pins")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(pins, 2);
    drop(conn);
    database
        .cancel_local_shared_state_never_dispatched(capture.candidate_id())
        .await
        .unwrap();
    let recapture = database
        .capture_local_shared_state_never_dispatched(dir.path())
        .await
        .unwrap();
    assert_ne!(recapture.candidate_id(), capture.candidate_id());
    assert_ne!(recapture.stream_id(), capture.stream_id());
    database
        .package_local_shared_state_never_dispatched(
            dir.path(),
            package_context(),
            &package_key(),
            [7; 32],
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn corrupt_or_missing_frozen_components_never_trigger_replacement() {
    for mutation in [
        "DELETE FROM local_shared_capture_packages",
        "DELETE FROM local_shared_capture_publication",
        "UPDATE local_shared_capture_publication SET descriptor = zeroblob(length(descriptor))",
        "UPDATE local_shared_capture_publication SET prefix_catalog = zeroblob(length(prefix_catalog))",
        "UPDATE local_shared_capture_packages SET state_total_plaintext_bytes = state_total_plaintext_bytes + 1",
        "UPDATE local_shared_capture_package_chunks SET record_commitment = zeroblob(32)",
        "DELETE FROM local_shared_capture_package_image_chunks",
        "UPDATE local_shared_capture_package_images SET total_plaintext_bytes = total_plaintext_bytes + 1",
    ] {
        let (dir, database, task) = source_with_history().await;
        add_selected_images(dir.path(), &database, &task).await;
        database
            .capture_local_shared_state_never_dispatched(dir.path())
            .await
            .unwrap();
        let frozen = database
            .package_local_shared_state_never_dispatched(
                dir.path(),
                package_context(),
                &package_key(),
                [7; 32],
            )
            .await
            .unwrap();
        let mut conn = database.acquire_writer().await.unwrap();
        sqlx::query(mutation).execute(&mut *conn).await.unwrap();
        drop(conn);
        drop(database);
        let database = Database::open(&dir.path().join("source.sqlite"))
            .await
            .unwrap();
        assert!(
            database
                .has_local_shared_state_package_never_dispatched()
                .await
                .unwrap()
        );
        for _ in 0..2 {
            assert!(
                database
                    .package_local_shared_state_never_dispatched(
                        dir.path(),
                        package_context(),
                        &package_key(),
                        [7; 32],
                    )
                    .await
                    .is_err(),
                "{mutation}"
            );
        }
        let mut conn = database.acquire_reader().await.unwrap();
        let commitment: Vec<u8> = sqlx::query_scalar(
            "SELECT frozen_descriptor_commitment FROM local_shared_capture_journal",
        )
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(commitment, sha256(&frozen.descriptor));
        let pins: i64 = sqlx::query_scalar("SELECT count(*) FROM local_shared_capture_pins")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(pins, 2);
        drop(conn);
        database
            .cancel_local_shared_state_never_dispatched(frozen.candidate_id())
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn process_exit_before_and_after_freeze_commit_preserves_ownership() {
    for committed in [false, true] {
        let (dir, database, task) = source_with_history().await;
        add_selected_images(dir.path(), &database, &task).await;
        let capture = database
            .capture_local_shared_state_never_dispatched(dir.path())
            .await
            .unwrap();
        drop(database);
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "sync::shared_state::package::durable_tests::process_freeze_worker",
                "--ignored",
            ])
            .env("AVEN_PACKAGE_TEST_ROOT", dir.path())
            .env(
                "AVEN_PACKAGE_TEST_COMMIT",
                if committed { "yes" } else { "no" },
            )
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(23),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let database = Database::open(&dir.path().join("source.sqlite"))
            .await
            .unwrap();
        assert_eq!(
            database
                .has_local_shared_state_package_never_dispatched()
                .await
                .unwrap(),
            committed
        );
        let resumed = database
            .resume_local_shared_state_never_dispatched()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(resumed.candidate_id(), capture.candidate_id());
        let mut conn = database.acquire_reader().await.unwrap();
        let counts: (i64, i64, i64) = sqlx::query_as(
            "SELECT (SELECT count(*) FROM local_shared_capture_pins),
                    (SELECT count(*) FROM local_shared_capture_publication),
                    (SELECT count(*) FROM local_shared_capture_package_image_chunks)",
        )
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(counts, if committed { (2, 1, 2) } else { (2, 0, 0) });
        drop(conn);
        let package = database
            .package_local_shared_state_never_dispatched(
                dir.path(),
                package_context(),
                &package_key(),
                [7; 32],
            )
            .await
            .unwrap();
        if committed {
            assert_eq!(
                fingerprint(&package).as_slice(),
                std::fs::read(dir.path().join("fingerprint")).unwrap()
            );
        }
        publication::validate_keyless(&package.upload_package()).unwrap();
    }
}

#[tokio::test]
#[ignore = "subprocess worker exits without destructors; invoked by the process-boundary test"]
async fn process_freeze_worker() {
    let Some(root) = std::env::var_os("AVEN_PACKAGE_TEST_ROOT") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let database = Database::open(&root.join("source.sqlite")).await.unwrap();
    let capture = database
        .resume_local_shared_state_never_dispatched()
        .await
        .unwrap()
        .unwrap();
    let mut selected = capture
        .images
        .iter()
        .filter(|image| image.classification != "unavailable")
        .map(|image| (image.sha256.clone(), image.classification.clone()))
        .collect::<Vec<_>>();
    selected.sort();
    let selected = load_selected_image_plaintexts(&root, &selected, &capture.capture.snapshot)
        .await
        .unwrap();
    let package = encrypt_package(
        &capture,
        package_context(),
        &selected,
        &package_key(),
        [7; 32],
    )
    .unwrap();
    std::fs::write(root.join("fingerprint"), fingerprint(&package)).unwrap();
    let mut conn = database.acquire_writer().await.unwrap();
    let mut tx = db::begin_immediate(&mut conn).await.unwrap();
    persist_package(&mut tx, &package).await.unwrap();
    if std::env::var("AVEN_PACKAGE_TEST_COMMIT").unwrap() == "yes" {
        tx.commit().await.unwrap();
    }
    std::process::exit(23);
}

#[tokio::test]
async fn concurrent_preparers_return_one_frozen_representation() {
    let (dir, first, task) = source_with_history().await;
    add_selected_images(dir.path(), &first, &task).await;
    first
        .capture_local_shared_state_never_dispatched(dir.path())
        .await
        .unwrap();
    let second = Database::open(&dir.path().join("source.sqlite"))
        .await
        .unwrap();
    let key = package_key();
    let (a, b) = tokio::join!(
        first.package_local_shared_state_never_dispatched(
            dir.path(),
            package_context(),
            &key,
            [7; 32]
        ),
        second.package_local_shared_state_never_dispatched(
            dir.path(),
            package_context(),
            &key,
            [7; 32]
        ),
    );
    assert_eq!(a.unwrap(), b.unwrap());
    assert!(
        second
            .package_local_shared_state_never_dispatched(
                dir.path(),
                package_context(),
                &key,
                [8; 32],
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn failed_freeze_marker_write_rolls_back_bytes_but_retains_capture_and_pins() {
    let (dir, database, task) = source_with_history().await;
    add_selected_images(dir.path(), &database, &task).await;
    let capture = database
        .capture_local_shared_state_never_dispatched(dir.path())
        .await
        .unwrap();
    let mut conn = database.acquire_writer().await.unwrap();
    sqlx::query(
        "CREATE TRIGGER fail_freeze BEFORE UPDATE OF frozen_descriptor_commitment
         ON local_shared_capture_journal BEGIN SELECT RAISE(ABORT, 'injected freeze failure'); END",
    )
    .execute(&mut *conn)
    .await
    .unwrap();
    drop(conn);
    assert!(
        database
            .package_local_shared_state_never_dispatched(
                dir.path(),
                package_context(),
                &package_key(),
                [7; 32],
            )
            .await
            .is_err()
    );
    drop(database);
    let database = Database::open(&dir.path().join("source.sqlite"))
        .await
        .unwrap();
    assert!(
        !database
            .has_local_shared_state_package_never_dispatched()
            .await
            .unwrap()
    );
    let mut conn = database.acquire_writer().await.unwrap();
    let counts: (i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM local_shared_capture_packages),
                (SELECT count(*) FROM local_shared_capture_publication),
                (SELECT count(*) FROM local_shared_capture_package_chunks),
                (SELECT count(*) FROM local_shared_capture_package_image_chunks),
                (SELECT count(*) FROM local_shared_capture_pins)",
    )
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(counts, (0, 0, 0, 0, 2));
    sqlx::query("DROP TRIGGER fail_freeze")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let retry = database
        .package_local_shared_state_never_dispatched(
            dir.path(),
            package_context(),
            &package_key(),
            [7; 32],
        )
        .await
        .unwrap();
    assert_eq!(retry.candidate_id(), capture.candidate_id());
    publication::validate_keyless(&retry.upload_package()).unwrap();
}
