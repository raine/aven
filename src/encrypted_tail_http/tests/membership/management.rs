use super::*;
use aven_core::sync::seed_claim::membership::{EvidenceRecord, MAX_CUTOFF};
use peer_enrollment_http::RemovalStatus;

async fn device(store: &ProtectedLocalKeyStore, db: &Database, origin: &str) -> [u8; 32] {
    store.active_inputs(db, origin).await.unwrap().device()
}

#[tokio::test]
async fn one_action_removes_seed_and_survivors_sync_tasks_and_images() {
    let f = fixture().await;
    let third = join(&f, "third", &f.peer, &f.peer_store).await;
    let enrollment = peer_enrollment_http::Client::new(&f.origin).unwrap();
    let seed = device(&f.seed_store, &f.seed, &f.origin).await;
    assert_eq!(
        enrollment
            .remove_device(&f.peer_store, &f.peer, seed)
            .await
            .unwrap(),
        RemovalStatus::Complete
    );
    let m = floor(&f.peer_store, &f.peer, &f.origin).await;
    assert_eq!(m.sequence(), 5);
    assert!(!m.rotation_pending());
    assert!(!m.has_device(seed));
    let client = Client::new(&f.origin).unwrap();
    assert!(
        client
            .round(&f.seed_store, &f.seed, f.root.path())
            .await
            .is_err()
    );
    // Original installed peers survive the removal of their inviter.
    enrollment.install(&third.store, &third.db).await.unwrap();
    attachments::add_image(&f).await;
    let w = third.db.list_workspaces().await.unwrap().remove(0);
    let task = third
        .db
        .create_task(&w, draft("survivor after removal"))
        .await
        .unwrap()
        .task;
    for _ in 0..8 {
        client
            .round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
            .await
            .unwrap();
        client
            .round(&third.store, &third.db, &third.blobs)
            .await
            .unwrap();
    }
    assert_eq!(
        title(&f.peer, task.id.as_str()).await,
        "survivor after removal"
    );
    assert_eq!(
        scalar(
            &third.db,
            "SELECT count(*) FROM task_attachments WHERE deleted=0"
        )
        .await,
        2
    );
    assert_eq!(
        client
            .round(&third.store, &third.db, &third.blobs)
            .await
            .unwrap()
            .images,
        ImageTransfer::Complete
    );
    // Repeating the user action resolves its retained intent, not another freeze.
    assert_eq!(
        enrollment
            .remove_device(&f.peer_store, &f.peer, seed)
            .await
            .unwrap(),
        RemovalStatus::Complete
    );
    assert_eq!(
        floor(&f.peer_store, &f.peer, &f.origin).await.head(),
        m.head()
    );

    // A fresh join installs the original snapshot, not a server-selected current
    // projection. Ordinary rounds then catch up using both historical and new keys.
    let fresh = join(&f, "fresh", &f.peer, &f.peer_store).await;
    assert_eq!(
        scalar(
            &fresh.db,
            "SELECT count(*) FROM tasks WHERE title='PRIVATE-HTTP-SEED-TASK'"
        )
        .await,
        1
    );
    assert_eq!(
        scalar(
            &fresh.db,
            "SELECT count(*) FROM tasks WHERE title='survivor after removal'"
        )
        .await,
        0
    );
    assert_eq!(
        scalar(
            &fresh.db,
            "SELECT count(*) FROM task_attachments WHERE deleted=0"
        )
        .await,
        1
    );
    {
        let inputs = fresh
            .store
            .active_inputs(&fresh.db, &f.origin)
            .await
            .unwrap();
        assert_eq!(inputs.membership.generations().len(), 2);
        inputs
            .generation_keys()
            .validate(&inputs.membership)
            .unwrap();
    }
    for _ in 0..8 {
        client
            .round(&fresh.store, &fresh.db, &fresh.blobs)
            .await
            .unwrap();
    }
    assert_eq!(
        title(&fresh.db, task.id.as_str()).await,
        "survivor after removal"
    );
    assert_eq!(
        scalar(
            &fresh.db,
            "SELECT count(*) FROM task_attachments WHERE deleted=0"
        )
        .await,
        2
    );
    assert_eq!(
        client
            .round(&fresh.store, &fresh.db, &fresh.blobs)
            .await
            .unwrap()
            .images,
        ImageTransfer::Complete
    );
    let cursor = fresh.db.meta("sync_cursor").await.unwrap();
    enrollment.install(&fresh.store, &fresh.db).await.unwrap();
    assert_eq!(fresh.db.meta("sync_cursor").await.unwrap(), cursor);
    assert_eq!(
        title(&fresh.db, task.id.as_str()).await,
        "survivor after removal"
    );
}

