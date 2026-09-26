use sha2::Digest;

use super::*;
use aven_core::sync::bootstrap_staging::Component;

struct Fixture {
    root: tempfile::TempDir,
    source: Database,
    server: Database,
    peer: Database,
    store: ProtectedLocalKeyStore,
    client: Client,
    task: tokio::task::JoinHandle<()>,
    package: aven_core::sync::bootstrap_format::Package,
    counts: Arc<ExchangeCounts>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn enrolled() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let (source, seed_store, seed, package) =
        e2ee_http::fixture_with_domain(root.path(), true).await;
    let server = Database::open(&root.path().join("server.sqlite"))
        .await
        .unwrap();
    let counts = Arc::new(ExchangeCounts::default());
    let (origin, task) = serve_counted(server.clone(), counts.clone()).await;
    e2ee_http::adopt(&origin, &source, &seed_store, &seed).await;
    let peer = Database::open(&root.path().join("peer.sqlite"))
        .await
        .unwrap();
    let store = isolated_store(&peer, &root.path().join("peer-keys")).await;
    let client = Client::new(&origin).unwrap();
    let invitation = client.invite(&seed_store, &source, expiry()).await.unwrap();
    client
        .request(&store, &peer, Some(invitation))
        .await
        .unwrap();
    assert!(client.admit(&seed_store, &source).await.unwrap());
    assert!(client.complete(&store, &peer).await.unwrap());
    Fixture {
        root,
        source,
        server,
        peer,
        store,
        client,
        task,
        package,
        counts,
    }
}
async fn count(db: &Database, table: &str) -> i64 {
    let mut conn = aven_core::test_support::acquire(db).await.unwrap();
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
        .fetch_one(&mut *conn)
        .await
        .unwrap()
}
async fn pending_peer(
    fixture: &Fixture,
    name: &str,
) -> (Database, ProtectedLocalKeyStore, [u8; 32]) {
    let seed_store = isolated_store(&fixture.source, &fixture.root.path().join("keys")).await;
    let invitation = fixture
        .client
        .invite(&seed_store, &fixture.source, expiry())
        .await
        .unwrap();
    let database = Database::open(&fixture.root.path().join(format!("{name}.sqlite")))
        .await
        .unwrap();
    let store = isolated_store(&database, &fixture.root.path().join(format!("{name}-keys"))).await;
    fixture
        .client
        .request(&store, &database, Some(invitation))
        .await
        .unwrap();
    let peer = store
        .prepare_peer(&database, &fixture.client.locator, None)
        .await
        .unwrap();
    let device = peer.device();
    (database, store, device)
}

async fn shared(db: &Database) -> serde_json::Value {
    let mut export = db.export_data("2026-09-22T00:00:00Z".into()).await.unwrap();
    export
        .tables
        .meta
        .retain(|r| r.key.starts_with("epic_membership_baseline:"));
    for row in &mut export.tables.blob_inventory {
        row.available = 0;
        row.last_verified_at = None;
        row.first_seen_at.clear();
    }
    let mut value = serde_json::to_value(export.tables).unwrap();
    for rows in value.as_object_mut().unwrap().values_mut() {
        if let Some(rows) = rows.as_array_mut() {
            rows.sort_by_key(|r| serde_json::to_string(r).unwrap());
        }
    }
    value
}
#[tokio::test]
async fn snapshot_download_refreshes_once_before_component_reads() {
    let f = enrolled().await;
    let peer = f
        .store
        .prepare_peer(&f.peer, &f.client.locator, None)
        .await
        .unwrap();
    f.counts.track(peer.device());
    f.counts.reset();

    f.client.install(&f.store, &f.peer).await.unwrap();

    let membership = f.counts.tracked_membership.load(Ordering::Relaxed);
    let published = f.counts.tracked_published.load(Ordering::Relaxed);
    assert_eq!(membership, 1);
    assert_eq!(published, 6);
    assert_eq!(f.counts.total.load(Ordering::Relaxed), 7);
}

