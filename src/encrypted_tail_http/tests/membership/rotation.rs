use super::*;
use aven_core::sync::seed_claim::membership::{Joiner, VerifiedKeys};

async fn server_rotate(f: &Fixture, driver: &Joined, targets: &[[u8; 32]]) {
    let enrollment = peer_enrollment_http::Client::new(&f.origin).unwrap();
    enrollment.refresh(&driver.store, &driver.db).await.unwrap();
    let signer = driver
        .store
        .prepare_peer(&driver.db, &f.origin, None)
        .await
        .unwrap();
    let inputs = driver
        .store
        .active_inputs(&driver.db, &f.origin)
        .await
        .unwrap();
    let mut m = inputs.membership.clone();
    let keys = VerifiedKeys::from_protected_storage(
        &m,
        &inputs.generation_keys().protected_storage_bytes(),
    )
    .unwrap();
    drop(inputs);
    let record = signer.authority().prepare_revoke(&m, targets).unwrap();
    f.server
        .apply_membership_management(&auth(&m, &signer), &record)
        .await
        .unwrap();
    m = m.append(&[], &[], &record).unwrap();
    let high = f
        .server
        .prepare_membership_management(&auth(&m, &signer))
        .await
        .unwrap()
        .high_water;
    let record = signer
        .authority()
        .prepare_rotation(&m, &keys, high)
        .unwrap();
    f.server
        .apply_membership_management(&auth(&m, &signer), &record)
        .await
        .unwrap();
}
fn auth<'a>(
    m: &Membership,
    signer: &'a Joiner,
) -> aven_core::sync::seed_claim::peer::Authentication<'a> {
    aven_core::sync::seed_claim::peer::Authentication {
        vault: m.genesis().context().vault_id,
        genesis: m.genesis().commitment(),
        head: m.head(),
        device: signer.device(),
        bearer: signer.bearer(),
    }
}
async fn pending_task(f: &Fixture, title: &str) -> String {
    let w = f.peer.list_workspaces().await.unwrap().remove(0);
    f.peer
        .create_task(&w, draft(title))
        .await
        .unwrap()
        .task
        .id
        .to_string()
}
async fn frozen(db: &Database) -> Vec<u8> {
    sqlx::query_scalar("SELECT record FROM local_e2ee_outbox WHERE singleton=1")
        .fetch_one(&mut *aven_core::test_support::acquire(db).await.unwrap())
        .await
        .unwrap()
}
async fn image_rounds(f: &Fixture, third: &Joined) {
    let client = Client::new(&f.origin).unwrap();
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
}

