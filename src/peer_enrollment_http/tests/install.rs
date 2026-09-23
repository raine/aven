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
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn enrolled() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let (source, seed_store, seed, package) =
        crate::seed_bootstrap_http::tests::fixture_with_domain(root.path(), true).await;
    let server = Database::open(&root.path().join("server.sqlite"))
        .await
        .unwrap();
    let (origin, task) = serve(server.clone()).await;
    let seed_http = seed_bootstrap_http::Client::new(&origin).unwrap();
    seed_http
        .claim(
            seed.genesis(),
            aven_core::sync::seed_claim::ClaimAuthentication::SetupSecret(&Secret::new([7; 32])),
        )
        .await
        .unwrap();
    seed_http.resume(&seed_store, &source).await.unwrap();
    let peer = Database::open(&root.path().join("peer.sqlite"))
        .await
        .unwrap();
    let store = isolated_store(peer.path(), &root.path().join("peer-keys"));
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
    }
}
async fn count(db: &Database, table: &str) -> i64 {
    let mut conn = aven_core::test_support::acquire(db).await.unwrap();
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
        .fetch_one(&mut *conn)
        .await
        .unwrap()
}
async fn shared(db: &Database) -> serde_json::Value {
    let mut export = db.export_data("2026-09-22T00:00:00Z".into()).await.unwrap();
    export
        .tables
        .meta
        .retain(|r| r.key.starts_with("epic_membership_baseline:"));
    for row in &mut export.tables.blob_inventory {
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
async fn independent_http_install_preserves_shared_domain_history_images_and_later_edits() {
    let f = enrolled().await;
    let identity = f.peer.meta("client_id").await.unwrap();
    assert_ne!(identity, f.source.meta("client_id").await.unwrap());
    let blobs = f.root.path().join("peer-blobs");
    assert_eq!(count(&f.peer, "tasks").await, 0);
    let result = f.client.install(&f.store, &f.peer, &blobs).await.unwrap();
    assert!(result.prefix_count > 4);
    assert_eq!(result.attachment_count, 1);
    assert_eq!(shared(&f.peer).await, shared(&f.source).await);
    assert_eq!(count(&f.peer, "conflicts").await, 1);
    assert_eq!(count(&f.peer, "local_peer_snapshot_install").await, 1);
    assert_eq!(f.peer.meta("client_id").await.unwrap(), identity);
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
    let store = isolated_store(reopened.path(), &f.root.path().join("peer-keys"));
    assert_eq!(
        f.client.install(&store, &reopened, &blobs).await.unwrap(),
        result
    );
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
        .peer_mailbox(peer.vault(), peer.handle())
        .await
        .unwrap();
    let evidence = mail.evidence.unwrap();
    let grant = peer.open_provisional(&evidence).unwrap();
    let context = Context {
        vault: peer.vault(),
        genesis: grant.genesis,
        device: peer.device(),
        credential_version: 1,
        head: grant.head,
    };
    let descriptor = sha2::Sha256::digest(&f.package.descriptor).into();
    for field in 0..7 {
        let mut wrong = context.clone();
        let mut expected = descriptor;
        match field {
            0 => wrong.vault = [0; 32],
            1 => wrong.genesis = [0; 32],
            2 => wrong.device = [0; 32],
            3 => wrong.credential_version = 2,
            4 => wrong.head = [0; 32],
            5 => expected = [0; 32],
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
                    Some(if field == 6 { &setup } else { peer.bearer() })
                )
                .await
                .is_err()
        );
    }
    let seed_store = isolated_store(f.source.path(), &f.root.path().join("keys"));
    let (seed, _, _) = seed_store
        .prepare_invitation(&f.source, &f.client.locator, None)
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
    assert!(
        f.client
            .install(&f.store, &f.peer, &f.root.path().join("peer-blobs"))
            .await
            .is_err()
    );
    assert_eq!(count(&f.peer, "tasks").await, 0);
    assert_eq!(count(&f.peer, "local_peer_snapshot_install").await, 0);
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
        assert!(f.client.install(&f.store, &f.peer, &blobs).await.is_err());
        assert_eq!(count(&f.peer, "tasks").await, 0);
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
    f.client.install(&f.store, &f.peer, &blobs).await.unwrap();
}

#[tokio::test]
async fn transactional_failure_and_nonempty_target_never_publish_partial_state() {
    let f = enrolled().await;
    let blobs = f.root.path().join("peer-blobs");
    let mut conn = aven_core::test_support::acquire(&f.peer).await.unwrap();
    sqlx::query("CREATE TRIGGER reject_peer_install BEFORE INSERT ON local_peer_snapshot_install BEGIN SELECT RAISE(ABORT,'test'); END").execute(&mut *conn).await.unwrap();
    drop(conn);
    assert!(f.client.install(&f.store, &f.peer, &blobs).await.is_err());
    assert_eq!(count(&f.peer, "tasks").await, 0);
    assert_eq!(count(&f.peer, "blob_inventory").await, 0);
    let mut conn = aven_core::test_support::acquire(&f.peer).await.unwrap();
    sqlx::query("DROP TRIGGER reject_peer_install")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let reopened = Database::open(f.peer.path()).await.unwrap();
    f.client.install(&f.store, &reopened, &blobs).await.unwrap();
    let state = shared(&reopened).await;
    // Receipt mismatch is a refusal, not a reinstall or cursor repair.
    let mut conn = aven_core::test_support::acquire(&reopened).await.unwrap();
    sqlx::query("UPDATE meta SET value='0' WHERE key='sync_cursor'")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    assert!(f.client.install(&f.store, &reopened, &blobs).await.is_err());
    assert_eq!(shared(&reopened).await, state);
}

#[tokio::test]
#[ignore = "subprocess snapshot persistence worker"]
async fn process_worker() {
    let root = std::path::PathBuf::from(std::env::var_os("AVEN_SNAPSHOT_ROOT").unwrap());
    let origin = std::env::var("AVEN_SNAPSHOT_ORIGIN").unwrap();
    let peer = Database::open(&root.join("peer.sqlite")).await.unwrap();
    let store = isolated_store(peer.path(), &root.join("peer-keys"));
    Client::new(&origin)
        .unwrap()
        .install(&store, &peer, &root.join("peer-blobs"))
        .await
        .unwrap();
    panic!("expected process exit");
}

#[tokio::test]
async fn process_restart_download_files_and_atomic_commit_boundaries() {
    let f = enrolled().await;
    let identity = f.peer.meta("client_id").await.unwrap();
    for stage in ["download", "files", "before-commit", "after-commit"] {
        let output = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "peer_enrollment_http::tests::install::process_worker",
                "--ignored",
                "--nocapture",
            ])
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
            if stage != "download" {
                assert_eq!(
                    std::fs::read_dir(blobs.join("objects/sha256"))
                        .unwrap()
                        .count(),
                    0
                );
            }
            assert_eq!(count(&peer, "tasks").await, 0);
            assert_eq!(count(&peer, "changes").await, 0);
        } else {
            assert_eq!(shared(&peer).await, shared(&f.source).await);
            assert_eq!(
                std::fs::read_dir(blobs.join("objects/sha256"))
                    .unwrap()
                    .count(),
                1
            );
        }
    }
    f.task.abort();
    let peer = Database::open(f.peer.path()).await.unwrap();
    let store = isolated_store(peer.path(), &f.root.path().join("peer-keys"));
    f.client
        .install(&store, &peer, &f.root.path().join("peer-blobs"))
        .await
        .unwrap();
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
    assert!(
        f.client
            .install(&f.store, &f.peer, &f.root.path().join("peer-blobs"))
            .await
            .is_err()
    );
    assert_eq!(before, shared(&f.peer).await);
    let f = enrolled().await;
    let blobs = f.root.path().join("peer-blobs");
    f.client.install(&f.store, &f.peer, &blobs).await.unwrap();
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
    let copied_store = isolated_store(copied.path(), &f.root.path().join("peer-keys"));
    assert!(
        f.client
            .install(&copied_store, &copied, &blobs)
            .await
            .is_err()
    );
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
    assert!(f.client.install(&f.store, &f.peer, &blobs).await.is_err());
    assert_eq!(before, shared(&f.peer).await);
}

