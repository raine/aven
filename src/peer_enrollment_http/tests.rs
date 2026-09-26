use sha2::Digest;

use super::*;
use crate::{
    protected_local_keys::{
        EnrollmentReadiness,
        peer::OutboundInvitation,
        tests::{BACKEND_LOADS, isolated_store},
    },
    test_support::e2ee_http::{self, fixture},
};
use aven_core::db::installation::InstallationGuard;
use aven_core::sync::seed_claim::{membership::Mailbox, peer};
use axum::{
    body::{Body, to_bytes},
    extract::Request,
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use std::collections::VecDeque;
use std::path::Path;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, AtomicUsize, Ordering},
};
use tokio::sync::Notify;

#[derive(Default)]
struct ExchangeCounts {
    total: AtomicUsize,
    membership: AtomicUsize,
    published: AtomicUsize,
    tracked_device: Mutex<Option<[u8; 32]>>,
    tracked_membership: AtomicUsize,
    tracked_published: AtomicUsize,
    stale_gates: Mutex<VecDeque<Arc<StaleGate>>>,
    /// Mailbox reads per handle.
    mailboxes: Mutex<std::collections::HashMap<[u8; 32], usize>>,
    /// Remaining mailbox reads per handle answered as busy.
    busy_mailboxes: Mutex<std::collections::HashMap<[u8; 32], usize>>,
}

struct StaleGate {
    started: Notify,
    finished: Notify,
}

impl ExchangeCounts {
    fn reset(&self) {
        self.total.store(0, Ordering::Relaxed);
        self.membership.store(0, Ordering::Relaxed);
        self.published.store(0, Ordering::Relaxed);
        self.tracked_membership.store(0, Ordering::Relaxed);
        self.tracked_published.store(0, Ordering::Relaxed);
        self.stale_gates.lock().unwrap().clear();
    }

    fn track(&self, device: [u8; 32]) {
        *self.tracked_device.lock().unwrap() = Some(device);
    }

    fn add_stale_gate(&self) -> Arc<StaleGate> {
        let gate = Arc::new(StaleGate {
            started: Notify::new(),
            finished: Notify::new(),
        });
        self.stale_gates.lock().unwrap().push_back(gate.clone());
        gate
    }
}