#[tokio::test]
async fn installed_survivors_sync_unfrozen_unaccepted_and_lost_ack_tasks_across_two_rotations() {
    for mode in 0..3 {
        let mut f = fixture().await;
        let third = join(&f, "third", &f.seed, &f.seed_store).await;
        let task = pending_task(&f, "offline task survives cutoffs").await;
        let original = if mode > 0 {
            let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
            let record = head_record(&f.peer, &inputs.authority).await;
            drop(inputs);
            Some(record)
        } else {
            None
        };
        if mode == 2 {
            let fault = Arc::new(HttpFault {
                reads: Default::default(),
                lose: Some("Append"),
                pause_operation: "Append",
                remaining: 1.into(),
                pause: None,
            });
            restart_fault_server(&mut f, fault).await;
            assert!(
                Client::new(&f.origin)
                    .unwrap()
                    .round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
                    .await
                    .is_err()
            );
            assert_eq!(frozen(&f.peer).await, *original.as_ref().unwrap());
        }
        let seed_id = f
            .seed_store
            .active_inputs(&f.seed, &f.origin)
            .await
            .unwrap()
            .device();
        server_rotate(&f, &third, &[seed_id]).await;
        server_rotate(&f, &third, &[]).await;
        let client = Client::new(&f.origin).unwrap();
        if mode == 1 {
            peer_enrollment_http::Client::new(&f.origin)
                .unwrap()
                .refresh(&f.peer_store, &f.peer)
                .await
                .unwrap();
            let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
            assert!(
                f.server
                    .encrypted_tail_exchange(
                        &inputs.authority.context,
                        &inputs.bearer,
                        Operation::Append {
                            record: original.as_ref().unwrap().clone(),
                            ticket: None
                        }
                    )
                    .await
                    .is_err()
            );
        }
        assert!(
            client
                .round(&f.seed_store, &f.seed, f.root.path())
                .await
                .is_err()
        );
        drain(&client, &f.peer_store, &f.peer).await;
        drain(&client, &third.store, &third.db).await;
        assert_eq!(
            title(&third.db, &task).await,
            "offline task survives cutoffs"
        );
        if let Some(old) = original {
            let accepted: Vec<u8> = sqlx::query_scalar("SELECT record FROM local_e2ee_accepted WHERE operation_id=(SELECT change_id FROM changes WHERE entity_id=? AND op_type='create_task')").bind(&task).fetch_one(&mut *aven_core::test_support::acquire(&f.peer).await.unwrap()).await.unwrap();
            assert_eq!(old == accepted, mode == 2);
            let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
            let replay = f
                .server
                .encrypted_tail_exchange(
                    &inputs.authority.context,
                    &inputs.bearer,
                    Operation::Append {
                        record: old,
                        ticket: None,
                    },
                )
                .await;
            // Accepted IDs win even for old bytes; never-admitted closed bytes cannot win.
            assert!(replay.is_ok());
        }
        let w = third.db.list_workspaces().await.unwrap().remove(0);
        let task = third
            .db
            .create_task(&w, draft("current generation return trip"))
            .await
            .unwrap()
            .task
            .id;
        drain(&client, &third.store, &third.db).await;
        drain(&client, &f.peer_store, &f.peer).await;
        assert_eq!(
            title(&f.peer, task.as_str()).await,
            "current generation return trip"
        );
    }
}

