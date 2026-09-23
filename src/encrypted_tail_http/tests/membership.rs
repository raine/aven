use super::*;
use crate::peer_enrollment_http;
use aven_core::sync::seed_claim::membership::Membership;

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
    keys: &ProtectedLocalKeyStore,
) -> Joined {
    let client = peer_enrollment_http::Client::new(&f.origin).unwrap();
    let db = Database::open(&f.root.path().join(format!("{name}.sqlite")))
        .await
        .unwrap();
    let store = isolated_store(db.path(), &f.root.path().join(format!("{name}-keys")));
    let invitation = client.invite(keys, inviter, expiry()).await.unwrap();
    client.request(&store, &db, Some(invitation)).await.unwrap();
    assert!(client.admit(keys, inviter).await.unwrap());
    assert!(client.complete(&store, &db).await.unwrap());
    client.install(&store, &db).await.unwrap();
    Joined {
        db,
        store,
        blobs: f.root.path().join(format!("{name}-blobs")),
    }
}
async fn floor(store: &ProtectedLocalKeyStore, db: &Database, origin: &str) -> Membership {
    store
        .active_inputs(db, origin)
        .await
        .unwrap()
        .membership
        .clone()
}
async fn converge_three(f: &Fixture, third: &Joined) {
    let client = Client::new(&f.origin).unwrap();
    for _ in 0..2 {
        for (db, keys) in [
            (&f.seed, &f.seed_store),
            (&f.peer, &f.peer_store),
            (&third.db, &third.store),
        ] {
            drain(&client, keys, db).await;
        }
    }
}
async fn exercise_equal_peers(peer_invites: bool) {
    let f = fixture().await;
    let original_peer = floor(&f.peer_store, &f.peer, &f.origin).await;
    let third = if peer_invites {
        join(&f, "third", &f.peer, &f.peer_store).await
    } else {
        join(&f, "third", &f.seed, &f.seed_store).await
    };
    assert_eq!(
        floor(&third.store, &third.db, &f.origin).await.sequence(),
        3
    );
    let client = Client::new(&f.origin).unwrap();
    let mut task_ids = Vec::new();
    let mut devices = Vec::new();
    let mut bearers = Vec::new();
    for (db, keys) in [
        (&f.seed, &f.seed_store),
        (&f.peer, &f.peer_store),
        (&third.db, &third.store),
    ] {
        let inputs = keys.active_inputs(db, &f.origin).await.unwrap();
        devices.push(inputs.device());
        bearers.push(*inputs.bearer().expose());
        drop(inputs);
        let w = db.list_workspaces().await.unwrap().remove(0);
        let task = db
            .create_task(&w, draft("three independent writers"))
            .await
            .unwrap()
            .task;
        db.add_note(&w, &task.id, "independent note".into())
            .await
            .unwrap();
        task_ids.push(task.id);
        recurrence::create(db).await;
    }
    assert!(devices[0] != devices[1] && devices[0] != devices[2] && devices[1] != devices[2]);
    assert!(bearers[0] != bearers[1] && bearers[0] != bearers[2] && bearers[1] != bearers[2]);
    converge_three(&f, &third).await;
    for (db, keys) in [
        (&f.seed, &f.seed_store),
        (&f.peer, &f.peer_store),
        (&third.db, &third.store),
    ] {
        assert!(floor(keys, db, &f.origin).await.extends(&original_peer));
        assert_eq!(floor(keys, db, &f.origin).await.sequence(), 3);
        for task in &task_ids {
            assert_eq!(title(db, task.as_str()).await, "three independent writers");
        }
        assert_eq!(
            scalar(
                db,
                "SELECT count(*) FROM notes WHERE body='independent note'"
            )
            .await,
            3
        );
        assert_eq!(
            scalar(db, "SELECT count(*) FROM recurrence_series").await,
            3
        );
    }
    // Each independent installation uploads an ordinary post-admission image.
    for (index, (db, _, blobs)) in [
        (&f.seed, &f.seed_store, f.root.path().join("blobs")),
        (&f.peer, &f.peer_store, f.root.path().join("peer-blobs")),
        (&third.db, &third.store, third.blobs.clone()),
    ]
    .into_iter()
    .enumerate()
    {
        let w = db.list_workspaces().await.unwrap().remove(0);
        let mut bytes = std::io::Cursor::new(Vec::new());
        ::image::DynamicImage::new_rgb8(5 + index as u32, 4)
            .write_to(&mut bytes, ::image::ImageFormat::Png)
            .unwrap();
        db.add_task_attachment(
            &w,
            &blobs,
            Default::default(),
            &task_ids[index],
            aven_core::operations::AttachmentAddInput {
                filename: None,
                alt_text: None,
                declared_media_type: None,
                bytes: bytes.into_inner(),
                optimization_policy: aven_core::attachments::ImageOptimizationPolicy::Preserve,
                dedupe_existing: false,
            },
        )
        .await
        .unwrap();
    }
    for _ in 0..12 {
        for (db, keys, blobs) in [
            (&f.seed, &f.seed_store, f.root.path().join("blobs")),
            (&f.peer, &f.peer_store, f.root.path().join("peer-blobs")),
            (&third.db, &third.store, third.blobs.clone()),
        ] {
            client.attachment_round(keys, db, &blobs).await.unwrap();
        }
    }
    for db in [&f.seed, &f.peer, &third.db] {
        assert_eq!(
            scalar(db, "SELECT count(*) FROM task_attachments WHERE deleted=0").await,
            4
        );
    }
    for (db, keys, blobs) in [
        (&f.seed, &f.seed_store, f.root.path().join("blobs")),
        (&f.peer, &f.peer_store, f.root.path().join("peer-blobs")),
        (&third.db, &third.store, third.blobs.clone()),
    ] {
        assert_eq!(
            client
                .attachment_round(keys, db, &blobs)
                .await
                .unwrap()
                .images,
            ImageTransfer::Complete
        );
    }
    let enrollment = peer_enrollment_http::Client::new(&f.origin).unwrap();
    let before = scalar(
        &f.peer,
        "SELECT CAST(value AS INTEGER) FROM meta WHERE key='sync_cursor'",
    )
    .await;
    assert!(enrollment.complete(&f.peer_store, &f.peer).await.unwrap());
    enrollment.install(&f.peer_store, &f.peer).await.unwrap();
    assert_eq!(
        scalar(
            &f.peer,
            "SELECT CAST(value AS INTEGER) FROM meta WHERE key='sync_cursor'"
        )
        .await,
        before
    );
    assert_eq!(floor(&f.peer_store, &f.peer, &f.origin).await.sequence(), 3);
    // Being installed cannot hide an unfinished outbound invitation.
    enrollment
        .invite(&third.store, &third.db, expiry())
        .await
        .unwrap();
    assert!(client.round(&third.store, &third.db).await.is_err());
    assert_eq!(
        third.store.enrollment_readiness(&third.db).await.unwrap(),
        crate::protected_local_keys::EnrollmentReadiness::Pending
    );
}
#[tokio::test]
async fn third_invited_by_seed_continues_real_content_and_images() {
    exercise_equal_peers(false).await;
}
#[tokio::test]
async fn third_invited_by_peer_continues_real_content_and_images() {
    exercise_equal_peers(true).await;
}