// Intercepts management only, preserving actual authorization and transactions.
struct ManagementFault {
    count: std::sync::atomic::AtomicUsize,
    ordinal: usize,
    lose: bool,
    reject_before: std::sync::atomic::AtomicBool,
    pause: Option<tokio::sync::mpsc::Sender<tokio::sync::oneshot::Sender<()>>>,
    records: std::sync::Mutex<Vec<Vec<u8>>>,
}
async fn management_fault(
    State(fault): State<Arc<ManagementFault>>,
    request: Request,
    next: axum::middleware::Next,
) -> Response {
    let (parts, body) = request.into_parts();
    let bytes = to_bytes(body, 5 * 1024 * 1024).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let manage = value.get("Manage");
    let intercept = if let Some(manage) = manage {
        fault.records.lock().unwrap().push(
            base64::Engine::decode(
                &base64::engine::general_purpose::STANDARD,
                manage["record"].as_str().unwrap(),
            )
            .unwrap(),
        );
        {
            let ordinal = fault
                .count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                + 1;
            fault.ordinal == 0 || ordinal == fault.ordinal
        }
    } else {
        false
    };
    if intercept && let Some(events) = &fault.pause {
        let (send, receive) = tokio::sync::oneshot::channel();
        events.send(send).await.unwrap();
        receive.await.unwrap();
    }
    if intercept
        && fault
            .reject_before
            .load(std::sync::atomic::Ordering::SeqCst)
    {
        return (
            StatusCode::BAD_GATEWAY,
            "test-undelivered-management-request",
        )
            .into_response();
    }
    let response = next
        .run(Request::from_parts(parts, axum::body::Body::from(bytes)))
        .await;
    if intercept && fault.lose && response.status() == StatusCode::OK {
        (StatusCode::BAD_GATEWAY, "test-lost-management-response").into_response()
    } else {
        response
    }
}
async fn install_fault(
    f: &mut Fixture,
    ordinal: usize,
    lose: bool,
    pause: Option<tokio::sync::mpsc::Sender<tokio::sync::oneshot::Sender<()>>>,
) -> Arc<ManagementFault> {
    f.task.abort();
    let _ = (&mut f.task).await;
    let fault = Arc::new(ManagementFault {
        count: Default::default(),
        ordinal,
        lose,
        reject_before: Default::default(),
        pause,
        records: Default::default(),
    });
    let app = peer_enrollment_http::router(f.server.clone())
        .merge(router(f.server.clone()))
        .layer(axum::middleware::from_fn_with_state(
            fault.clone(),
            management_fault,
        ));
    (_, f.task) = e2ee_http::serve(app, f.origin.strip_prefix("http://").unwrap()).await;
    fault
}

