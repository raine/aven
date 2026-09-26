use super::*;
use crate::encrypted_tail_http::tests::attachments::add_image;
use crate::peer_enrollment_http;
use aven_core::sync::{
    encrypted_tail::{Operation, Reply, attachments},
    seed_claim::Secret,
};
use sha2::Digest;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct Joined {
    db: Database,
    store: ProtectedLocalKeyStore,
    blobs: std::path::PathBuf,
}

fn expiry() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600
}

async fn join(
    f: &Fixture,
    name: &str,
    inviter: &Database,
    inviter_store: &ProtectedLocalKeyStore,
) -> Joined {
    let enrollment = peer_enrollment_http::Client::new(&f.origin).unwrap();
    let db = Database::open(&f.root.path().join(format!("{name}.sqlite")))
        .await
        .unwrap();
    let store = isolated_store(&db, &f.root.path().join(format!("{name}-keys"))).await;
    let invitation = enrollment
        .invite(inviter_store, inviter, expiry())
        .await
        .unwrap();
    enrollment
        .request(&store, &db, Some(invitation))
        .await
        .unwrap();
    assert!(enrollment.admit(inviter_store, inviter).await.unwrap());
    assert!(enrollment.complete(&store, &db).await.unwrap());
    enrollment.install(&store, &db).await.unwrap();
    Joined {
        db,
        store,
        blobs: f.root.path().join(format!("{name}-blobs")),
    }
}

async fn device(store: &ProtectedLocalKeyStore, db: &Database, origin: &str) -> [u8; 32] {
    store.active_inputs(db, origin).await.unwrap().device()
}

async fn rotate_twice_while_peer_is_offline(f: &Fixture, driver: &Joined, removed: [u8; 32]) {
    let enrollment = peer_enrollment_http::Client::new(&f.origin).unwrap();
    let seed = device(&f.seed_store, &f.seed, &f.origin).await;
    enrollment
        .remove_device(&driver.store, &driver.db, seed)
        .await
        .unwrap();
    enrollment
        .remove_device(&driver.store, &driver.db, removed)
        .await
        .unwrap();
}

async fn restart_server(f: &mut Fixture, fault: Arc<HttpFault>) {
    restart_server_with_policy(
        f,
        fault,
        crate::config::AttachmentLifecycleConfig::default().server_policy(),
    )
    .await;
}

async fn restart_server_with_policy(
    f: &mut Fixture,
    fault: Arc<HttpFault>,
    image_policy: aven_core::attachments::LifecyclePolicy,
) {
    f.task.abort();
    let _ = (&mut f.task).await;
    let app = peer_enrollment_http::router(f.server.clone())
        .merge(router_with_policy(f.server.clone(), image_policy))
        .layer(axum::middleware::from_fn_with_state(fault, fault_request));
    (_, f.task) = e2ee_http::serve(app, f.origin.strip_prefix("http://").unwrap()).await;
}

struct HttpFault {
    lose: Option<&'static str>,
    remaining: AtomicUsize,
    bodies: std::sync::Mutex<Vec<(String, Vec<u8>)>>,
}

fn no_fault() -> Arc<HttpFault> {
    Arc::new(HttpFault {
        lose: None,
        remaining: AtomicUsize::new(0),
        bodies: Default::default(),
    })
}

fn operation_value(bytes: &[u8]) -> serde_json::Value {
    let value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    value.get("operation").cloned().unwrap_or(value)
}

fn operation_name(value: &serde_json::Value) -> Option<&str> {
    value
        .get("operation")
        .and_then(serde_json::Value::as_object)
        .and_then(|object| object.keys().next().map(String::as_str))
        .or_else(|| {
            value
                .as_object()
                .and_then(|object| object.keys().next().map(String::as_str))
        })
}