#[tokio::test]
async fn lost_put_complete_and_ref_replies_cut_over_without_rewriting_accepted_images() {
    for stage in ["Put", "Complete", "Append"] {
        let mut f = fixture().await;
        let third = join(&f, "third", &f.seed, &f.seed_store).await;
        let reference = attachments::add_image(&f).await;
        let fault = Arc::new(HttpFault {
            reads: Default::default(),
            lose: Some(stage),
            pause_operation: stage,
            remaining: 1.into(),
            pause: None,
        });
        restart_fault_server(&mut f, fault.clone()).await;
        let client = Client::new(&f.origin).unwrap();
        let result = client
            .round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
            .await;
        if stage == "Append" {
            assert!(result.is_err());
        } else {
            assert_eq!(result.unwrap().images, ImageTransfer::Failed);
        }
        assert_eq!(fault.remaining.load(std::sync::atomic::Ordering::SeqCst), 0);
        let old = frozen(&f.peer).await;
        let old_descriptor: Vec<u8> =
            sqlx::query_scalar("SELECT descriptor FROM local_e2ee_image_preparation")
                .fetch_one(&mut *aven_core::test_support::acquire(&f.peer).await.unwrap())
                .await
                .unwrap();
        let seed_id = f
            .seed_store
            .active_inputs(&f.seed, &f.origin)
            .await
            .unwrap()
            .device();
        server_rotate(&f, &third, &[seed_id]).await;
        server_rotate(&f, &third, &[]).await;
        if stage == "Append" {
            // Lost accepted Ref remains authoritative even when ciphertext is no longer available.
            execute_local(&f.server, "DELETE FROM server_e2ee_image_chunks WHERE object IN (SELECT object FROM server_e2ee_images WHERE origin IS NOT NULL AND origin!='bootstrap')").await;
            execute_local(&f.server, "UPDATE server_e2ee_images SET complete=0 WHERE origin IS NOT NULL AND origin!='bootstrap'").await;
        }
        image_rounds(&f, &third).await;
        let descriptor: Vec<u8> = sqlx::query_scalar("SELECT o.descriptor FROM local_e2ee_image_objects o JOIN local_e2ee_image_references r ON r.object=o.object WHERE r.reference=?").bind(&reference).fetch_one(&mut *aven_core::test_support::acquire(&f.peer).await.unwrap()).await.unwrap();
        assert_eq!(descriptor == old_descriptor, stage == "Append");
        let accepted: Vec<u8> = sqlx::query_scalar("SELECT record FROM local_e2ee_accepted WHERE operation_id=(SELECT created_by_change_id FROM task_attachments WHERE attachment_id=?)").bind(&reference).fetch_one(&mut *aven_core::test_support::acquire(&f.peer).await.unwrap()).await.unwrap();
        assert_eq!(accepted == old, stage == "Append");
        let w = f.peer.list_workspaces().await.unwrap().remove(0);
        client
            .repair_attachment(
                &f.peer_store,
                &f.peer,
                &f.root.path().join("peer-blobs"),
                w.id.as_str(),
                &reference,
            )
            .await
            .unwrap();
        image_rounds(&f, &third).await;
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM task_attachments a JOIN blob_inventory b ON b.sha256=a.sha256 WHERE a.attachment_id=? AND b.available=1").bind(&reference).fetch_one(&mut *aven_core::test_support::acquire(&third.db).await.unwrap()).await.unwrap();
        assert_eq!(count, 1);
        if stage == "Append" {
            let reused = attachments::add_image(&f).await;
            image_rounds(&f, &third).await;
            let reused_descriptor: Vec<u8> = sqlx::query_scalar("SELECT o.descriptor FROM local_e2ee_image_objects o JOIN local_e2ee_image_references r ON r.object=o.object WHERE r.reference=?").bind(reused).fetch_one(&mut *aven_core::test_support::acquire(&third.db).await.unwrap()).await.unwrap();
            assert_eq!(reused_descriptor, descriptor);
        }
    }
}