#[tokio::test]
async fn old_head_lost_append_and_original_enrollment_retry_preserve_exact_work() {
    let mut f = fixture().await;
    let enrollment = peer_enrollment_http::Client::new(&f.origin).unwrap();
    let client = Client::new(&f.origin).unwrap();
    let w = f.peer.list_workspaces().await.unwrap().remove(0);
    f.peer
        .create_task(&w, draft("accepted before third"))
        .await
        .unwrap();
    restart_fault_server(
        &mut f,
        Arc::new(HttpFault {
            reads: Default::default(),
            lose: Some("Append"),
            pause_operation: "Append",
            remaining: 1.into(),
            pause: None,
        }),
    )
    .await;
    let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
    let record = f
        .peer
        .prepare_encrypted_tail(&inputs.authority)
        .await
        .unwrap()
        .unwrap();
    let (id, _) = f
        .peer
        .encrypted_tail_frozen_record(&inputs.authority)
        .await
        .unwrap()
        .unwrap();
    assert!(
        client
            .exchange(
                &inputs.authority.context,
                &inputs.bearer,
                Operation::Append {
                    ticket: None,
                    record: record.clone()
                }
            )
            .await
            .is_err()
    );
    let Reply::Found(accepted) = client
        .exchange(
            &inputs.authority.context,
            &inputs.bearer,
            Operation::Lookup {
                operation_id: id,
                expected: None,
            },
        )
        .await
        .unwrap()
    else {
        panic!("accepted append");
    };
    assert_eq!(accepted.record, record);
    let mapping = accepted.mapping;
    // Inspect the committed result without acknowledging it in the local outbox.
    drop(inputs);
    let third = join(&f, "third", &f.seed, &f.seed_store).await;
    let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
    assert_eq!(inputs.authority.context.head, floor_head(&f.peer).await);
    assert_eq!(
        f.peer
            .prepare_encrypted_tail(&inputs.authority)
            .await
            .unwrap()
            .unwrap(),
        record
    );
    assert!(
        client
            .exchange(
                &inputs.authority.context,
                &inputs.bearer,
                Operation::Append {
                    ticket: None,
                    record: record.clone()
                }
            )
            .await
            .err()
            .unwrap()
            .is::<aven_core::sync::seed_claim::membership::StaleContext>()
    );
    drop(inputs);
    converge_three(&f, &third).await;
    let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
    let Reply::Appended(retry) = client
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
        panic!("append");
    };
    assert!(mapping == retry);
    drop(inputs);
    let peer = f
        .peer_store
        .prepare_peer(&f.peer, &f.origin, None)
        .await
        .unwrap();
    let original_identity = peer.protected_storage_bytes();
    let before = floor(&f.peer_store, &f.peer, &f.origin).await;
    // A fourth actual admission exercises completion of earlier outcomes at later heads.
    let fourth = join(&f, "fourth", &third.db, &third.store).await;
    assert_eq!(
        floor(&fourth.store, &fourth.db, &f.origin).await.sequence(),
        4
    );
    enrollment.complete(&f.peer_store, &f.peer).await.unwrap();
    assert!(
        floor(&f.peer_store, &f.peer, &f.origin)
            .await
            .extends(&before)
    );
    assert_eq!(
        f.peer_store
            .prepare_peer(&f.peer, &f.origin, None)
            .await
            .unwrap()
            .protected_storage_bytes()
            .as_slice(),
        original_identity.as_slice()
    );
    enrollment.install(&f.peer_store, &f.peer).await.unwrap();
}
async fn floor_head(db: &Database) -> [u8; 32] {
    db.membership_checkpoint_mirror().await.unwrap().unwrap().2
}

