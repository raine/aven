//! Publishing around an invitation whose grant may have been sent: nothing
//! blocks before expiry; afterwards only the withdrawal rotation unblocks it.
use super::*;
use crate::protected_local_keys::peer::{OutboundInvitation, PublishingBlocked};

/// Leaves time to register, request and prepare a grant before expiry.
fn soon() -> u64 {
    expiry() - 3600 + 20
}

async fn past(expiry: u64) {
    while std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        < expiry
    {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

/// The seed invites a new device, which requests admission.
async fn invite(f: &Fixture, name: &str) -> u64 {
    let enrollment = peer_enrollment_http::Client::new(&f.origin).unwrap();
    let expires = soon();
    let invitation = enrollment
        .invite(&f.seed_store, &f.seed, expires)
        .await
        .unwrap();
    let db = Database::open(&f.root.path().join(format!("{name}.sqlite")))
        .await
        .unwrap();
    let store = isolated_store(&db, &f.root.path().join(format!("{name}-keys"))).await;
    enrollment
        .request(&store, &db, Some(invitation))
        .await
        .unwrap();
    expires
}

/// Prepares and marks sent the seed's grant for the open invitation without
/// delivering it, as when the admission reply is lost or withheld.
async fn send_grant(f: &Fixture) {
    let inputs = f
        .seed_store
        .active_inputs(&f.seed, &f.origin)
        .await
        .unwrap();
    let journal = f
        .seed_store
        .prepare_invitation(&f.seed, &inputs, None, None)
        .await
        .unwrap();
    let mail = f
        .server
        .membership_mailbox(
            inputs.membership.genesis().context().vault_id,
            journal.handle,
        )
        .await
        .unwrap();
    f.seed_store
        .prepare_admission(&f.seed, &inputs, &journal, mail.request.as_ref().unwrap())
        .await
        .unwrap();
}

async fn create(db: &Database, title: &str) -> aven_core::ids::TaskId {
    let w = db.list_workspaces().await.unwrap().remove(0);
    db.create_task(&w, draft(title)).await.unwrap().task.id
}

async fn exists(db: &Database, id: &aven_core::ids::TaskId) -> bool {
    let mut c = aven_core::test_support::acquire(db).await.unwrap();
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tasks WHERE id=?)")
        .bind(id)
        .fetch_one(&mut *c)
        .await
        .unwrap()
}

fn operations(fault: &HttpFault) -> Vec<String> {
    fault
        .bodies
        .lock()
        .unwrap()
        .iter()
        .map(|(operation, _)| operation.clone())
        .collect()
}

#[tokio::test]
async fn invitations_opened_during_a_drain_block_publishing_only_after_expiry() {
    let f = fixture().await;
    converge(&f).await;
    let client = Client::new(&f.origin).unwrap();
    let mut drain = client.start_drain(&f.seed_store, &f.seed).await.unwrap();

    // An invitation with no grant sent never blocks.
    let expires = invite(&f, "joiner").await;
    let pending = create(&f.seed, "while invitation pending").await;
    let round = client
        .round_in_drain(&f.seed_store, &f.seed, &blobs(&f.seed), &mut drain)
        .await
        .unwrap();
    assert!(!round.publishing_blocked);
    assert!(round.metadata_caught_up);

    // Nor does a possibly sent grant before the invitation expires.
    send_grant(&f).await;
    assert_eq!(
        f.seed_store.outbound_invitation(&f.seed).await.unwrap(),
        Some(OutboundInvitation::Disclosed)
    );
    let disclosed = create(&f.seed, "while grant unresolved").await;
    let round = client
        .round_in_drain(&f.seed_store, &f.seed, &blobs(&f.seed), &mut drain)
        .await
        .unwrap();
    assert!(!round.publishing_blocked);
    assert!(round.metadata_caught_up);
    drain_peer(&f, &client).await;
    assert!(exists(&f.peer, &pending).await);
    assert!(exists(&f.peer, &disclosed).await);

    // Expiry during the drain stops publishing in the same drain.
    past(expires).await;
    let after = create(&f.seed, "after expiry").await;
    let round = client
        .round_in_drain(&f.seed_store, &f.seed, &blobs(&f.seed), &mut drain)
        .await
        .unwrap();
    assert!(round.publishing_blocked);
    drain_peer(&f, &client).await;
    assert!(!exists(&f.peer, &after).await);

    // The next drain rotates first, then publishes.
    crate::sync::encrypted::drain(&client, &f.seed_store, &f.seed, &blobs(&f.seed), 100)
        .await
        .unwrap();
    assert_eq!(
        f.seed_store.outbound_invitation(&f.seed).await.unwrap(),
        None
    );
    drain_peer(&f, &client).await;
    assert!(exists(&f.peer, &after).await);
}

async fn drain_peer(f: &Fixture, client: &Client) {
    drain(client, &f.peer_store, &f.peer).await;
}

#[tokio::test]
async fn failed_withdrawal_rotation_withholds_publishing_but_still_pulls() {
    let mut f = fixture().await;
    converge(&f).await;
    let expires = invite(&f, "joiner").await;
    send_grant(&f).await;
    // Remote work for the seed to pull and download.
    add_image(&f).await;
    let remote = create(&f.peer, "from peer").await;
    drain(&Client::new(&f.origin).unwrap(), &f.peer_store, &f.peer).await;
    // Local work, including an image, that must wait for the rotation.
    let local = create(&f.seed, "withheld").await;
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    let mut png = std::io::Cursor::new(Vec::new());
    ::image::DynamicImage::new_rgb8(5, 4)
        .write_to(&mut png, ::image::ImageFormat::Png)
        .unwrap();
    f.seed
        .add_task_attachment(
            &w,
            &blobs(&f.seed),
            Default::default(),
            &local,
            aven_core::operations::AttachmentAddInput {
                filename: None,
                alt_text: None,
                declared_media_type: None,
                bytes: png.into_inner(),
                optimization_policy: aven_core::attachments::ImageOptimizationPolicy::Preserve,
                dedupe_existing: false,
            },
        )
        .await
        .unwrap();
    past(expires).await;

    // The server commits the first management record but its reply is lost.
    let fault = Arc::new(HttpFault {
        lose: Some("Manage"),
        remaining: AtomicUsize::new(1),
        bodies: Default::default(),
    });
    restart_server(&mut f, fault.clone()).await;
    let client = Client::new(&f.origin).unwrap();
    let error =
        crate::sync::encrypted::drain(&client, &f.seed_store, &f.seed, &blobs(&f.seed), 100)
            .await
            .unwrap_err();
    assert!(error.is::<PublishingBlocked>(), "{error:#}");
    let blocked = operations(&fault);
    assert!(blocked.iter().any(|op| op == "Manage"), "{blocked:?}");
    assert!(blocked.iter().any(|op| op == "Pull"), "{blocked:?}");
    assert!(blocked.iter().any(|op| op == "Read"), "{blocked:?}");
    for publishing in ["Append", "Declare", "Put", "Complete"] {
        assert!(!blocked.iter().any(|op| op == publishing), "{blocked:?}");
    }
    assert!(exists(&f.seed, &remote).await);
    assert!(
        scalar(
            &f.seed,
            "SELECT count(*) FROM changes WHERE server_seq IS NULL"
        )
        .await
            > 0
    );

    // A later drain finishes the rotation, then uploads the withheld work.
    crate::sync::encrypted::drain(&client, &f.seed_store, &f.seed, &blobs(&f.seed), 100)
        .await
        .unwrap();
    let after = operations(&fault)[blocked.len()..].to_vec();
    for publishing in ["Append", "Declare", "Complete"] {
        assert!(after.iter().any(|op| op == publishing), "{after:?}");
    }
    assert_eq!(
        f.seed_store.outbound_invitation(&f.seed).await.unwrap(),
        None
    );
    drain(&client, &f.peer_store, &f.peer).await;
    assert!(exists(&f.peer, &local).await);
}