#[tokio::test]
async fn lost_revoke_and_rotate_replies_reopen_and_resolve_before_replacement() {
    for ordinal in [1, 2] {
        let mut f = fixture().await;
        let third = join(&f, "third", &f.seed, &f.seed_store).await;
        let target = device(&third.store, &third.db, &f.origin).await;
        let fault = install_fault(&mut f, ordinal, true, None).await;
        let enrollment = peer_enrollment_http::Client::new(&f.origin).unwrap();
        assert!(
            enrollment
                .remove_device(&f.peer_store, &f.peer, target)
                .await
                .is_err()
        );
        assert_eq!(fault.records.lock().unwrap().len(), ordinal);
        let db = Database::open(f.peer.path()).await.unwrap();
        let store = isolated_store(db.path(), &f.root.path().join("peer-keys"));
        Client::new(&f.origin)
            .unwrap()
            .round(&store, &db, &f.root.path().join("peer-blobs"))
            .await
            .unwrap();
        let m = floor(&store, &db, &f.origin).await;
        assert_eq!(m.sequence(), 5);
        assert!(!m.rotation_pending());
        assert!(!m.has_device(target));
        assert_eq!(
            fault.records.lock().unwrap().len(),
            2,
            "accepted records must never be replaced or resent"
        );
        assert_eq!(
            enrollment.remove_device(&store, &db, target).await.unwrap(),
            RemovalStatus::Complete
        );
    }
}

#[tokio::test]
async fn self_removal_leaves_pending_pull_only_is_read_only_and_last_device_refuses() {
    let f = fixture().await;
    let enrollment = peer_enrollment_http::Client::new(&f.origin).unwrap();
    let target = device(&f.peer_store, &f.peer, &f.origin).await;
    assert_eq!(
        enrollment
            .remove_device(&f.peer_store, &f.peer, target)
            .await
            .unwrap(),
        RemovalStatus::SelfRevoked
    );
    let client = Client::new(&f.origin).unwrap();
    assert!(
        client
            .round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
            .await
            .is_err()
    );
    client
        .pull_only_round(&f.seed_store, &f.seed)
        .await
        .unwrap();
    assert!(
        floor(&f.seed_store, &f.seed, &f.origin)
            .await
            .rotation_pending()
    );
    client
        .round(&f.seed_store, &f.seed, f.root.path())
        .await
        .unwrap();
    let before = floor(&f.seed_store, &f.seed, &f.origin).await;
    assert!(!before.rotation_pending());
    let seed = device(&f.seed_store, &f.seed, &f.origin).await;
    assert!(
        enrollment
            .remove_device(&f.seed_store, &f.seed, seed)
            .await
            .is_err()
    );
    assert_eq!(
        floor(&f.seed_store, &f.seed, &f.origin).await.head(),
        before.head()
    );
}

#[tokio::test]
async fn competing_survivor_finishes_losing_candidate_through_one_stale_retry() {
    let mut f = fixture().await;
    let third = join(&f, "third", &f.seed, &f.seed_store).await;
    let enrollment = peer_enrollment_http::Client::new(&f.origin).unwrap();
    let target = device(&third.store, &third.db, &f.origin).await;
    assert_eq!(
        enrollment
            .remove_device(&third.store, &third.db, target)
            .await
            .unwrap(),
        RemovalStatus::SelfRevoked
    );
    let (send, mut receive) = tokio::sync::mpsc::channel(1);
    let fault = install_fault(&mut f, 1, false, Some(send)).await;
    let client = Client::new(&f.origin).unwrap();
    let peer_blobs = f.root.path().join("peer-blobs");
    let first = client.round(&f.peer_store, &f.peer, &peer_blobs);
    let second = async {
        let release = receive.recv().await.unwrap();
        client
            .round(&f.seed_store, &f.seed, f.root.path())
            .await
            .unwrap();
        release.send(()).unwrap();
    };
    let (result, _) = tokio::join!(first, second);
    result.unwrap();
    let a = floor(&f.peer_store, &f.peer, &f.origin).await;
    let b = floor(&f.seed_store, &f.seed, &f.origin).await;
    assert_eq!(a.head(), b.head());
    assert_eq!(a.sequence(), 5);
    assert!(!a.rotation_pending());
    let records = fault.records.lock().unwrap();
    assert_eq!(records.len(), 2);
    assert_ne!(records[0], records[1]);
}