#[tokio::test]
async fn snapshot_download_refreshes_after_stale_and_keeps_one_retry_budget() {
    let f = enrolled().await;
    let peer = f
        .store
        .prepare_peer(&f.peer, &f.client.locator, None)
        .await
        .unwrap();
    let (_, _, second_device) = pending_peer(&f, "second").await;
    f.counts.track(peer.device());
    f.counts.reset();
    let gate = f.counts.add_stale_gate();
    let source = f.source.clone();
    let source_store = isolated_store(&f.source, &f.root.path().join("keys")).await;
    let client = Client::new(&f.client.locator).unwrap();
    let admission = tokio::spawn(async move {
        gate.started.notified().await;
        let result = client.admit(&source_store, &source).await;
        gate.finished.notify_one();
        result
    });

    f.client.install(&f.store, &f.peer).await.unwrap();
    assert!(admission.await.unwrap().unwrap());
    assert_ne!(second_device, peer.device());
    assert_eq!(f.counts.tracked_membership.load(Ordering::Relaxed), 2);
    assert_eq!(f.counts.tracked_published.load(Ordering::Relaxed), 7);
}

#[tokio::test]
async fn snapshot_download_removal_fails_closed_without_stale_retry() {
    let f = enrolled().await;
    let peer = f
        .store
        .prepare_peer(&f.peer, &f.client.locator, None)
        .await
        .unwrap();
    f.counts.track(peer.device());
    f.counts.reset();
    let gate = f.counts.add_stale_gate();
    let source = f.source.clone();
    let source_store = isolated_store(&f.source, &f.root.path().join("keys")).await;
    let client = Client::new(&f.client.locator).unwrap();
    let removal = tokio::spawn(async move {
        gate.started.notified().await;
        let result = client
            .remove_device(&source_store, &source, peer.device())
            .await;
        gate.finished.notify_one();
        result
    });

    let error = f.client.install(&f.store, &f.peer).await.unwrap_err();
    assert!(!enrollment::is_stale(&error));
    assert_eq!(removal.await.unwrap().unwrap(), RemovalStatus::Complete);
    assert_eq!(f.counts.tracked_membership.load(Ordering::Relaxed), 1);
    assert_eq!(f.counts.tracked_published.load(Ordering::Relaxed), 1);
    assert_eq!(count(&f.peer, "local_peer_snapshot_install").await, 0);
}

#[tokio::test]
async fn snapshot_download_bounds_exhausted_stale_retry() {
    let f = enrolled().await;
    let peer = f
        .store
        .prepare_peer(&f.peer, &f.client.locator, None)
        .await
        .unwrap();
    let _ = pending_peer(&f, "second").await;
    f.counts.track(peer.device());
    f.counts.reset();
    let first_gate = f.counts.add_stale_gate();
    let second_gate = f.counts.add_stale_gate();
    let keys_path = f.root.path().join("keys");
    let origin = f.client.locator.clone();
    let first_source = f.source.clone();
    let first_store = isolated_store(&f.source, &keys_path).await;
    let first_client = Client::new(&origin).unwrap();
    let third_root = f.root.path().to_path_buf();
    let first_admission = tokio::spawn(async move {
        first_gate.started.notified().await;
        let result = async {
            let admitted = first_client.admit(&first_store, &first_source).await?;
            if admitted {
                let invitation = first_client
                    .invite(&first_store, &first_source, expiry())
                    .await?;
                let third_database = Database::open(&third_root.join("third.sqlite")).await?;
                let third_store =
                    isolated_store(&third_database, &third_root.join("third-keys")).await;
                first_client
                    .request(&third_store, &third_database, Some(invitation))
                    .await?;
            }
            Ok::<_, anyhow::Error>(admitted)
        }
        .await;
        first_gate.finished.notify_one();
        result
    });
    let second_source = f.source.clone();
    let second_store = isolated_store(&f.source, &keys_path).await;
    let second_client = Client::new(&origin).unwrap();
    let second_admission = tokio::spawn(async move {
        second_gate.started.notified().await;
        let result = second_client.admit(&second_store, &second_source).await;
        second_gate.finished.notify_one();
        result
    });

    let error = f.client.install(&f.store, &f.peer).await.unwrap_err();
    assert!(enrollment::is_stale(&error));
    assert!(first_admission.await.unwrap().unwrap());
    assert!(second_admission.await.unwrap().unwrap());
    assert_eq!(f.counts.tracked_membership.load(Ordering::Relaxed), 2);
    assert_eq!(f.counts.tracked_published.load(Ordering::Relaxed), 2);
    assert_eq!(count(&f.peer, "local_peer_snapshot_install").await, 0);
}