#[tokio::test]
async fn foreign_image_and_dishonest_descriptor_response_cannot_select_a_publication() {
    let mut f = enrolled().await;
    let other = enrolled().await;
    let blobs = f.root.path().join("peer-blobs");
    let mut conn = aven_core::test_support::acquire(&f.server).await.unwrap();
    sqlx::query("UPDATE server_e2ee_image_chunks SET bytes=? WHERE chunk_index=0")
        .bind(&other.package.images[0].records[0])
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    assert!(f.client.install(&f.store, &f.peer, &blobs).await.is_err());
    assert_eq!(count(&f.peer, "tasks").await, 0);
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
    let error = client.install(&f.store, &f.peer, &blobs).await.unwrap_err();
    assert_eq!(error.to_string(), "error snapshot-descriptor-substitution");
    assert_eq!(count(&f.peer, "tasks").await, 0);
}

#[tokio::test]
async fn control_request_cap_rejects_padding_while_large_published_image_installs() {
    let f = enrolled().await;
    let peer = f
        .store
        .prepare_peer(&f.peer, &f.client.locator, None)
        .await
        .unwrap();
    let evidence = f
        .server
        .peer_mailbox(peer.vault(), peer.handle())
        .await
        .unwrap()
        .evidence
        .unwrap();
    let grant = peer.open_provisional(&evidence).unwrap();
    let op = Operation::Published {
        context: Context {
            vault: peer.vault(),
            genesis: grant.genesis,
            device: peer.device(),
            credential_version: 1,
            head: grant.head,
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
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(response.text().await.unwrap(), "enrollment-refused");

    let reply = f.client.exchange(op, Some(peer.bearer())).await.unwrap();
    let encoded_length = serde_json::to_vec(&reply).unwrap().len();
    assert!(encoded_length > CONTROL_LIMIT && encoded_length < PUBLISHED_RESPONSE_LIMIT);
    let Reply::Published(bytes) = reply else {
        panic!("expected published image");
    };
    assert_eq!(bytes, f.package.images[0].records[0]);
    f.client
        .install(&f.store, &f.peer, &f.root.path().join("peer-blobs"))
        .await
        .unwrap();
    assert_eq!(shared(&f.peer).await, shared(&f.source).await);
}