async fn count_exchange(
    axum::extract::State(counts): axum::extract::State<Arc<ExchangeCounts>>,
    request: Request,
    next: Next,
) -> Response {
    let (parts, body) = request.into_parts();
    let bytes = to_bytes(body, PUBLISHED_RESPONSE_LIMIT)
        .await
        .unwrap_or_default();
    counts.total.fetch_add(1, Ordering::Relaxed);
    let operation = serde_json::from_slice::<Operation>(&bytes).ok();
    let device = match operation.as_ref() {
        Some(Operation::Membership { context }) | Some(Operation::Published { context, .. }) => {
            Some(context.device)
        }
        _ => None,
    };
    if matches!(&operation, Some(Operation::Membership { .. })) {
        counts.membership.fetch_add(1, Ordering::Relaxed);
    }
    if matches!(&operation, Some(Operation::Published { .. })) {
        counts.published.fetch_add(1, Ordering::Relaxed);
    }
    if device == *counts.tracked_device.lock().unwrap() {
        match operation.as_ref() {
            Some(Operation::Membership { .. }) => {
                counts.tracked_membership.fetch_add(1, Ordering::Relaxed);
            }
            Some(Operation::Published { .. }) => {
                counts.tracked_published.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
    }
    if let Some(Operation::Mailbox { handle, .. }) = &operation {
        *counts.mailboxes.lock().unwrap().entry(*handle).or_default() += 1;
        if let Some(remaining) = counts.busy_mailboxes.lock().unwrap().get_mut(handle)
            && *remaining > 0
        {
            *remaining -= 1;
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                [
                    (header::RETRY_AFTER, "0"),
                    (header::CONTENT_TYPE, "application/json"),
                ],
                r#"{"error":"enrollment-busy"}"#,
            )
                .into_response();
        }
    }
    let stale_gate = if matches!(&operation, Some(Operation::Published { .. })) {
        counts.stale_gates.lock().unwrap().pop_front()
    } else {
        None
    };
    if let Some(stale_gate) = stale_gate {
        stale_gate.started.notify_one();
        stale_gate.finished.notified().await;
    }
    next.run(Request::from_parts(parts, Body::from(bytes)))
        .await
}

async fn serve_counted(
    db: Database,
    counts: Arc<ExchangeCounts>,
) -> (String, tokio::task::JoinHandle<()>) {
    let app = e2ee_http::router(db).await;
    serve_counted_app(app, counts).await
}
async fn serve_counted_with_clock(
    db: Database,
    counts: Arc<ExchangeCounts>,
    clock: Arc<AtomicU64>,
) -> (String, tokio::task::JoinHandle<()>) {
    e2ee_http::issue_setup(&db).await;
    let app = seed_bootstrap_http::router(db.clone(), Default::default())
        .merge(router_with_clock(db.clone(), clock))
        .merge(crate::encrypted_tail_http::router(db));
    serve_counted_app(app, counts).await
}
async fn serve_counted_app(
    app: Router,
    counts: Arc<ExchangeCounts>,
) -> (String, tokio::task::JoinHandle<()>) {
    let app = app.layer(middleware::from_fn_with_state(counts, count_exchange));
    e2ee_http::serve(app, "127.0.0.1:0").await
}
fn test_clock() -> Arc<AtomicU64> {
    Arc::new(AtomicU64::new(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    ))
}
fn advance_clock(clock: &AtomicU64, to: u64) {
    clock.store(to, Ordering::SeqCst);
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
    adopted_with(root, None).await
}
async fn adopted_with(
    root: &Path,
    counts: Option<Arc<ExchangeCounts>>,
) -> (
    Database,
    ProtectedLocalKeyStore,
    Database,
    String,
    tokio::task::JoinHandle<()>,
) {
    adopted_inner(root, counts, None).await
}
async fn adopted_with_clock(
    root: &Path,
    counts: Option<Arc<ExchangeCounts>>,
    clock: Arc<AtomicU64>,
) -> (
    Database,
    ProtectedLocalKeyStore,
    Database,
    String,
    tokio::task::JoinHandle<()>,
) {
    adopted_inner(root, counts, Some(clock)).await
}
async fn adopted_inner(
    root: &Path,
    counts: Option<Arc<ExchangeCounts>>,
    clock: Option<Arc<AtomicU64>>,
) -> (
    Database,
    ProtectedLocalKeyStore,
    Database,
    String,
    tokio::task::JoinHandle<()>,
) {
    let (db, mut store, seed, _) = fixture(root).await;
    if let Some(clock) = &clock {
        store.set_enrollment_clock(clock.clone());
    }
    let server = Database::open(&root.join("server.sqlite")).await.unwrap();
    let counts = counts.unwrap_or_default();
    let (origin, task) = match clock {
        Some(clock) => serve_counted_with_clock(server.clone(), counts, clock).await,
        None => serve_counted(server.clone(), counts).await,
    };
    e2ee_http::adopt(&origin, &db, &store, &seed).await;
    (db, store, server, origin, task)
}

#[tokio::test]
async fn journal_scan_loads_bound_and_ready_once() {
    let root = tempfile::tempdir().unwrap();
    let (db, store, _, origin, task) = adopted(root.path()).await;
    let client = Client::new(&origin).unwrap();
    client.invite(&store, &db, expiry()).await.unwrap();

    let (invitation, loads) = BACKEND_LOADS.measure(store.outbound_invitation(&db)).await;
    assert_eq!(invitation.unwrap(), Some(OutboundInvitation::Pending));
    // The store lock lists existing items once, so absent slots and phases
    // cost no loads; only existing items are loaded.
    assert!(loads <= 4, "{loads} loads");
    task.abort();
}

#[tokio::test]
async fn invitation_loads_scale_with_existing_items() {
    let root = tempfile::tempdir().unwrap();
    let (db, store, _, origin, task) = adopted(root.path()).await;
    let client = Client::new(&origin).unwrap();
    let mut loads = Vec::new();
    for _ in 0..3 {
        let (invitation, count) = BACKEND_LOADS
            .measure(client.invite(&store, &db, expiry()))
            .await;
        invitation.unwrap();
        loads.push(count);
    }
    // Loads follow existing items, not slot capacity (257 membership floors,
    // 128 outbound journals).
    assert!(loads[0] <= 32, "{loads:?}");
    assert!(loads[2] <= loads[1], "{loads:?}");
    task.abort();
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
    let peer_store = isolated_store(&peer_db, &root.path().join("peer-keys")).await;
    assert_eq!(
        peer_store.enrollment_readiness(&peer_db).await.unwrap(),
        EnrollmentReadiness::NotSelected
    );
    client
        .request(&peer_store, &peer_db, Some(invitation))
        .await
        .unwrap();
    assert!(!client.complete(&peer_store, &peer_db).await.unwrap());
    // An open invitation never blocks the inviter; the joiner is not enrolled.
    let tail = store.tail_inputs(&db, &origin).await.unwrap();
    assert!(!tail.publishing_blocked().unwrap());
    drop(tail);
    let error = peer_store
        .tail_inputs(&peer_db, &origin)
        .await
        .err()
        .unwrap();
    assert_eq!(error.to_string(), "error enrollment-unresolved");
    let peer = peer_store
        .prepare_peer(&peer_db, &origin, None)
        .await
        .unwrap();
    let exact = peer.protected_storage_bytes();
    let inputs = store.active_inputs(&db, &origin).await.unwrap();
    let d = store
        .prepare_invitation(&db, &inputs, None, None)
        .await
        .unwrap();
    let context = Context::active(&inputs);
    let bearer = Secret::new(*inputs.bearer().expose());
    assert_ne!(peer.device(), inputs.device());
    assert_ne!(peer.bearer().expose(), inputs.bearer().expose());
    let mail = server
        .membership_mailbox(peer.vault(), peer.handle())
        .await
        .unwrap();
    let candidate = store
        .prepare_admission(&db, &inputs, &d, mail.request.as_ref().unwrap())
        .await
        .unwrap();
    drop(inputs);
    assert_eq!(
        store.outbound_invitation(&db).await.unwrap(),
        Some(OutboundInvitation::Disclosed)
    );
    assert!(matches!(
        store.enrollment_readiness(&db).await.unwrap(),
        EnrollmentReadiness::Enrolled { .. }
    ));
    // Before expiry a possible disclosure blocks nothing.
    let tail = store.tail_inputs(&db, &origin).await.unwrap();
    assert!(!tail.publishing_blocked().unwrap());
    tail.require_publishing_ready().unwrap();
    drop(tail);
    let error = store
        .tail_inputs(&db, "https://other.invalid")
        .await
        .err()
        .unwrap();
    assert_eq!(error.to_string(), "error enrollment-context");
    // Real server commit with a deliberately unconsumed success result models a
    // lost reply. Restarted host resends the already protected candidate.
    client
        .exchange(
            Operation::Admit {
                context: context.clone(),
                handle: d.handle,
                record: candidate.clone(),
            },
            Some(&bearer),
        )
        .await
        .unwrap();
    let wrong = Secret::new([6; 32]);
    assert!(
        client
            .exchange(
                Operation::Admit {
                    context: context.clone(),
                    handle: d.handle,
                    record: candidate.clone()
                },
                Some(&wrong)
            )
            .await
            .is_err()
    );
    assert!(
        client
            .exchange(
                Operation::Published {
                    context,
                    descriptor: [0; 32],
                    component: None,
                    index: 0
                },
                Some(&bearer)
            )
            .await
            .is_err()
    );
    task.abort();
    let _ = task.await;
    let reopened_server = Database::open(server.path()).await.unwrap();
    let listener = tokio::net::TcpListener::bind(origin.strip_prefix("http://").unwrap())
        .await
        .unwrap();
    let app = seed_bootstrap_http::router(reopened_server.clone(), Default::default())
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
    let store = isolated_store(&db, &root.path().join("keys")).await;
    let peer_db = Database::open(&root.path().join("peer.sqlite"))
        .await
        .unwrap();
    let peer_store = isolated_store(&peer_db, &root.path().join("peer-keys")).await;
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

    assert!(
        peer_store
            .prepare_seed_claim(&peer_db, [9; 32])
            .await
            .is_err()
    );
    assert!(peer_store.load_or_create().is_err());
    let head = sha2::Sha256::digest(&candidate).into();
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
    // Completing enrollment leaves the peer with no task rows.
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
    let backup_path = root.path().join("detached-peer-backup.sqlite");
    aven_core::db::backup_database(peer_db.path(), &backup_path)
        .await
        .unwrap();
    let backup_pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", backup_path.display()))
        .await
        .unwrap();
    let enrolled_peers: i64 = sqlx::query_scalar("SELECT count(*) FROM local_peer_enrollment")
        .fetch_one(&backup_pool)
        .await
        .unwrap();
    assert_eq!(enrolled_peers, 0);
    let preserved_tasks: i64 =
        sqlx::query_scalar("SELECT count(*) FROM tasks WHERE title='LATER LOCAL EDIT'")
            .fetch_one(&backup_pool)
            .await
            .unwrap();
    assert_eq!(preserved_tasks, 1);
    backup_pool.close().await;
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
    let saved: Vec<u8> =
        sqlx::query_scalar("SELECT declaration FROM server_membership_invitations")
            .fetch_one(&server_pool)
            .await
            .unwrap();
    assert_eq!(saved, d.declaration);
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
    let target = isolated_store(&occupied, &root.path().join("occupied-keys")).await;
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
    let peer_store = isolated_store(&target, &keys).await;
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
        .membership_mailbox(peer.vault(), peer.handle())
        .await
        .unwrap();
    let mut bad = evidence.clone();
    bad.admission.as_mut().unwrap()[1200] ^= 1;
    assert!(peer_store.open_peer_response(&target, &bad).await.is_err());
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
    let (origin, task) = e2ee_http::serve(e2ee_http::router(db).await, "127.0.0.1:0").await;
    let http = reqwest::Client::new();
    for (body, status, code) in [
        (
            "PRIVATE-INVALID-CONTENT".to_string(),
            StatusCode::UNAUTHORIZED,
            "enrollment-credential",
        ),
        (
            "x".repeat(CONTROL_LIMIT + 1),
            StatusCode::PAYLOAD_TOO_LARGE,
            "enrollment-limit",
        ),
    ] {
        let response = http
            .post(format!("{origin}{PATH}"))
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, "Bearer PRIVATE-TOKEN")
            .body(body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        let text = response.text().await.unwrap();
        assert_eq!(text, format!(r#"{{"error":"{code}"}}"#));
    }
    task.abort();
}

async fn crash_worker(root: &Path, origin: &str, role: &str, kind: &str) {
    let output = e2ee_http::worker("peer_enrollment_http::tests::process_worker")
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
    let peer_store = isolated_store(&peer_db, &root.path().join("peer-keys")).await;
    client
        .request(&peer_store, &peer_db, Some(invitation))
        .await
        .unwrap();
    for kind in ["peer-bound", "peer-candidate", "peer-sent"] {
        crash_worker(root.path(), &origin, "inviter", kind).await;
    }
    assert_eq!(
        store.outbound_invitation(&db).await.unwrap(),
        Some(OutboundInvitation::Disclosed)
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
    let store = isolated_store(&db, &root.join(keys)).await;
    let client = Client::new(&origin).unwrap();
    if role == "inviter" {
        client.admit(&store, &db).await.unwrap();
    } else if role == "replace" {
        let bytes = zeroize::Zeroizing::new(std::fs::read(root.join("replacement.test")).unwrap());
        let invitation = Invitation::from_protected_storage(&bytes).unwrap();
        client.replace(&store, &db, invitation).await.unwrap();
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
    let peer_store = isolated_store(&target, &keys).await;
    assert!(client.request(&peer_store, &target, None).await.is_err());
    assert!(
        server
            .membership_mailbox(vault, handle)
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

#[test]
fn maximal_published_chunk_fits_response_limit() {
    let reply = Reply::Published(vec![
        255;
        aven_core::sync::bootstrap_staging::MAX_REQUEST_BYTES
    ]);
    assert!(serde_json::to_vec(&reply).unwrap().len() <= PUBLISHED_RESPONSE_LIMIT);
}

#[tokio::test]
async fn control_exchange_retains_small_response_cap() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = Client::new(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let body = serde_json::to_vec(&Reply::Mailbox(Mailbox {
        request: Some(vec![0; CONTROL_LIMIT]),
        declaration: vec![],
        admission: None,
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

#[tokio::test]
async fn enrollment_permit_timeout_is_retryable_and_uncacheable() {
    let root = tempfile::tempdir().unwrap();
    let db = Database::open(&root.path().join("server.sqlite"))
        .await
        .unwrap();
    tokio::time::pause();
    let server = Arc::new(Server {
        db,
        gate: crate::http_admission::Admission::new(1),
        enrollment_clock: None,
    });
    let permit = server.gate.hold_operation().await;
    let request = Request::builder()
        .method("POST")
        .uri(PATH)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{}"))
        .unwrap();
    let task = tokio::spawn(handle(State(server.clone()), request));
    tokio::task::yield_now().await;
    tokio::time::advance(REQUEST_TIMEOUT).await;
    let response = task.await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(response.headers()[header::RETRY_AFTER], "1");
    assert_eq!(
        to_bytes(response.into_body(), 256).await.unwrap(),
        r#"{"error":"enrollment-busy"}"#
    );
    assert_eq!(server.gate.available_operations(), 0);
    drop(permit);
}

#[tokio::test]
async fn management_loopback_authenticates_removed_seed_before_stale_hint() {
    let root = tempfile::tempdir().unwrap();
    let (db, store, server, origin, task) = adopted(root.path()).await;
    let seed = store.prepare_seed_claim(&db, [9; 32]).await.unwrap();
    let inputs = store.active_inputs(&db, &origin).await.unwrap();
    let m = inputs.membership.clone();
    drop(inputs);
    let keys = m
        .verify_initial_key(store.load_required().unwrap().package_key())
        .unwrap();
    let device = membership::Device::seed(&seed);
    let (inv, d) = device.prepare_invitation(&m, expiry()).unwrap();
    let joiner = membership::Joiner::generate(
        Invitation::from_protected_storage(&inv.protected_storage_bytes()).unwrap(),
    )
    .unwrap();
    let record = device
        .prepare_admission(&m, &d, &inv, joiner.request(), &keys)
        .unwrap();
    let mut context = Context {
        vault: m.genesis().context().vault_id,
        genesis: m.genesis().commitment(),
        device: seed.genesis().device_id(),
        head: m.head(),
    };
    let client = Client::new(&origin).unwrap();
    client
        .exchange(
            Operation::Register {
                context: context.clone(),
                declaration: d.record().to_vec(),
            },
            Some(seed.bearer()),
        )
        .await
        .unwrap();
    client
        .exchange(
            Operation::Post {
                vault: context.vault,
                handle: d.handle(),
                request: joiner.request().to_vec(),
            },
            None,
        )
        .await
        .unwrap();
    client
        .exchange(
            Operation::Admit {
                context: context.clone(),
                handle: d.handle(),
                record: record.clone(),
            },
            Some(seed.bearer()),
        )
        .await
        .unwrap();
    let m = m.append(d.record(), joiner.request(), &record).unwrap();
    context.device = joiner.device();
    context.head = m.head();
    let Reply::PreparedManagement(prep) = client
        .exchange(
            Operation::PrepareManagement {
                context: context.clone(),
            },
            Some(joiner.bearer()),
        )
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(prep.evidence.verify().unwrap().head(), m.head());
    let revoke = joiner
        .authority()
        .prepare_revoke(&m, &[seed.genesis().device_id()])
        .unwrap();
    client
        .exchange(
            Operation::Manage {
                context: context.clone(),
                record: revoke.clone(),
            },
            Some(joiner.bearer()),
        )
        .await
        .unwrap();
    let stale = client
        .exchange(
            Operation::PrepareManagement {
                context: context.clone(),
            },
            Some(joiner.bearer()),
        )
        .await
        .err()
        .unwrap();
    assert!(enrollment::is_stale(&stale));
    context.device = seed.genesis().device_id();
    for op in [
        Operation::PrepareManagement {
            context: context.clone(),
        },
        Operation::Membership {
            context: context.clone(),
        },
        Operation::Manage {
            context: context.clone(),
            record: revoke.clone(),
        },
        Operation::Admit {
            context: context.clone(),
            handle: d.handle(),
            record,
        },
    ] {
        let e = client
            .exchange(op, Some(seed.bearer()))
            .await
            .err()
            .unwrap();
        assert!(!enrollment::is_stale(&e));
    }
    let pending = m.append(&[], &[], &revoke).unwrap();
    context.device = joiner.device();
    context.head = pending.head();
    let rotation = joiner
        .authority()
        .prepare_rotation(&pending, &keys, prep.high_water)
        .unwrap();
    client
        .exchange(
            Operation::Manage {
                context: context.clone(),
                record: rotation.clone(),
            },
            Some(joiner.bearer()),
        )
        .await
        .unwrap();
    let rotated = pending.append(&[], &[], &rotation).unwrap();
    context.head = rotated.head();
    let Reply::PreparedManagement(after) = client
        .exchange(
            Operation::PrepareManagement { context },
            Some(joiner.bearer()),
        )
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(after.high_water, prep.high_water);
    assert_eq!(after.evidence.verify().unwrap().head(), rotated.head());
    assert!(
        server
            .membership_evidence(&peer::Authentication {
                vault: m.genesis().context().vault_id,
                genesis: m.genesis().commitment(),
                device: seed.genesis().device_id(),
                head: rotated.head(),
                bearer: seed.bearer()
            })
            .await
            .is_err()
    );
    task.abort();
}

mod retry;
mod rotation;

fn soon() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 20
}

#[tokio::test]
async fn production_clock_rejects_an_already_expired_invitation() {
    let root = tempfile::tempdir().unwrap();
    let (db, store, _, origin, task) = adopted(root.path()).await;
    let client = Client::new(&origin).unwrap();
    let error = client.invite(&store, &db, 0).await.unwrap_err();
    assert_eq!(error.to_string(), "error enrollment-invitation-unavailable");
    task.abort();
}

#[tokio::test]
async fn expired_unsent_invitation_retires_but_sent_candidate_stays_blocked() {
    let root = tempfile::tempdir().unwrap();
    let clock = test_clock();
    let (db, store, _server, origin, task) =
        adopted_with_clock(root.path(), None, clock.clone()).await;
    let client = Client::new(&origin).unwrap();
    let peer = |name: &str| {
        let root = root.path().to_path_buf();
        let name = name.to_string();
        async move {
            let db = Database::open(&root.join(format!("{name}.sqlite")))
                .await
                .unwrap();
            let keys = isolated_store(&db, &root.join(format!("{name}-keys"))).await;
            (db, keys)
        }
    };

    // A requested but never admitted invitation retires at its declared expiry.
    let expires = soon();
    let stale = client.invite(&store, &db, expires).await.unwrap();
    let stale_handle = stale.handle();
    let (late_db, late_keys) = peer("late").await;
    client
        .request(&late_keys, &late_db, Some(stale))
        .await
        .unwrap();
    assert_eq!(
        store.outbound_invitation(&db).await.unwrap(),
        Some(OutboundInvitation::Pending)
    );
    advance_clock(&clock, expires);
    drop(store.tail_inputs(&db, &origin).await.unwrap());
    assert_eq!(store.outbound_invitation(&db).await.unwrap(), None);
    // A stale admission attempt for the retired handle can never send a grant.
    let error = client
        .admit_handle(&store, &db, Some(stale_handle))
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "error enrollment-invitation-retired");
    assert!(!client.complete(&late_keys, &late_db).await.unwrap());

    // A new invitation gets a new identity and admits normally.
    let fresh = client.invite(&store, &db, expiry()).await.unwrap();
    assert_ne!(fresh.handle(), stale_handle);
    let (fresh_db, fresh_keys) = peer("fresh").await;
    client
        .request(&fresh_keys, &fresh_db, Some(fresh))
        .await
        .unwrap();
    assert!(client.admit(&store, &db).await.unwrap());
    assert!(client.complete(&fresh_keys, &fresh_db).await.unwrap());
    drop(store.tail_inputs(&db, &origin).await.unwrap());

    // A candidate that may have been sent is never retired by expiry.
    let expires = soon();
    let sent = client.invite(&store, &db, expires).await.unwrap();
    let (sent_db, sent_keys) = peer("sent").await;
    client
        .request(&sent_keys, &sent_db, Some(sent))
        .await
        .unwrap();
    {
        let inputs = store.active_inputs(&db, &origin).await.unwrap();
        let journal = store
            .prepare_invitation(&db, &inputs, None, None)
            .await
            .unwrap();
        let mail = client
            .exchange(
                Operation::Mailbox {
                    vault: inputs.membership.genesis().context().vault_id,
                    handle: journal.handle,
                },
                None,
            )
            .await
            .unwrap();
        let Reply::Mailbox(mail) = mail else {
            panic!("mailbox reply")
        };
        store
            .prepare_admission(&db, &inputs, &journal, mail.request.as_ref().unwrap())
            .await
            .unwrap();
    }
    let tail = store.tail_inputs(&db, &origin).await.unwrap();
    assert!(!tail.publishing_blocked().unwrap());
    advance_clock(&clock, expires);
    // The same snapshot starts refusing publication at the declared expiry.
    assert!(
        tail.require_publishing_ready()
            .unwrap_err()
            .is::<crate::protected_local_keys::peer::PublishingBlocked>()
    );
    drop(tail);
    let tail = store.tail_inputs(&db, &origin).await.unwrap();
    assert!(tail.publishing_blocked().unwrap());
    drop(tail);
    assert_eq!(
        store.outbound_invitation(&db).await.unwrap(),
        Some(OutboundInvitation::Disclosed)
    );
    task.abort();
}

/// Prepares and marks sent one grant for `joiner`'s request without delivering it.
async fn sent_candidate(
    client: &Client,
    store: &ProtectedLocalKeyStore,
    db: &Database,
    origin: &str,
) -> ([u8; 32], Vec<u8>) {
    let inputs = store.active_inputs(db, origin).await.unwrap();
    let journal = store
        .prepare_invitation(db, &inputs, None, None)
        .await
        .unwrap();
    let Reply::Mailbox(mail) = client
        .exchange(
            Operation::Mailbox {
                vault: inputs.membership.genesis().context().vault_id,
                handle: journal.handle,
            },
            None,
        )
        .await
        .unwrap()
    else {
        panic!("mailbox reply")
    };
    let record = store
        .prepare_admission(db, &inputs, &journal, mail.request.as_ref().unwrap())
        .await
        .unwrap();
    (journal.handle, record)
}

async fn generations(store: &ProtectedLocalKeyStore, db: &Database, origin: &str) -> usize {
    store
        .active_inputs(db, origin)
        .await
        .unwrap()
        .membership
        .generations()
        .len()
}

#[tokio::test]
async fn expired_sent_invitation_withdraws_by_rotation_unless_admission_won() {
    let root = tempfile::tempdir().unwrap();
    let clock = test_clock();
    let (db, store, server, origin, task) =
        adopted_with_clock(root.path(), None, clock.clone()).await;
    let client = Client::new(&origin).unwrap();
    let peer = |name: &str| {
        let root = root.path().to_path_buf();
        let name = name.to_string();
        async move {
            let db = Database::open(&root.join(format!("{name}.sqlite")))
                .await
                .unwrap();
            let keys = isolated_store(&db, &root.join(format!("{name}-keys"))).await;
            (db, keys)
        }
    };
    // An installed survivor must receive the replacement generation.
    let (survivor_db, survivor_keys) = peer("survivor").await;
    let invitation = client.invite(&store, &db, expiry()).await.unwrap();
    client
        .request(&survivor_keys, &survivor_db, Some(invitation))
        .await
        .unwrap();
    assert!(client.admit(&store, &db).await.unwrap());
    assert!(client.complete(&survivor_keys, &survivor_db).await.unwrap());
    client.install(&survivor_keys, &survivor_db).await.unwrap();
    let initial = generations(&store, &db, &origin).await;

    // A grant marked sent but never admitted.
    let expires = soon();
    let invitation = client.invite(&store, &db, expires).await.unwrap();
    let (joiner_db, joiner_keys) = peer("joiner").await;
    client
        .request(&joiner_keys, &joiner_db, Some(invitation))
        .await
        .unwrap();
    let (handle, record) = sent_candidate(&client, &store, &db, &origin).await;
    advance_clock(&clock, expires);

    // Faults before the final phase: the fence is written, the server commits
    // the cancellation but the reply is lost, and the freeze and rotation
    // commit before the process stops.
    {
        let inputs = store.active_inputs(&db, &origin).await.unwrap();
        let journal = store
            .prepare_invitation(&db, &inputs, None, Some(handle))
            .await
            .unwrap();
        store
            .mark_withdrawing(&db, &inputs, &journal)
            .await
            .unwrap();
        let error = store
            .prepare_admission(&db, &inputs, &journal, b"any")
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "error enrollment-invitation-withdrawing");
        assert_eq!(
            membership::cancel_membership_invitation_at(
                &server,
                &Context::active(&inputs).auth(inputs.bearer()),
                handle,
                i64::try_from(expires).unwrap(),
            )
            .await
            .unwrap(),
            aven_core::sync::seed_claim::membership::CancelStatus::Cancelled
        );
        store
            .management_intent(&db, &inputs, None, Some(handle))
            .await
            .unwrap()
            .unwrap();
    }
    client
        .manage(&store, &db, None, Some(handle))
        .await
        .unwrap();
    assert_eq!(generations(&store, &db, &origin).await, initial + 1);
    assert_eq!(
        store.outbound_invitation(&db).await.unwrap(),
        Some(OutboundInvitation::Disclosed)
    );

    // Staged boundary, not a process exit: reopen the database and protected
    // store after the committed rotation but before `withdrawn`. Ordinary
    // rounds reuse that rotation as proof, unblock publishing and never rotate
    // again.
    let path = db.path().to_path_buf();
    drop(store);
    drop(db);
    let db = Database::open(&path).await.unwrap();
    let mut store = isolated_store(&db, &root.path().join("keys")).await;
    store.set_enrollment_clock(clock.clone());
    let rounds = crate::encrypted_tail_http::Client::new(&origin).unwrap();
    for _ in 0..2 {
        rounds.round(&store, &db, root.path()).await.unwrap();
        assert_eq!(generations(&store, &db, &origin).await, initial + 1);
        assert_eq!(store.outbound_invitation(&db).await.unwrap(), None);
    }
    assert!(!client.complete(&joiner_keys, &joiner_db).await.unwrap());
    {
        let inputs = store.active_inputs(&db, &origin).await.unwrap();
        assert!(
            membership::admit_membership_device_at(
                &server,
                &Context::active(&inputs).auth(inputs.bearer()),
                handle,
                &record,
                i64::try_from(expires).unwrap(),
            )
            .await
            .is_err()
        );
    }
    rounds
        .round(
            &survivor_keys,
            &survivor_db,
            &root.path().join("survivor-blobs"),
        )
        .await
        .unwrap();
    assert_eq!(
        generations(&survivor_keys, &survivor_db, &origin).await,
        initial + 1
    );

    // The automatic path fences, cancels over HTTP, freezes and rotates.
    let expires = soon();
    let invitation = client.invite(&store, &db, expires).await.unwrap();
    let (other_db, other_keys) = peer("other").await;
    client
        .request(&other_keys, &other_db, Some(invitation))
        .await
        .unwrap();
    let (handle, record) = sent_candidate(&client, &store, &db, &origin).await;
    advance_clock(&clock, expires);
    client.finish_pending_management(&store, &db).await.unwrap();
    assert_eq!(generations(&store, &db, &origin).await, initial + 2);
    assert_eq!(store.outbound_invitation(&db).await.unwrap(), None);
    {
        let inputs = store.active_inputs(&db, &origin).await.unwrap();
        let error = membership::admit_membership_device_at(
            &server,
            &Context::active(&inputs).auth(inputs.bearer()),
            handle,
            &record,
            i64::try_from(expires).unwrap(),
        )
        .await
        .unwrap_err();
        assert_eq!(error.to_string(), "error enrollment-expired");
    }
    assert!(!client.complete(&other_keys, &other_db).await.unwrap());

    // Admission that won before withdrawal resolves ready without rotating.
    let expires = soon();
    let invitation = client.invite(&store, &db, expires).await.unwrap();
    let (late_db, late_keys) = peer("late").await;
    client
        .request(&late_keys, &late_db, Some(invitation))
        .await
        .unwrap();
    let (handle, record) = sent_candidate(&client, &store, &db, &origin).await;
    {
        let inputs = store.active_inputs(&db, &origin).await.unwrap();
        membership::admit_membership_device_at(
            &server,
            &Context::active(&inputs).auth(inputs.bearer()),
            handle,
            &record,
            i64::try_from(expires - 1).unwrap(),
        )
        .await
        .unwrap();
    }
    advance_clock(&clock, expires);
    client.finish_pending_management(&store, &db).await.unwrap();
    assert_eq!(generations(&store, &db, &origin).await, initial + 2);
    assert_eq!(store.outbound_invitation(&db).await.unwrap(), None);
    assert!(client.complete(&late_keys, &late_db).await.unwrap());
    task.abort();
}

#[tokio::test]
async fn only_authentication_refusals_read_as_access_refusals() {
    use aven_core::sync::client::errors::is_access_refusal;
    use axum::routing::post;

    let cases = [
        (
            StatusCode::FORBIDDEN,
            "enrollment-unauthorized",
            "error enrollment-unauthorized",
            true,
        ),
        (
            StatusCode::BAD_REQUEST,
            "enrollment-refused",
            "error enrollment-refused outcome-unknown",
            false,
        ),
        (
            StatusCode::REQUEST_TIMEOUT,
            "enrollment-timeout",
            "error enrollment-timeout outcome-unknown",
            false,
        ),
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "enrollment-server-error",
            "error enrollment-server outcome-unknown",
            false,
        ),
        (
            StatusCode::BAD_GATEWAY,
            "",
            "error enrollment-server outcome-unknown",
            false,
        ),
    ];
    for (status, body, expected, refusal) in cases {
        let app = Router::new().route(
            PATH,
            post(move || async move { crate::http_admission::refusal(status, body) }),
        );
        let (origin, task) = e2ee_http::serve(app, "127.0.0.1:0").await;
        let error = Client::new(&origin)
            .unwrap()
            .exchange(
                Operation::Mailbox {
                    vault: [0; 32],
                    handle: [0; 32],
                },
                None,
            )
            .await
            .map(|_| ())
            .unwrap_err();
        assert_eq!(error.to_string(), expected);
        assert_eq!(is_access_refusal(&error), refusal, "{expected}");
        task.abort();
    }
    // An oversized error body is a server failure, not a refusal.
    let app = Router::new().route(
        PATH,
        post(|| async { (StatusCode::BAD_REQUEST, "x".repeat(1024)) }),
    );
    let (origin, task) = e2ee_http::serve(app, "127.0.0.1:0").await;
    let error = Client::new(&origin)
        .unwrap()
        .exchange(
            Operation::Mailbox {
                vault: [0; 32],
                handle: [0; 32],
            },
            None,
        )
        .await
        .map(|_| ())
        .unwrap_err();
    assert!(!is_access_refusal(&error), "{error}");
    task.abort();
}

#[tokio::test]
async fn late_or_invalid_mailbox_requests_never_become_candidates() {
    let root = tempfile::tempdir().unwrap();
    let clock = test_clock();
    let (db, store, _server, origin, task) =
        adopted_with_clock(root.path(), None, clock.clone()).await;
    let client = Client::new(&origin).unwrap();
    let target = Database::open(&root.path().join("peer.sqlite"))
        .await
        .unwrap();
    let peer_store = isolated_store(&target, &root.path().join("peer-keys")).await;
    let expires = soon();
    let invitation = client.invite(&store, &db, expires).await.unwrap();
    client
        .request(&peer_store, &target, Some(invitation))
        .await
        .unwrap();
    // Inputs loaded before expiry model a mailbox reply delayed past it.
    let inputs = store.active_inputs(&db, &origin).await.unwrap();
    let journal = store
        .prepare_invitation(&db, &inputs, None, None)
        .await
        .unwrap();
    let Reply::Mailbox(mail) = client
        .exchange(
            Operation::Mailbox {
                vault: inputs.membership.genesis().context().vault_id,
                handle: journal.handle,
            },
            None,
        )
        .await
        .unwrap()
    else {
        panic!("mailbox reply")
    };
    let request = mail.request.unwrap();

    // An invalid request fails validation without binding the journal.
    let mut forged = request.clone();
    *forged.last_mut().unwrap() ^= 1;
    assert!(
        store
            .prepare_admission(&db, &inputs, &journal, &forged)
            .await
            .is_err()
    );

    advance_clock(&clock, expires);
    let error = store
        .prepare_admission(&db, &inputs, &journal, &request)
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "error enrollment-expired");
    drop(inputs);
    // Nothing could have been sent, so the invitation simply retires.
    drop(store.tail_inputs(&db, &origin).await.unwrap());
    assert_eq!(store.outbound_invitation(&db).await.unwrap(), None);
    task.abort();
}

#[tokio::test]
async fn sent_candidate_resends_exactly_after_expiry() {
    let root = tempfile::tempdir().unwrap();
    let clock = test_clock();
    let (db, store, _server, origin, task) =
        adopted_with_clock(root.path(), None, clock.clone()).await;
    let client = Client::new(&origin).unwrap();
    let target = Database::open(&root.path().join("peer.sqlite"))
        .await
        .unwrap();
    let peer_store = isolated_store(&target, &root.path().join("peer-keys")).await;
    let expires = soon();
    let invitation = client.invite(&store, &db, expires).await.unwrap();
    client
        .request(&peer_store, &target, Some(invitation))
        .await
        .unwrap();
    let inputs = store.active_inputs(&db, &origin).await.unwrap();
    let journal = store
        .prepare_invitation(&db, &inputs, None, None)
        .await
        .unwrap();
    let Reply::Mailbox(mail) = client
        .exchange(
            Operation::Mailbox {
                vault: inputs.membership.genesis().context().vault_id,
                handle: journal.handle,
            },
            None,
        )
        .await
        .unwrap()
    else {
        panic!("mailbox reply")
    };
    let request = mail.request.unwrap();
    let record = store
        .prepare_admission(&db, &inputs, &journal, &request)
        .await
        .unwrap();
    advance_clock(&clock, expires);
    // A possibly sent candidate stays resendable, byte for byte.
    assert_eq!(
        store
            .prepare_admission(&db, &inputs, &journal, &request)
            .await
            .unwrap(),
        record
    );
    drop(inputs);
    assert_eq!(
        store.outbound_invitation(&db).await.unwrap(),
        Some(OutboundInvitation::Disclosed)
    );
    task.abort();
}

#[tokio::test]
async fn forged_admission_signature_is_not_pinned_and_correct_response_completes() {
    let root = tempfile::tempdir().unwrap();
    let (db, store, server, origin, task) = adopted(root.path()).await;
    let client = Client::new(&origin).unwrap();
    let invitation = client.invite(&store, &db, expiry()).await.unwrap();
    let target = Database::open(&root.path().join("peer.sqlite"))
        .await
        .unwrap();
    let peer_store = isolated_store(&target, &root.path().join("peer-keys")).await;
    client
        .request(&peer_store, &target, Some(invitation))
        .await
        .unwrap();
    assert!(client.admit(&store, &db).await.unwrap());
    let peer = peer_store
        .prepare_peer(&target, &origin, None)
        .await
        .unwrap();
    let mut forged = server
        .membership_mailbox(peer.vault(), peer.handle())
        .await
        .unwrap();
    // The signature is the final component; HPKE opening ignores it.
    *forged.admission.as_mut().unwrap().last_mut().unwrap() ^= 1;
    let grant = peer_store
        .open_peer_response(&target, &forged)
        .await
        .unwrap();
    assert!(
        client
            .finish(&peer_store, &target, &peer, &forged, grant)
            .await
            .is_err()
    );
    assert_eq!(
        peer_store.enrollment_readiness(&target).await.unwrap(),
        EnrollmentReadiness::Pending
    );
    assert!(client.complete(&peer_store, &target).await.unwrap());
    task.abort();
}