struct HttpFault {
    reads: std::sync::Mutex<Vec<serde_json::Value>>,
    lose: Option<&'static str>,
    pause_operation: &'static str,
    remaining: std::sync::atomic::AtomicUsize,
    pause: Option<tokio::sync::mpsc::Sender<tokio::sync::oneshot::Sender<()>>>,
}
async fn fault_request(
    State(fault): State<Arc<HttpFault>>,
    request: Request,
    next: axum::middleware::Next,
) -> Response {
    let (parts, body) = request.into_parts();
    let bytes = to_bytes(body, 5 * 1024 * 1024).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let operation = value
        .get("operation")
        .unwrap_or(&value)
        .as_object()
        .and_then(|o| o.keys().next())
        .map(String::as_str);
    if operation == Some("Read") {
        fault
            .reads
            .lock()
            .unwrap()
            .push(value["operation"]["Read"]["object"].clone());
    }
    let matches = (fault.lose.is_some() && operation == fault.lose)
        || (fault.pause.is_some()
            && operation == Some(fault.pause_operation)
            && (operation != Some("Published") || !value["Published"]["component"].is_null()));
    let intercept = matches
        && fault
            .remaining
            .fetch_update(
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
                |n| n.checked_sub(1),
            )
            .is_ok();
    if intercept && let Some(events) = &fault.pause {
        let (send, receive) = tokio::sync::oneshot::channel();
        events.send(send).await.unwrap();
        receive.await.unwrap();
    }
    let response = next
        .run(Request::from_parts(parts, axum::body::Body::from(bytes)))
        .await;
    if intercept && fault.lose.is_some() && response.status() == StatusCode::OK {
        (StatusCode::BAD_GATEWAY, "test-lost-response").into_response()
    } else {
        response
    }
}
async fn restart_fault_server(f: &mut Fixture, fault: Arc<HttpFault>) {
    f.task.abort();
    let _ = (&mut f.task).await;
    let listener = tokio::net::TcpListener::bind(f.origin.strip_prefix("http://").unwrap())
        .await
        .unwrap();
    let app = peer_enrollment_http::router(f.server.clone())
        .merge(router(f.server.clone()))
        .layer(axum::middleware::from_fn_with_state(fault, fault_request));
    f.task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
}
async fn lost_image_response(stage: &'static str) {
    use aven_core::sync::encrypted_tail::attachments::{
        Operation as Op, Reply as ImageReply, Ticket,
    };
    let mut f = fixture().await;
    attachments::add_image(&f).await;
    let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
    let upload = f
        .peer
        .prepare_encrypted_image(&inputs.authority, &f.root.path().join("peer-blobs"))
        .await
        .unwrap()
        .unwrap();
    let record = f
        .peer
        .prepare_encrypted_tail(&inputs.authority)
        .await
        .unwrap()
        .unwrap();
    let (id, _) = f
        .peer
        .encrypted_tail_frozen_record(&inputs.authority)
        .await
        .unwrap()
        .unwrap();
    let context = inputs.authority.context.clone();
    let bearer = Secret::new(*inputs.bearer.expose());
    drop(inputs);
    let fault = Arc::new(HttpFault {
        reads: Default::default(),
        pause_operation: "Append",
        lose: Some(stage),
        remaining: 1.into(),
        pause: None,
    });
    restart_fault_server(&mut f, fault.clone()).await;
    let client = Client::new(&f.origin).unwrap();
    let ImageReply::Status(status) = client
        .image_exchange(
            &context,
            &bearer,
            Op::Declare {
                workspace: upload.workspace.clone(),
                descriptor: upload.descriptor.clone(),
            },
        )
        .await
        .unwrap()
    else {
        panic!("declare");
    };
    let ticket = Ticket {
        epoch: status.epoch,
        reservation: status.reservation.unwrap(),
    };
    let put = || Op::Put {
        workspace: upload.workspace.clone(),
        object: upload.object,
        descriptor_commitment: upload.commitment,
        epoch: ticket.epoch,
        reservation: ticket.reservation,
        index: 0,
        record: upload.records[0].clone(),
    };
    let complete = || Op::Complete {
        workspace: upload.workspace.clone(),
        object: upload.object,
        descriptor_commitment: upload.commitment,
        epoch: ticket.epoch,
        reservation: ticket.reservation,
    };
    let result = client.image_exchange(&context, &bearer, put()).await;
    if stage == "Put" {
        assert!(result.is_err());
    } else {
        result.unwrap();
        let result = client.image_exchange(&context, &bearer, complete()).await;
        if stage == "Complete" {
            assert!(result.is_err());
        } else {
            result.unwrap();
            assert!(
                client
                    .exchange(
                        &context,
                        &bearer,
                        Operation::Append {
                            ticket: Some(ticket.clone()),
                            record: record.clone()
                        }
                    )
                    .await
                    .is_err()
            );
        }
    }
    assert_eq!(fault.remaining.load(std::sync::atomic::Ordering::SeqCst), 0);
    let third = join(&f, "third", &f.seed, &f.seed_store).await;
    peer_enrollment_http::Client::new(&f.origin)
        .unwrap()
        .refresh(&f.peer_store, &f.peer)
        .await
        .unwrap();
    let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
    assert_eq!(
        f.peer
            .prepare_encrypted_tail(&inputs.authority)
            .await
            .unwrap()
            .unwrap(),
        record
    );
    if stage != "Append" {
        // The original device-owned ticket and epoch survive head advancement.
        client
            .image_exchange(&inputs.authority.context, &inputs.bearer, put())
            .await
            .unwrap();
        client
            .image_exchange(&inputs.authority.context, &inputs.bearer, complete())
            .await
            .unwrap();
    }
    drop(inputs);
    for _ in 0..8 {
        client
            .attachment_round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
            .await
            .unwrap();
        client
            .attachment_round(&third.store, &third.db, &third.blobs)
            .await
            .unwrap();
        client
            .attachment_round(&f.seed_store, &f.seed, &f.root.path().join("blobs"))
            .await
            .unwrap();
    }
    let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
    let Reply::Found(accepted) = client
        .exchange(
            &inputs.authority.context,
            &inputs.bearer,
            Operation::Lookup {
                operation_id: id,
                expected: None,
            },
        )
        .await
        .unwrap()
    else {
        panic!("accepted");
    };
    assert_eq!(accepted.record, record);
    let ImageReply::Chunk(bytes) = client
        .image_exchange(
            &inputs.authority.context,
            &inputs.bearer,
            Op::Read {
                workspace: upload.workspace,
                object: upload.object,
                descriptor_commitment: upload.commitment,
                index: 0,
            },
        )
        .await
        .unwrap()
    else {
        panic!("image");
    };
    assert_eq!(bytes, upload.records[0]);
    drop(inputs);
    assert_eq!(
        client
            .attachment_round(&third.store, &third.db, &third.blobs)
            .await
            .unwrap()
            .images,
        ImageTransfer::Complete
    );
}
#[tokio::test]
async fn lost_http_put_reply_reuses_recipe_and_ticket_after_membership_advance() {
    lost_image_response("Put").await;
}
#[tokio::test]
async fn lost_http_complete_reply_reuses_recipe_and_ticket_after_membership_advance() {
    lost_image_response("Complete").await;
}
#[tokio::test]
async fn lost_http_ref_reply_reuses_accepted_record_after_membership_advance() {
    lost_image_response("Append").await;
}