#[tokio::test]
async fn frozen_rounds_pull_history_and_resolve_accepted_refs_without_image_mutation() {
    for accepted in [false, true] {
        let f = fixture().await;
        let third = join(&f, "third", &f.seed, &f.seed_store).await;
        attachments::add_image(&f).await;
        let client = Client::new(&f.origin).unwrap();
        let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
        let upload = f
            .peer
            .prepare_encrypted_push(&inputs.authority, &f.root.path().join("peer-blobs"))
            .await
            .unwrap()
            .unwrap()
            .upload
            .unwrap();
        let original = frozen(&f.peer).await;
        if accepted {
            use aven_core::sync::encrypted_tail::attachments::{
                Operation as Op, Reply as R, Ticket,
            };
            let R::Status(status) = client
                .image_exchange(
                    &inputs.authority.context,
                    &inputs.bearer,
                    Op::Declare {
                        workspace: upload.workspace.clone(),
                        descriptor: upload.descriptor.clone(),
                    },
                )
                .await
                .unwrap()
            else {
                panic!()
            };
            let ticket = Ticket {
                reservation: status.reservation.unwrap(),
            };
            for (index, record) in upload.records.into_iter().enumerate() {
                client
                    .image_exchange(
                        &inputs.authority.context,
                        &inputs.bearer,
                        Op::Put {
                            workspace: upload.workspace.clone(),
                            object: upload.object,
                            descriptor_commitment: upload.commitment,
                            reservation: ticket.reservation,
                            index,
                            record,
                        },
                    )
                    .await
                    .unwrap();
            }
            client
                .image_exchange(
                    &inputs.authority.context,
                    &inputs.bearer,
                    Op::Complete {
                        workspace: upload.workspace,
                        object: upload.object,
                        descriptor_commitment: upload.commitment,
                        reservation: ticket.reservation,
                    },
                )
                .await
                .unwrap();
            client
                .exchange(
                    &inputs.authority.context,
                    &inputs.bearer,
                    Operation::Append {
                        record: original.clone(),
                        ticket: Some(ticket),
                    },
                )
                .await
                .unwrap();
        }
        drop(inputs);
        let w = third.db.list_workspaces().await.unwrap().remove(0);
        let id = third
            .db
            .create_task(&w, draft("history progresses during freeze"))
            .await
            .unwrap()
            .task
            .id;
        drain(&client, &third.store, &third.db).await;
        let signer = third
            .store
            .prepare_peer(&third.db, &f.origin, None)
            .await
            .unwrap();
        let m = floor(&third.store, &third.db, &f.origin).await;
        let revoke = signer.authority().prepare_revoke(&m, &[]).unwrap();
        f.server
            .apply_membership_management(&auth(&m, &signer), &revoke)
            .await
            .unwrap();
        let before: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT high_water FROM server_e2ee_allocator),(SELECT count(*) FROM server_e2ee_image_chunks),(SELECT count(*) FROM server_e2ee_image_tickets)").fetch_one(&mut *aven_core::test_support::acquire(&f.server).await.unwrap()).await.unwrap();
        client
            .pull_only_round(&f.peer_store, &f.peer)
            .await
            .unwrap();
        assert_eq!(
            title(&f.peer, id.as_str()).await,
            "history progresses during freeze"
        );
        let after: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT high_water FROM server_e2ee_allocator),(SELECT count(*) FROM server_e2ee_image_chunks),(SELECT count(*) FROM server_e2ee_image_tickets)").fetch_one(&mut *aven_core::test_support::acquire(&f.server).await.unwrap()).await.unwrap();
        assert_eq!(before, after);
        if accepted {
            assert_eq!(
                scalar(&f.peer, "SELECT count(*) FROM local_e2ee_outbox").await,
                0
            );
        } else {
            assert_eq!(frozen(&f.peer).await, original);
        }
    }
}

#[tokio::test]
#[ignore = "subprocess atomic supersession worker"]
async fn supersession_worker() {
    let root = std::path::PathBuf::from(std::env::var_os("AVEN_SUPER_ROOT").unwrap());
    let origin = std::env::var("AVEN_SUPER_ORIGIN").unwrap();
    let db = Database::open(&root.join("peer.sqlite")).await.unwrap();
    let store = isolated_store(&db, &root.join("peer-keys")).await;
    let client = Client::new(&origin).unwrap();
    client
        .round(&store, &db, &root.join("peer-blobs"))
        .await
        .unwrap();
    panic!("supersession crash not reached");
}

