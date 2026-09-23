use super::*;
use crate::protected_local_keys::tests::isolated_store;

#[tokio::test]
async fn seed_adopts_real_publication_preserving_later_edits_and_retry_progress() {
    use aven_core::sync::bootstrap_staging::{
        Authentication, Budget, Component, PublishBootstrap, PutChunk, Status,
    };
    use aven_core::sync::seed_claim::{ClaimAuthentication, Secret, SetupAuthority};

    let root = tempfile::tempdir().unwrap();
    let client = Database::open(&root.path().join("client.sqlite"))
        .await
        .unwrap();
    let workspace = client.list_workspaces().await.unwrap().remove(0);
    let task = client
        .create_task(
            &workspace,
            aven_core::operations::TaskDraft {
                title: "PRIVATE-PUBLICATION-TITLE".into(),
                description: String::new(),
                project: Some("app".into()),
                status: "todo".into(),
                priority: "none".into(),
                source: aven_core::choices::TaskSource::Cli,
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
    for width in [2, 3, 4] {
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::new(width, 1))
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        let attachment = client
            .add_task_attachment(
                &workspace,
                root.path(),
                Default::default(),
                &task.id,
                aven_core::operations::AttachmentAddInput {
                    filename: Some("PRIVATE-PUBLICATION-IMAGE.png".into()),
                    alt_text: None,
                    declared_media_type: None,
                    bytes: bytes.into_inner(),
                    optimization_policy: aven_core::attachments::ImageOptimizationPolicy::Preserve,
                    dedupe_existing: false,
                },
            )
            .await
            .unwrap()
            .outcome
            .attachment;
        if width != 2 {
            client
                .delete_task_attachment(&workspace, &attachment.attachment_id)
                .await
                .unwrap();
        }
        if width == 4 {
            let mut conn = aven_core::test_support::acquire(&client).await.unwrap();
            sqlx::query("UPDATE blob_inventory SET available = 0 WHERE sha256 = ?")
                .bind(&attachment.sha256)
                .execute(&mut *conn)
                .await
                .unwrap();
            fs::remove_file(
                aven_core::attachments::object_path(root.path(), &attachment.sha256).unwrap(),
            )
            .unwrap();
        }
    }
    use aven_core::operations::{CreateRecurrenceSeriesParams, RecurrenceSeriesDraft};
    use aven_core::recurrence::{RecurrenceDuePolicy, RecurrenceRule, RecurrenceSchedule};
    use chrono::TimeZone;
    let at = chrono::Utc.with_ymd_and_hms(2100, 9, 21, 12, 0, 0).unwrap();
    let series = client
        .create_recurrence_series(
            &workspace,
            CreateRecurrenceSeriesParams::new(RecurrenceSeriesDraft {
                title: "daily".into(),
                description: "template".into(),
                project: "app".into(),
                priority: "none".into(),
                initial_status: "todo".into(),
                labels: vec![],
                metadata: vec![aven_core::metadata::TaskMetadataInput {
                    expected_field_id: None,
                    key: "ticket".into(),
                    value: "42".into(),
                }],
                schedule: RecurrenceSchedule::new(
                    RecurrenceRule::daily(),
                    "UTC".parse().unwrap(),
                    at.date_naive(),
                    None,
                    RecurrenceDuePolicy::SameDay,
                ),
            })
            .at(at),
        )
        .await
        .unwrap();
    let store = isolated_store(client.path(), &root.path().join("keys"));
    {
        let mut conn = aven_core::test_support::acquire(&client).await.unwrap();
        sqlx::query("UPDATE changes SET server_seq = local_seq * 3 WHERE change_id IN (SELECT change_id FROM changes ORDER BY local_seq LIMIT 3)").execute(&mut *conn).await.unwrap();
        sqlx::query("INSERT INTO shared_history_provenance(change_id, source_server_seq, source_pending_rank) SELECT change_id, 999, NULL FROM changes ORDER BY local_seq LIMIT 1").execute(&mut *conn).await.unwrap();
    }
    let original = store.prepare_seed_claim(&client, [9; 32]).await.unwrap();
    let protected = original.protected_storage_bytes();
    drop(original);
    let mut stale_session = aven_core::sync::SyncSession::start(
        client.clone(),
        "https://legacy.test".into(),
        None,
        None,
    )
    .await
    .unwrap();
    let stale_request = stale_session.prepare_request().await.unwrap().unwrap();
    let stale_page = client
        .prepare_client_sync_page("https://legacy.test".into(), 0, 10)
        .await
        .unwrap();
    store.prepare_seed_source(&client).await.unwrap();
    client
        .capture_local_shared_state_never_dispatched(root.path())
        .await
        .unwrap();
    let local = store
        .package_seed_capture(&client, root.path(), [9; 32])
        .await
        .unwrap();
    let package = local.upload_package();
    assert_eq!(package.images.len(), 2);
    drop(client);
    drop(store);
    let client = Database::open(&root.path().join("client.sqlite"))
        .await
        .unwrap();
    let store = isolated_store(client.path(), &root.path().join("keys"));
    let seed = store.prepare_seed_claim(&client, [9; 32]).await.unwrap();
    assert_eq!(protected, seed.protected_storage_bytes());
    let reopened = store
        .package_seed_capture(&client, root.path(), [9; 32])
        .await
        .unwrap();
    assert_eq!(local, reopened);
    let authority = store.load_required().unwrap();
    let signed = seed
        .prepare_bootstrap_publication(&reopened.upload_package(), authority.package_key())
        .unwrap();
    let intent = store.prepare_seed_adoption_intent(&client).await.unwrap();
    assert_eq!(intent.publication(seed.genesis()).unwrap(), signed);
    assert!(
        client
            .cancel_local_shared_state_never_dispatched(local.candidate_id())
            .await
            .is_err()
    );
    client
        .update_task(
            &workspace,
            &task.id,
            aven_core::operations::TaskUpdate {
                title: Some("AFTER-CAPTURE".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    client
        .reconcile_recurrence_series(
            &workspace,
            &series.series.id,
            at + chrono::Duration::days(1),
        )
        .await
        .unwrap();
    let related = client
        .create_task(
            &workspace,
            aven_core::operations::TaskDraft {
                title: "later related".into(),
                description: String::new(),
                project: Some("app".into()),
                status: "todo".into(),
                priority: "none".into(),
                source: aven_core::choices::TaskSource::Cli,
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
    client
        .add_task_to_epic(&workspace, &related.id, &task.id)
        .await
        .unwrap();
    client
        .add_task_related_link(&workspace, &related.id, &task.id)
        .await
        .unwrap();
    client
        .add_task_dependency(&workspace, &related.id, &task.id)
        .await
        .unwrap();
    client
        .add_note(&workspace, &task.id, "later note".into())
        .await
        .unwrap();
    let mut image = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image::RgbaImage::new(7, 1))
        .write_to(&mut image, image::ImageFormat::Png)
        .unwrap();
    client
        .add_task_attachment(
            &workspace,
            root.path(),
            Default::default(),
            &task.id,
            aven_core::operations::AttachmentAddInput {
                filename: None,
                alt_text: None,
                declared_media_type: None,
                bytes: image.into_inner(),
                optimization_policy: aven_core::attachments::ImageOptimizationPolicy::Preserve,
                dedupe_existing: false,
            },
        )
        .await
        .unwrap();
    {
        let mut conn = aven_core::test_support::acquire(&client).await.unwrap();
        sqlx::query("INSERT INTO conflicts(workspace_id, entity_type, entity_id, task_id, field, local_value, remote_value, local_change_id, remote_change_id, variant_a, variant_b, created_at) VALUES (?, 'task', ?, ?, 'description', 'local', 'remote', NULL, 'fixture-conflict', 'vlocal', 'vremote', '2100-09-21T12:00:00Z')")
            .bind(&workspace.id).bind(&task.id).bind(&task.id).execute(&mut *conn).await.unwrap();
        let baseline =
            serde_json::json!({"epic_task_id": task.id, "created_at": related.created_at});
        sqlx::query("INSERT INTO meta(key, value) VALUES (?, ?)")
            .bind(format!(
                "epic_membership_baseline:{}:{}",
                workspace.id, related.id
            ))
            .bind(baseline.to_string())
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    let before = client.export_data("before".into()).await.unwrap();
    let server_path = root.path().join("server.sqlite");
    let server = Database::open(&server_path).await.unwrap();
    let setup_secret = Secret::generate().unwrap();
    let setup =
        SetupAuthority::from_verifier([9; 32], SetupAuthority::verifier([9; 32], &setup_secret));
    server
        .admit_seed_claim(
            &seed.genesis().claim_bytes(),
            Some(&setup),
            ClaimAuthentication::SetupSecret(&setup_secret),
        )
        .await
        .unwrap();
    let auth = Authentication {
        vault_id: seed.genesis().context().vault_id,
        genesis_commitment: seed.genesis().commitment(),
        bearer: seed.bearer(),
    };
    let binding = signed.binding();
    let mut components = Vec::new();
    for (index, component) in [
        Component::DataCatalog,
        Component::PrefixCatalog,
        Component::ImageCatalog,
    ]
    .into_iter()
    .enumerate()
    {
        components.push((
            component,
            package.catalogs[index]
                .chunks(1_048_576)
                .collect::<Vec<_>>(),
        ));
    }
    components.push((
        Component::Manifest,
        package.manifest.iter().map(Vec::as_slice).collect(),
    ));
    components.push((
        Component::State,
        package.state.iter().map(Vec::as_slice).collect(),
    ));
    for image in &package.images {
        components.push((
            Component::Image(image.object_id),
            image.records.iter().map(Vec::as_slice).collect(),
        ));
    }
    let budget = Budget {
        bytes: components
            .iter()
            .flat_map(|(_, chunks)| chunks)
            .map(|b| b.len() as u64)
            .sum(),
        chunks: components
            .iter()
            .map(|(_, chunks)| chunks.len() as u64)
            .sum(),
    };
    let staging = server
        .declare_bootstrap_staging(&auth, &package.descriptor, budget)
        .await
        .unwrap();
    for (component, chunks) in components {
        for (index, bytes) in chunks.into_iter().enumerate() {
            server
                .put_bootstrap_chunk(
                    &auth,
                    PutChunk {
                        bootstrap_id: binding.bootstrap_id,
                        descriptor_commitment: binding.descriptor_commitment,
                        epoch: staging.epoch,
                        component,
                        index: index as u64,
                        bytes,
                    },
                )
                .await
                .unwrap();
        }
    }
    let request = || PublishBootstrap {
        bootstrap_id: binding.bootstrap_id,
        descriptor_commitment: binding.descriptor_commitment,
        epoch: staging.epoch,
        record: signed.record(),
    };
    let accepted = server
        .publish_bootstrap(&auth, request(), Default::default())
        .await
        .unwrap();
    drop(server);
    // The observed response is deliberately unused for recovery; exact frozen
    // intent and protected seed are sufficient to ask the reopened core.
    let server = Database::open(&server_path).await.unwrap();
    assert_eq!(
        server
            .bootstrap_staging_status(&auth, binding.bootstrap_id)
            .await
            .unwrap(),
        Status::Published(accepted.clone())
    );
    assert_eq!(
        server
            .publish_bootstrap(&auth, request(), Default::default())
            .await
            .unwrap(),
        accepted
    );
    accepted
        .validate_expected(seed.genesis(), &package.descriptor)
        .unwrap();
    let mut conn = aven_core::test_support::acquire(&server).await.unwrap();
    let unmapped: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM server_e2ee_image_references WHERE object IS NULL",
    )
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(unmapped, 1);
    let (owned, grace): (i64, i64) =
        sqlx::query_as("SELECT count(*), count(unreferenced_at) FROM server_e2ee_images")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!((owned, grace), (2, 1));
    drop(conn);
    assert_eq!(seed.protected_storage_bytes(), protected);
    let before_failure =
        serde_json::to_value(client.export_data("fixed".into()).await.unwrap()).unwrap();
    let captured_row = before
        .tables
        .changes
        .iter()
        .find(|r| r.server_seq.is_some())
        .unwrap();
    for (sql, undo) in [
        (
            "UPDATE meta SET value = CAST(value AS INTEGER) + 1 WHERE key = 'sync_generation'",
            "UPDATE meta SET value = CAST(value AS INTEGER) - 1 WHERE key = 'sync_generation'",
        ),
        (
            "UPDATE changes SET server_seq = 100000 WHERE payload LIKE '%AFTER-CAPTURE%'",
            "UPDATE changes SET server_seq = NULL WHERE payload LIKE '%AFTER-CAPTURE%'",
        ),
        (
            "UPDATE shared_history_provenance SET source_server_seq = 998",
            "UPDATE shared_history_provenance SET source_server_seq = 999",
        ),
    ] {
        let mut conn = aven_core::test_support::acquire(&client).await.unwrap();
        sqlx::query(sql).execute(&mut *conn).await.unwrap();
        drop(conn);
        assert!(
            store
                .adopt_seed_publication(&client, &accepted)
                .await
                .is_err()
        );
        let mut conn = aven_core::test_support::acquire(&client).await.unwrap();
        sqlx::query(undo).execute(&mut *conn).await.unwrap();
    }
    let mut conn = aven_core::test_support::acquire(&client).await.unwrap();
    sqlx::query("UPDATE changes SET created_at = '2100-01-01T00:00:00Z' WHERE change_id = ?")
        .bind(&captured_row.change_id)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    assert!(
        store
            .adopt_seed_publication(&client, &accepted)
            .await
            .is_err()
    );
    let mut conn = aven_core::test_support::acquire(&client).await.unwrap();
    sqlx::query("UPDATE changes SET created_at = ? WHERE change_id = ?")
        .bind(&captured_row.created_at)
        .bind(&captured_row.change_id)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER fail_adoption BEFORE UPDATE OF state ON local_seed_publication_intent WHEN NEW.state = 'adopted' BEGIN SELECT RAISE(ABORT, 'injected adoption failure'); END").execute(&mut *conn).await.unwrap();
    drop(conn);
    assert!(
        store
            .adopt_seed_publication(&client, &accepted)
            .await
            .is_err()
    );
    assert_eq!(
        serde_json::to_value(client.export_data("fixed".into()).await.unwrap()).unwrap(),
        before_failure
    );
    assert_eq!(
        server
            .bootstrap_staging_status(&auth, binding.bootstrap_id)
            .await
            .unwrap(),
        Status::Published(accepted.clone())
    );
    let mut conn = aven_core::test_support::acquire(&client).await.unwrap();
    let baseline_rows: i64 = sqlx::query_scalar("SELECT (SELECT count(*) FROM local_e2ee_dependency_baseline) + (SELECT count(*) FROM local_e2ee_dependency_edges)")
        .fetch_one(&mut *conn).await.unwrap();
    assert_eq!(baseline_rows, 0);
    sqlx::query("DROP TRIGGER fail_adoption")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER fail_cleanup BEFORE DELETE ON local_shared_capture_journal BEGIN SELECT RAISE(ABORT, 'injected cleanup failure'); END").execute(&mut *conn).await.unwrap();
    drop(conn);
    assert!(
        store
            .adopt_seed_publication(&client, &accepted)
            .await
            .is_err()
    );
    assert_eq!(
        client
            .seed_publication_intent_bytes()
            .await
            .unwrap()
            .unwrap()
            .1,
        "adopted"
    );
    let mut conn = aven_core::test_support::acquire(&client).await.unwrap();
    let pins: i64 = sqlx::query_scalar("SELECT count(*) FROM local_shared_capture_pins")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert!(pins > 0);
    sqlx::query("DROP TRIGGER fail_cleanup")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    assert!(
        !store
            .adopt_seed_publication(&client, &accepted)
            .await
            .unwrap()
    );
    let after = client.export_data("after".into()).await.unwrap();
    let mut ranks = after
        .tables
        .changes
        .iter()
        .filter_map(|r| r.server_seq)
        .collect::<Vec<_>>();
    ranks.sort();
    assert_eq!(ranks, (1..=binding.prefix_count as i64).collect::<Vec<_>>());
    for (old, new) in [
        (
            serde_json::to_value(&before.tables.recurrence_series).unwrap(),
            serde_json::to_value(&after.tables.recurrence_series).unwrap(),
        ),
        (
            serde_json::to_value(&before.tables.recurrence_occurrences).unwrap(),
            serde_json::to_value(&after.tables.recurrence_occurrences).unwrap(),
        ),
        (
            serde_json::to_value(&before.tables.recurrence_series_metadata).unwrap(),
            serde_json::to_value(&after.tables.recurrence_series_metadata).unwrap(),
        ),
        (
            serde_json::to_value(&before.tables.task_metadata).unwrap(),
            serde_json::to_value(&after.tables.task_metadata).unwrap(),
        ),
        (
            serde_json::to_value(&before.tables.notes).unwrap(),
            serde_json::to_value(&after.tables.notes).unwrap(),
        ),
        (
            serde_json::to_value(&before.tables.conflicts).unwrap(),
            serde_json::to_value(&after.tables.conflicts).unwrap(),
        ),
        (
            serde_json::to_value(&before.tables.task_epic_links).unwrap(),
            serde_json::to_value(&after.tables.task_epic_links).unwrap(),
        ),
        (
            serde_json::to_value(&before.tables.task_related_links).unwrap(),
            serde_json::to_value(&after.tables.task_related_links).unwrap(),
        ),
        (
            serde_json::to_value(&before.tables.task_dependencies).unwrap(),
            serde_json::to_value(&after.tables.task_dependencies).unwrap(),
        ),
    ] {
        assert_eq!(old, new);
    }
    for row in before.tables.changes.iter().filter(|r| {
        after
            .tables
            .shared_history_provenance
            .iter()
            .all(|p| p.change_id != r.change_id)
    }) {
        let retained = after
            .tables
            .changes
            .iter()
            .find(|r| r.change_id == row.change_id)
            .unwrap();
        assert_eq!(
            serde_json::to_value(row).unwrap(),
            serde_json::to_value(retained).unwrap()
        );
        assert!(retained.server_seq.is_none());
    }
    assert!(!after.tables.conflicts.is_empty());
    for row in before
        .tables
        .meta
        .iter()
        .filter(|m| m.key.starts_with("epic_membership_baseline:"))
    {
        assert!(
            after
                .tables
                .meta
                .iter()
                .any(|m| m.key == row.key && m.value == row.value)
        );
    }
    let original_provenance = before.tables.shared_history_provenance.first().unwrap();
    assert!(
        after
            .tables
            .shared_history_provenance
            .contains(original_provenance)
    );
    assert_eq!(
        serde_json::to_value(&before.tables.tasks).unwrap(),
        serde_json::to_value(&after.tables.tasks).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&before.tables.field_versions).unwrap(),
        serde_json::to_value(&after.tables.field_versions).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&before.tables.task_attachments).unwrap(),
        serde_json::to_value(&after.tables.task_attachments).unwrap()
    );
    for row in before
        .tables
        .changes
        .iter()
        .filter(|r| r.payload.contains("AFTER-CAPTURE"))
    {
        let retained = after
            .tables
            .changes
            .iter()
            .find(|r| r.change_id == row.change_id)
            .unwrap();
        assert!(retained.server_seq.is_none());
        assert_eq!(retained.payload, row.payload);
    }
    let mut conn = aven_core::test_support::acquire(&client).await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM local_shared_capture_pins")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(count, 0);
    sqlx::query("UPDATE meta SET value = ? WHERE key = 'sync_cursor'")
        .bind((binding.prefix_count + 12).to_string())
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    drop(client);
    let client = Database::open(&root.path().join("client.sqlite"))
        .await
        .unwrap();
    assert!(
        !store
            .adopt_seed_publication(&client, &accepted)
            .await
            .unwrap()
    );
    let mut conn = aven_core::test_support::acquire(&client).await.unwrap();
    let cursor: String = sqlx::query_scalar("SELECT value FROM meta WHERE key = 'sync_cursor'")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(cursor, (binding.prefix_count + 12).to_string());
    drop(conn);
    assert!(
        stale_session
            .prepare_request()
            .await
            .err()
            .unwrap()
            .to_string()
            .contains("e2ee-installation-fenced")
    );
    assert!(
        stale_session
            .accept_response(
                &stale_request.context,
                aven_core::sync::SyncHttpResponse {
                    status: 200,
                    headers: vec![],
                    body: vec![]
                }
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("e2ee-installation-fenced")
    );
    let response = aven_core::sync::wire::SyncResponse {
        protocol_version: aven_core::sync::wire::SYNC_PROTOCOL_VERSION,
        changes: vec![],
        push_acks: vec![],
        cursor: stale_page.request.after,
        has_more: false,
    };
    assert!(
        client
            .apply_client_sync_page(aven_core::sync::ApplySyncPage {
                request: stale_page.request,
                sync_generation: stale_page.sync_generation,
                response,
                attempted_at: "2100-09-21T12:00:00Z".into(),
                previous_pushed: 0,
                previous_pulled: 0
            })
            .await
            .unwrap_err()
            .to_string()
            .contains("e2ee-installation-fenced")
    );
    assert!(
        client
            .prepare_client_sync_page("http://localhost:9999".into(), 1, 1)
            .await
            .is_err()
    );
    assert!(client.import_data(&before).await.is_err());
    assert!(
        aven_core::db::backup_database(client.path(), &root.path().join("backup.sqlite"))
            .await
            .is_err()
    );
    let other = Database::open(&root.path().join("other.sqlite"))
        .await
        .unwrap();
    assert!(other.import_data(&after).await.is_err());
}

async fn local_intent_fixture() -> (tempfile::TempDir, Database, ProtectedLocalKeyStore) {
    let root = tempfile::tempdir().unwrap();
    let database = Database::open(&root.path().join("client.sqlite"))
        .await
        .unwrap();
    let store = isolated_store(database.path(), &root.path().join("keys"));
    store.prepare_seed_claim(&database, [9; 32]).await.unwrap();
    store.prepare_seed_source(&database).await.unwrap();
    database
        .capture_local_shared_state_never_dispatched(root.path())
        .await
        .unwrap();
    store
        .package_seed_capture(&database, root.path(), [9; 32])
        .await
        .unwrap();
    (root, database, store)
}

fn protected_path(store: &ProtectedLocalKeyStore, kind: &str) -> PathBuf {
    let Backend::File(backend) = store.adoption_backend(kind) else {
        panic!("file fixture")
    };
    backend.path
}

#[tokio::test]
async fn preparing_intent_resumes_exact_bytes_and_missing_sealed_authority_fails_closed() {
    let (root, database, store) = local_intent_fixture().await;
    let package = store.load_required().unwrap();
    let seed = store.required_seed(&package).unwrap();
    let source = store
        .decode_source(
            &store
                .load_adoption_record("source", 104, true)
                .unwrap()
                .unwrap(),
            &seed,
        )
        .unwrap();
    let intent = database
        .prepare_seed_publication_intent(&source, &seed, package.package_key())
        .await
        .unwrap();
    let saved = intent.protected_storage_bytes().to_vec();
    assert!(!protected_path(&store, "intent").exists());
    drop(database);
    let database = Database::open(&root.path().join("client.sqlite"))
        .await
        .unwrap();
    let resumed = store.prepare_seed_adoption_intent(&database).await.unwrap();
    assert_eq!(resumed.protected_storage_bytes(), saved);
    assert_eq!(
        store
            .prepare_seed_adoption_intent(&database)
            .await
            .unwrap()
            .protected_storage_bytes(),
        saved
    );
    fs::remove_file(protected_path(&store, "intent")).unwrap();
    assert!(store.prepare_seed_adoption_intent(&database).await.is_err());
    fs::remove_file(store.adoption_marker("intent")).unwrap();
    assert!(store.prepare_seed_adoption_intent(&database).await.is_err());
    assert!(
        database
            .cancel_local_shared_state_never_dispatched("anything")
            .await
            .is_err()
    );
    assert_eq!(
        database
            .seed_publication_intent_bytes()
            .await
            .unwrap()
            .unwrap()
            .0,
        saved
    );
}

#[tokio::test]
async fn cancel_first_cannot_create_protected_intent_and_source_loss_never_regenerates() {
    let (_root, database, store) = local_intent_fixture().await;
    let capture = database
        .resume_local_shared_state_never_dispatched()
        .await
        .unwrap()
        .unwrap();
    assert!(
        database
            .cancel_local_shared_state_never_dispatched(capture.candidate_id())
            .await
            .unwrap()
    );
    assert!(store.prepare_seed_adoption_intent(&database).await.is_err());
    assert!(!protected_path(&store, "intent").exists());
    let pin = database.seed_source_pin().await.unwrap();
    fs::remove_file(protected_path(&store, "source")).unwrap();
    assert!(store.prepare_seed_source(&database).await.is_err());
    assert_eq!(database.seed_source_pin().await.unwrap(), pin);
}

#[tokio::test]
async fn protected_intent_write_failure_retains_sqlite_fence_and_retries_exactly() {
    let (_root, database, store) = local_intent_fixture().await;
    // The target path being a directory causes the atomic protected create to fail.
    fs::create_dir(protected_path(&store, "intent")).unwrap();
    assert!(store.prepare_seed_adoption_intent(&database).await.is_err());
    // Failure during initial protected lookup precedes SQLite intent preparation.
    assert!(
        database
            .seed_publication_intent_bytes()
            .await
            .unwrap()
            .is_none()
    );
    fs::remove_dir(protected_path(&store, "intent")).unwrap();
    let package = store.load_required().unwrap();
    let seed = store.required_seed(&package).unwrap();
    let source = store
        .decode_source(
            &store
                .load_adoption_record("source", 104, true)
                .unwrap()
                .unwrap(),
            &seed,
        )
        .unwrap();
    let intent = database
        .prepare_seed_publication_intent(&source, &seed, package.package_key())
        .await
        .unwrap();
    fs::create_dir(protected_path(&store, "intent")).unwrap();
    assert!(
        store
            .create_adoption_record("intent", intent.protected_storage_bytes(), 65536)
            .is_err()
    );
    assert!(
        database
            .cancel_local_shared_state_never_dispatched("anything")
            .await
            .is_err()
    );
    fs::remove_dir(protected_path(&store, "intent")).unwrap();
    assert_eq!(
        store
            .prepare_seed_adoption_intent(&database)
            .await
            .unwrap()
            .protected_storage_bytes(),
        intent.protected_storage_bytes()
    );
}

#[tokio::test]
async fn copied_database_and_missing_database_cannot_reacquire_source_authority() {
    let (root, database, store) = local_intent_fixture().await;
    store.prepare_seed_adoption_intent(&database).await.unwrap();
    // A raw SQLite snapshot copies no host authority. This is a fixture, not a
    // supported backup API, which refuses the bound installation.
    let copied_path = root.path().join("copied.sqlite");
    let mut conn = aven_core::test_support::acquire(&database).await.unwrap();
    sqlx::query("VACUUM INTO ?")
        .bind(copied_path.to_str().unwrap())
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let copied = Database::open(&copied_path).await.unwrap();
    let copied_store = isolated_store(&copied_path, &root.path().join("keys"));
    assert!(copied_store.prepare_seed_source(&copied).await.is_err());
    assert!(
        copied_store
            .prepare_seed_adoption_intent(&copied)
            .await
            .is_err()
    );
    let source = Database::open(&root.path().join("restore-source.sqlite"))
        .await
        .unwrap();
    assert!(
        aven_core::db::restore_database_file(database.path(), source.path())
            .await
            .is_err()
    );
    let path = database.path().to_path_buf();
    drop(database);
    // No live writer is retained; deletion simulates missing replaceable state.
    fs::remove_file(&path).unwrap();
    assert!(
        aven_core::db::restore_database_file(&path, source.path())
            .await
            .is_err()
    );
}

async fn publish_empty_package(
    server: &Database,
    seed: &SeedAuthority,
    package: &aven_core::sync::bootstrap_format::Package,
    key: &aven_core::sync::LocalSharedStatePackageKey,
) -> PublicationOutcome {
    use aven_core::sync::bootstrap_staging::{
        Authentication, Budget, Component, PublishBootstrap, PutChunk,
    };
    use aven_core::sync::seed_claim::{ClaimAuthentication, Secret, SetupAuthority};
    let secret = Secret::generate().unwrap();
    let setup = SetupAuthority::from_verifier(
        seed.genesis().setup_id(),
        SetupAuthority::verifier(seed.genesis().setup_id(), &secret),
    );
    server
        .admit_seed_claim(
            &seed.genesis().claim_bytes(),
            Some(&setup),
            ClaimAuthentication::SetupSecret(&secret),
        )
        .await
        .unwrap();
    let signed = seed.prepare_bootstrap_publication(package, key).unwrap();
    let binding = signed.binding();
    let auth = Authentication {
        vault_id: binding.vault_id,
        genesis_commitment: binding.genesis_commitment,
        bearer: seed.bearer(),
    };
    let mut components = Vec::new();
    for (index, component) in [
        Component::DataCatalog,
        Component::PrefixCatalog,
        Component::ImageCatalog,
    ]
    .into_iter()
    .enumerate()
    {
        components.push((
            component,
            package.catalogs[index]
                .chunks(1_048_576)
                .collect::<Vec<_>>(),
        ));
    }
    components.push((
        Component::State,
        package.state.iter().map(Vec::as_slice).collect(),
    ));
    components.push((
        Component::Manifest,
        package.manifest.iter().map(Vec::as_slice).collect(),
    ));
    let budget = Budget {
        bytes: components
            .iter()
            .flat_map(|(_, chunks)| chunks)
            .map(|b| b.len() as u64)
            .sum(),
        chunks: components
            .iter()
            .map(|(_, chunks)| chunks.len() as u64)
            .sum(),
    };
    let staging = server
        .declare_bootstrap_staging(&auth, &package.descriptor, budget)
        .await
        .unwrap();
    for (component, chunks) in components {
        for (index, bytes) in chunks.into_iter().enumerate() {
            server
                .put_bootstrap_chunk(
                    &auth,
                    PutChunk {
                        bootstrap_id: binding.bootstrap_id,
                        descriptor_commitment: binding.descriptor_commitment,
                        epoch: staging.epoch,
                        component,
                        index: index as u64,
                        bytes,
                    },
                )
                .await
                .unwrap();
        }
    }
    server
        .publish_bootstrap(
            &auth,
            PublishBootstrap {
                bootstrap_id: binding.bootstrap_id,
                descriptor_commitment: binding.descriptor_commitment,
                epoch: staging.epoch,
                record: signed.record(),
            },
            Default::default(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn valid_signed_other_publication_cannot_adopt_this_capture() {
    let (root, database, store) = local_intent_fixture().await;
    store.prepare_seed_adoption_intent(&database).await.unwrap();
    let authority = store.load_required().unwrap();
    let seed = store.required_seed(&authority).unwrap();
    let other = Database::open(&root.path().join("other.sqlite"))
        .await
        .unwrap();
    other
        .capture_local_shared_state_never_dispatched(root.path())
        .await
        .unwrap();
    let package = other
        .package_local_shared_state_never_dispatched(
            root.path(),
            authority.context(),
            authority.package_key(),
            seed.genesis().commitment(),
        )
        .await
        .unwrap();
    let server = Database::open(&root.path().join("server.sqlite"))
        .await
        .unwrap();
    let outcome = publish_empty_package(
        &server,
        &seed,
        &package.upload_package(),
        authority.package_key(),
    )
    .await;
    let before = serde_json::to_value(database.export_data("fixed".into()).await.unwrap()).unwrap();
    assert!(
        store
            .adopt_seed_publication(&database, &outcome)
            .await
            .is_err()
    );
    assert_eq!(
        before,
        serde_json::to_value(database.export_data("fixed".into()).await.unwrap()).unwrap()
    );
    assert_eq!(
        database
            .seed_publication_intent_bytes()
            .await
            .unwrap()
            .unwrap()
            .1,
        "sealed"
    );
}

#[tokio::test]
async fn actual_source_preparation_excludes_sqlite_and_archive_restore_at_boundary() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("target.sqlite");
    let database = Database::open(&path).await.unwrap();
    let store = isolated_store(&path, &root.path().join("keys"));
    store.prepare_seed_claim(&database, [9; 32]).await.unwrap();
    let source = Database::open(&root.path().join("source.sqlite"))
        .await
        .unwrap();
    let archive = root.path().join("source.tar.zst");
    source
        .create_backup_archive(&root.path().join("source-blobs"), &archive)
        .await
        .unwrap();
    let (entered, waiting) = tokio::sync::oneshot::channel();
    let (release, resume) = tokio::sync::oneshot::channel();
    *SOURCE_BARRIER.lock().unwrap() = Some((path.clone(), entered, resume));
    let preparing = tokio::spawn(async move { store.prepare_seed_source(&database).await });
    waiting.await.unwrap();
    assert!(
        aven_core::db::restore_database_file(&path, source.path())
            .await
            .unwrap_err()
            .to_string()
            .contains("installation-busy")
    );
    assert!(
        aven_core::data_safety::restore_backup_archive(
            &path,
            &root.path().join("target-blobs"),
            &archive
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("installation-busy")
    );
    release.send(()).unwrap();
    preparing.await.unwrap().unwrap();
    assert!(
        aven_core::db::restore_database_file(&path, source.path())
            .await
            .unwrap_err()
            .to_string()
            .contains("e2ee-installation-fenced")
    );
    assert!(
        aven_core::data_safety::restore_backup_archive(
            &path,
            &root.path().join("target-blobs"),
            &archive
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("e2ee-installation-fenced")
    );
    assert!(!root.path().join("target-blobs").exists());
}

#[tokio::test]
async fn process_exit_reopens_preparing_sealed_and_adopted_before_cleanup() {
    for phase in ["preparing", "sealed", "adopted"] {
        let (root, database, store) = local_intent_fixture().await;
        drop(database);
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "protected_local_keys::adoption::tests::adoption_process_worker",
                "--ignored",
            ])
            .env("AVEN_ADOPTION_TEST_ROOT", root.path())
            .env("AVEN_ADOPTION_TEST_PHASE", phase)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(23),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let database = Database::open(&root.path().join("client.sqlite"))
            .await
            .unwrap();
        let (before, state) = database
            .seed_publication_intent_bytes()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(state, phase);
        assert!(
            database
                .cancel_local_shared_state_never_dispatched("anything")
                .await
                .is_err()
        );
        let resumed = store.prepare_seed_adoption_intent(&database).await.unwrap();
        assert_eq!(resumed.protected_storage_bytes(), before);
        if phase == "adopted" {
            let package = store.load_required().unwrap();
            let seed = store.required_seed(&package).unwrap();
            let publication = resumed.publication(seed.genesis()).unwrap();
            let server = Database::open(&root.path().join("server.sqlite"))
                .await
                .unwrap();
            let auth = aven_core::sync::bootstrap_staging::Authentication {
                vault_id: publication.binding().vault_id,
                genesis_commitment: publication.binding().genesis_commitment,
                bearer: seed.bearer(),
            };
            let aven_core::sync::bootstrap_staging::Status::Published(outcome) = server
                .bootstrap_staging_status(&auth, publication.binding().bootstrap_id)
                .await
                .unwrap()
            else {
                panic!("published result")
            };
            assert!(
                !store
                    .adopt_seed_publication(&database, &outcome)
                    .await
                    .unwrap()
            );
            assert!(
                !store
                    .adopt_seed_publication(&database, &outcome)
                    .await
                    .unwrap()
            );
            let mut conn = aven_core::test_support::acquire(&database).await.unwrap();
            assert_eq!(
                sqlx::query_scalar::<_, i64>("SELECT count(*) FROM local_shared_capture_journal")
                    .fetch_one(&mut *conn)
                    .await
                    .unwrap(),
                0
            );
        }
    }
}

#[tokio::test]
#[ignore = "subprocess worker exits without destructors; invoked by the process-boundary test"]
async fn adoption_process_worker() {
    let Some(root) = std::env::var_os("AVEN_ADOPTION_TEST_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let database = Database::open(&root.join("client.sqlite")).await.unwrap();
    let store = isolated_store(database.path(), &root.join("keys"));
    let package = store.load_required().unwrap();
    let seed = store.required_seed(&package).unwrap();
    let source = store
        .decode_source(
            &store
                .load_adoption_record("source", 104, true)
                .unwrap()
                .unwrap(),
            &seed,
        )
        .unwrap();
    let upload = store
        .package_seed_capture(&database, &root, [9; 32])
        .await
        .unwrap()
        .upload_package();
    let phase = std::env::var("AVEN_ADOPTION_TEST_PHASE").unwrap();
    if phase == "preparing" {
        database
            .prepare_seed_publication_intent(&source, &seed, package.package_key())
            .await
            .unwrap();
    } else {
        let intent = store.prepare_seed_adoption_intent(&database).await.unwrap();
        if phase == "adopted" {
            let server = Database::open(&root.join("server.sqlite")).await.unwrap();
            let outcome =
                publish_empty_package(&server, &seed, &upload, package.package_key()).await;
            assert!(
                database
                    .adopt_seed_publication(
                        &source,
                        &intent,
                        &seed,
                        package.package_key(),
                        &outcome
                    )
                    .await
                    .unwrap()
            );
        }
    }
    std::process::exit(23);
}

#[tokio::test]
async fn seal_failure_reuses_protected_intent_and_corrupt_authority_never_resets() {
    let (_root, database, store) = local_intent_fixture().await;
    let mut conn = aven_core::test_support::acquire(&database).await.unwrap();
    sqlx::query("CREATE TRIGGER fail_seal BEFORE UPDATE OF state ON local_seed_publication_intent WHEN NEW.state = 'sealed' BEGIN SELECT RAISE(ABORT, 'injected seal failure'); END").execute(&mut *conn).await.unwrap();
    drop(conn);
    assert!(store.prepare_seed_adoption_intent(&database).await.is_err());
    let (bytes, state) = database
        .seed_publication_intent_bytes()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state, "preparing");
    assert_eq!(
        store
            .load_adoption_record("intent", 65536, true)
            .unwrap()
            .unwrap(),
        bytes
    );
    assert!(
        database
            .cancel_local_shared_state_never_dispatched("anything")
            .await
            .is_err()
    );
    let mut conn = aven_core::test_support::acquire(&database).await.unwrap();
    sqlx::query("DROP TRIGGER fail_seal")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    assert_eq!(
        store
            .prepare_seed_adoption_intent(&database)
            .await
            .unwrap()
            .protected_storage_bytes(),
        bytes
    );
    let path = protected_path(&store, "intent");
    let mut corrupt = fs::read(&path).unwrap();
    corrupt[10] ^= 1;
    fs::write(&path, &corrupt).unwrap();
    assert!(store.prepare_seed_adoption_intent(&database).await.is_err());
    assert_eq!(fs::read(&path).unwrap(), corrupt);
    assert_eq!(
        database
            .seed_publication_intent_bytes()
            .await
            .unwrap()
            .unwrap()
            .0,
        bytes
    );
}