async fn stale_round_race(count: usize) {
    let mut f = fixture().await;
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    f.seed.create_task(&w, draft("head race")).await.unwrap();
    let inputs = f.seed_store.tail_inputs(&f.seed, &f.origin).await.unwrap();
    let original = f
        .seed
        .prepare_encrypted_tail(&inputs.authority)
        .await
        .unwrap()
        .unwrap();
    drop(inputs);
    let (events, mut incoming) = tokio::sync::mpsc::channel(1);
    let fault = Arc::new(HttpFault {
        reads: Default::default(),
        pause_operation: "Append",
        lose: None,
        remaining: count.into(),
        pause: Some(events),
    });
    restart_fault_server(&mut f, fault.clone()).await;
    let client = Client::new(&f.origin).unwrap();
    let (result, ()) = tokio::join!(client.round(&f.seed_store, &f.seed), async {
        for index in 0..count {
            let resume = incoming.recv().await.unwrap();
            let joined = join(&f, &format!("competitor-{index}"), &f.peer, &f.peer_store).await;
            assert_eq!(
                floor(&joined.store, &joined.db, &f.origin).await.sequence(),
                3 + index as u64
            );
            resume.send(()).unwrap();
        }
    });
    assert_eq!(fault.remaining.load(std::sync::atomic::Ordering::SeqCst), 0);
    if count == 1 {
        result.unwrap();
    } else {
        assert!(
            result
                .err()
                .unwrap()
                .is::<aven_core::sync::seed_claim::membership::StaleContext>()
        );
        let inputs = f.seed_store.tail_inputs(&f.seed, &f.origin).await.unwrap();
        assert_eq!(
            f.seed
                .prepare_encrypted_tail(&inputs.authority)
                .await
                .unwrap()
                .unwrap(),
            original
        );
        drop(inputs);
    }
    drain(&client, &f.seed_store, &f.seed).await;
    drain(&client, &f.peer_store, &f.peer).await;
    assert_eq!(
        scalar(
            &f.peer,
            "SELECT count(*) FROM tasks WHERE title='head race'"
        )
        .await,
        1
    );
}
#[tokio::test]
async fn head_race_between_refresh_and_append_retries_once() {
    stale_round_race(1).await;
}
#[tokio::test]
async fn second_head_race_stops_with_exact_pending_ciphertext() {
    stale_round_race(2).await;
}