#[tokio::test]
async fn process_restart_before_and_after_task_and_image_supersession_preserves_atomic_ownership() {
    for image in [false, true] {
        let f = fixture().await;
        let third = join(&f, "third", &f.seed, &f.seed_store).await;
        if image {
            attachments::add_image(&f).await;
        } else {
            pending_task(&f, "survives atomic re-envelope").await;
        }
        let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
        f.peer
            .prepare_encrypted_push(&inputs.authority, &blobs(&f.peer))
            .await
            .unwrap();
        drop(inputs);
        let old = frozen(&f.peer).await;
        let owner: (String, String, i64) = sqlx::query_as(
            "SELECT operation_id,association,sync_generation FROM local_e2ee_outbox",
        )
        .fetch_one(&mut *aven_core::test_support::acquire(&f.peer).await.unwrap())
        .await
        .unwrap();
        if image {
            let sha: String = sqlx::query_scalar("SELECT sha256 FROM local_e2ee_image_preparation")
                .fetch_one(&mut *aven_core::test_support::acquire(&f.peer).await.unwrap())
                .await
                .unwrap();
            std::fs::remove_file(f.root.path().join("peer-blobs/objects/sha256").join(sha))
                .unwrap();
        }
        let descriptor: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT descriptor FROM local_e2ee_image_preparation")
                .fetch_optional(&mut *aven_core::test_support::acquire(&f.peer).await.unwrap())
                .await
                .unwrap();
        server_rotate(&f, &third, &[]).await;
        server_rotate(&f, &third, &[]).await;
        for boundary in ["before-supersede-commit", "after-supersede-commit"] {
            let output = e2ee_http::worker(
                "encrypted_tail_http::tests::membership::rotation::supersession_worker",
            )
            .env("AVEN_SUPER_ROOT", f.root.path())
            .env("AVEN_SUPER_ORIGIN", &f.origin)
            .env("AVEN_TAIL_CRASH", boundary)
            .output()
            .await
            .unwrap();
            assert_eq!(
                output.status.code(),
                Some(84),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(
                frozen(&f.peer).await == old,
                boundary == "before-supersede-commit"
            );
            let retained_owner: (String, String, i64) = sqlx::query_as(
                "SELECT operation_id,association,sync_generation FROM local_e2ee_outbox",
            )
            .fetch_one(&mut *aven_core::test_support::acquire(&f.peer).await.unwrap())
            .await
            .unwrap();
            assert_eq!(retained_owner, owner);
            if image {
                let now: Vec<u8> =
                    sqlx::query_scalar("SELECT descriptor FROM local_e2ee_image_preparation")
                        .fetch_one(&mut *aven_core::test_support::acquire(&f.peer).await.unwrap())
                        .await
                        .unwrap();
                assert_eq!(
                    Some(now) == descriptor,
                    boundary == "before-supersede-commit"
                );
                assert_eq!(
                    scalar(&f.peer, "SELECT count(*) FROM local_e2ee_image_staging").await,
                    1
                );
                assert_eq!(
                    scalar(&f.peer, "SELECT count(*) FROM local_e2ee_image_preparation").await,
                    1
                );
            }
        }
        let new = frozen(&f.peer).await;
        let reopen = Database::open(f.peer.path()).await.unwrap();
        let store = isolated_store(&reopen, &f.root.path().join("peer-keys")).await;
        let inputs = store.tail_inputs(&reopen, &f.origin).await.unwrap();
        let (id, record) = reopen
            .encrypted_tail_frozen_record(&inputs.authority)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record, new);
        drop(inputs);
        let client = Client::new(&f.origin).unwrap();
        if image {
            image_rounds(&f, &third).await;
        } else {
            drain(&client, &store, &reopen).await;
        }
        let accepted: Vec<u8> =
            sqlx::query_scalar("SELECT record FROM local_e2ee_accepted WHERE operation_id=?")
                .bind(id)
                .fetch_one(&mut *aven_core::test_support::acquire(&reopen).await.unwrap())
                .await
                .unwrap();
        assert_eq!(accepted, new);
    }
}

