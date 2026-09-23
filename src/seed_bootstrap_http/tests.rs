use super::*;
use crate::protected_local_keys::tests::isolated_store;
use aven_core::sync::bootstrap_format::Package;
use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
};

pub(crate) fn setup() -> SetupAuthority {
    SetupAuthority::from_verifier(
        [9; 32],
        SetupAuthority::verifier([9; 32], &Secret::new([7; 32])),
    )
}

pub(crate) async fn fixture(
    root: &Path,
) -> (
    Database,
    ProtectedLocalKeyStore,
    aven_core::sync::seed_claim::SeedAuthority,
    Package,
) {
    let db = Database::open(&root.join("client.sqlite")).await.unwrap();
    let workspace = db.list_workspaces().await.unwrap().remove(0);
    let task = db
        .create_task(
            &workspace,
            aven_core::operations::TaskDraft {
                title: "PRIVATE-HTTP-SEED-TASK".into(),
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
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image::RgbaImage::new(3, 2))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    db.add_task_attachment(
        &workspace,
        root,
        Default::default(),
        &task.id,
        aven_core::operations::AttachmentAddInput {
            filename: Some("PRIVATE-HTTP-IMAGE.png".into()),
            alt_text: None,
            declared_media_type: None,
            bytes: bytes.into_inner(),
            optimization_policy: aven_core::attachments::ImageOptimizationPolicy::Preserve,
            dedupe_existing: false,
        },
    )
    .await
    .unwrap();
    let store = isolated_store(db.path(), &root.join("keys"));
    let seed = store.prepare_seed_claim(&db, [9; 32]).await.unwrap();
    store.prepare_seed_source(&db).await.unwrap();
    db.capture_local_shared_state_never_dispatched(root)
        .await
        .unwrap();
    let package = store
        .package_seed_capture(&db, root, [9; 32])
        .await
        .unwrap()
        .upload_package();
    db.update_task(
        &workspace,
        &task.id,
        aven_core::operations::TaskUpdate {
            title: Some("AFTER-CAPTURE".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    (db, store, seed, package)
}

async fn serve(db: Database) -> (Client, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = Client::new(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, router(db, Some(setup()), Default::default()))
            .await
            .unwrap();
    });
    (client, task)
}

fn budget(package: &Package) -> staging::Budget {
    let chunks = components(package);
    staging::Budget {
        bytes: chunks
            .iter()
            .flat_map(|(_, c)| c)
            .map(|c| c.len() as u64)
            .sum(),
        chunks: chunks.iter().map(|(_, c)| c.len() as u64).sum(),
    }
}

#[tokio::test]
async fn loopback_rejects_bad_authority_context_bytes_and_epochs_without_mutation() {
    let root = tempfile::tempdir().unwrap();
    let (db, store, seed, package) = fixture(root.path()).await;
    let server = Database::open(&root.path().join("server.sqlite"))
        .await
        .unwrap();
    let (http, task) = serve(server.clone()).await;
    assert!(
        http.claim(
            seed.genesis(),
            ClaimAuthentication::SetupSecret(&Secret::new([0; 32]))
        )
        .await
        .is_err()
    );
    // An otherwise valid setup credential with wrong context cannot claim.
    let bad = Envelope {
        vault: [0; 32],
        genesis: seed.genesis().commitment(),
        operation: Operation::ClaimSetup {
            bytes: seed.genesis().claim_bytes(),
        },
    };
    let response = http
        .http
        .post(http.endpoint.clone())
        .header(header::CONTENT_TYPE, "application/json")
        .bearer_auth(hex::encode([7; 32]))
        .body(serde_json::to_vec(&bad).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let mut conn = aven_core::test_support::acquire(&server).await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM server_seed_claim")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(count, 0);
    drop(conn);
    assert!(
        http.claim(
            seed.genesis(),
            ClaimAuthentication::SeedBearer(seed.bearer())
        )
        .await
        .is_err()
    );
    http.claim(
        seed.genesis(),
        ClaimAuthentication::SetupSecret(&Secret::new([7; 32])),
    )
    .await
    .unwrap();
    http.claim(
        seed.genesis(),
        ClaimAuthentication::SetupSecret(&Secret::new([7; 32])),
    )
    .await
    .unwrap();
    http.claim(
        seed.genesis(),
        ClaimAuthentication::SeedBearer(seed.bearer()),
    )
    .await
    .unwrap();
    let intent = store.prepare_seed_adoption_intent(&db).await.unwrap();
    let signed = intent.publication(seed.genesis()).unwrap();
    let b = signed.binding();
    for secret in [Secret::new([0; 32]), Secret::new([7; 32])] {
        assert!(
            http.exchange(
                seed.genesis(),
                &secret,
                Operation::Declare {
                    descriptor: package.descriptor.clone(),
                    budget: budget(&package)
                }
            )
            .await
            .is_err()
        );
        assert!(
            http.exchange(
                seed.genesis(),
                &secret,
                Operation::Status {
                    bootstrap: b.bootstrap_id
                }
            )
            .await
            .is_err()
        );
    }
    assert!(matches!(
        http.exchange(
            seed.genesis(),
            seed.bearer(),
            Operation::Status {
                bootstrap: b.bootstrap_id
            }
        )
        .await
        .unwrap(),
        Reply::Missing
    ));
    let Reply::Staging(initial) = http
        .exchange(
            seed.genesis(),
            seed.bearer(),
            Operation::Declare {
                descriptor: package.descriptor.clone(),
                budget: budget(&package),
            },
        )
        .await
        .unwrap()
    else {
        panic!()
    };
    let status = || Operation::Status {
        bootstrap: b.bootstrap_id,
    };
    let before = serde_json::to_vec(
        &http
            .exchange(seed.genesis(), seed.bearer(), status())
            .await
            .unwrap(),
    )
    .unwrap();
    let publish = || Operation::Publish {
        bootstrap: b.bootstrap_id,
        commitment: b.descriptor_commitment,
        epoch: initial.epoch,
        record: signed.record().to_vec(),
    };
    assert!(
        http.exchange(seed.genesis(), seed.bearer(), publish())
            .await
            .is_err()
    ); // incomplete catalogs/artifacts
    let put = |epoch, bytes| Operation::Put {
        bootstrap: b.bootstrap_id,
        commitment: b.descriptor_commitment,
        epoch,
        component: staging::Component::DataCatalog,
        index: 0,
        bytes,
    };
    assert!(
        http.exchange(
            seed.genesis(),
            seed.bearer(),
            put(initial.epoch + 1, package.catalogs[0].clone())
        )
        .await
        .is_err()
    );
    let mut changed = package.descriptor.clone();
    changed[0] ^= 1;
    assert!(
        http.exchange(
            seed.genesis(),
            seed.bearer(),
            Operation::Declare {
                descriptor: changed,
                budget: budget(&package)
            }
        )
        .await
        .is_err()
    );
    for bytes in [
        b"{\"invalid\":true}".to_vec(),
        vec![b' '; REQUEST_LIMIT + 1],
    ] {
        let response = http
            .http
            .post(http.endpoint.clone())
            .header(header::CONTENT_TYPE, "application/json")
            .bearer_auth(hex::encode(seed.bearer().expose()))
            .body(bytes)
            .send()
            .await
            .unwrap();
        assert!(matches!(
            response.status(),
            StatusCode::BAD_REQUEST | StatusCode::PAYLOAD_TOO_LARGE
        ));
        assert_eq!(
            response.text().await.unwrap(),
            "{\"error\":\"bootstrap-refused\"}"
        );
    }
    assert_eq!(
        before,
        serde_json::to_vec(
            &http
                .exchange(seed.genesis(), seed.bearer(), status())
                .await
                .unwrap()
        )
        .unwrap()
    );
    http.exchange(
        seed.genesis(),
        seed.bearer(),
        put(initial.epoch, package.catalogs[0].clone()),
    )
    .await
    .unwrap();
    let before = serde_json::to_vec(
        &http
            .exchange(seed.genesis(), seed.bearer(), status())
            .await
            .unwrap(),
    )
    .unwrap();
    let mut changed = package.catalogs[0].clone();
    changed[0] ^= 1;
    assert!(
        http.exchange(seed.genesis(), seed.bearer(), put(initial.epoch, changed))
            .await
            .is_err()
    );
    assert_eq!(
        before,
        serde_json::to_vec(
            &http
                .exchange(seed.genesis(), seed.bearer(), status())
                .await
                .unwrap()
        )
        .unwrap()
    );
    // Operator reclamation advances the real core epoch; a previously valid
    // HTTP request cannot be silently relabeled to the resumed epoch.
    server
        .reclaim_bootstrap_staging(
            &staging::Authentication {
                vault_id: seed.genesis().context().vault_id,
                genesis_commitment: seed.genesis().commitment(),
                bearer: seed.bearer(),
            },
            b.bootstrap_id,
            b.descriptor_commitment,
            initial.epoch,
            staging::Reclaim::Quarantine,
        )
        .await
        .unwrap();
    let fenced = serde_json::to_vec(
        &http
            .exchange(seed.genesis(), seed.bearer(), status())
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(
        http.exchange(
            seed.genesis(),
            seed.bearer(),
            put(initial.epoch, package.catalogs[0].clone())
        )
        .await
        .is_err()
    );
    assert_eq!(
        fenced,
        serde_json::to_vec(
            &http
                .exchange(seed.genesis(), seed.bearer(), status())
                .await
                .unwrap()
        )
        .unwrap()
    );
    assert!(http.resume(&store, &db).await.unwrap());
    assert!(!http.resume(&store, &db).await.unwrap());
    assert!(
        http.claim(
            seed.genesis(),
            ClaimAuthentication::SetupSecret(&Secret::new([7; 32]))
        )
        .await
        .is_err()
    );
    let Reply::Published(retried) = http
        .exchange(seed.genesis(), seed.bearer(), publish())
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(retried, signed.record().as_slice());
    let Reply::Published(canceled) = http
        .exchange(
            seed.genesis(),
            seed.bearer(),
            Operation::Cancel {
                bootstrap: b.bootstrap_id,
            },
        )
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(retried, canceled);
    assert!(
        http.exchange(seed.genesis(), &Secret::new([7; 32]), status())
            .await
            .is_err()
    );
    assert_eq!(
        http.http
            .post(http.endpoint.join("/sync").unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    task.abort();
}

struct ProcessServer {
    child: Child,
    origin: String,
}
impl Drop for ProcessServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

async fn process_server(root: &Path, fault: &str) -> ProcessServer {
    let ready = root.join("ready");
    let _ = std::fs::remove_file(&ready);
    let log = std::fs::File::create(root.join(format!("server-{fault}.log"))).unwrap();
    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "seed_bootstrap_http::tests::server_worker",
            "--nocapture",
        ])
        .env("AVEN_HTTP_TEST_ROOT", root)
        .env("AVEN_HTTP_TEST_FAULT", fault)
        .stdout(Stdio::from(log.try_clone().unwrap()))
        .stderr(Stdio::from(log))
        .spawn()
        .unwrap();
    let mut server = ProcessServer {
        child,
        origin: String::new(),
    };
    for _ in 0..500 {
        if let Ok(origin) = std::fs::read_to_string(&ready) {
            server.origin = origin;
            return server;
        }
        assert!(
            server.child.try_wait().unwrap().is_none(),
            "server exited before listening"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("server did not listen");
}

fn process_client(root: &Path, origin: &str, expected: &str) {
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "seed_bootstrap_http::tests::client_worker",
            "--nocapture",
        ])
        .env("AVEN_HTTP_TEST_ROOT", root)
        .env("AVEN_HTTP_TEST_ORIGIN", origin)
        .env("AVEN_HTTP_TEST_EXPECTED", expected)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn loopback_process_restart_recovers_exact_upload_and_remote_commit_before_adoption() {
    let root = tempfile::tempdir().unwrap();
    let (db, store, seed, package) = fixture(root.path()).await;
    let server = process_server(root.path(), "put").await;
    let http = Client::new(&server.origin).unwrap();
    http.claim(
        seed.genesis(),
        ClaimAuthentication::SetupSecret(&Secret::new([7; 32])),
    )
    .await
    .unwrap();
    drop(http);
    drop(store);
    drop(db);
    // Server exits after the first PUT commits, before writing its response.
    process_client(root.path(), &server.origin, "uncertain");
    drop(server);
    let db = Database::open(&root.path().join("client.sqlite"))
        .await
        .unwrap();
    let store = isolated_store(db.path(), &root.path().join("keys"));
    let intent = store.prepare_seed_adoption_intent(&db).await.unwrap();
    let exact_intent = intent.protected_storage_bytes().to_vec();
    let signed = intent.publication(seed.genesis()).unwrap();
    let b = signed.binding();
    assert!(
        db.cancel_local_shared_state_never_dispatched(&hex::encode(b.bootstrap_id))
            .await
            .is_err()
    );
    let (_, _, reloaded) = store.seed_http_inputs(&db).await.unwrap();
    assert!(reloaded.unwrap() == package);
    drop(store);
    drop(db);
    let server_db = Database::open(&root.path().join("server.sqlite"))
        .await
        .unwrap();
    let mut conn = aven_core::test_support::acquire(&server_db).await.unwrap();
    let staged: Vec<u8> = sqlx::query_scalar("SELECT bytes FROM server_bootstrap_chunks")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(staged, package.catalogs[0]);
    drop(conn);
    drop(server_db);
    // Both client and server are new processes. Already committed upload bytes
    // are retried exactly; server now exits after publication before response.
    let server = process_server(root.path(), "publish").await;
    process_client(root.path(), &server.origin, "uncertain");
    drop(server);
    let server_db = Database::open(&root.path().join("server.sqlite"))
        .await
        .unwrap();
    let mut conn = aven_core::test_support::acquire(&server_db).await.unwrap();
    let remote: Vec<u8> =
        sqlx::query_scalar("SELECT signed_record FROM server_bootstrap_publication")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(remote, signed.record().as_slice());
    let candidates: i64 = sqlx::query_scalar("SELECT count(*) FROM server_bootstrap_candidates")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(candidates, 1);
    drop(conn);
    drop(server_db);
    let db = Database::open(&root.path().join("client.sqlite"))
        .await
        .unwrap();
    let store = isolated_store(db.path(), &root.path().join("keys"));
    assert_eq!(
        store
            .prepare_seed_adoption_intent(&db)
            .await
            .unwrap()
            .protected_storage_bytes(),
        exact_intent
    );
    let before = db.export_data("fixed".into()).await.unwrap();
    assert!(before.tables.changes.iter().all(|c| c.server_seq.is_none()));
    let pending: Vec<_> = before
        .tables
        .changes
        .iter()
        .filter(|c| c.payload.contains("AFTER-CAPTURE"))
        .map(|c| c.change_id.clone())
        .collect();
    assert!(!pending.is_empty());
    drop(store);
    drop(db);
    let server = process_server(root.path(), "none").await;
    process_client(root.path(), &server.origin, "adopted");
    process_client(root.path(), &server.origin, "already-adopted");
    let db = Database::open(&root.path().join("client.sqlite"))
        .await
        .unwrap();
    let after = db.export_data("fixed".into()).await.unwrap();
    assert_eq!(
        serde_json::to_value(&before.tables.tasks).unwrap(),
        serde_json::to_value(&after.tables.tasks).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&before.tables.task_attachments).unwrap(),
        serde_json::to_value(&after.tables.task_attachments).unwrap()
    );
    assert!(
        after
            .tables
            .changes
            .iter()
            .filter(|c| pending.contains(&c.change_id))
            .all(|c| c.server_seq.is_none())
    );
    assert_eq!(
        after
            .tables
            .changes
            .iter()
            .filter(|c| c.server_seq.is_some())
            .count() as u64,
        b.prefix_count
    );
    let mut conn = aven_core::test_support::acquire(&db).await.unwrap();
    let (state, generation): (String, i64) =
        sqlx::query_as("SELECT state, association_generation FROM local_seed_publication_intent")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(state, "adopted");
    assert!(generation > 0);
    let pins: i64 = sqlx::query_scalar("SELECT count(*) FROM local_shared_capture_journal")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(pins, 0);
    drop(conn);
    let server_db = Database::open(&root.path().join("server.sqlite"))
        .await
        .unwrap();
    let mut conn = aven_core::test_support::acquire(&server_db).await.unwrap();
    let records: Vec<Vec<u8>> =
        sqlx::query_scalar("SELECT signed_record FROM server_bootstrap_publication")
            .fetch_all(&mut *conn)
            .await
            .unwrap();
    assert_eq!(records, vec![signed.record().to_vec()]);
    let high: i64 = sqlx::query_scalar("SELECT high_water FROM server_e2ee_allocator")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(high as u64, b.prefix_count);
    let image: Vec<u8> = sqlx::query_scalar("SELECT bytes FROM server_e2ee_image_chunks")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(image, package.images[0].records[0]);
    drop(conn);
    let http = Client::new(&server.origin).unwrap();
    let Reply::Published(record) = http
        .exchange(
            seed.genesis(),
            seed.bearer(),
            Operation::Publish {
                bootstrap: b.bootstrap_id,
                commitment: b.descriptor_commitment,
                epoch: 0,
                record: signed.record().to_vec(),
            },
        )
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(record, signed.record().as_slice());
}

#[tokio::test]
#[ignore = "subprocess worker invoked by loopback restart test"]
async fn server_worker() {
    let root = PathBuf::from(std::env::var_os("AVEN_HTTP_TEST_ROOT").unwrap());
    let fault = std::env::var("AVEN_HTTP_TEST_FAULT").unwrap();
    let database = Database::open(&root.join("server.sqlite")).await.unwrap();
    let app = router(database, Some(setup()), Default::default()).layer(axum::middleware::from_fn(
        move |request: Request, next: axum::middleware::Next| {
            let fault = fault.clone();
            async move {
                let (parts, body) = request.into_parts();
                let bytes = to_bytes(body, REQUEST_LIMIT).await.unwrap();
                let envelope: Envelope = serde_json::from_slice(&bytes).unwrap();
                let should_exit = matches!(
                    (&*fault, &envelope.operation),
                    ("put", Operation::Put { .. }) | ("publish", Operation::Publish { .. })
                );
                let response = next
                    .run(Request::from_parts(parts, axum::body::Body::from(bytes)))
                    .await;
                if should_exit && response.status() == StatusCode::OK {
                    std::process::exit(0);
                }
                response
            }
        },
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    std::fs::write(
        root.join("ready"),
        format!("http://{}", listener.local_addr().unwrap()),
    )
    .unwrap();
    axum::serve(listener, app).await.unwrap();
}

#[tokio::test]
#[ignore = "subprocess worker invoked by loopback restart test"]
async fn client_worker() {
    let root = PathBuf::from(std::env::var_os("AVEN_HTTP_TEST_ROOT").unwrap());
    let db = Database::open(&root.join("client.sqlite")).await.unwrap();
    let store = isolated_store(db.path(), &root.join("keys"));
    let result = Client::new(&std::env::var("AVEN_HTTP_TEST_ORIGIN").unwrap())
        .unwrap()
        .resume(&store, &db)
        .await;
    match std::env::var("AVEN_HTTP_TEST_EXPECTED").unwrap().as_str() {
        "uncertain" => assert!(result.is_err()),
        "adopted" => assert!(result.unwrap()),
        "already-adopted" => assert!(!result.unwrap()),
        _ => panic!(),
    }
}

#[tokio::test]
async fn invalid_http_outcome_preserves_sealed_intent_and_capture_until_verified_retry() {
    let root = tempfile::tempdir().unwrap();
    let (db, store, seed, _) = fixture(root.path()).await;
    let server = Database::open(&root.path().join("server.sqlite"))
        .await
        .unwrap();
    let app =
        router(server.clone(), Some(setup()), Default::default()).layer(axum::middleware::from_fn(
            |request: Request, next: axum::middleware::Next| async move {
                let response = next.run(request).await;
                let (parts, body) = response.into_parts();
                let bytes = to_bytes(body, RESPONSE_LIMIT).await.unwrap();
                let replacement = match serde_json::from_slice::<Reply>(&bytes) {
                    Ok(Reply::Published(mut record)) => {
                        *record.last_mut().unwrap() ^= 1;
                        serde_json::to_vec(&Reply::Published(record)).unwrap()
                    }
                    _ => bytes.to_vec(),
                };
                Response::from_parts(parts, axum::body::Body::from(replacement))
            },
        ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http = Client::new(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    http.claim(
        seed.genesis(),
        ClaimAuthentication::SetupSecret(&Secret::new([7; 32])),
    )
    .await
    .unwrap();
    assert!(http.resume(&store, &db).await.is_err());
    let mut conn = aven_core::test_support::acquire(&db).await.unwrap();
    let state: String = sqlx::query_scalar("SELECT state FROM local_seed_publication_intent")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(state, "sealed");
    let captured: i64 = sqlx::query_scalar("SELECT count(*) FROM local_shared_capture_journal")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(captured, 1);
    drop(conn);
    assert!(
        db.export_data("fixed".into())
            .await
            .unwrap()
            .tables
            .changes
            .iter()
            .all(|c| c.server_seq.is_none())
    );
    let intent = store
        .prepare_seed_adoption_intent(&db)
        .await
        .unwrap()
        .protected_storage_bytes()
        .to_vec();
    task.abort();
    let (http, task) = serve(server).await;
    assert!(http.resume(&store, &db).await.unwrap());
    assert_eq!(
        store
            .prepare_seed_adoption_intent(&db)
            .await
            .unwrap()
            .protected_storage_bytes(),
        intent
    );
    task.abort();
}

#[tokio::test]
async fn client_bounds_responses_and_rejects_redirects_and_unsafe_origins() {
    for origin in [
        "http://remote.example",
        "https://user:secret@example.com",
        "https://example.com/private",
        "https://example.com/?token=secret",
    ] {
        assert_eq!(
            Client::new(origin).err().unwrap().to_string(),
            "error bootstrap-origin"
        );
    }
    let root = tempfile::tempdir().unwrap();
    let (_, _, seed, _) = fixture(root.path()).await;
    for (status, body, message) in [
        (
            StatusCode::OK,
            vec![b' '; RESPONSE_LIMIT + 1],
            "error bootstrap-response-limit",
        ),
        (
            StatusCode::OK,
            b"PRIVATE-NOT-JSON".to_vec(),
            "error bootstrap-response",
        ),
        (
            StatusCode::TEMPORARY_REDIRECT,
            Vec::new(),
            "error bootstrap-refused outcome-unknown",
        ),
    ] {
        let app = Router::new().route(
            PATH,
            post(move || async move {
                (
                    status,
                    [
                        (header::CONTENT_TYPE, "application/json"),
                        (header::LOCATION, "http://127.0.0.1:1/secret"),
                    ],
                    body,
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let http = Client::new(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let error = http
            .claim(
                seed.genesis(),
                ClaimAuthentication::SetupSecret(&Secret::new([7; 32])),
            )
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), message);
        task.abort();
    }
}