#[tokio::test]
async fn competing_host_candidates_retain_same_recipient_and_complete_after_later_head() {
    let f = fixture().await;
    let client = peer_enrollment_http::Client::new(&f.origin).unwrap();
    let third = Database::open(&f.root.path().join("third.sqlite"))
        .await
        .unwrap();
    let third_keys = isolated_store(third.path(), &f.root.path().join("third-keys"));
    let fourth = Database::open(&f.root.path().join("fourth.sqlite"))
        .await
        .unwrap();
    let fourth_keys = isolated_store(fourth.path(), &f.root.path().join("fourth-keys"));
    let seed_invitation = client
        .invite(&f.seed_store, &f.seed, expiry())
        .await
        .unwrap();
    let peer_invitation = client
        .invite(&f.peer_store, &f.peer, expiry())
        .await
        .unwrap();
    let peer_handle = peer_invitation.handle();
    client
        .request(&third_keys, &third, Some(seed_invitation))
        .await
        .unwrap();
    client
        .request(&fourth_keys, &fourth, Some(peer_invitation))
        .await
        .unwrap();
    let third_identity = third_keys
        .prepare_peer(&third, &f.origin, None)
        .await
        .unwrap();
    let fourth_identity = fourth_keys
        .prepare_peer(&fourth, &f.origin, None)
        .await
        .unwrap();
    let original_fourth = fourth_identity.protected_storage_bytes();
    let mut candidates = Vec::new();
    let mut alternate = None;
    for (db, keys, request) in [
        (&f.seed, &f.seed_store, third_identity.request()),
        (&f.peer, &f.peer_store, fourth_identity.request()),
    ] {
        let inputs = keys.active_inputs(db, &f.origin).await.unwrap();
        let journal = keys
            .prepare_invitation(db, &inputs, None, None)
            .await
            .unwrap();
        candidates.push(
            keys.prepare_admission(db, &inputs, &journal, request)
                .await
                .unwrap(),
        );
        if db.path() == f.peer.path() {
            let mut evidence = inputs.evidence.clone();
            evidence
                .admissions
                .push(aven_core::sync::seed_claim::membership::EvidenceRecord {
                    declaration: journal.declaration.clone(),
                    request: request.to_vec(),
                    record: candidates.last().unwrap().clone(),
                });
            evidence.verify().unwrap();
            alternate = Some(evidence);
        }
    }
    assert_ne!(candidates[0], candidates[1]);
    assert!(
        Client::new(&f.origin)
            .unwrap()
            .round(&f.peer_store, &f.peer)
            .await
            .is_err()
    );
    assert_eq!(
        f.peer_store.enrollment_readiness(&f.peer).await.unwrap(),
        crate::protected_local_keys::EnrollmentReadiness::UnresolvedDisclosure
    );
    client.complete(&f.peer_store, &f.peer).await.unwrap();
    assert_eq!(
        f.peer_store.enrollment_readiness(&f.peer).await.unwrap(),
        crate::protected_local_keys::EnrollmentReadiness::UnresolvedDisclosure
    );
    client.admit(&f.seed_store, &f.seed).await.unwrap();
    client.admit(&f.peer_store, &f.peer).await.unwrap();
    {
        let mut inputs = f
            .peer_store
            .active_inputs(&f.peer, &f.origin)
            .await
            .unwrap();
        let before = f.peer.membership_checkpoint_mirror().await.unwrap();
        assert!(
            f.peer_store
                .adopt_refresh(&f.peer, &mut inputs, alternate.unwrap())
                .await
                .is_err()
        );
        let mut rollback = inputs.evidence.clone();
        rollback.admissions.clear();
        assert!(
            f.peer_store
                .adopt_refresh(&f.peer, &mut inputs, rollback)
                .await
                .is_err()
        );
        let mut missing = inputs.evidence.clone();
        missing.admissions.remove(0);
        assert!(
            f.peer_store
                .adopt_refresh(&f.peer, &mut inputs, missing)
                .await
                .is_err()
        );
        assert_eq!(f.peer.membership_checkpoint_mirror().await.unwrap(), before);
    }
    let accepted = f
        .server
        .membership_mailbox(fourth_identity.vault(), peer_handle)
        .await
        .unwrap();
    assert_ne!(accepted.admission.as_deref().unwrap(), candidates[1]);
    assert_eq!(
        accepted.request.as_deref().unwrap(),
        fourth_identity.request()
    );
    client.complete(&third_keys, &third).await.unwrap();
    client.complete(&fourth_keys, &fourth).await.unwrap();
    client.install(&third_keys, &third).await.unwrap();
    client.install(&fourth_keys, &fourth).await.unwrap();
    for (db, keys) in [(&third, &third_keys), (&fourth, &fourth_keys)] {
        assert_eq!(floor(keys, db, &f.origin).await.sequence(), 4);
    }
    assert_eq!(
        fourth_keys
            .prepare_peer(&fourth, &f.origin, None)
            .await
            .unwrap()
            .protected_storage_bytes()
            .as_slice(),
        original_fourth.as_slice()
    );
    let entries = std::fs::read_dir(f.root.path().join("peer-keys"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect::<Vec<_>>();
    for attempt in 0..2 {
        let suffix = format!("invite-{}-candidate-{attempt}", hex::encode(peer_handle));
        assert_eq!(
            entries
                .iter()
                .filter(|p| p.file_name().unwrap().to_str().unwrap().ends_with(&suffix))
                .count(),
            1
        );
    }
}

async fn execute_local(db: &Database, sql: &'static str) {
    sqlx::query(sql)
        .execute(&mut *aven_core::test_support::acquire(db).await.unwrap())
        .await
        .unwrap();
}
#[tokio::test]
async fn protected_ahead_sqlite_failure_recovers_forward_and_missing_evidence_refuses() {
    let f = fixture().await;
    let w = f.peer.list_workspaces().await.unwrap().remove(0);
    f.peer
        .create_task(&w, draft("survives checkpoint failure"))
        .await
        .unwrap();
    let before_cursor = f.peer.meta("sync_cursor").await.unwrap();
    let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
    let frozen = f
        .peer
        .prepare_encrypted_tail(&inputs.authority)
        .await
        .unwrap()
        .unwrap();
    drop(inputs);
    let _third = join(&f, "third", &f.seed, &f.seed_store).await;
    execute_local(&f.peer,"CREATE TRIGGER mirror_fault BEFORE UPDATE ON local_membership_checkpoint WHEN NEW.sequence=3 BEGIN SELECT RAISE(ABORT,'test checkpoint fault'); END").await;
    let client = peer_enrollment_http::Client::new(&f.origin).unwrap();
    assert!(client.refresh(&f.peer_store, &f.peer).await.is_err());
    assert_eq!(
        f.peer
            .membership_checkpoint_mirror()
            .await
            .unwrap()
            .unwrap()
            .1,
        2
    );
    execute_local(&f.peer, "DROP TRIGGER mirror_fault").await;
    let reopened = Database::open(f.peer.path()).await.unwrap();
    let keys = isolated_store(reopened.path(), &f.root.path().join("peer-keys"));
    client.refresh(&keys, &reopened).await.unwrap();
    assert_eq!(
        reopened
            .membership_checkpoint_mirror()
            .await
            .unwrap()
            .unwrap()
            .1,
        3
    );
    assert_eq!(reopened.meta("sync_cursor").await.unwrap(), before_cursor);
    let inputs = keys.tail_inputs(&reopened, &f.origin).await.unwrap();
    assert_eq!(
        reopened
            .prepare_encrypted_tail(&inputs.authority)
            .await
            .unwrap()
            .unwrap(),
        frozen
    );
    drop(inputs);
    let digest = reopened
        .membership_checkpoint_mirror()
        .await
        .unwrap()
        .unwrap()
        .3;
    let path = std::fs::read_dir(f.root.path().join("peer-keys"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .ends_with(&format!("membership-evidence-{}", hex::encode(digest)))
        })
        .unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(client.refresh(&keys, &reopened).await.is_err());
    assert_eq!(
        scalar(
            &reopened,
            "SELECT count(*) FROM tasks WHERE title='survives checkpoint failure'"
        )
        .await,
        1
    );
    assert_eq!(
        reopened
            .membership_checkpoint_mirror()
            .await
            .unwrap()
            .unwrap()
            .1,
        3
    );
}

#[tokio::test]
async fn database_ahead_or_missing_protected_floor_cannot_reconstruct_trust() {
    let f = fixture().await;
    execute_local(
        &f.peer,
        "UPDATE local_membership_checkpoint SET sequence=3,head=zeroblob(32)",
    )
    .await;
    let client = peer_enrollment_http::Client::new(&f.origin).unwrap();
    assert!(client.refresh(&f.peer_store, &f.peer).await.is_err());
    assert_eq!(
        f.peer
            .membership_checkpoint_mirror()
            .await
            .unwrap()
            .unwrap()
            .1,
        3
    );
    let g = fixture().await;
    let path = std::fs::read_dir(g.root.path().join("peer-keys"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .ends_with("membership-floor-2")
        })
        .unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(
        peer_enrollment_http::Client::new(&g.origin)
            .unwrap()
            .refresh(&g.peer_store, &g.peer)
            .await
            .is_err()
    );
    assert_eq!(
        g.peer
            .membership_checkpoint_mirror()
            .await
            .unwrap()
            .unwrap()
            .1,
        2
    );
}
#[tokio::test]
#[ignore = "subprocess protected membership checkpoint worker"]
async fn floor_worker() {
    let root = std::path::PathBuf::from(std::env::var_os("AVEN_MEMBERSHIP_ROOT").unwrap());
    let origin = std::env::var("AVEN_MEMBERSHIP_ORIGIN").unwrap();
    let peer = Database::open(&root.join("peer.sqlite")).await.unwrap();
    let keys = isolated_store(peer.path(), &root.join("peer-keys"));
    peer_enrollment_http::Client::new(&origin)
        .unwrap()
        .refresh(&keys, &peer)
        .await
        .unwrap();
    panic!("checkpoint crash not reached");
}
#[tokio::test]
async fn process_exit_before_floor_and_after_readback_recovers_without_reinstalling() {
    let f = fixture().await;
    let _third = join(&f, "third", &f.seed, &f.seed_store).await;
    let cursor = f.peer.meta("sync_cursor").await.unwrap();
    for boundary in ["membership-evidence", "membership-floor-3"] {
        let output = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "encrypted_tail_http::tests::membership::floor_worker",
                "--ignored",
                "--nocapture",
            ])
            .env("AVEN_MEMBERSHIP_ROOT", f.root.path())
            .env("AVEN_MEMBERSHIP_ORIGIN", &f.origin)
            .env("AVEN_PEER_CRASH_KIND", boundary)
            .output()
            .await
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(79),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            f.peer
                .membership_checkpoint_mirror()
                .await
                .unwrap()
                .unwrap()
                .1,
            2
        );
        assert_eq!(f.peer.meta("sync_cursor").await.unwrap(), cursor);
    }
    peer_enrollment_http::Client::new(&f.origin)
        .unwrap()
        .refresh(&f.peer_store, &f.peer)
        .await
        .unwrap();
    assert_eq!(
        f.peer
            .membership_checkpoint_mirror()
            .await
            .unwrap()
            .unwrap()
            .1,
        3
    );
    assert_eq!(f.peer.meta("sync_cursor").await.unwrap(), cursor);
}

