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
        .capture_local_shared_state_never_dispatched()
        .await
        .unwrap();
    let key = package_key();
    let frozen = database
        .package_local_shared_state_never_dispatched(dir.path(), package_context(), &key, [7; 32])
        .await
        .unwrap();
    let exact = frozen.upload_package();
    assert_eq!(frozen.descriptor(), exact.descriptor);
    let stream = decode_context_id(capture.stream_id(), "stream").unwrap();
    assert_eq!(
        publication::validate_keyless(&exact).unwrap().image_count,
        2
    );
    publication::authenticate(
        &exact,
        &key,
        package_context(),
        stream,
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
            stream,
            decode_context_id(retry.candidate_id(), "candidate").unwrap(),
        ),
        (
            package_context(),
            [2; 32],
            decode_context_id(retry.candidate_id(), "candidate").unwrap(),
        ),
        (package_context(), stream, [3; 32]),
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
            stream,
            decode_context_id(retry.candidate_id(), "candidate").unwrap(),
            [7; 32]
        )
        .is_err()
    );
    let decoded = publication::decrypt_capture(&exact, &key).unwrap();
    assert_eq!(decoded.snapshot.tables.tasks[0].title, "captured title");
    assert_eq!(publication::decrypt_images(&exact, &key).unwrap().len(), 2);
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
    let counts: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM local_shared_capture_publication),
                (SELECT count(*) FROM local_shared_capture_package_records),
                (SELECT count(*) FROM local_shared_capture_pins)",
    )
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(counts, (0, 0, 0));
}

#[tokio::test]
async fn corrupt_or_missing_frozen_components_never_trigger_replacement() {
    let first_image = "(SELECT min(object_id) FROM local_shared_capture_package_records
                        WHERE component = 'image')";
    let mutations: Vec<(&str, Vec<String>)> = vec![
        (
            "missing publication row",
            vec!["DELETE FROM local_shared_capture_publication".into()],
        ),
        (
            "zeroed descriptor",
            vec!["UPDATE local_shared_capture_publication SET descriptor = zeroblob(length(descriptor))".into()],
        ),
        (
            "zeroed prefix catalog",
            vec!["UPDATE local_shared_capture_publication SET prefix_catalog = zeroblob(length(prefix_catalog))".into()],
        ),
        (
            "corrupt state record",
            vec!["UPDATE local_shared_capture_package_records SET record = zeroblob(length(record)) WHERE component = 'state' AND chunk_index = 0".into()],
        ),
        (
            "corrupt manifest record",
            vec!["UPDATE local_shared_capture_package_records SET record = zeroblob(length(record)) WHERE component = 'manifest'".into()],
        ),
        (
            "corrupt image record",
            vec![format!("UPDATE local_shared_capture_package_records SET record = zeroblob(length(record)) WHERE component = 'image' AND object_id = {first_image}")],
        ),
        (
            "missing state records",
            vec!["DELETE FROM local_shared_capture_package_records WHERE component = 'state'".into()],
        ),
        (
            "missing image component",
            vec![format!("DELETE FROM local_shared_capture_package_records WHERE component = 'image' AND object_id = {first_image}")],
        ),
        (
            "missing manifest index",
            vec!["UPDATE local_shared_capture_package_records SET chunk_index = chunk_index + 1 WHERE component = 'manifest'".into()],
        ),
        (
            "extra manifest index",
            vec!["INSERT INTO local_shared_capture_package_records SELECT candidate_id, component, object_id, chunk_index + 1, record FROM local_shared_capture_package_records WHERE component = 'manifest'".into()],
        ),
        (
            "foreign image component",
            vec!["INSERT INTO local_shared_capture_package_records SELECT candidate_id, component, randomblob(32), chunk_index, record FROM local_shared_capture_package_records WHERE component = 'image' LIMIT 1".into()],
        ),
        (
            "swapped image components",
            [
                "CREATE TEMP TABLE swap AS SELECT min(object_id) AS a, max(object_id) AS b
                 FROM local_shared_capture_package_records WHERE component = 'image'",
                "UPDATE local_shared_capture_package_records SET object_id = zeroblob(32)
                 WHERE component = 'image' AND object_id = (SELECT a FROM swap)",
                "UPDATE local_shared_capture_package_records SET object_id = (SELECT a FROM swap)
                 WHERE component = 'image' AND object_id = (SELECT b FROM swap)",
                "UPDATE local_shared_capture_package_records SET object_id = (SELECT b FROM swap)
                 WHERE component = 'image' AND object_id = zeroblob(32)",
                "DROP TABLE swap",
            ]
            .map(String::from)
            .to_vec(),
        ),
    ];
    for (name, statements) in mutations {
        let (dir, database, task) = source_with_history().await;
        add_selected_images(dir.path(), &database, &task).await;
        database
            .capture_local_shared_state_never_dispatched()
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
        for statement in &statements {
            sqlx::query(sqlx::AssertSqlSafe(statement.as_str()))
                .execute(&mut *conn)
                .await
                .unwrap();
        }
        let corrupted = stored_package_rows(&mut conn).await;
        drop(conn);
        drop(database);
        let database = Database::open(&dir.path().join("source.sqlite"))
            .await
            .unwrap();
        assert!(
            database
                .has_local_shared_state_package_never_dispatched()
                .await
                .unwrap(),
            "{name}"
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
                "{name}"
            );
        }
        let mut conn = database.acquire_reader().await.unwrap();
        assert_eq!(stored_package_rows(&mut conn).await, corrupted, "{name}");
        let commitment: Vec<u8> = sqlx::query_scalar(
            "SELECT frozen_descriptor_commitment FROM local_shared_capture_journal",
        )
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(commitment, sha256(frozen.descriptor()), "{name}");
        let pins: i64 = sqlx::query_scalar("SELECT count(*) FROM local_shared_capture_pins")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(pins, 2, "{name}");
        drop(conn);
        database
            .cancel_local_shared_state_never_dispatched(frozen.candidate_id())
            .await
            .unwrap();
    }
}

type StoredPackageRows = (
    Vec<(Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>)>,
    Vec<(String, Vec<u8>, i64, Vec<u8>)>,
);

async fn stored_package_rows(conn: &mut sqlx::SqliteConnection) -> StoredPackageRows {
    let publication = sqlx::query_as(
        "SELECT descriptor, data_catalog, prefix_catalog, image_catalog
         FROM local_shared_capture_publication",
    )
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    let records = sqlx::query_as(
        "SELECT component, object_id, chunk_index, record
         FROM local_shared_capture_package_records
         ORDER BY component, object_id, chunk_index",
    )
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    (publication, records)
}

#[tokio::test]
async fn process_exit_before_and_after_freeze_commit_preserves_ownership() {
    for committed in [false, true] {
        let (dir, database, task) = source_with_history().await;
        add_selected_images(dir.path(), &database, &task).await;
        let capture = database
            .capture_local_shared_state_never_dispatched()
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
                    (SELECT count(*) FROM local_shared_capture_package_records
                     WHERE component = 'image')",
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
        .capture_local_shared_state_never_dispatched()
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
        .capture_local_shared_state_never_dispatched()
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
    let counts: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM local_shared_capture_publication),
                (SELECT count(*) FROM local_shared_capture_package_records),
                (SELECT count(*) FROM local_shared_capture_pins)",
    )
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(counts, (0, 0, 2));
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