async fn fault_request(
    State(fault): State<Arc<HttpFault>>,
    request: Request,
    next: axum::middleware::Next,
) -> Response {
    let (parts, body) = request.into_parts();
    let bytes = to_bytes(body, 5 * 1024 * 1024).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let operation = operation_name(&value);
    let intercept = fault.lose == operation
        && fault
            .remaining
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |value| {
                value.checked_sub(1)
            })
            .is_ok();
    if let Some(operation) = operation {
        fault
            .bodies
            .lock()
            .unwrap()
            .push((operation.to_owned(), bytes.to_vec()));
    }
    let response = next
        .run(Request::from_parts(parts, axum::body::Body::from(bytes)))
        .await;
    if intercept && response.status() == StatusCode::OK {
        (StatusCode::BAD_GATEWAY, "checkpoint-fault-lost-reply").into_response()
    } else {
        response
    }
}

async fn frozen(db: &Database) -> Vec<u8> {
    sqlx::query_scalar("SELECT record FROM local_e2ee_outbox WHERE singleton=1")
        .fetch_one(&mut *aven_core::test_support::acquire(db).await.unwrap())
        .await
        .unwrap()
}

async fn accepted_task(f: &Fixture, title: &str) -> (String, String, Vec<u8>) {
    let workspace = f.peer.list_workspaces().await.unwrap().remove(0);
    let task = f
        .peer
        .create_task(&workspace, draft(title))
        .await
        .unwrap()
        .task;
    let id: String = sqlx::query_scalar(
        "SELECT change_id FROM changes WHERE entity_id=? AND op_type='create_task'",
    )
    .bind(&task.id)
    .fetch_one(&mut *aven_core::test_support::acquire(&f.peer).await.unwrap())
    .await
    .unwrap();
    let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
    let record = head_record(&f.peer, &inputs.authority).await;
    let Reply::Appended(_) = Client::new(&f.origin)
        .unwrap()
        .exchange(
            &inputs.authority.context,
            &inputs.bearer,
            Operation::Append {
                ticket: None,
                record: record.clone(),
            },
        )
        .await
        .unwrap()
    else {
        panic!("accepted task append");
    };
    assert_eq!(
        scalar(&f.peer, "SELECT count(*) FROM local_e2ee_accepted").await,
        0
    );
    assert_eq!(frozen(&f.peer).await, record);
    (id, task.id.to_string(), record)
}

async fn accepted_image(f: &Fixture, accept_ref: bool) -> (String, Vec<u8>, Vec<u8>, [u8; 32]) {
    let reference = add_image(f).await;
    let sha: String =
        sqlx::query_scalar("SELECT sha256 FROM task_attachments WHERE attachment_id=?")
            .bind(&reference)
            .fetch_one(&mut *aven_core::test_support::acquire(&f.peer).await.unwrap())
            .await
            .unwrap();
    let source = std::fs::read(f.root.path().join("peer-blobs/objects/sha256").join(&sha)).unwrap();
    let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
    let upload = f
        .peer
        .prepare_encrypted_push(&inputs.authority, &f.root.path().join("peer-blobs"))
        .await
        .unwrap()
        .unwrap()
        .upload
        .unwrap();
    let record = head_record(&f.peer, &inputs.authority).await;
    let client = Client::new(&f.origin).unwrap();
    let aven_core::sync::encrypted_tail::attachments::Reply::Status(status) = client
        .image_exchange(
            &inputs.authority.context,
            &inputs.bearer,
            attachments::Operation::Declare {
                workspace: upload.workspace.clone(),
                descriptor: upload.descriptor.clone(),
            },
        )
        .await
        .unwrap()
    else {
        panic!("image declaration");
    };
    let ticket = attachments::Ticket {
        reservation: status.reservation.unwrap(),
    };
    for (index, chunk) in upload.records.iter().cloned().enumerate() {
        client
            .image_exchange(
                &inputs.authority.context,
                &inputs.bearer,
                attachments::Operation::Put {
                    workspace: upload.workspace.clone(),
                    object: upload.object,
                    descriptor_commitment: upload.commitment,
                    reservation: ticket.reservation,
                    index,
                    record: chunk,
                },
            )
            .await
            .unwrap();
    }
    client
        .image_exchange(
            &inputs.authority.context,
            &inputs.bearer,
            attachments::Operation::Complete {
                workspace: upload.workspace.clone(),
                object: upload.object,
                descriptor_commitment: upload.commitment,
                reservation: ticket.reservation,
            },
        )
        .await
        .unwrap();
    if accept_ref {
        let Reply::Appended(_) = client
            .exchange(
                &inputs.authority.context,
                &inputs.bearer,
                Operation::Append {
                    ticket: Some(ticket),
                    record: record.clone(),
                },
            )
            .await
            .unwrap()
        else {
            panic!("accepted image ref");
        };
    }
    assert_eq!(
        scalar(&f.peer, "SELECT count(*) FROM local_e2ee_outbox").await,
        1
    );
    (reference, source, record, upload.object)
}