#[tokio::test]
async fn metadata_download_refresh_preserves_pruned_mapping_and_finite_tail_watermark() {
    use aven_core::sync::encrypted_tail::attachments::{Operation as Op, Reply as ImageReply};
    let mut f = fixture().await;
    let tail = Client::new(&f.origin).unwrap();
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    let reference: String =
        sqlx::query_scalar("SELECT attachment_id FROM task_attachments LIMIT 1")
            .fetch_one(&mut *aven_core::test_support::acquire(&f.seed).await.unwrap())
            .await
            .unwrap();
    f.seed.delete_task_attachment(&w, &reference).await.unwrap();
    drain(&tail, &f.seed_store, &f.seed).await;
    let inputs = f.seed_store.tail_inputs(&f.seed, &f.origin).await.unwrap();
    let mut policy = crate::config::AttachmentLifecycleConfig::default().server_policy();
    policy.grace = std::time::Duration::ZERO;
    assert!(matches!(
        f.server
            .encrypted_image_exchange(
                &inputs.authority.context,
                &inputs.bearer,
                Op::Prune { limit: 128 },
                policy
            )
            .await
            .unwrap(),
        ImageReply::Pruned(1)
    ));
    drop(inputs);
    for n in 0..18 {
        f.seed
            .create_task(&w, draft(&format!("finite tail {n}")))
            .await
            .unwrap();
    }
    drain(&tail, &f.seed_store, &f.seed).await;
    let client = peer_enrollment_http::Client::new(&f.origin).unwrap();
    let third = Database::open(&f.root.path().join("third.sqlite"))
        .await
        .unwrap();
    let keys = isolated_store(third.path(), &f.root.path().join("third-keys"));
    let invitation = client
        .invite(&f.seed_store, &f.seed, expiry())
        .await
        .unwrap();
    client
        .request(&keys, &third, Some(invitation))
        .await
        .unwrap();
    client.admit(&f.seed_store, &f.seed).await.unwrap();
    client.complete(&keys, &third).await.unwrap();
    let (send, mut events) = tokio::sync::mpsc::channel(1);
    let fault = Arc::new(HttpFault {
        reads: Default::default(),
        lose: None,
        pause_operation: "Published",
        remaining: 1.into(),
        pause: Some(send),
    });
    restart_fault_server(&mut f, fault.clone()).await;
    let (installed, ()) = tokio::join!(client.install(&keys, &third), async {
        let resume = events.recv().await.unwrap();
        let fourth = join(&f, "fourth", &f.peer, &f.peer_store).await;
        assert_eq!(
            floor(&fourth.store, &fourth.db, &f.origin).await.sequence(),
            4
        );
        resume.send(()).unwrap();
    });
    let receipt = installed.unwrap();
    assert_eq!(floor(&keys, &third, &f.origin).await.sequence(), 4);
    assert_eq!(
        third
            .meta("e2ee_initial_image_watermark")
            .await
            .unwrap()
            .as_deref(),
        Some("pending")
    );
    let blobs = f.root.path().join("third-blobs");
    assert_eq!(
        tail.attachment_round(&keys, &third, &blobs)
            .await
            .unwrap()
            .images,
        ImageTransfer::Pending
    );
    let watermark = third
        .meta("e2ee_initial_image_watermark")
        .await
        .unwrap()
        .unwrap();
    assert!(watermark.parse::<u64>().unwrap() > receipt.prefix_count + 16);
    f.seed
        .create_task(&w, draft("after captured watermark"))
        .await
        .unwrap();
    drain(&tail, &f.seed_store, &f.seed).await;
    assert_eq!(client.install(&keys, &third).await.unwrap(), receipt);
    assert_eq!(
        third
            .meta("e2ee_initial_image_watermark")
            .await
            .unwrap()
            .unwrap(),
        watermark
    );
    let result = tail.attachment_round(&keys, &third, &blobs).await.unwrap();
    assert_eq!(result.images, ImageTransfer::Complete);
    assert_eq!(third.meta("sync_cursor").await.unwrap().unwrap(), watermark);
    assert_eq!(
        scalar(
            &third,
            "SELECT count(*) FROM task_attachments WHERE deleted=0"
        )
        .await,
        0
    );
    assert!(!blobs.join("objects/sha256").exists());
    drain(&tail, &keys, &third).await;
    assert_eq!(
        scalar(
            &third,
            "SELECT count(*) FROM tasks WHERE title='after captured watermark'"
        )
        .await,
        1
    );
}

