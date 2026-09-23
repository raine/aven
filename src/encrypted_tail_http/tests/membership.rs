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
    let f = fixture().await;
    let enrollment = peer_enrollment_http::Client::new(&f.origin).unwrap();
    let client = Client::new(&f.origin).unwrap();
    let w = f.peer.list_workspaces().await.unwrap().remove(0);
    f.peer
        .create_task(&w, draft("accepted before third"))
        .await
        .unwrap();
    let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
    let record = f
        .peer
        .prepare_encrypted_tail(&inputs.authority)
        .await
        .unwrap()
        .unwrap();
    let Reply::Appended(mapping) = client
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
    // Drop the successful response before local acknowledgement.
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