async fn run_task_checkpoint(accepted: bool) {
    let f = fixture().await;
    let driver = join(&f, "checkpoint-driver", &f.peer, &f.peer_store).await;
    let removed = join(&f, "checkpoint-removed", &f.peer, &f.peer_store).await;
    let (operation_id, task_id, old_record) = if accepted {
        accepted_task(&f, "accepted but unacknowledged task").await
    } else {
        let workspace = f.peer.list_workspaces().await.unwrap().remove(0);
        let task = f
            .peer
            .create_task(&workspace, draft("frozen unaccepted task"))
            .await
            .unwrap()
            .task;
        let id: String = sqlx::query_scalar(
            "SELECT change_id FROM changes WHERE entity_id=? AND op_type='create_task'",
        )
        .bind(&task.id)
        .fetch_one(&mut *aven_core::test_support::acquire(&f.peer).await.unwrap())
        .await
        .unwrap();
        let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
        let record = head_record(&f.peer, &inputs.authority).await;
        (id, task.id.to_string(), record)
    };
    let driver_id = device(&removed.store, &removed.db, &f.origin).await;
    rotate_twice_while_peer_is_offline(&f, &driver, driver_id).await;

    let reopened = Database::open(f.peer.path()).await.unwrap();
    let store = isolated_store(&reopened, &f.root.path().join("peer-keys")).await;
    let client = Client::new(&f.origin).unwrap();
    drain(&client, &store, &reopened).await;
    assert_eq!(
        title(&reopened, &task_id).await,
        if accepted {
            "accepted but unacknowledged task"
        } else {
            "frozen unaccepted task"
        }
    );
    let accepted_record: Vec<u8> =
        sqlx::query_scalar("SELECT record FROM local_e2ee_accepted WHERE operation_id=?")
            .bind(&operation_id)
            .fetch_one(&mut *aven_core::test_support::acquire(&reopened).await.unwrap())
            .await
            .unwrap();
    if accepted {
        assert_eq!(accepted_record, old_record);
    } else {
        assert_ne!(accepted_record, old_record);
    }
    let membership = store
        .active_inputs(&reopened, &f.origin)
        .await
        .unwrap()
        .membership;
    assert_eq!(membership.generations().len(), 3);
    assert!(!membership.has_device(device(&f.seed_store, &f.seed, &f.origin).await));
}

#[tokio::test]
async fn offline_task_states_cross_two_real_rotations_after_database_reopen() {
    run_task_checkpoint(false).await;
    run_task_checkpoint(true).await;
}