#[tokio::test]
async fn membership_change_during_image_read_retries_the_same_selected_object() {
    let mut f = fixture().await;
    attachments::add_image(&f).await;
    let client = Client::new(&f.origin).unwrap();
    client
        .attachment_round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
        .await
        .unwrap();
    let third = join(&f, "third", &f.seed, &f.seed_store).await;
    let (send, mut events) = tokio::sync::mpsc::channel(1);
    let fault = Arc::new(HttpFault {
        reads: Default::default(),
        lose: None,
        pause_operation: "Read",
        remaining: 1.into(),
        pause: Some(send),
    });
    restart_fault_server(&mut f, fault.clone()).await;
    let (result, ()) = tokio::join!(
        client.attachment_round(&third.store, &third.db, &third.blobs),
        async {
            let resume = events.recv().await.unwrap();
            let fourth = join(&f, "fourth", &f.peer, &f.peer_store).await;
            assert_eq!(
                floor(&fourth.store, &fourth.db, &f.origin).await.sequence(),
                4
            );
            resume.send(()).unwrap();
        }
    );
    result.unwrap();
    let reads = fault.reads.lock().unwrap().clone();
    assert_eq!(reads.len(), 2);
    assert_eq!(reads[0], reads[1]);
    assert_eq!(
        floor(&third.store, &third.db, &f.origin).await.sequence(),
        4
    );
    assert_eq!(
        client
            .attachment_round(&third.store, &third.db, &third.blobs)
            .await
            .unwrap()
            .images,
        ImageTransfer::Complete
    );
}

