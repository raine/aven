use super::*;
use crate::{
    protected_local_keys::{EnrollmentReadiness, tests::isolated_store},
    seed_bootstrap_http::tests::{fixture, setup},
};
use aven_core::db::installation::InstallationGuard;
use std::path::Path;

async fn serve(db: Database) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let app = seed_bootstrap_http::router(db.clone(), Some(setup()), Default::default())
        .merge(router(db));
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (origin, task)
}
fn expiry() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3500
}
async fn adopted(
    root: &Path,
) -> (
    Database,
    ProtectedLocalKeyStore,
    Database,
    String,
    tokio::task::JoinHandle<()>,
) {
    let (db, store, seed, _) = fixture(root).await;
    let server = Database::open(&root.join("server.sqlite")).await.unwrap();
    let (origin, task) = serve(server.clone()).await;
    let transport = seed_bootstrap_http::Client::new(&origin).unwrap();
    transport
        .claim(
            seed.genesis(),
            aven_core::sync::seed_claim::ClaimAuthentication::SetupSecret(&Secret::new([7; 32])),
        )
        .await
        .unwrap();
    transport.resume(&store, &db).await.unwrap();
    (db, store, server, origin, task)
}

#[tokio::test]
async fn loopback_independent_peer_exact_reopen_and_current_authorization() {
    let root = tempfile::tempdir().unwrap();
    let (db, store, server, origin, task) = adopted(root.path()).await;
    let client = Client::new(&origin).unwrap();
    let invitation = client.invite(&store, &db, expiry()).await.unwrap();
    let peer_db = Database::open(&root.path().join("peer.sqlite"))
        .await
        .unwrap();
    let peer_store = isolated_store(peer_db.path(), &root.path().join("peer-keys"));
    assert_eq!(
        peer_store.enrollment_readiness(&peer_db).await.unwrap(),
        EnrollmentReadiness::NotSelected
    );
    client
        .request(&peer_store, &peer_db, Some(invitation))
        .await
        .unwrap();
    assert!(!client.complete(&peer_store, &peer_db).await.unwrap());
    for (keys, database) in [(&store, &db), (&peer_store, &peer_db)] {
        let error = keys.tail_inputs(database, &origin).await.err().unwrap();
        assert_eq!(error.to_string(), "error enrollment-unresolved");
    }
    let peer = peer_store
        .prepare_peer(&peer_db, &origin, None)
        .await
        .unwrap();
    let exact = peer.protected_storage_bytes();
    let (seed, p, d) = store.prepare_invitation(&db, &origin, None).await.unwrap();
    assert_ne!(peer.device(), seed.genesis().device_id());
    assert_ne!(peer.bearer().expose(), seed.bearer().expose());
    let mail = server
        .peer_mailbox(peer.vault(), peer.handle())
        .await
        .unwrap();
    let (_, _, candidate) = store
        .prepare_admission(&db, &origin, mail.request.as_ref().unwrap())
        .await
        .unwrap();
    assert_eq!(
        store.enrollment_readiness(&db).await.unwrap(),
        EnrollmentReadiness::UnresolvedDisclosure
    );
    assert!(
        store
            .enrollment_readiness(&db)
            .await
            .unwrap()
            .require_resolved_disclosure()
            .is_err()
    );
    let error = store.tail_inputs(&db, &origin).await.err().unwrap();
    assert_eq!(error.to_string(), "error withdrawal-required-unsupported");
    // The unresolved-disclosure fence precedes ordinary locator validation.
    let error = store
        .tail_inputs(&db, "https://other.invalid")
        .await
        .err()
        .unwrap();
    assert_eq!(error.to_string(), "error withdrawal-required-unsupported");
    // Real server commit with a deliberately unconsumed success result models a
    // lost reply. Restarted host resends the already protected candidate.
    let context = Context {
        vault: peer.vault(),
        genesis: seed.genesis().commitment(),
        device: seed.genesis().device_id(),
        credential_version: 1,
        head: p.commitment(),
    };
    client
        .exchange(
            Operation::Admit {
                context: context.clone(),
                record: candidate.record().to_vec(),
            },
            Some(seed.bearer()),
        )
        .await
        .unwrap();
    let wrong = Secret::new([6; 32]);
    assert!(
        client
            .exchange(
                Operation::Admit {
                    context: context.clone(),
                    record: candidate.record().to_vec()
                },
                Some(&wrong)
            )
            .await
            .is_err()
    );
    assert!(
        client
            .exchange(Operation::Descriptor { context }, Some(seed.bearer()))
            .await
            .is_err()
    );
    task.abort();
    let _ = task.await;
    let reopened_server = Database::open(server.path()).await.unwrap();
    let listener = tokio::net::TcpListener::bind(origin.strip_prefix("http://").unwrap())
        .await
        .unwrap();
    let app =
        seed_bootstrap_http::router(reopened_server.clone(), Some(setup()), Default::default())
            .merge(router(reopened_server));
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    drop(store);
    drop(peer_store);
    drop(db);
    drop(peer_db);
    let db = Database::open(&root.path().join("client.sqlite"))
        .await
        .unwrap();
    let store = isolated_store(db.path(), &root.path().join("keys"));
    let peer_db = Database::open(&root.path().join("peer.sqlite"))
        .await
        .unwrap();
    let peer_store = isolated_store(peer_db.path(), &root.path().join("peer-keys"));
    assert!(client.admit(&store, &db).await.unwrap());
    assert!(client.complete(&peer_store, &peer_db).await.unwrap());
    assert_eq!(
        peer_store
            .prepare_peer(&peer_db, &origin, None)
            .await
            .unwrap()
            .protected_storage_bytes()
            .as_slice(),
        exact.as_slice()
    );
    assert!(client.invite(&store, &db, expiry()).await.is_err());
    assert!(
        peer_store
            .prepare_seed_claim(&peer_db, [9; 32])
            .await
            .is_err()
    );
    assert!(peer_store.load_or_create().is_err());
    let head = candidate.commitment();
    assert_eq!(
        store.enrollment_readiness(&db).await.unwrap(),
        EnrollmentReadiness::Enrolled { head }
    );
    assert_eq!(
        peer_store.enrollment_readiness(&peer_db).await.unwrap(),
        EnrollmentReadiness::Enrolled { head }
    );
    assert!(client.complete(&peer_store, &peer_db).await.unwrap());
    assert!(client.admit(&store, &db).await.unwrap());
    // This slice deliberately installs no captured task rows.
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", peer_db.path().display()))
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM tasks")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    let old_client: String = sqlx::query_scalar("SELECT value FROM meta WHERE key='client_id'")
        .fetch_one(&pool)
        .await
        .unwrap();
    client.request(&peer_store, &peer_db, None).await.unwrap();
    let new_client: String = sqlx::query_scalar("SELECT value FROM meta WHERE key='client_id'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(old_client, new_client);
    let workspace = peer_db.list_workspaces().await.unwrap().remove(0);
    peer_db
        .create_task(
            &workspace,
            aven_core::operations::TaskDraft {
                title: "LATER LOCAL EDIT".into(),
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
        .unwrap();
    client.complete(&peer_store, &peer_db).await.unwrap();
    let title: String = sqlx::query_scalar("SELECT title FROM tasks")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(title, "LATER LOCAL EDIT");
    let exported =
        serde_json::to_value(peer_db.export_data("fixed".into()).await.unwrap()).unwrap();
    assert!(
        exported["tables"]["meta"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["key"] == "e2ee_data_only")
    );
    assert!(
        !serde_json::to_string(&exported)
            .unwrap()
            .contains(&hex::encode(peer.bearer().expose()))
    );
    assert!(
        peer_db
            .prepare_client_sync_page("http://localhost:9999".into(), 1, 1)
            .await
            .is_err()
    );
    assert!(
        aven_core::db::backup_database(
            peer_db.path(),
            &root.path().join("forbidden-backup.sqlite")
        )
        .await
        .is_err()
    );
    let server_pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", server.path().display()))
        .await
        .unwrap();
    // Unsupported-head injection is not a simulated implementation of removal.
    sqlx::query("UPDATE server_e2ee_membership_head SET sequence=3")
        .execute(&server_pool)
        .await
        .unwrap();
    assert!(client.complete(&peer_store, &peer_db).await.is_err());
    assert!(client.admit(&store, &db).await.is_err());
    let saved: Vec<u8> = sqlx::query_scalar("SELECT declaration FROM server_peer_invitation")
        .fetch_one(&server_pool)
        .await
        .unwrap();
    assert_eq!(saved, d.record());
    task.abort();
}

#[tokio::test]
async fn occupied_target_preflight_does_not_fence_plaintext_or_erase_data() {
    let root = tempfile::tempdir().unwrap();
    let (db, store, _, origin, task) = adopted(root.path()).await;
    let client = Client::new(&origin).unwrap();
    let invitation = client.invite(&store, &db, expiry()).await.unwrap();
    let occupied = Database::open(&root.path().join("occupied.sqlite"))
        .await
        .unwrap();
    let workspace = occupied.list_workspaces().await.unwrap().remove(0);
    occupied
        .create_task(
            &workspace,
            aven_core::operations::TaskDraft {
                title: "KEEP LOCAL DATA".into(),
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
        .unwrap();
    let target = isolated_store(occupied.path(), &root.path().join("occupied-keys"));
    assert!(
        client
            .request(&target, &occupied, Some(invitation))
            .await
            .is_err()
    );
    InstallationGuard::acquire(occupied.path())
        .unwrap()
        .ensure_unbound()
        .unwrap();
    assert!(occupied.enrollment_pin().await.unwrap().is_none());
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", occupied.path().display()))
        .await
        .unwrap();
    let title: String = sqlx::query_scalar("SELECT title FROM tasks")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(title, "KEEP LOCAL DATA");
    task.abort();
}

#[tokio::test]
async fn tampered_grant_and_protected_loss_cannot_complete_or_regenerate() {
    let root = tempfile::tempdir().unwrap();
    let (db, store, server, origin, task) = adopted(root.path()).await;
    let client = Client::new(&origin).unwrap();
    let invitation = client.invite(&store, &db, expiry()).await.unwrap();
    let target = Database::open(&root.path().join("peer.sqlite"))
        .await
        .unwrap();
    let keys = root.path().join("peer-keys");
    let peer_store = isolated_store(target.path(), &keys);
    client
        .request(&peer_store, &target, Some(invitation))
        .await
        .unwrap();
    client.admit(&store, &db).await.unwrap();
    let peer = peer_store
        .prepare_peer(&target, &origin, None)
        .await
        .unwrap();
    let evidence = server
        .peer_mailbox(peer.vault(), peer.handle())
        .await
        .unwrap()
        .evidence
        .unwrap();
    let mut bad = evidence.clone();
    bad.admission[1200] ^= 1;
    assert!(peer_store.pin_peer_response(&target, &bad).await.is_err());
    assert_eq!(
        peer_store.enrollment_readiness(&target).await.unwrap(),
        EnrollmentReadiness::Pending
    );
    client.complete(&peer_store, &target).await.unwrap();
    let before = peer.protected_storage_bytes();
    let identity = std::fs::read_dir(&keys)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .ends_with(".peer-identity")
        })
        .unwrap();
    let saved = std::fs::read(&identity).unwrap();
    std::fs::remove_file(&identity).unwrap();
    assert!(client.request(&peer_store, &target, None).await.is_err());
    assert!(!identity.exists());
    std::fs::write(&identity, saved).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&identity, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    assert_eq!(
        peer_store
            .prepare_peer(&target, &origin, None)
            .await
            .unwrap()
            .protected_storage_bytes()
            .as_slice(),
        before.as_slice()
    );
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", target.path().display()))
        .await
        .unwrap();
    sqlx::query("DELETE FROM local_peer_enrollment_artifacts")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM local_peer_enrollment")
        .execute(&pool)
        .await
        .unwrap();
    assert!(client.request(&peer_store, &target, None).await.is_err());
    task.abort();
}

#[tokio::test]
async fn bounded_http_redacts_refusals_and_rejects_unsafe_origins() {
    for origin in [
        "http://example.com",
        "https://user:secret@example.com",
        "https://example.com/private",
        "https://example.com/?token=secret",
    ] {
        assert!(Client::new(origin).is_err());
    }
    let root = tempfile::tempdir().unwrap();
    let db = Database::open(&root.path().join("server.sqlite"))
        .await
        .unwrap();
    let (origin, task) = serve(db).await;
    let http = reqwest::Client::new();
    for body in [
        "PRIVATE-INVALID-CONTENT".to_string(),
        "x".repeat(CONTROL_LIMIT + 1),
    ] {
        let response = http
            .post(format!("{origin}{PATH}"))
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, "Bearer PRIVATE-TOKEN")
            .body(body)
            .send()
            .await
            .unwrap();
        assert!(!response.status().is_success());
        let text = response.text().await.unwrap();
        assert_eq!(text, "enrollment-refused");
    }
    task.abort();
}

async fn crash_worker(root: &Path, origin: &str, role: &str, kind: &str) {
    let output = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "peer_enrollment_http::tests::process_worker",
            "--ignored",
            "--nocapture",
        ])
        .env("AVEN_PEER_ROOT", root)
        .env("AVEN_PEER_ORIGIN", origin)
        .env("AVEN_PEER_ROLE", role)
        .env("AVEN_PEER_CRASH_KIND", kind)
        .output()
        .await
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(79),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn process_exit_at_protected_dispatch_and_completion_boundaries() {
    let root = tempfile::tempdir().unwrap();
    let (db, store, _, origin, task) = adopted(root.path()).await;
    let client = Client::new(&origin).unwrap();
    let invitation = client.invite(&store, &db, expiry()).await.unwrap();
    let peer_db = Database::open(&root.path().join("peer.sqlite"))
        .await
        .unwrap();
    let peer_store = isolated_store(peer_db.path(), &root.path().join("peer-keys"));
    client
        .request(&peer_store, &peer_db, Some(invitation))
        .await
        .unwrap();
    for kind in ["peer-bound", "peer-candidate", "peer-sent"] {
        crash_worker(root.path(), &origin, "inviter", kind).await;
    }
    assert_eq!(
        store.enrollment_readiness(&db).await.unwrap(),
        EnrollmentReadiness::UnresolvedDisclosure
    );
    crash_worker(root.path(), &origin, "inviter", "peer-ready").await;
    assert!(client.admit(&store, &db).await.unwrap());
    for kind in ["peer-response", "peer-verified", "peer-ready"] {
        crash_worker(root.path(), &origin, "peer", kind).await;
    }
    assert!(client.complete(&peer_store, &peer_db).await.unwrap());
    assert!(matches!(
        peer_store.enrollment_readiness(&peer_db).await.unwrap(),
        EnrollmentReadiness::Enrolled { .. }
    ));
    task.abort();
}

#[tokio::test]
#[ignore = "subprocess boundary worker"]
async fn process_worker() {
    let root = std::path::PathBuf::from(std::env::var_os("AVEN_PEER_ROOT").unwrap());
    let origin = std::env::var("AVEN_PEER_ORIGIN").unwrap();
    let role = std::env::var("AVEN_PEER_ROLE").unwrap();
    let (name, keys) = if role == "inviter" {
        ("client.sqlite", "keys")
    } else {
        ("peer.sqlite", "peer-keys")
    };
    let db = Database::open(&root.join(name)).await.unwrap();
    let store = isolated_store(db.path(), &root.join(keys));
    let client = Client::new(&origin).unwrap();
    if role == "inviter" {
        client.admit(&store, &db).await.unwrap();
    } else if role == "initial" {
        let bytes = zeroize::Zeroizing::new(std::fs::read(root.join("invitation.test")).unwrap());
        let invitation = Invitation::from_protected_storage(&bytes).unwrap();
        client.request(&store, &db, Some(invitation)).await.unwrap();
    } else {
        client.complete(&store, &db).await.unwrap();
    }
    panic!("expected crash boundary");
}

#[tokio::test]
async fn edit_after_protected_identity_before_pin_survives_refused_completion() {
    let root = tempfile::tempdir().unwrap();
    let (db, store, server, origin, task) = adopted(root.path()).await;
    let client = Client::new(&origin).unwrap();
    let invitation = client.invite(&store, &db, expiry()).await.unwrap();
    let vault = invitation.vault();
    let handle = invitation.handle();
    {
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(root.path().join("invitation.test")).unwrap();
        file.write_all(&invitation.protected_storage_bytes())
            .unwrap();
    }
    let target = Database::open(&root.path().join("peer.sqlite"))
        .await
        .unwrap();
    crash_worker(root.path(), &origin, "initial", "peer-identity").await;
    assert!(target.enrollment_pin().await.unwrap().is_none());
    let workspace = target.list_workspaces().await.unwrap().remove(0);
    target
        .create_task(
            &workspace,
            aven_core::operations::TaskDraft {
                title: "CONCURRENT LOCAL EDIT".into(),
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
        .unwrap();
    let keys = root.path().join("peer-keys");
    let peer_store = isolated_store(target.path(), &keys);
    assert!(client.request(&peer_store, &target, None).await.is_err());
    assert!(
        server
            .peer_mailbox(vault, handle)
            .await
            .unwrap()
            .request
            .is_none()
    );
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", target.path().display()))
        .await
        .unwrap();
    let title: String = sqlx::query_scalar("SELECT title FROM tasks")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(title, "CONCURRENT LOCAL EDIT");
    assert!(
        InstallationGuard::acquire(target.path())
            .unwrap()
            .ensure_unbound()
            .is_err()
    );
    task.abort();
}

mod install;

#[tokio::test]
async fn control_exchange_retains_small_response_cap() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = Client::new(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let body = serde_json::to_vec(&Reply::Mailbox(Mailbox {
        request: Some(vec![0; CONTROL_LIMIT]),
        evidence: None,
    }))
    .unwrap();
    assert!(body.len() > CONTROL_LIMIT && body.len() < PUBLISHED_RESPONSE_LIMIT);
    let app = Router::new().route(
        PATH,
        post(move || {
            let body = body.clone();
            async move { ([(header::CONTENT_TYPE, "application/json")], body) }
        }),
    );
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let error = client
        .exchange(
            Operation::Mailbox {
                vault: [0; 32],
                handle: [0; 32],
            },
            None,
        )
        .await
        .err()
        .unwrap();
    assert_eq!(error.to_string(), "error enrollment-limit");
    task.abort();
}