#[tokio::test]
async fn accepted_image_ref_survives_two_rotations_and_reopen_without_corruption() {
    let f = fixture().await;
    let driver = join(&f, "image-driver", &f.peer, &f.peer_store).await;
    let removed = join(&f, "image-removed", &f.peer, &f.peer_store).await;
    let (reference, source, record, old_object) = accepted_image(&f, true).await;
    let driver_id = device(&removed.store, &removed.db, &f.origin).await;
    rotate_twice_while_peer_is_offline(&f, &driver, driver_id).await;

    let reopened = Database::open(f.peer.path()).await.unwrap();
    let store = isolated_store(&reopened, &f.root.path().join("peer-keys")).await;
    let client = Client::new(&f.origin).unwrap();
    for _ in 0..8 {
        client
            .round(&store, &reopened, &f.root.path().join("peer-blobs"))
            .await
            .unwrap();
        client
            .round(&driver.store, &driver.db, &driver.blobs)
            .await
            .unwrap();
    }
    let sha: String =
        sqlx::query_scalar("SELECT sha256 FROM task_attachments WHERE attachment_id=?")
            .bind(&reference)
            .fetch_one(&mut *aven_core::test_support::acquire(&reopened).await.unwrap())
            .await
            .unwrap();
    assert_eq!(
        std::fs::read(f.root.path().join("peer-blobs/objects/sha256").join(&sha)).unwrap(),
        source
    );
    let accepted: Vec<u8> = sqlx::query_scalar(
        "SELECT record FROM local_e2ee_accepted WHERE operation_id=(SELECT created_by_change_id FROM task_attachments WHERE attachment_id=?)",
    )
    .bind(&reference)
    .fetch_one(&mut *aven_core::test_support::acquire(&reopened).await.unwrap())
    .await
    .unwrap();
    assert_eq!(accepted, record);
    assert_eq!(
        scalar(&reopened, "SELECT count(*) FROM local_e2ee_outbox").await,
        0
    );
    let driver_sha: String =
        sqlx::query_scalar("SELECT sha256 FROM task_attachments WHERE attachment_id=?")
            .bind(&reference)
            .fetch_one(&mut *aven_core::test_support::acquire(&driver.db).await.unwrap())
            .await
            .unwrap();
    assert_eq!(
        std::fs::read(driver.blobs.join("objects/sha256").join(driver_sha)).unwrap(),
        source
    );
    let (peer_object, driver_object): (Vec<u8>, Vec<u8>) = (
        sqlx::query_scalar("SELECT object FROM local_e2ee_image_references WHERE reference=?")
            .bind(&reference)
            .fetch_one(&mut *aven_core::test_support::acquire(&reopened).await.unwrap())
            .await
            .unwrap(),
        sqlx::query_scalar("SELECT object FROM local_e2ee_image_references WHERE reference=?")
            .bind(&reference)
            .fetch_one(&mut *aven_core::test_support::acquire(&driver.db).await.unwrap())
            .await
            .unwrap(),
    );
    assert_eq!(peer_object, driver_object);
    assert_eq!(peer_object, old_object.to_vec());
}

#[tokio::test]
async fn frozen_unaccepted_image_ref_is_replaced_after_two_rotations() {
    let f = fixture().await;
    let driver = join(&f, "image-frozen-driver", &f.peer, &f.peer_store).await;
    let removed = join(&f, "image-frozen-removed", &f.peer, &f.peer_store).await;
    let (reference, source, old_record, old_object) = accepted_image(&f, false).await;
    let removed_id = device(&removed.store, &removed.db, &f.origin).await;
    rotate_twice_while_peer_is_offline(&f, &driver, removed_id).await;

    let reopened = Database::open(f.peer.path()).await.unwrap();
    let store = isolated_store(&reopened, &f.root.path().join("peer-keys")).await;
    let client = Client::new(&f.origin).unwrap();
    for _ in 0..8 {
        client
            .round(&store, &reopened, &f.root.path().join("peer-blobs"))
            .await
            .unwrap();
        client
            .round(&driver.store, &driver.db, &driver.blobs)
            .await
            .unwrap();
    }
    let accepted: Vec<u8> = sqlx::query_scalar(
        "SELECT record FROM local_e2ee_accepted WHERE operation_id=(SELECT created_by_change_id FROM task_attachments WHERE attachment_id=?)",
    )
    .bind(&reference)
    .fetch_one(&mut *aven_core::test_support::acquire(&reopened).await.unwrap())
    .await
    .unwrap();
    assert_ne!(accepted, old_record);
    let object: Vec<u8> =
        sqlx::query_scalar("SELECT object FROM local_e2ee_image_references WHERE reference=?")
            .bind(&reference)
            .fetch_one(&mut *aven_core::test_support::acquire(&reopened).await.unwrap())
            .await
            .unwrap();
    assert_ne!(object, old_object);
    let sha: String =
        sqlx::query_scalar("SELECT sha256 FROM task_attachments WHERE attachment_id=?")
            .bind(&reference)
            .fetch_one(&mut *aven_core::test_support::acquire(&driver.db).await.unwrap())
            .await
            .unwrap();
    assert_eq!(
        std::fs::read(driver.blobs.join("objects/sha256").join(sha)).unwrap(),
        source
    );
    let driver_object: Vec<u8> =
        sqlx::query_scalar("SELECT object FROM local_e2ee_image_references WHERE reference=?")
            .bind(&reference)
            .fetch_one(&mut *aven_core::test_support::acquire(&driver.db).await.unwrap())
            .await
            .unwrap();
    assert_eq!(driver_object, object);
}