async fn pull_only_stale_race(count: usize) {
    let mut f = fixture().await;
    let workspace = f.peer.list_workspaces().await.unwrap().remove(0);
    f.peer
        .create_task(&workspace, draft("pull-only pending task"))
        .await
        .unwrap();
    let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
    let frozen = f
        .peer
        .prepare_encrypted_tail(&inputs.authority)
        .await
        .unwrap()
        .unwrap();
    drop(inputs);
    let cursor = f.peer.meta("sync_cursor").await.unwrap();
    let watermark = f.peer.meta("e2ee_initial_image_watermark").await.unwrap();
    let pending = scalar(
        &f.peer,
        "SELECT count(*) FROM changes WHERE server_seq IS NULL",
    )
    .await;
    let high_water = scalar(
        &f.server,
        "SELECT high_water FROM server_e2ee_allocator WHERE singleton=1",
    )
    .await;
    let (events, mut incoming) = tokio::sync::mpsc::channel(1);
    let fault = Arc::new(HttpFault {
        reads: Default::default(),
        pause_operation: "Pull",
        lose: None,
        remaining: count.into(),
        pause: Some(events),
    });
    restart_fault_server(&mut f, fault.clone()).await;
    let client = Client::new(&f.origin).unwrap();
    let (result, ()) = tokio::join!(client.pull_only_round(&f.peer_store, &f.peer), async {
        for index in 0..count {
            let resume = incoming.recv().await.unwrap();
            let joined = join(
                &f,
                &format!("pull-competitor-{index}"),
                &f.seed,
                &f.seed_store,
            )
            .await;
            assert_eq!(
                floor(&joined.store, &joined.db, &f.origin).await.sequence(),
                3 + index as u64
            );
            resume.send(()).unwrap();
        }
    });
    assert_eq!(fault.remaining.load(std::sync::atomic::Ordering::SeqCst), 0);
    if count == 1 {
        assert!(result.unwrap());
    } else {
        assert!(
            result
                .err()
                .unwrap()
                .is::<aven_core::sync::seed_claim::membership::StaleContext>()
        );
    }
    assert_eq!(f.peer.meta("sync_cursor").await.unwrap(), cursor);
    assert_eq!(
        f.peer.meta("e2ee_initial_image_watermark").await.unwrap(),
        watermark
    );
    assert_eq!(
        scalar(
            &f.peer,
            "SELECT count(*) FROM changes WHERE server_seq IS NULL"
        )
        .await,
        pending
    );
    assert_eq!(
        scalar(
            &f.server,
            "SELECT high_water FROM server_e2ee_allocator WHERE singleton=1"
        )
        .await,
        high_water
    );
    let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
    assert_eq!(
        f.peer
            .encrypted_tail_frozen_record(&inputs.authority)
            .await
            .unwrap()
            .unwrap()
            .1,
        frozen
    );
}

#[tokio::test]
async fn pull_only_retries_one_head_race_without_uploading() {
    pull_only_stale_race(1).await;
}

#[tokio::test]
async fn pull_only_stops_after_second_head_race_without_uploading() {
    pull_only_stale_race(2).await;
}