#[tokio::test]
async fn admission_wins_rotate_slot_replacement_uses_fresh_material_and_coverage() {
    let mut f = fixture().await;
    let third = join(&f, "third", &f.seed, &f.seed_store).await;
    let enrollment = peer_enrollment_http::Client::new(&f.origin).unwrap();
    let target = device(&third.store, &third.db, &f.origin).await;
    enrollment
        .remove_device(&third.store, &third.db, target)
        .await
        .unwrap();
    let (send, mut receive) = tokio::sync::mpsc::channel(1);
    let fault = install_fault(&mut f, 1, false, Some(send)).await;
    let client = Client::new(&f.origin).unwrap();
    let peer_blobs = f.root.path().join("peer-blobs");
    let first = client.round(&f.peer_store, &f.peer, &peer_blobs);
    let second = async {
        let release = receive.recv().await.unwrap();
        let fourth = join(&f, "fourth", &f.seed, &f.seed_store).await;
        release.send(()).unwrap();
        fourth
    };
    let (result, fourth) = tokio::join!(first, second);
    result.unwrap();
    client
        .round(&fourth.store, &fourth.db, &fourth.blobs)
        .await
        .unwrap();
    let m = floor(&f.peer_store, &f.peer, &f.origin).await;
    assert_eq!(m.sequence(), 6);
    assert_eq!(
        floor(&fourth.store, &fourth.db, &f.origin).await.head(),
        m.head()
    );
    let records = fault.records.lock().unwrap();
    assert_eq!(records.len(), 2);
    assert_ne!(records[0], records[1]);
    drop(records);
    let a = std::fs::read(owned(&f, "management-0-material-0")).unwrap();
    let b = std::fs::read(owned(&f, "management-0-material-1")).unwrap();
    // AVRM payload has generation, secret, then HPKE randomness. The outer frame
    // binds installation ownership; compare the actual protected payload fields.
    let payload = |bytes: Vec<u8>| {
        let start = bytes.windows(6).position(|w| w == b"AVRM\0\x01").unwrap();
        bytes[start + 6..start + 102].to_vec()
    };
    let a = payload(a);
    let b = payload(b);
    for offset in [0, 32, 64] {
        assert_ne!(&a[offset..offset + 32], &b[offset..offset + 32]);
    }
    assert_eq!(&b[..32], m.current_generation().id);
    assert_ne!(&a[..32], m.current_generation().id);
}

fn owned(f: &Fixture, suffix: &str) -> std::path::PathBuf {
    std::fs::read_dir(f.root.path().join("peer-keys"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with(&format!(".{suffix}"))
        })
        .unwrap()
}

#[tokio::test]
async fn safe_management_does_not_close_open_invitation() {
    let f = fixture().await;
    let third = join(&f, "third", &f.seed, &f.seed_store).await;
    let enrollment = peer_enrollment_http::Client::new(&f.origin).unwrap();
    enrollment
        .invite(&f.peer_store, &f.peer, expiry())
        .await
        .unwrap();
    let target = device(&third.store, &third.db, &f.origin).await;
    enrollment
        .remove_device(&third.store, &third.db, target)
        .await
        .unwrap();
    let round = Client::new(&f.origin)
        .unwrap()
        .round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
        .await
        .unwrap();
    assert!(!round.publishing_blocked);
    assert!(
        !floor(&f.peer_store, &f.peer, &f.origin)
            .await
            .rotation_pending()
    );
    assert_eq!(
        f.peer_store.outbound_invitation(&f.peer).await.unwrap(),
        Some(crate::protected_local_keys::peer::OutboundInvitation::Pending)
    );
}