#[tokio::test]
async fn independent_http_install_preserves_shared_domain_history_images_and_later_edits() {
    let f = enrolled().await;
    let identity = f.peer.meta("client_id").await.unwrap();
    assert_ne!(identity, f.source.meta("client_id").await.unwrap());
    let blobs = f.root.path().join("peer-blobs");
    assert_eq!(count(&f.peer, "tasks").await, 0);
    let result = f.client.install(&f.store, &f.peer).await.unwrap();
    assert!(result.prefix_count > 4);
    assert_eq!(result.attachment_count, 1);
    assert_eq!(shared(&f.peer).await, shared(&f.source).await);
    assert_eq!(count(&f.peer, "conflicts").await, 1);
    assert_eq!(count(&f.peer, "local_peer_snapshot_install").await, 1);
    assert_eq!(f.peer.meta("client_id").await.unwrap(), identity);
    assert_eq!(
        f.peer
            .meta("e2ee_initial_image_watermark")
            .await
            .unwrap()
            .as_deref(),
        Some("pending")
    );
    assert!(!blobs.join("objects/sha256").exists());
    let available: i64 =
        sqlx::query_scalar("SELECT count(*) FROM blob_inventory WHERE available=1")
            .fetch_one(&mut *aven_core::test_support::acquire(&f.peer).await.unwrap())
            .await
            .unwrap();
    assert_eq!(available, 0);
    let transfer = crate::encrypted_tail_http::Client::new(&f.client.locator)
        .unwrap()
        .round(&f.store, &f.peer, &blobs)
        .await
        .unwrap();
    assert_eq!(
        transfer.images,
        crate::encrypted_tail_http::ImageTransfer::Complete
    );
    for image in std::fs::read_dir(f.root.path().join("objects/sha256")).unwrap() {
        let image = image.unwrap();
        assert_eq!(
            std::fs::read(image.path()).unwrap(),
            std::fs::read(blobs.join("objects/sha256").join(image.file_name())).unwrap()
        );
    }
    let workspace = f.peer.list_workspaces().await.unwrap().remove(0);
    let export = f.peer.export_data("now".into()).await.unwrap();
    let task = &export
        .tables
        .tasks
        .iter()
        .find(|t| t.title == "epic")
        .unwrap()
        .id;
    f.peer
        .update_task(
            &workspace,
            task,
            aven_core::operations::TaskUpdate {
                title: Some("KEEP LATER EDIT".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let after = shared(&f.peer).await;
    f.task.abort();
    let reopened = Database::open(f.peer.path()).await.unwrap();
    let store = isolated_store(&reopened, &f.root.path().join("peer-keys")).await;
    assert_eq!(f.client.install(&store, &reopened).await.unwrap(), result);
    assert_eq!(shared(&reopened).await, after);
    assert!(
        reopened
            .install_shared_state(&f.source.capture_shared_state().await.unwrap())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn published_http_requires_exact_current_context_and_never_restores_missing_images() {
    let f = enrolled().await;
    let peer = f
        .store
        .prepare_peer(&f.peer, &f.client.locator, None)
        .await
        .unwrap();
    let mail = f
        .server
        .membership_mailbox(peer.vault(), peer.handle())
        .await
        .unwrap();
    let evidence = mail;
    let grant = peer
        .open_provisional(
            &evidence.declaration,
            evidence.admission.as_deref().unwrap(),
        )
        .unwrap();
    let context = Context {
        vault: peer.vault(),
        genesis: grant.genesis,
        device: peer.device(),
        head: grant.outcome,
    };
    let descriptor = sha2::Sha256::digest(&f.package.descriptor).into();
    for field in 0..6 {
        let mut wrong = context.clone();
        let mut expected = descriptor;
        match field {
            0 => wrong.vault = [0; 32],
            1 => wrong.genesis = [0; 32],
            2 => wrong.device = [0; 32],
            3 => wrong.head = [0; 32],
            4 => expected = [0; 32],
            _ => {}
        }
        let setup = Secret::new([7; 32]);
        assert!(
            f.client
                .exchange(
                    Operation::Published {
                        context: wrong,
                        descriptor: expected,
                        component: None,
                        index: 0
                    },
                    Some(if field == 5 { &setup } else { peer.bearer() })
                )
                .await
                .is_err()
        );
    }
    let seed_store = isolated_store(&f.source, &f.root.path().join("keys")).await;
    let seed = seed_store
        .active_inputs(&f.source, &f.client.locator)
        .await
        .unwrap();
    assert!(
        f.client
            .exchange(
                Operation::Published {
                    context: context.clone(),
                    descriptor,
                    component: None,
                    index: 0
                },
                Some(seed.bearer())
            )
            .await
            .is_err()
    );
    let owned = f
        .server
        .published_snapshot_read(
            &context.auth(peer.bearer()),
            descriptor,
            Some(Component::Image(f.package.images[0].object_id)),
            0,
        )
        .await
        .unwrap();
    assert_eq!(owned, f.package.images[0].records[0]);
    let mut conn = aven_core::test_support::acquire(&f.server).await.unwrap();
    sqlx::query("DELETE FROM server_e2ee_image_chunks")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    f.client.install(&f.store, &f.peer).await.unwrap();
    assert!(count(&f.peer, "tasks").await > 0);
    assert_eq!(count(&f.peer, "local_peer_snapshot_install").await, 1);
    let transfer = crate::encrypted_tail_http::Client::new(&f.client.locator)
        .unwrap()
        .round(&f.store, &f.peer, &f.root.path().join("peer-blobs"))
        .await
        .unwrap();
    assert!(transfer.metadata_caught_up);
    assert_eq!(
        transfer.images,
        crate::encrypted_tail_http::ImageTransfer::Unavailable
    );
    assert_eq!(count(&f.server, "server_e2ee_image_chunks").await, 0);
    assert_eq!(owned, f.package.images[0].records[0]);
    let mut conn = aven_core::test_support::acquire(&f.server).await.unwrap();
    sqlx::query("UPDATE server_e2ee_membership_head SET sequence=3")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    assert!(
        f.client
            .exchange(
                Operation::Published {
                    context: context.clone(),
                    descriptor,
                    component: None,
                    index: 0
                },
                Some(peer.bearer())
            )
            .await
            .is_err()
    );

    assert!(
        f.client
            .exchange(
                Operation::Published {
                    context,
                    descriptor,
                    component: Some(Component::Image([0; 32])),
                    index: 0
                },
                Some(peer.bearer())
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn substituted_http_catalog_and_chunk_refuse_without_visible_domain() {
    let f = enrolled().await;
    let blobs = f.root.path().join("peer-blobs");
    for component in [vec![0_u8], vec![3_u8], vec![4_u8]] {
        let mut conn = aven_core::test_support::acquire(&f.server).await.unwrap();
        let original: Vec<u8> = sqlx::query_scalar(
            "SELECT bytes FROM server_bootstrap_chunks WHERE component=? AND chunk_index=0",
        )
        .bind(&component)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        let mut bad = original.clone();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        sqlx::query(
            "UPDATE server_bootstrap_chunks SET bytes=? WHERE component=? AND chunk_index=0",
        )
        .bind(bad)
        .bind(&component)
        .execute(&mut *conn)
        .await
        .unwrap();
        drop(conn);
        assert!(f.client.install(&f.store, &f.peer).await.is_err());
        assert_eq!(count(&f.peer, "tasks").await, 0);
        assert_eq!(count(&f.peer, "local_peer_snapshot_install").await, 0);
        assert_eq!(count(&f.peer, "local_e2ee_image_initialization").await, 0);
        assert_eq!(count(&f.peer, "local_e2ee_image_objects").await, 0);
        assert_eq!(
            f.peer.meta("e2ee_initial_image_watermark").await.unwrap(),
            None
        );
        assert!(!blobs.exists());
        let mut conn = aven_core::test_support::acquire(&f.server).await.unwrap();
        sqlx::query(
            "UPDATE server_bootstrap_chunks SET bytes=? WHERE component=? AND chunk_index=0",
        )
        .bind(original)
        .bind(&component)
        .execute(&mut *conn)
        .await
        .unwrap();
    }
    f.client.install(&f.store, &f.peer).await.unwrap();
}

#[tokio::test]
async fn transactional_failure_and_nonempty_target_never_publish_partial_state() {
    for fault in [
        "CREATE TRIGGER reject_peer_install BEFORE INSERT ON local_peer_snapshot_install BEGIN SELECT RAISE(ABORT,'test'); END",
        "CREATE TRIGGER reject_peer_install BEFORE INSERT ON local_e2ee_dependency_baseline BEGIN SELECT RAISE(ABORT,'test'); END",
    ] {
        let f = enrolled().await;
        let blobs = f.root.path().join("peer-blobs");
        let mut conn = aven_core::test_support::acquire(&f.peer).await.unwrap();
        sqlx::query(fault).execute(&mut *conn).await.unwrap();
        drop(conn);
        assert!(f.client.install(&f.store, &f.peer).await.is_err());
        assert_eq!(count(&f.peer, "tasks").await, 0);
        assert_eq!(count(&f.peer, "blob_inventory").await, 0);
        assert_eq!(count(&f.peer, "local_e2ee_image_initialization").await, 0);
        assert_eq!(
            f.peer.meta("e2ee_initial_image_watermark").await.unwrap(),
            None
        );
        assert!(!blobs.exists());
        assert_eq!(count(&f.peer, "local_e2ee_dependency_baseline").await, 0);
        assert_eq!(count(&f.peer, "local_e2ee_dependency_edges").await, 0);
        let mut conn = aven_core::test_support::acquire(&f.peer).await.unwrap();
        sqlx::query("DROP TRIGGER reject_peer_install")
            .execute(&mut *conn)
            .await
            .unwrap();
        drop(conn);
        let reopened = Database::open(f.peer.path()).await.unwrap();
        f.client.install(&f.store, &reopened).await.unwrap();
        let state = shared(&reopened).await;
        // Receipt mismatch is a refusal, not a reinstall or cursor repair.
        let mut conn = aven_core::test_support::acquire(&reopened).await.unwrap();
        sqlx::query("UPDATE meta SET value='0' WHERE key='sync_cursor'")
            .execute(&mut *conn)
            .await
            .unwrap();
        drop(conn);
        assert!(f.client.install(&f.store, &reopened).await.is_err());
        assert_eq!(shared(&reopened).await, state);
    }
}

#[tokio::test]
#[ignore = "subprocess snapshot persistence worker"]
async fn process_worker() {
    let root = std::path::PathBuf::from(std::env::var_os("AVEN_SNAPSHOT_ROOT").unwrap());
    let origin = std::env::var("AVEN_SNAPSHOT_ORIGIN").unwrap();
    let peer = Database::open(&root.join("peer.sqlite")).await.unwrap();
    let store = isolated_store(&peer, &root.join("peer-keys")).await;
    Client::new(&origin)
        .unwrap()
        .install(&store, &peer)
        .await
        .unwrap();
    panic!("expected process exit");
}

#[tokio::test]
async fn process_restart_download_metadata_and_atomic_commit_boundaries() {
    let f = enrolled().await;
    let identity = f.peer.meta("client_id").await.unwrap();
    for stage in ["download", "metadata", "before-commit", "after-commit"] {
        let output = e2ee_http::worker("peer_enrollment_http::tests::install::process_worker")
            .env("AVEN_SNAPSHOT_ROOT", f.root.path())
            .env("AVEN_SNAPSHOT_ORIGIN", &f.client.locator)
            .env("AVEN_SNAPSHOT_CRASH", stage)
            .output()
            .await
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(83),
            "stage={stage} {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let peer = Database::open(f.peer.path()).await.unwrap();
        assert_eq!(peer.meta("client_id").await.unwrap(), identity);
        assert_eq!(
            count(&peer, "local_peer_snapshot_install").await,
            i64::from(stage == "after-commit")
        );
        let blobs = f.root.path().join("peer-blobs");
        peer.prune_attachments(
            &blobs,
            aven_core::attachments::LifecyclePolicy {
                grace: std::time::Duration::ZERO,
                ..Default::default()
            },
            true,
        )
        .await
        .unwrap();
        if stage != "after-commit" {
            assert_eq!(count(&peer, "tasks").await, 0);
            assert_eq!(count(&peer, "changes").await, 0);
        } else {
            assert_eq!(shared(&peer).await, shared(&f.source).await);
            assert!(!blobs.join("objects/sha256").exists());
            assert_eq!(
                peer.meta("e2ee_initial_image_watermark")
                    .await
                    .unwrap()
                    .as_deref(),
                Some("pending")
            );
        }
    }
    f.task.abort();
    let peer = Database::open(f.peer.path()).await.unwrap();
    let store = isolated_store(&peer, &f.root.path().join("peer-keys")).await;
    f.client.install(&store, &peer).await.unwrap();
    assert_eq!(count(&peer, "local_peer_snapshot_install").await, 1);
    assert_eq!(shared(&peer).await, shared(&f.source).await);
}

#[tokio::test]
async fn local_edit_after_enrollment_blocks_install_and_protected_loss_blocks_receipt() {
    let f = enrolled().await;
    let ws = f.peer.list_workspaces().await.unwrap().remove(0);
    f.peer
        .create_task(
            &ws,
            aven_core::operations::TaskDraft {
                title: "KEEP LOCAL".into(),
                description: "".into(),
                project: Some("local".into()),
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
        .unwrap();
    let before = shared(&f.peer).await;
    assert!(f.client.install(&f.store, &f.peer).await.is_err());
    assert_eq!(before, shared(&f.peer).await);
    let f = enrolled().await;
    f.client.install(&f.store, &f.peer).await.unwrap();
    let before = shared(&f.peer).await;
    let copy_path = f.root.path().join("copied.sqlite");
    let mut conn = aven_core::test_support::acquire(&f.peer).await.unwrap();
    sqlx::query("VACUUM INTO ?")
        .bind(copy_path.to_str().unwrap())
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let copied = Database::open(&copy_path).await.unwrap();
    let copied_store = isolated_store(&copied, &f.root.path().join("peer-keys")).await;
    assert!(f.client.install(&copied_store, &copied).await.is_err());
    assert_eq!(shared(&copied).await, before);
    let path = std::fs::read_dir(f.root.path().join("peer-keys"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .ends_with(".peer-verified")
        })
        .unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(f.client.install(&f.store, &f.peer).await.is_err());
    assert_eq!(before, shared(&f.peer).await);
}

#[tokio::test]
async fn foreign_image_fails_transfer_and_dishonest_descriptor_refuses_install() {
    let f = enrolled().await;
    let other = enrolled().await;
    let blobs = f.root.path().join("peer-blobs");
    let mut conn = aven_core::test_support::acquire(&f.server).await.unwrap();
    sqlx::query("UPDATE server_e2ee_image_chunks SET bytes=? WHERE chunk_index=0")
        .bind(&other.package.images[0].records[0])
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    f.client.install(&f.store, &f.peer).await.unwrap();
    assert!(count(&f.peer, "tasks").await > 0);
    let transfer = crate::encrypted_tail_http::Client::new(&f.client.locator)
        .unwrap()
        .round(&f.store, &f.peer, &blobs)
        .await
        .unwrap();
    assert_eq!(
        transfer.images,
        crate::encrypted_tail_http::ImageTransfer::Failed
    );
    assert!(!blobs.join("objects/sha256").exists());

    let mut f = enrolled().await;
    f.task.abort();
    let _ = (&mut f.task).await;
    let listener = tokio::net::TcpListener::bind(f.client.locator.strip_prefix("http://").unwrap())
        .await
        .unwrap();
    let descriptor = other.package.descriptor.clone();
    let app = Router::new().route(
        PATH,
        post(move || {
            let descriptor = descriptor.clone();
            async move { axum::Json(Reply::Published(descriptor)) }
        }),
    );
    f.task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = Client::new(&f.client.locator).unwrap();
    let error = client.install(&f.store, &f.peer).await.unwrap_err();
    assert_eq!(error.to_string(), "error membership-response");
    assert_eq!(count(&f.peer, "tasks").await, 0);
}

#[tokio::test]
async fn control_request_cap_rejects_padding_while_large_published_image_reads_succeed() {
    let f = enrolled().await;
    let peer = f
        .store
        .prepare_peer(&f.peer, &f.client.locator, None)
        .await
        .unwrap();
    let evidence = f
        .server
        .membership_mailbox(peer.vault(), peer.handle())
        .await
        .unwrap();
    let grant = peer
        .open_provisional(
            &evidence.declaration,
            evidence.admission.as_deref().unwrap(),
        )
        .unwrap();
    let op = Operation::Published {
        context: Context {
            vault: peer.vault(),
            genesis: grant.genesis,
            device: peer.device(),
            head: grant.outcome,
        },
        descriptor: sha2::Sha256::digest(&f.package.descriptor).into(),
        component: Some(Component::Image(f.package.images[0].object_id)),
        index: 0,
    };
    let mut body = serde_json::to_vec(&op).unwrap();
    assert!(body.len() < CONTROL_LIMIT);
    body.resize(CONTROL_LIMIT + 1, b' ');
    assert!(body.len() < PUBLISHED_RESPONSE_LIMIT);
    // Valid JSON and authority: only the request size should cause refusal.
    assert!(serde_json::from_slice::<Operation>(&body).is_ok());
    let response = f
        .client
        .transport
        .http
        .post(f.client.transport.endpoint.clone())
        .header(header::CONTENT_TYPE, "application/json")
        .bearer_auth(hex::encode(peer.bearer().expose()))
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        response.text().await.unwrap(),
        r#"{"error":"enrollment-limit"}"#
    );

    let reply = f.client.exchange(op, Some(peer.bearer())).await.unwrap();
    let encoded_length = serde_json::to_vec(&reply).unwrap().len();
    assert!(encoded_length > CONTROL_LIMIT && encoded_length < PUBLISHED_RESPONSE_LIMIT);
    let Reply::Published(bytes) = reply else {
        panic!("expected published image");
    };
    assert_eq!(bytes, f.package.images[0].records[0]);
    f.client.install(&f.store, &f.peer).await.unwrap();
    assert_eq!(shared(&f.peer).await, shared(&f.source).await);
}

mod attachments;
