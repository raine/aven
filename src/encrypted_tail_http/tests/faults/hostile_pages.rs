use super::*;
use crate::test_support::e2ee_http::protected_state::ProtectedState;
use aven_core::sync::seed_claim::membership::Evidence;
use peer_enrollment_http::RemovalStatus;

struct Splice {
    delivered: AtomicUsize,
    record: (String, Vec<u8>),
}

/// Replaces the first record of every successful Page reply with `record`,
/// recomputing the relay-controlled commitment so only authentication refuses it.
async fn splice_page(
    State(attack): State<Arc<Splice>>,
    request: Request,
    next: axum::middleware::Next,
) -> Response {
    let response = next.run(request).await;
    if response.status() != StatusCode::OK {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let bytes = to_bytes(body, tail::RESPONSE_LIMIT).await.unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    if value["operation"].get("Page").is_some() {
        let mut envelope: Envelope<Reply> = serde_json::from_value(value.clone()).unwrap();
        let Reply::Page(ref mut page) = envelope.operation else {
            unreachable!()
        };
        assert!(!page.records.is_empty(), "attack must deliver a record");
        let accepted = &mut page.records[0];
        let (id, record) = &attack.record;
        accepted.record.clone_from(record);
        accepted.mapping.operation_id.clone_from(id);
        accepted.mapping.commitment = sha2::Sha256::digest(&accepted.record).into();
        value = serde_json::to_value(envelope).unwrap();
        attack.delivered.fetch_add(1, Ordering::SeqCst);
    }
    parts.headers.remove(header::CONTENT_LENGTH);
    Response::from_parts(
        parts,
        axum::body::Body::from(serde_json::to_vec(&value).unwrap()),
    )
}

/// Restarts the real server behind `layer`, which rewrites its replies.
async fn restart_hostile(f: &mut Fixture, layer: impl FnOnce(Router) -> Router) {
    f.task.abort();
    let _ = (&mut f.task).await;
    let app = peer_enrollment_http::router(f.server.clone()).merge(router(
        f.server.clone(),
        crate::server_config::AttachmentsConfig::default().policy(),
    ));
    (_, f.task) = e2ee_http::serve(layer(app), f.origin.strip_prefix("http://").unwrap()).await;
}

#[tokio::test]
async fn valid_foreign_vault_record_leaves_protected_state_unchanged() {
    let mut f = fixture().await;
    // Freeze and append a real valid record without acknowledging it to its author.
    let (_, _, record) = accepted_task(&f, "hostile page must not apply").await;
    let workspace = f.seed.list_workspaces().await.unwrap().remove(0);
    f.seed
        .create_task(&workspace, draft("pending protected local edit"))
        .await
        .unwrap();
    let inputs = f.seed_store.tail_inputs(&f.seed, &f.origin).await.unwrap();
    let frozen_local = head_record(&f.seed, &inputs.authority).await;
    assert_eq!(frozen(&f.seed).await, frozen_local);
    let after = f
        .seed
        .encrypted_round_state(&inputs.authority)
        .await
        .unwrap()
        .cursor;
    let client = Client::new(&f.origin).unwrap();
    let Reply::Page(control) = client
        .exchange(
            &inputs.authority.context,
            &inputs.bearer,
            Operation::Pull {
                after,
                limit: 1,
                watermark: None,
            },
        )
        .await
        .unwrap()
    else {
        panic!("control page")
    };
    assert_eq!(
        control.records[0].record, record,
        "honest response must contain our valid record"
    );
    drop(inputs);
    let other = fixture().await;
    let (id, _, foreign) = accepted_task(&other, "valid foreign vault record").await;
    let attack = Arc::new(Splice {
        delivered: AtomicUsize::new(0),
        record: (id, foreign),
    });
    restart_hostile(&mut f, |app| {
        app.layer(axum::middleware::from_fn_with_state(
            attack.clone(),
            splice_page,
        ))
    })
    .await;
    let keys = f.root.path().join("keys");
    let before = ProtectedState::capture(&f.seed, &keys).await;
    let error = client
        .pull_only_round(&f.seed_store, &f.seed)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("encrypted-tail"), "{error:#}");
    assert_eq!(attack.delivered.load(Ordering::SeqCst), 1);
    before.assert_unchanged(&f.seed, &keys).await;
    assert_eq!(frozen(&f.seed).await, frozen_local);
    // Restore honest service and prove the same valid page still applies.
    restart_server(&mut f, no_fault()).await;
    client
        .pull_only_round(&f.seed_store, &f.seed)
        .await
        .unwrap();
    assert_eq!(
        scalar(
            &f.seed,
            "SELECT count(*) FROM tasks WHERE title='hostile page must not apply'"
        )
        .await,
        1
    );
}

struct Replay {
    delivered: AtomicUsize,
    evidence: Evidence,
}

/// Answers every membership refresh with recorded, validly signed evidence.
async fn replay_membership(
    State(attack): State<Arc<Replay>>,
    request: Request,
    next: axum::middleware::Next,
) -> Response {
    let response = next.run(request).await;
    let (mut parts, body) = response.into_parts();
    let mut bytes = to_bytes(body, tail::RESPONSE_LIMIT).await.unwrap().to_vec();
    let value: Option<serde_json::Value> = serde_json::from_slice(&bytes).ok();
    if value.is_some_and(|value| value.get("Membership").is_some()) {
        bytes = serde_json::to_vec(&serde_json::json!({ "Membership": attack.evidence })).unwrap();
        attack.delivered.fetch_add(1, Ordering::SeqCst);
    }
    parts.headers.remove(header::CONTENT_LENGTH);
    Response::from_parts(parts, axum::body::Body::from(bytes))
}

#[tokio::test]
async fn pre_rotation_membership_replay_leaves_protected_state_unchanged() {
    let mut f = fixture().await;
    let third = join(&f, "third", &f.seed, &f.seed_store).await;
    let removed = device(&third.store, &third.db, &f.origin).await;
    // The chain that still admits `removed` with the original generation current.
    let old = f
        .seed_store
        .active_inputs(&f.seed, &f.origin)
        .await
        .unwrap()
        .evidence
        .clone();
    assert_eq!(
        peer_enrollment_http::Client::new(&f.origin)
            .unwrap()
            .remove_device(&f.seed_store, &f.seed, removed)
            .await
            .unwrap(),
        RemovalStatus::Complete
    );
    converge(&f).await;
    let observed = f
        .peer_store
        .active_inputs(&f.peer, &f.origin)
        .await
        .unwrap()
        .membership
        .clone();
    assert!(!observed.has_device(removed) && observed.generations().len() == 2);
    let attack = Arc::new(Replay {
        delivered: AtomicUsize::new(0),
        evidence: old,
    });
    restart_hostile(&mut f, |app| {
        app.layer(axum::middleware::from_fn_with_state(
            attack.clone(),
            replay_membership,
        ))
    })
    .await;
    let keys = f.root.path().join("peer-keys");
    let before = ProtectedState::capture(&f.peer, &keys).await;
    let client = Client::new(&f.origin).unwrap();
    let error = client
        .pull_only_round(&f.peer_store, &f.peer)
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "error membership-floor-fork");
    assert_eq!(attack.delivered.load(Ordering::SeqCst), 1);
    before.assert_unchanged(&f.peer, &keys).await;
    restart_server(&mut f, no_fault()).await;
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    f.seed
        .create_task(&w, draft("after honest service resumes"))
        .await
        .unwrap();
    converge(&f).await;
    assert_eq!(
        scalar(
            &f.peer,
            "SELECT count(*) FROM tasks WHERE title='after honest service resumes'"
        )
        .await,
        1
    );
}