#[tokio::test]
async fn lost_self_removal_reply_is_not_signed_local_revocation_evidence() {
    let mut f = fixture().await;
    let target = device(&f.peer_store, &f.peer, &f.origin).await;
    let before = floor(&f.peer_store, &f.peer, &f.origin).await.head();
    install_fault(&mut f, 1, true, None).await;
    let enrollment = peer_enrollment_http::Client::new(&f.origin).unwrap();
    assert!(
        enrollment
            .remove_device(&f.peer_store, &f.peer, target)
            .await
            .is_err()
    );
    assert!(
        enrollment
            .remove_device(&f.peer_store, &f.peer, target)
            .await
            .is_err()
    );
    assert_eq!(
        floor(&f.peer_store, &f.peer, &f.origin).await.head(),
        before
    );
    Client::new(&f.origin)
        .unwrap()
        .round(&f.seed_store, &f.seed, f.root.path())
        .await
        .unwrap();
    assert!(
        !floor(&f.seed_store, &f.seed, &f.origin)
            .await
            .has_device(target)
    );
}

#[tokio::test]
#[ignore = "subprocess management protected phase worker"]
async fn management_worker() {
    let root = std::path::PathBuf::from(std::env::var_os("AVEN_MANAGEMENT_ROOT").unwrap());
    let origin = std::env::var("AVEN_MANAGEMENT_ORIGIN").unwrap();
    let db = Database::open(&root.join("peer.sqlite")).await.unwrap();
    let store = isolated_store(db.path(), &root.join("peer-keys"));
    Client::new(&origin)
        .unwrap()
        .round(&store, &db, &root.join("peer-blobs"))
        .await
        .unwrap();
    panic!("management crash not reached");
}

#[tokio::test]
async fn protected_phase_crashes_resume_exact_material_and_candidate_without_sqlite_secrets() {
    let f = fixture().await;
    let enrollment = peer_enrollment_http::Client::new(&f.origin).unwrap();
    let target = device(&f.seed_store, &f.seed, &f.origin).await;
    enrollment
        .remove_device(&f.seed_store, &f.seed, target)
        .await
        .unwrap();
    let mut retained_material = None;
    let mut retained_candidate = None;
    for phase in ["intent", "plan-0", "material-0", "candidate-0", "sent-0"] {
        let boundary = format!("management-0-{phase}");
        let output = e2ee_http::worker(
            "encrypted_tail_http::tests::membership::management::management_worker",
        )
        .env("AVEN_MANAGEMENT_ROOT", f.root.path())
        .env("AVEN_MANAGEMENT_ORIGIN", &f.origin)
        .env("AVEN_PEER_CRASH_KIND", &boundary)
        .output()
        .await
        .unwrap();
        assert_eq!(
            output.status.code(),
            Some(79),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        if phase == "material-0" {
            retained_material = Some(std::fs::read(owned(&f, "management-0-material-0")).unwrap());
        }
        if phase == "candidate-0" {
            retained_candidate =
                Some(std::fs::read(owned(&f, "management-0-candidate-0")).unwrap());
        }
    }
    let db = Database::open(f.peer.path()).await.unwrap();
    let store = isolated_store(db.path(), &f.root.path().join("peer-keys"));
    Client::new(&f.origin)
        .unwrap()
        .round(&store, &db, &f.root.path().join("peer-blobs"))
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(owned(&f, "management-0-material-0")).unwrap(),
        retained_material.unwrap()
    );
    assert_eq!(
        std::fs::read(owned(&f, "management-0-candidate-0")).unwrap(),
        retained_candidate.unwrap()
    );
    assert!(!floor(&store, &db, &f.origin).await.rotation_pending());
    let material = std::fs::read(owned(&f, "management-0-material-0")).unwrap();
    let start = material
        .windows(6)
        .position(|w| w == b"AVRM\0\x01")
        .unwrap();
    for path in [
        db.path().to_path_buf(),
        std::path::PathBuf::from(format!("{}-wal", db.path().display())),
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            assert!(
                !bytes
                    .windows(32)
                    .any(|w| w == &material[start + 38..start + 70])
            );
        }
    }
    // Established loss refuses even though the generation has already committed.
    std::fs::remove_file(owned(&f, "management-0-material-0")).unwrap();
    assert!(
        Client::new(&f.origin)
            .unwrap()
            .round(&store, &db, &f.root.path().join("peer-blobs"))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn second_stale_race_stops_and_next_round_resolves_before_fresh_candidate() {
    let mut f = fixture().await;
    let third = join(&f, "third", &f.seed, &f.seed_store).await;
    let enrollment = peer_enrollment_http::Client::new(&f.origin).unwrap();
    let target = device(&third.store, &third.db, &f.origin).await;
    enrollment
        .remove_device(&third.store, &third.db, target)
        .await
        .unwrap();
    let (send, mut receive) = tokio::sync::mpsc::channel(1);
    let fault = install_fault(&mut f, 0, false, Some(send)).await;
    let client = Client::new(&f.origin).unwrap();
    let peer_blobs = f.root.path().join("peer-blobs");
    let first = client.round(&f.peer_store, &f.peer, &peer_blobs);
    let second = async {
        for name in ["fourth", "fifth"] {
            let release = receive.recv().await.unwrap();
            join(&f, name, &f.seed, &f.seed_store).await;
            release.send(()).unwrap();
        }
    };
    let (result, _) = tokio::join!(first, second);
    assert!(
        result
            .unwrap_err()
            .is::<aven_core::sync::seed_claim::membership::StaleContext>()
    );
    assert_eq!(fault.records.lock().unwrap().len(), 2);
    assert!(
        floor(&f.peer_store, &f.peer, &f.origin)
            .await
            .rotation_pending()
    );
    let retained = std::fs::read(owned(&f, "management-0-candidate-1")).unwrap();
    install_fault(&mut f, usize::MAX, false, None).await;
    let db = Database::open(f.peer.path()).await.unwrap();
    let store = isolated_store(db.path(), &f.root.path().join("peer-keys"));
    client
        .round(&store, &db, &f.root.path().join("peer-blobs"))
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(owned(&f, "management-0-candidate-1")).unwrap(),
        retained
    );
    assert!(!floor(&store, &db, &f.origin).await.rotation_pending());
    assert!(owned(&f, "management-0-material-2").exists());
}