#[tokio::test]
async fn lost_append_put_complete_and_manage_replies_resume_in_new_process() {
    for stage in ["Append", "Put", "Complete", "Manage"] {
        let mut f = fixture().await;
        let third = join(&f, "fault-third", &f.peer, &f.peer_store).await;
        let image = stage == "Put" || stage == "Complete";
        let (reference, image_source) = if image {
            let reference = add_image(&f).await;
            let sha: String =
                sqlx::query_scalar("SELECT sha256 FROM task_attachments WHERE attachment_id=?")
                    .bind(&reference)
                    .fetch_one(&mut *aven_core::test_support::acquire(&f.peer).await.unwrap())
                    .await
                    .unwrap();
            let source =
                std::fs::read(f.root.path().join("peer-blobs/objects/sha256").join(sha)).unwrap();
            (Some(reference), Some(source))
        } else {
            (None, None)
        };
        if stage == "Append" {
            let workspace = f.peer.list_workspaces().await.unwrap().remove(0);
            f.peer
                .create_task(&workspace, draft("lost append reply"))
                .await
                .unwrap();
        }
        let fault = Arc::new(HttpFault {
            lose: Some(stage),
            remaining: AtomicUsize::new(1),
            bodies: Default::default(),
        });
        restart_server(&mut f, fault.clone()).await;
        if stage == "Manage" {
            let target = device(&third.store, &third.db, &f.origin).await;
            assert!(
                peer_enrollment_http::Client::new(&f.origin)
                    .unwrap()
                    .remove_device(&f.peer_store, &f.peer, target)
                    .await
                    .is_err()
            );
        } else if image {
            let result = Client::new(&f.origin)
                .unwrap()
                .round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
                .await;
            assert!(result.is_err() || result.unwrap().images == ImageTransfer::Failed);
        } else {
            assert!(
                Client::new(&f.origin)
                    .unwrap()
                    .round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
                    .await
                    .is_err()
            );
        }
        assert_eq!(fault.remaining.load(Ordering::SeqCst), 0);
        restart_server(&mut f, fault.clone()).await;
        let output =
            e2ee_http::worker("encrypted_tail_http::tests::checkpoint_faults::recovery_worker")
                .env("AVEN_CHECKPOINT_ROOT", f.root.path())
                .env("AVEN_CHECKPOINT_ORIGIN", &f.origin)
                .output()
                .await
                .unwrap();
        assert_eq!(
            output.status.code(),
            Some(84),
            "{stage}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let reopened = Database::open(f.peer.path()).await.unwrap();
        let store = isolated_store(&reopened, &f.root.path().join("peer-keys")).await;
        if stage == "Manage" {
            let target = device(&third.store, &third.db, &f.origin).await;
            let m = store
                .active_inputs(&reopened, &f.origin)
                .await
                .unwrap()
                .membership;
            assert!(!m.has_device(target));
            assert!(
                Client::new(&f.origin)
                    .unwrap()
                    .round(&third.store, &third.db, &third.blobs)
                    .await
                    .is_err()
            );
            let manage_count = {
                let bodies = fault.bodies.lock().unwrap();
                bodies
                    .iter()
                    .filter(|(operation, _)| operation == "Manage")
                    .count()
            };
            assert_eq!(
                manage_count, 2,
                "recovery must advance from Revoke to Rotate"
            );
        } else if image {
            let (attempt_count, first_operation, second_operation) = {
                let bodies = fault.bodies.lock().unwrap();
                let attempts = bodies
                    .iter()
                    .filter(|(operation, _)| operation == stage)
                    .collect::<Vec<_>>();
                (
                    attempts.len(),
                    attempts.first().map(|(_, bytes)| operation_value(bytes)),
                    attempts.get(1).map(|(_, bytes)| operation_value(bytes)),
                )
            };
            assert!(attempt_count > 0);
            if let (Some(first), Some(second)) = (first_operation, second_operation) {
                assert_eq!(first, second);
            }
            for _ in 0..8 {
                Client::new(&f.origin)
                    .unwrap()
                    .round(&store, &reopened, &f.root.path().join("peer-blobs"))
                    .await
                    .unwrap();
                Client::new(&f.origin)
                    .unwrap()
                    .round(&third.store, &third.db, &third.blobs)
                    .await
                    .unwrap();
            }
            let reference = reference.unwrap();
            let sha: String =
                sqlx::query_scalar("SELECT sha256 FROM task_attachments WHERE attachment_id=?")
                    .bind(reference)
                    .fetch_one(&mut *aven_core::test_support::acquire(&reopened).await.unwrap())
                    .await
                    .unwrap();
            let downloaded = f
                .root
                .path()
                .join("fault-third-blobs/objects/sha256")
                .join(sha);
            assert_eq!(std::fs::read(downloaded).unwrap(), image_source.unwrap());
        } else {
            let (attempt_count, first_operation, second_operation) = {
                let bodies = fault.bodies.lock().unwrap();
                let attempts = bodies
                    .iter()
                    .filter(|(operation, _)| operation == "Append")
                    .collect::<Vec<_>>();
                (
                    attempts.len(),
                    attempts.first().map(|(_, bytes)| operation_value(bytes)),
                    attempts.get(1).map(|(_, bytes)| operation_value(bytes)),
                )
            };
            assert!(attempt_count > 0);
            if let (Some(first), Some(second)) = (first_operation, second_operation) {
                assert_eq!(first, second);
            }
            drain(&Client::new(&f.origin).unwrap(), &store, &reopened).await;
            assert_eq!(
                scalar(
                    &reopened,
                    "SELECT count(*) FROM changes WHERE server_seq IS NULL"
                )
                .await,
                0
            );
        }
    }
}

#[tokio::test]
#[ignore = "checkpoint fault process-boundary worker"]
async fn recovery_worker() {
    let root = std::path::PathBuf::from(std::env::var_os("AVEN_CHECKPOINT_ROOT").unwrap());
    let origin = std::env::var("AVEN_CHECKPOINT_ORIGIN").unwrap();
    let db = Database::open(&root.join("peer.sqlite")).await.unwrap();
    let store = isolated_store(&db, &root.join("peer-keys")).await;
    let client = Client::new(&origin).unwrap();
    client
        .round(&store, &db, &root.join("peer-blobs"))
        .await
        .unwrap();
    std::process::exit(84);
}

fn expect_error<T>(result: anyhow::Result<T>, message: &str) -> anyhow::Error {
    match result {
        Ok(_) => panic!("{message}"),
        Err(error) => error,
    }
}

fn assert_removed(error: anyhow::Error) {
    assert!(
        !error.is::<aven_core::sync::seed_claim::membership::StaleContext>(),
        "removed credentials must not receive stale context: {error:#}"
    );
}

#[tokio::test]
async fn removed_credentials_cannot_retry_history_or_protected_routes_and_keep_plaintext() {
    let f = fixture().await;
    let enrollment = peer_enrollment_http::Client::new(&f.origin).unwrap();
    let old = f
        .seed_store
        .active_inputs(&f.seed, &f.origin)
        .await
        .unwrap();
    let seed_id = old.device();
    drop(old);
    let old_inputs = f.seed_store.tail_inputs(&f.seed, &f.origin).await.unwrap();
    let old_context = old_inputs.authority.context.clone();
    let old_after = f
        .seed
        .encrypted_round_state(&old_inputs.authority)
        .await
        .unwrap()
        .cursor;
    let old_bearer = Secret::new(*old_inputs.bearer.expose());
    drop(old_inputs);
    let client = Client::new(&f.origin).unwrap();
    let lookup_id: String =
        sqlx::query_scalar("SELECT operation_id FROM server_bootstrap_prefix LIMIT 1")
            .fetch_one(&mut *aven_core::test_support::acquire(&f.server).await.unwrap())
            .await
            .unwrap();
    let (image_workspace, object_bytes, descriptor): (String, Vec<u8>, Vec<u8>) = sqlx::query_as(
        "SELECT r.workspace,i.object,i.descriptor FROM server_e2ee_image_references r JOIN server_e2ee_images i ON i.object=r.object WHERE i.bootstrap IS NOT NULL LIMIT 1",
    )
    .fetch_one(&mut *aven_core::test_support::acquire(&f.server).await.unwrap())
    .await
    .unwrap();
    let object: [u8; 32] = object_bytes.try_into().unwrap();
    let commitment: [u8; 32] = sha2::Sha256::digest(&descriptor).into();
    assert!(
        client
            .exchange(
                &old_context,
                &old_bearer,
                Operation::Pull {
                    after: old_after,
                    limit: 1,
                    watermark: None,
                },
            )
            .await
            .is_ok()
    );
    assert!(
        client
            .exchange(
                &old_context,
                &old_bearer,
                Operation::Lookup {
                    operation_id: lookup_id.clone(),
                    expected: None,
                },
            )
            .await
            .is_ok()
    );
    assert!(
        client
            .image_exchange(
                &old_context,
                &old_bearer,
                attachments::Operation::Status {
                    workspace: image_workspace.clone(),
                    object,
                    descriptor_commitment: commitment,
                },
            )
            .await
            .is_ok()
    );
    assert!(
        client
            .image_exchange(
                &old_context,
                &old_bearer,
                attachments::Operation::Read {
                    workspace: image_workspace.clone(),
                    object,
                    descriptor_commitment: commitment,
                    index: 0,
                },
            )
            .await
            .is_ok()
    );
    assert!(
        seed_bootstrap_http::Client::new(&f.origin)
            .unwrap()
            .resume(&f.seed_store, &f.seed)
            .await
            .is_ok()
    );

    let workspace = f.seed.list_workspaces().await.unwrap().remove(0);
    let local_task = f
        .seed
        .create_task(&workspace, draft("plaintext remains after removal"))
        .await
        .unwrap()
        .task;
    enrollment
        .remove_device(&f.peer_store, &f.peer, seed_id)
        .await
        .unwrap();
    for operation in [
        Operation::Pull {
            after: old_after,
            limit: 1,
            watermark: None,
        },
        Operation::Lookup {
            operation_id: lookup_id,
            expected: None,
        },
        Operation::Append {
            ticket: None,
            record: vec![],
        },
    ] {
        assert_removed(
            client
                .exchange(&old_context, &old_bearer, operation)
                .await
                .err()
                .expect("removed tail request unexpectedly succeeded"),
        );
    }
    for operation in [
        attachments::Operation::Status {
            workspace: image_workspace.clone(),
            object,
            descriptor_commitment: commitment,
        },
        attachments::Operation::Put {
            workspace: image_workspace.clone(),
            object,
            descriptor_commitment: commitment,
            reservation: [0; 32],
            index: 0,
            record: vec![],
        },
        attachments::Operation::Complete {
            workspace: image_workspace.clone(),
            object,
            descriptor_commitment: commitment,
            reservation: [0; 32],
        },
        attachments::Operation::Read {
            workspace: image_workspace.clone(),
            object,
            descriptor_commitment: commitment,
            index: 0,
        },
        attachments::Operation::Release {
            workspace: image_workspace,
            object,
            descriptor_commitment: commitment,
            reservation: [0; 32],
        },
        attachments::Operation::Declare {
            workspace: "removed-device".into(),
            descriptor: vec![],
        },
    ] {
        assert_removed(
            client
                .image_exchange(&old_context, &old_bearer, operation)
                .await
                .err()
                .expect("removed image request unexpectedly succeeded"),
        );
    }
    assert_removed(expect_error(
        enrollment.refresh(&f.seed_store, &f.seed).await,
        "removed membership refresh unexpectedly succeeded",
    ));
    assert_removed(expect_error(
        seed_bootstrap_http::Client::new(&f.origin)
            .unwrap()
            .resume(&f.seed_store, &f.seed)
            .await,
        "removed bootstrap resume unexpectedly succeeded",
    ));
    assert_eq!(
        title(&f.seed, local_task.id.as_str()).await,
        "plaintext remains after removal"
    );
    f.seed
        .update_task(
            &workspace,
            &local_task.id,
            TaskUpdate {
                title: Some("local write still works".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        title(&f.seed, local_task.id.as_str()).await,
        "local write still works"
    );
}

#[tokio::test]
async fn pruned_bootstrap_image_does_not_block_fresh_post_rotation_metadata() {
    let mut f = fixture().await;
    let driver = join(&f, "prune-driver", &f.peer, &f.peer_store).await;
    let removed = join(&f, "prune-removed", &f.peer, &f.peer_store).await;
    let workspace = f.peer.list_workspaces().await.unwrap().remove(0);
    let reference: String =
        sqlx::query_scalar("SELECT attachment_id FROM task_attachments LIMIT 1")
            .fetch_one(&mut *aven_core::test_support::acquire(&f.peer).await.unwrap())
            .await
            .unwrap();
    f.peer
        .delete_task_attachment(&workspace, &reference)
        .await
        .unwrap();
    let client = Client::new(&f.origin).unwrap();
    client
        .round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
        .await
        .unwrap();
    let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
    drop(inputs);
    let mut policy = crate::config::AttachmentLifecycleConfig::default().server_policy();
    policy.grace = std::time::Duration::ZERO;
    restart_server_with_policy(&mut f, no_fault(), policy).await;
    let pruned = f
        .server
        .prune_encrypted_images(policy.grace, 128)
        .await
        .unwrap();
    assert_eq!(pruned, 1);
    let removed_id = device(&removed.store, &removed.db, &f.origin).await;
    rotate_twice_while_peer_is_offline(&f, &driver, removed_id).await;
    let fresh = join(&f, "prune-fresh", &driver.db, &driver.store).await;
    assert_eq!(
        scalar(&fresh.db, "SELECT count(*) FROM tasks").await,
        scalar(&f.peer, "SELECT count(*) FROM tasks").await
    );
    assert_eq!(
        scalar(
            &fresh.db,
            "SELECT count(*) FROM tasks WHERE title='PRIVATE-HTTP-SEED-TASK'",
        )
        .await,
        1
    );
    let result = client
        .round(&fresh.store, &fresh.db, &fresh.blobs)
        .await
        .unwrap();
    assert!(result.metadata_caught_up);
    assert_ne!(result.images, ImageTransfer::Failed);
}

mod withdrawal;
