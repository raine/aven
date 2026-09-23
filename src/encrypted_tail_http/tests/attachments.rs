use super::*;
use aven_core::sync::encrypted_tail::attachments::{
    self as image, Operation as Op, Reply as ImageReply, Ticket,
};

async fn add_image(f: &Fixture) -> String {
    add_image_with_width(f, 7).await
}

async fn add_image_with_width(f: &Fixture, width: u32) -> String {
    let w = f.peer.list_workspaces().await.unwrap().remove(0);
    let task = f
        .peer
        .create_task(&w, draft("attachment parent"))
        .await
        .unwrap()
        .task;
    drain(&Client::new(&f.origin).unwrap(), &f.peer_store, &f.peer).await;
    let mut bytes = std::io::Cursor::new(Vec::new());
    ::image::DynamicImage::new_rgb8(width, 3)
        .write_to(&mut bytes, ::image::ImageFormat::Png)
        .unwrap();
    f.peer
        .add_task_attachment(
            &w,
            &f.root.path().join("peer-blobs"),
            Default::default(),
            &task.id,
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
        .unwrap()
        .outcome
        .attachment
        .attachment_id
}
fn policy() -> aven_core::attachments::LifecyclePolicy {
    crate::config::AttachmentLifecycleConfig::default().server_policy()
}
async fn exec(db: &Database, sql: &str) {
    sqlx::query(sqlx::AssertSqlSafe(sql.to_owned()))
        .execute(&mut *aven_core::test_support::acquire(db).await.unwrap())
        .await
        .unwrap();
}
async fn declare(
    f: &Fixture,
    a: &tail::Authority,
    bearer: &Secret,
    upload: &image::Upload,
) -> Ticket {
    let ImageReply::Status(s) = f
        .server
        .encrypted_image_exchange(
            &a.context,
            bearer,
            Op::Declare {
                workspace: upload.workspace.clone(),
                descriptor: upload.descriptor.clone(),
            },
            policy(),
        )
        .await
        .unwrap()
    else {
        panic!("status")
    };
    Ticket {
        epoch: s.epoch,
        reservation: s.reservation.unwrap(),
    }
}
fn put(upload: &image::Upload, t: &Ticket) -> Op {
    Op::Put {
        workspace: upload.workspace.clone(),
        object: upload.object,
        descriptor_commitment: upload.commitment,
        epoch: t.epoch,
        reservation: t.reservation,
        index: 0,
        record: upload.records[0].clone(),
    }
}
fn complete(upload: &image::Upload, t: &Ticket) -> Op {
    Op::Complete {
        workspace: upload.workspace.clone(),
        object: upload.object,
        descriptor_commitment: upload.commitment,
        epoch: t.epoch,
        reservation: t.reservation,
    }
}

#[tokio::test]
async fn incomplete_ref_ticket_ownership_expiry_and_exact_retry() {
    let f = fixture().await;
    converge(&f).await;
    add_image(&f).await;
    let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
    let a = &inputs.authority;
    let upload = f
        .peer
        .prepare_encrypted_image(a, &f.root.path().join("peer-blobs"))
        .await
        .unwrap()
        .unwrap();
    let record = f.peer.prepare_encrypted_tail(a).await.unwrap().unwrap();
    let ticket = declare(&f, a, &inputs.bearer, &upload).await;
    assert!(
        f.server
            .encrypted_tail_exchange(
                &a.context,
                &inputs.bearer,
                Operation::Append {
                    ticket: Some(ticket.clone()),
                    record: record.clone()
                }
            )
            .await
            .is_err()
    );
    let before = scalar(&f.server, "SELECT count(*) FROM server_e2ee_tail").await;
    let other = f.seed_store.tail_inputs(&f.seed, &f.origin).await.unwrap();
    assert!(
        f.server
            .encrypted_image_exchange(
                &other.authority.context,
                &other.bearer,
                put(&upload, &ticket),
                policy()
            )
            .await
            .is_err()
    );
    for _ in 0..2 {
        f.server
            .encrypted_image_exchange(&a.context, &inputs.bearer, put(&upload, &ticket), policy())
            .await
            .unwrap();
    }
    f.server
        .encrypted_image_exchange(
            &a.context,
            &inputs.bearer,
            complete(&upload, &ticket),
            policy(),
        )
        .await
        .unwrap();
    assert!(
        f.server
            .encrypted_tail_exchange(
                &a.context,
                &inputs.bearer,
                Operation::Append {
                    ticket: None,
                    record: record.clone()
                }
            )
            .await
            .is_err()
    );
    exec(
        &f.server,
        "UPDATE server_e2ee_image_tickets SET expires_at=0",
    )
    .await;
    assert!(
        f.server
            .encrypted_image_exchange(&a.context, &inputs.bearer, put(&upload, &ticket), policy())
            .await
            .is_err()
    );
    let ImageReply::Status(renewed) = f
        .server
        .encrypted_image_exchange(
            &a.context,
            &inputs.bearer,
            Op::Ensure {
                workspace: upload.workspace.clone(),
                object: upload.object,
                descriptor_commitment: upload.commitment,
                expected_epoch: ticket.epoch,
            },
            policy(),
        )
        .await
        .unwrap()
    else {
        panic!("status")
    };
    let renewed = Ticket {
        epoch: renewed.epoch,
        reservation: renewed.reservation.unwrap(),
    };
    assert_ne!(ticket, renewed);
    assert!(
        f.server
            .encrypted_image_exchange(
                &a.context,
                &inputs.bearer,
                Op::Release {
                    workspace: upload.workspace.clone(),
                    object: upload.object,
                    descriptor_commitment: upload.commitment,
                    epoch: ticket.epoch,
                    reservation: ticket.reservation
                },
                policy()
            )
            .await
            .is_err()
    );
    let Reply::Appended(mapping) = f
        .server
        .encrypted_tail_exchange(
            &a.context,
            &inputs.bearer,
            Operation::Append {
                ticket: Some(renewed),
                record: record.clone(),
            },
        )
        .await
        .unwrap()
    else {
        panic!("append")
    };
    let Reply::Appended(retry) = f
        .server
        .encrypted_tail_exchange(
            &a.context,
            &inputs.bearer,
            Operation::Append {
                ticket: None,
                record,
            },
        )
        .await
        .unwrap()
    else {
        panic!("append")
    };
    assert!(mapping == retry);
    assert_eq!(
        scalar(&f.server, "SELECT count(*) FROM server_e2ee_tail").await,
        before + 1
    );
}

#[tokio::test]
async fn pruning_retains_mapping_and_exact_targeted_repair() {
    let f = fixture().await;
    converge(&f).await;
    let reference = add_image(&f).await;
    let c = Client::new(&f.origin).unwrap();
    assert!(
        c.attachment_round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
            .await
            .unwrap()
            .metadata_caught_up
    );
    let w = f.peer.list_workspaces().await.unwrap().remove(0);
    f.peer.delete_task_attachment(&w, &reference).await.unwrap();
    c.attachment_round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
        .await
        .unwrap();
    let (object,descriptor,old_bytes):(Vec<u8>,Vec<u8>,Vec<u8>)=sqlx::query_as("SELECT i.object,i.descriptor,c.bytes FROM server_e2ee_images i JOIN server_e2ee_image_chunks c ON c.object=i.object WHERE i.bootstrap IS NULL").fetch_one(&mut *aven_core::test_support::acquire(&f.server).await.unwrap()).await.unwrap();
    {
        let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
        let mut p = policy();
        p.grace = std::time::Duration::ZERO;
        let ImageReply::Pruned(count) = f
            .server
            .encrypted_image_exchange(
                &inputs.authority.context,
                &inputs.bearer,
                Op::Prune { limit: 128 },
                p,
            )
            .await
            .unwrap()
        else {
            panic!("prune")
        };
        assert_eq!(count, 1);
        let mut changed = descriptor.clone();
        let last = changed.len() - 1;
        changed[last] ^= 1;
        assert!(
            f.server
                .encrypted_image_exchange(
                    &inputs.authority.context,
                    &inputs.bearer,
                    Op::Declare {
                        workspace: w.id.to_string(),
                        descriptor: changed
                    },
                    policy()
                )
                .await
                .is_err()
        );
    }
    c.repair_attachment(
        &f.peer_store,
        &f.peer,
        &f.root.path().join("peer-blobs"),
        w.id.as_str(),
        &reference,
    )
    .await
    .unwrap();
    let restored: Vec<u8> =
        sqlx::query_scalar("SELECT bytes FROM server_e2ee_image_chunks WHERE object=?")
            .bind(&object)
            .fetch_one(&mut *aven_core::test_support::acquire(&f.server).await.unwrap())
            .await
            .unwrap();
    assert_eq!(restored, old_bytes);
    assert_eq!(
        scalar(
            &f.server,
            "SELECT count(*) FROM server_e2ee_image_references WHERE deleted=1"
        )
        .await,
        1
    );
}

#[tokio::test]
async fn metadata_commits_when_remote_image_is_unavailable() {
    let f = fixture().await;
    converge(&f).await;
    add_image(&f).await;
    let c = Client::new(&f.origin).unwrap();
    c.attachment_round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
        .await
        .unwrap();
    exec(&f.server,"DELETE FROM server_e2ee_image_chunks WHERE object IN (SELECT object FROM server_e2ee_images WHERE bootstrap IS NULL)").await;
    let result = c
        .attachment_round(&f.seed_store, &f.seed, f.root.path())
        .await
        .unwrap();
    assert!(result.metadata_caught_up);
    assert_eq!(result.images, ImageTransfer::Unavailable);
    assert_eq!(
        scalar(&f.seed, "SELECT count(*) FROM task_attachments").await,
        2
    );
    assert_eq!(
        scalar(
            &f.seed,
            "SELECT count(*) FROM local_e2ee_image_objects WHERE verified=0"
        )
        .await,
        1
    );
}

#[tokio::test]
async fn missing_initialization_refuses_without_erasing_domain() {
    let f = fixture().await;
    converge(&f).await;
    let count = scalar(&f.peer, "SELECT count(*) FROM tasks").await;
    exec(&f.peer, "DELETE FROM local_e2ee_image_initialization").await;
    assert!(
        Client::new(&f.origin)
            .unwrap()
            .attachment_round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
            .await
            .is_err()
    );
    assert_eq!(scalar(&f.peer, "SELECT count(*) FROM tasks").await, count);
}

#[tokio::test]
async fn lost_ref_ack_after_unref_and_prune_needs_no_upload_source() {
    let f = fixture().await;
    converge(&f).await;
    let reference = add_image(&f).await;
    let c = Client::new(&f.origin).unwrap();
    let frozen;
    {
        let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
        let a = &inputs.authority;
        let upload = f
            .peer
            .prepare_encrypted_image(a, &f.root.path().join("peer-blobs"))
            .await
            .unwrap()
            .unwrap();
        frozen = f.peer.prepare_encrypted_tail(a).await.unwrap().unwrap();
        let ticket = declare(&f, a, &inputs.bearer, &upload).await;
        f.server
            .encrypted_image_exchange(&a.context, &inputs.bearer, put(&upload, &ticket), policy())
            .await
            .unwrap();
        f.server
            .encrypted_image_exchange(
                &a.context,
                &inputs.bearer,
                complete(&upload, &ticket),
                policy(),
            )
            .await
            .unwrap();
        f.server
            .encrypted_tail_exchange(
                &a.context,
                &inputs.bearer,
                Operation::Append {
                    record: frozen.clone(),
                    ticket: Some(ticket),
                },
            )
            .await
            .unwrap();
    }
    assert!(c.pull_only_round(&f.seed_store, &f.seed).await.unwrap());
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    f.seed.delete_task_attachment(&w, &reference).await.unwrap();
    drain(&c, &f.seed_store, &f.seed).await;
    {
        let inputs = f.seed_store.tail_inputs(&f.seed, &f.origin).await.unwrap();
        let mut p = policy();
        p.grace = std::time::Duration::ZERO;
        assert!(matches!(
            f.server
                .encrypted_image_exchange(
                    &inputs.authority.context,
                    &inputs.bearer,
                    Op::Prune { limit: 128 },
                    p
                )
                .await
                .unwrap(),
            ImageReply::Pruned(1)
        ));
    }
    exec(&f.peer, "DELETE FROM local_e2ee_image_staging").await;
    let sha: String = sqlx::query_scalar("SELECT sha256 FROM local_e2ee_image_preparation")
        .fetch_one(&mut *aven_core::test_support::acquire(&f.peer).await.unwrap())
        .await
        .unwrap();
    std::fs::remove_file(f.root.path().join("peer-blobs/objects/sha256").join(sha)).unwrap();
    let result = c
        .attachment_round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
        .await
        .unwrap();
    assert!(result.metadata_caught_up);
    assert_eq!(
        scalar(&f.peer, "SELECT count(*) FROM local_e2ee_outbox").await,
        0
    );
    assert_eq!(
        scalar(
            &f.server,
            "SELECT count(*) FROM server_e2ee_image_references WHERE deleted=1"
        )
        .await,
        1
    );
    let accepted:Vec<u8>=sqlx::query_scalar("SELECT record FROM local_e2ee_accepted WHERE operation_id=(SELECT created_by_change_id FROM task_attachments WHERE attachment_id=?)").bind(reference).fetch_one(&mut *aven_core::test_support::acquire(&f.peer).await.unwrap()).await.unwrap();
    assert_eq!(accepted, frozen);
}

#[tokio::test]
async fn reservation_promises_shared_accounting_and_put_rollback() {
    let f = fixture().await;
    converge(&f).await;
    add_image(&f).await;
    let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
    let a = &inputs.authority;
    let upload = f
        .peer
        .prepare_encrypted_image(a, &f.root.path().join("peer-blobs"))
        .await
        .unwrap()
        .unwrap();
    let ticket = declare(&f, a, &inputs.bearer, &upload).await;
    let other = f.seed_store.tail_inputs(&f.seed, &f.origin).await.unwrap();
    let mut over = policy();
    over.quota_bytes = 0;
    let same = Op::Declare {
        workspace: upload.workspace.clone(),
        descriptor: upload.descriptor.clone(),
    };
    assert!(
        f.server
            .encrypted_image_exchange(&other.authority.context, &other.bearer, same, over)
            .await
            .is_ok()
    );
    let mut different = upload.descriptor.clone();
    different[104] ^= 1;
    assert!(
        f.server
            .encrypted_image_exchange(
                &a.context,
                &inputs.bearer,
                Op::Declare {
                    workspace: upload.workspace.clone(),
                    descriptor: different
                },
                over
            )
            .await
            .is_err()
    );
    exec(&f.server,"CREATE TRIGGER fail_image_put BEFORE INSERT ON server_e2ee_image_chunks BEGIN SELECT RAISE(ABORT,'fault'); END").await;
    assert!(
        f.server
            .encrypted_image_exchange(&a.context, &inputs.bearer, put(&upload, &ticket), policy())
            .await
            .is_err()
    );
    exec(&f.server, "DROP TRIGGER fail_image_put").await;
    f.server
        .encrypted_image_exchange(&a.context, &inputs.bearer, put(&upload, &ticket), over)
        .await
        .unwrap();
    f.server
        .encrypted_image_exchange(&a.context, &inputs.bearer, complete(&upload, &ticket), over)
        .await
        .unwrap();
    let mut prune = over;
    prune.grace = std::time::Duration::ZERO;
    assert!(matches!(
        f.server
            .encrypted_image_exchange(&a.context, &inputs.bearer, Op::Prune { limit: 128 }, prune)
            .await
            .unwrap(),
        ImageReply::Pruned(0)
    ));
    let record = f.peer.prepare_encrypted_tail(a).await.unwrap().unwrap();
    f.server
        .encrypted_tail_exchange(
            &a.context,
            &inputs.bearer,
            Operation::Append {
                record,
                ticket: Some(ticket),
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn image_process_exit_retries_exact_preparation_put_and_admission() {
    for stage in ["image-frozen", "image-put", "after-append"] {
        let f = fixture().await;
        converge(&f).await;
        add_image(&f).await;
        let result = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "encrypted_tail_http::tests::process_worker",
                "--ignored",
                "--nocapture",
            ])
            .env("AVEN_TAIL_ROOT", f.root.path())
            .env("AVEN_TAIL_ORIGIN", &f.origin)
            .env("AVEN_TAIL_CRASH", stage)
            .env("AVEN_TAIL_IMAGES", "1")
            .output()
            .await
            .unwrap();
        assert_eq!(
            result.status.code(),
            Some(84),
            "{stage}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let frozen: Vec<u8> = sqlx::query_scalar("SELECT record FROM local_e2ee_outbox")
            .fetch_one(&mut *aven_core::test_support::acquire(&f.peer).await.unwrap())
            .await
            .unwrap();
        let db = Database::open(f.peer.path()).await.unwrap();
        let store = isolated_store(db.path(), &f.root.path().join("peer-keys"));
        let client = Client::new(&f.origin).unwrap();
        let resumed = client
            .attachment_round(&store, &db, &f.root.path().join("peer-blobs"))
            .await
            .unwrap();
        assert!(resumed.metadata_caught_up);
        assert_eq!(resumed.images, ImageTransfer::Complete);
        let accepted: Vec<u8> = sqlx::query_scalar(
            "SELECT record FROM local_e2ee_accepted ORDER BY sequence DESC LIMIT 1",
        )
        .fetch_one(&mut *aven_core::test_support::acquire(&db).await.unwrap())
        .await
        .unwrap();
        assert_eq!(accepted, frozen);
    }
}

#[tokio::test]
async fn ref_hint_disagreement_is_sticky_and_explicit_unref_releases() {
    let f = fixture().await;
    converge(&f).await;
    let reference = add_image(&f).await;
    let w = f.peer.list_workspaces().await.unwrap().remove(0);
    let task: aven_core::ids::TaskId =
        sqlx::query_scalar("SELECT task_id FROM task_attachments WHERE attachment_id=?")
            .bind(&reference)
            .fetch_one(&mut *aven_core::test_support::acquire(&f.peer).await.unwrap())
            .await
            .unwrap();
    // The queued Ref precedes this deletion, but preparation observes the newer
    // local version. The server must not turn that hint into deletion evidence.
    f.peer
        .update_task(
            &w,
            &task,
            TaskUpdate {
                deleted: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let client = Client::new(&f.origin).unwrap();
    for _ in 0..3 {
        client
            .attachment_round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
            .await
            .unwrap();
    }
    assert_eq!(
        scalar(
            &f.server,
            "SELECT count(*) FROM server_e2ee_image_parents WHERE protected=1"
        )
        .await,
        1
    );
    {
        let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
        let mut p = policy();
        p.grace = std::time::Duration::ZERO;
        assert!(matches!(
            f.server
                .encrypted_image_exchange(
                    &inputs.authority.context,
                    &inputs.bearer,
                    Op::Prune { limit: 128 },
                    p
                )
                .await
                .unwrap(),
            ImageReply::Pruned(0)
        ));
    }
    f.peer.delete_task_attachment(&w, &reference).await.unwrap();
    client
        .attachment_round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
        .await
        .unwrap();
    let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
    let mut p = policy();
    p.grace = std::time::Duration::ZERO;
    assert!(matches!(
        f.server
            .encrypted_image_exchange(
                &inputs.authority.context,
                &inputs.bearer,
                Op::Prune { limit: 128 },
                p
            )
            .await
            .unwrap(),
        ImageReply::Pruned(1)
    ));
}

#[tokio::test]
async fn explicitly_unmapped_bootstrap_reference_is_initialized_and_deletable() {
    let f = fixture_with_image_availability(false, None, false, false, true).await;
    let (reference, task): (String, aven_core::ids::TaskId) =
        sqlx::query_as("SELECT attachment_id,task_id FROM task_attachments")
            .fetch_one(&mut *aven_core::test_support::acquire(&f.peer).await.unwrap())
            .await
            .unwrap();
    assert_eq!(
        scalar(
            &f.peer,
            "SELECT count(*) FROM local_e2ee_image_initialization"
        )
        .await,
        1
    );
    assert_eq!(
        scalar(
            &f.peer,
            "SELECT count(*) FROM local_e2ee_image_references WHERE object IS NULL"
        )
        .await,
        1
    );
    let w = f.peer.list_workspaces().await.unwrap().remove(0);
    f.peer
        .update_task(
            &w,
            &task,
            TaskUpdate {
                deleted: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let c = Client::new(&f.origin).unwrap();
    let result = c
        .attachment_round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
        .await
        .unwrap();
    assert_eq!(result.images, ImageTransfer::Complete);
    f.peer.delete_task_attachment(&w, &reference).await.unwrap();
    c.attachment_round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
        .await
        .unwrap();
    assert_eq!(
        scalar(
            &f.server,
            "SELECT count(*) FROM server_e2ee_image_references WHERE deleted=1 AND object IS NULL"
        )
        .await,
        1
    );
    assert!(
        c.repair_attachment(
            &f.peer_store,
            &f.peer,
            &f.root.path().join("peer-blobs"),
            w.id.as_str(),
            &reference
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn shared_refs_count_once_and_last_unref_starts_grace() {
    let f = fixture().await;
    converge(&f).await;
    let c = Client::new(&f.origin).unwrap();
    let first = add_image(&f).await;
    c.attachment_round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
        .await
        .unwrap();
    let second = add_image(&f).await;
    c.attachment_round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
        .await
        .unwrap();
    assert_eq!(
        scalar(
            &f.server,
            "SELECT count(*) FROM server_e2ee_images WHERE bootstrap IS NULL"
        )
        .await,
        1
    );
    let w = f.peer.list_workspaces().await.unwrap().remove(0);
    f.peer.delete_task_attachment(&w, &first).await.unwrap();
    c.attachment_round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
        .await
        .unwrap();
    assert_eq!(scalar(&f.server,"SELECT count(*) FROM server_e2ee_images WHERE bootstrap IS NULL AND unreferenced_at IS NULL").await,1);
    f.peer.delete_task_attachment(&w, &second).await.unwrap();
    c.attachment_round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
        .await
        .unwrap();
    assert_eq!(scalar(&f.server,"SELECT count(*) FROM server_e2ee_images WHERE bootstrap IS NULL AND unreferenced_at IS NOT NULL").await,1);
}

#[tokio::test]
async fn unavailable_first_image_does_not_starve_later_downloads() {
    failed_first_image_does_not_starve_later_downloads(false).await;
}

#[tokio::test]
async fn corrupt_first_image_does_not_starve_later_downloads() {
    failed_first_image_does_not_starve_later_downloads(true).await;
}

async fn failed_first_image_does_not_starve_later_downloads(corrupt: bool) {
    let f = fixture().await;
    converge(&f).await;
    let client = Client::new(&f.origin).unwrap();
    for width in [7, 8] {
        add_image_with_width(&f, width).await;
        let result = client
            .attachment_round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
            .await
            .unwrap();
        assert!(result.metadata_caught_up);
        assert_eq!(result.images, ImageTransfer::Complete);
    }
    let objects: Vec<(Vec<u8>, String)> = sqlx::query_as(
        "SELECT object,sha256 FROM local_e2ee_image_objects WHERE origin!='bootstrap' ORDER BY object",
    ).fetch_all(&mut *aven_core::test_support::acquire(&f.peer).await.unwrap()).await.unwrap();
    assert_eq!(objects.len(), 2);
    let fault = if corrupt {
        "UPDATE server_e2ee_image_chunks SET bytes=zeroblob(length(bytes)) WHERE object=?"
    } else {
        "DELETE FROM server_e2ee_image_chunks WHERE object=?"
    };
    sqlx::query(sqlx::AssertSqlSafe(fault.to_owned()))
        .bind(&objects[0].0)
        .execute(&mut *aven_core::test_support::acquire(&f.server).await.unwrap())
        .await
        .unwrap();
    let missing_path = f.root.path().join("objects/sha256").join(&objects[0].1);
    let available_path = f.root.path().join("objects/sha256").join(&objects[1].1);
    assert!(!missing_path.exists() && !available_path.exists());
    for round in 0..3 {
        // Reopening the receiver cannot reset selection to the failing object.
        let receiver = Database::open(f.seed.path()).await.unwrap();
        let result = Client::new(&f.origin)
            .unwrap()
            .attachment_round(&f.seed_store, &receiver, f.root.path())
            .await
            .unwrap();
        assert!(result.metadata_caught_up);
        assert_ne!(result.images, ImageTransfer::Complete);
        if round == 0 {
            assert_eq!(
                result.images,
                if corrupt {
                    ImageTransfer::Failed
                } else {
                    ImageTransfer::Unavailable
                }
            );
            assert_eq!(
                scalar(&f.seed, "SELECT count(*) FROM task_attachments").await,
                3
            );
        }
    }
    assert!(
        available_path.exists(),
        "an unavailable earlier object must not starve this image"
    );
    assert!(!missing_path.exists());
    let states: Vec<(String, bool)> = sqlx::query_as(
        "SELECT sha256,verified FROM local_e2ee_image_objects WHERE origin!='bootstrap' ORDER BY object",
    ).fetch_all(&mut *aven_core::test_support::acquire(&f.seed).await.unwrap()).await.unwrap();
    assert_eq!(
        states,
        vec![(objects[0].1.clone(), false), (objects[1].1.clone(), true)]
    );
    assert_eq!(
        std::fs::read(available_path).unwrap(),
        std::fs::read(
            f.root
                .path()
                .join("peer-blobs/objects/sha256")
                .join(&objects[1].1),
        )
        .unwrap()
    );
}