#[tokio::test]
async fn unknown_undelivered_candidate_retries_exact_bytes_after_reopen() {
    for ordinal in [1, 2] {
        let mut f = fixture().await;
        let third = join(&f, "third", &f.seed, &f.seed_store).await;
        let target = device(&third.store, &third.db, &f.origin).await;
        let fault = install_fault(&mut f, ordinal, false, None).await;
        fault
            .reject_before
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let enrollment = peer_enrollment_http::Client::new(&f.origin).unwrap();
        assert!(
            enrollment
                .remove_device(&f.peer_store, &f.peer, target)
                .await
                .is_err()
        );
        let db = Database::open(f.peer.path()).await.unwrap();
        let store = isolated_store(db.path(), &f.root.path().join("peer-keys"));
        Client::new(&f.origin)
            .unwrap()
            .round(&store, &db, &f.root.path().join("peer-blobs"))
            .await
            .unwrap();
        {
            let records = fault.records.lock().unwrap();
            assert_eq!(records.len(), 3);
            assert_eq!(records[ordinal - 1], records[ordinal]);
        }
        assert_eq!(floor(&store, &db, &f.origin).await.sequence(), 5);
    }
}

#[tokio::test]
async fn completed_removal_stays_complete_across_later_freeze_and_new_intent() {
    let f = fixture().await;
    let third = join(&f, "third", &f.seed, &f.seed_store).await;
    let enrollment = peer_enrollment_http::Client::new(&f.origin).unwrap();
    let seed = device(&f.seed_store, &f.seed, &f.origin).await;
    enrollment
        .remove_device(&f.peer_store, &f.peer, seed)
        .await
        .unwrap();
    let ready = std::fs::read(owned(&f, "management-0-ready")).unwrap();
    let target = device(&third.store, &third.db, &f.origin).await;
    enrollment
        .remove_device(&third.store, &third.db, target)
        .await
        .unwrap();
    let client = Client::new(&f.origin).unwrap();
    client
        .pull_only_round(&f.peer_store, &f.peer)
        .await
        .unwrap();
    assert!(
        floor(&f.peer_store, &f.peer, &f.origin)
            .await
            .rotation_pending()
    );
    client
        .round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
        .await
        .unwrap();
    assert_eq!(floor(&f.peer_store, &f.peer, &f.origin).await.sequence(), 7);
    assert_eq!(
        std::fs::read(owned(&f, "management-0-ready")).unwrap(),
        ready
    );
    assert!(owned(&f, "management-1-ready").exists());
    assert_eq!(
        enrollment
            .remove_device(&f.peer_store, &f.peer, seed)
            .await
            .unwrap(),
        RemovalStatus::Complete
    );
}