#[tokio::test]
async fn signed_intervals_fence_fast_ack_pages_and_absence_with_observed_acceptance() {
    use sha2::{Digest, Sha256};
    let f = fixture().await;
    let third = join(&f, "third", &f.seed, &f.seed_store).await;
    pending_task(&f, "interval-negative pending task").await;
    let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
    let old = head_record(&f.peer, &inputs.authority).await;
    let (id, _) = f
        .peer
        .encrypted_tail_frozen_record(&inputs.authority)
        .await
        .unwrap()
        .unwrap();
    let stale_absence = inputs
        .authority
        .confirm_absent(&old, &Reply::Absent)
        .unwrap();
    assert!(
        inputs
            .authority
            .confirm_absent(&old, &Reply::Bootstrap)
            .is_err()
    );
    assert!(
        f.peer
            .reconcile_encrypted_tail_absence(&inputs.authority, &stale_absence, &blobs(&f.peer))
            .await
            .unwrap()
    );
    assert_eq!(frozen(&f.peer).await, old);
    drop(inputs);
    server_rotate(&f, &third, &[]).await;
    server_rotate(&f, &third, &[]).await;
    peer_enrollment_http::Client::new(&f.origin)
        .unwrap()
        .refresh(&f.peer_store, &f.peer)
        .await
        .unwrap();
    let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
    let a = &inputs.authority;
    assert!(
        f.peer
            .reconcile_encrypted_tail_absence(a, &stale_absence, &blobs(&f.peer))
            .await
            .is_err()
    );
    let high = a.membership.current_generation().starts_after as i64;
    let bad = Accepted {
        mapping: tail::Mapping {
            operation_id: id.clone(),
            sequence: high + 1,
            commitment: Sha256::digest(&old).into(),
        },
        record: old.clone(),
    };
    assert!(
        f.peer
            .observe_encrypted_tail(a, &bad.mapping)
            .await
            .is_err()
    );
    assert!(f.peer.verify_encrypted_tail_outcome(a, &bad).await.is_err());
    assert!(
        f.peer
            .apply_encrypted_tail_page(
                a,
                &tail::Page {
                    after: high,
                    watermark: high + 1,
                    cursor: high + 1,
                    has_more: false,
                    records: vec![bad]
                }
            )
            .await
            .is_err()
    );
    assert_eq!(
        scalar(
            &f.peer,
            "SELECT count(*) FROM local_e2ee_outbox WHERE observed_sequence IS NOT NULL"
        )
        .await,
        0
    );
    // A different immutable representation remains unknown until fetched and checked.
    let observed = tail::Mapping {
        operation_id: id,
        sequence: high + 1,
        commitment: [99; 32],
    };
    f.peer.observe_encrypted_tail(a, &observed).await.unwrap();
    let absence = a.confirm_absent(&old, &Reply::Absent).unwrap();
    assert!(
        f.peer
            .reconcile_encrypted_tail_absence(a, &absence, &blobs(&f.peer))
            .await
            .is_err()
    );
    assert_eq!(frozen(&f.peer).await, old);
    drop(inputs);
    assert!(
        Client::new(&f.origin)
            .unwrap()
            .round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
            .await
            .is_err()
    );
    assert_eq!(frozen(&f.peer).await, old);
}

#[tokio::test]
async fn rotation_during_historical_image_read_keeps_the_selected_download() {
    let mut f = fixture().await;
    attachments::add_image(&f).await;
    let client = Client::new(&f.origin).unwrap();
    client
        .round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
        .await
        .unwrap();
    let third = join(&f, "third", &f.seed, &f.seed_store).await;
    let driver = Joined {
        db: f.peer.clone(),
        store: isolated_store(&f.peer, &f.root.path().join("peer-keys")).await,
        blobs: f.root.path().join("peer-blobs"),
    };
    let (send, mut events) = tokio::sync::mpsc::channel(1);
    let fault = Arc::new(HttpFault {
        reads: Default::default(),
        lose: None,
        pause_operation: "Read",
        remaining: 1.into(),
        pause: Some(send),
    });
    restart_fault_server(&mut f, fault.clone()).await;
    let (result, ()) = tokio::join!(client.round(&third.store, &third.db, &third.blobs), async {
        let resume = events.recv().await.unwrap();
        server_rotate(&f, &driver, &[]).await;
        server_rotate(&f, &driver, &[]).await;
        resume.send(()).unwrap();
    });
    result.unwrap();
    let reads = fault.reads.lock().unwrap().clone();
    assert_eq!(reads.len(), 2);
    assert_eq!(reads[0], reads[1]);
    assert_eq!(
        floor(&third.store, &third.db, &f.origin).await.sequence(),
        7
    );
    assert_eq!(
        client
            .round(&third.store, &third.db, &third.blobs)
            .await
            .unwrap()
            .images,
        ImageTransfer::Complete
    );
}