/// Serves management from `evidence`, accepting every signed transition.
async fn forged_high_water(
    f: &Fixture,
    evidence: aven_core::sync::seed_claim::membership::Evidence,
    high_water: u64,
    managed: Arc<std::sync::atomic::AtomicBool>,
) -> tokio::task::JoinHandle<()> {
    use serde_json::json;
    let state = Arc::new(tokio::sync::Mutex::new(evidence));
    let app = Router::new().route(
        "/e2ee/enrollment/v1",
        post(move |axum::Json(op): axum::Json<serde_json::Value>| {
            let (state, managed) = (state.clone(), managed.clone());
            async move {
                let mut e = state.lock().await;
                axum::Json(if op.get("PrepareManagement").is_some() {
                    json!({"PreparedManagement": {"evidence": *e, "high_water": high_water}})
                } else if let Some(manage) = op.get("Manage") {
                    managed.store(true, std::sync::atomic::Ordering::SeqCst);
                    let record: String = serde_json::from_value(manage["record"].clone()).unwrap();
                    e.transitions.push(EvidenceRecord {
                        declaration: Vec::new(),
                        request: Vec::new(),
                        record: base64::Engine::decode(
                            &base64::engine::general_purpose::STANDARD,
                            record,
                        )
                        .unwrap(),
                    });
                    json!({"Managed": manage["record"]})
                } else {
                    json!({"Membership": *e})
                })
            }
        }),
    );
    f.task.abort();
    let address = f.origin.strip_prefix("http://").unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    e2ee_http::serve(app, address).await.1
}

#[tokio::test]
async fn forged_high_water_beyond_tail_ranks_is_refused_before_signing() {
    for (high_water, valid) in [(MAX_CUTOFF, true), (MAX_CUTOFF + 1, false)] {
        let f = fixture().await;
        let evidence = f
            .seed_store
            .active_inputs(&f.seed, &f.origin)
            .await
            .unwrap()
            .evidence
            .clone();
        let target = device(&f.peer_store, &f.peer, &f.origin).await;
        let before = floor(&f.seed_store, &f.seed, &f.origin).await;
        let managed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let task = forged_high_water(&f, evidence, high_water, managed.clone()).await;
        let removal = peer_enrollment_http::Client::new(&f.origin)
            .unwrap()
            .remove_device(&f.seed_store, &f.seed, target)
            .await;
        let after = floor(&f.seed_store, &f.seed, &f.origin).await;
        if valid {
            assert_eq!(removal.unwrap(), RemovalStatus::Complete);
            assert_eq!(after.current_generation().starts_after, MAX_CUTOFF);
        } else {
            assert!(removal.is_err());
            assert!(!managed.load(std::sync::atomic::Ordering::SeqCst));
            assert_eq!(after.head(), before.head());
        }
        task.abort();
    }
}
