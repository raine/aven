use super::*;
use aven_core::sync::encrypted_tail::attachments::{
    self as image, Operation as Op, Reply as ImageReply, Ticket,
};

pub(super) async fn add_image(f: &Fixture) -> String {
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
        reservation: s.reservation.unwrap(),
    }
}
fn put(upload: &image::Upload, t: &Ticket) -> Op {
    Op::Put {
        workspace: upload.workspace.clone(),
        object: upload.object,
        descriptor_commitment: upload.commitment,
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
        .prepare_encrypted_push(a, &f.root.path().join("peer-blobs"))
        .await
        .unwrap()
        .unwrap()
        .upload
        .unwrap();
    let record = head_record(&f.peer, a).await;
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
    let renewed = declare(&f, a, &inputs.bearer, &upload).await;
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

/// Random per-device reservations, ticket expiry and prune's exclusion of
/// objects with live tickets reject stale uploaders across prune and
/// reactivation.
#[tokio::test]
async fn stale_image_uploaders_are_rejected_across_prune_expiry_and_reactivation() {
    let f = fixture().await;
    converge(&f).await;
    add_image(&f).await;
    let peer = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
    let seed = f.seed_store.tail_inputs(&f.seed, &f.origin).await.unwrap();
    let a = &peer.authority;
    let upload = f
        .peer
        .prepare_encrypted_push(a, &f.root.path().join("peer-blobs"))
        .await
        .unwrap()
        .unwrap()
        .upload
        .unwrap();
    assert_eq!(upload.records.len(), 1);
    let record = head_record(&f.peer, a).await;
    let peer_op = async |op| {
        f.server
            .encrypted_image_exchange(&a.context, &peer.bearer, op, policy())
            .await
    };
    let append = async |ticket| {
        f.server
            .encrypted_tail_exchange(
                &a.context,
                &peer.bearer,
                Operation::Append {
                    ticket,
                    record: record.clone(),
                },
            )
            .await
    };
    let prune = async || {
        let mut p = policy();
        p.grace = std::time::Duration::ZERO;
        f.server.prune_encrypted_images(p.grace, 128).await.unwrap()
    };
    let chunks = async || {
        scalar(
            &f.server,
            "SELECT count(*) FROM server_e2ee_image_chunks c JOIN server_e2ee_images i ON i.object=c.object WHERE i.bootstrap IS NULL",
        )
        .await
    };

    let old = declare(&f, a, &peer.bearer, &upload).await;
    peer_op(put(&upload, &old)).await.unwrap();
    // A live ticket keeps an object out of prune however long it was unreferenced.
    exec(
        &f.server,
        "UPDATE server_e2ee_images SET unreferenced_at=0 WHERE bootstrap IS NULL",
    )
    .await;
    assert_eq!(prune().await, 0);
    assert_eq!(chunks().await, 1);
    peer_op(complete(&upload, &old)).await.unwrap();

    // Expiry rejects the old uploader before and after its bytes are pruned.
    exec(
        &f.server,
        "UPDATE server_e2ee_image_tickets SET expires_at=0",
    )
    .await;
    assert!(peer_op(put(&upload, &old)).await.is_err());
    assert_eq!(prune().await, 1);
    assert_eq!(chunks().await, 0);
    assert!(peer_op(put(&upload, &old)).await.is_err());
    assert!(peer_op(complete(&upload, &old)).await.is_err());
    assert!(append(Some(old.clone())).await.is_err());

    // Reactivation by the same device replaces its reservation.
    let new = declare(&f, a, &peer.bearer, &upload).await;
    assert_ne!(new.reservation, old.reservation);
    assert!(peer_op(put(&upload, &old)).await.is_err());
    assert!(peer_op(complete(&upload, &old)).await.is_err());
    assert_eq!(chunks().await, 0);

    // A new uploader on another device holds its own ticket.
    let other = declare(&f, &seed.authority, &seed.bearer, &upload).await;
    assert!(peer_op(put(&upload, &other)).await.is_err());
    f.server
        .encrypted_image_exchange(
            &seed.authority.context,
            &seed.bearer,
            put(&upload, &other),
            policy(),
        )
        .await
        .unwrap();
    assert!(peer_op(complete(&upload, &old)).await.is_err());
    peer_op(put(&upload, &new)).await.unwrap();
    peer_op(complete(&upload, &new)).await.unwrap();
    assert!(append(Some(old)).await.is_err());
    let Reply::Appended(_) = append(Some(new)).await.unwrap() else {
        panic!("append")
    };
}

#[tokio::test]
async fn pruning_retains_mapping_and_exact_targeted_repair() {
    let f = fixture().await;
    converge(&f).await;
    let reference = add_image(&f).await;
    let c = Client::new(&f.origin).unwrap();
    assert!(
        c.round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
            .await
            .unwrap()
            .metadata_caught_up
    );
    let w = f.peer.list_workspaces().await.unwrap().remove(0);
    f.peer.delete_task_attachment(&w, &reference).await.unwrap();
    c.round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
        .await
        .unwrap();
    let (object,descriptor,old_bytes):(Vec<u8>,Vec<u8>,Vec<u8>)=sqlx::query_as("SELECT i.object,i.descriptor,c.bytes FROM server_e2ee_images i JOIN server_e2ee_image_chunks c ON c.object=i.object WHERE i.bootstrap IS NULL").fetch_one(&mut *aven_core::test_support::acquire(&f.server).await.unwrap()).await.unwrap();
    {
        let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
        let mut p = policy();
        p.grace = std::time::Duration::ZERO;
        let count = f.server.prune_encrypted_images(p.grace, 128).await.unwrap();
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
    c.round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
        .await
        .unwrap();
    exec(&f.server,"DELETE FROM server_e2ee_image_chunks WHERE object IN (SELECT object FROM server_e2ee_images WHERE bootstrap IS NULL)").await;
    let result = c
        .round(&f.seed_store, &f.seed, f.root.path())
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
            .round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
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
            .prepare_encrypted_push(a, &f.root.path().join("peer-blobs"))
            .await
            .unwrap()
            .unwrap()
            .upload
            .unwrap();
        frozen = head_record(&f.peer, a).await;
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
    assert_eq!(
        f.server
            .prune_encrypted_images(std::time::Duration::ZERO, 128)
            .await
            .unwrap(),
        1
    );
    exec(&f.peer, "DELETE FROM local_e2ee_image_staging").await;
    let sha: String = sqlx::query_scalar("SELECT sha256 FROM local_e2ee_image_preparation")
        .fetch_one(&mut *aven_core::test_support::acquire(&f.peer).await.unwrap())
        .await
        .unwrap();
    std::fs::remove_file(f.root.path().join("peer-blobs/objects/sha256").join(sha)).unwrap();
    let result = c
        .round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
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
        .prepare_encrypted_push(a, &f.root.path().join("peer-blobs"))
        .await
        .unwrap()
        .unwrap()
        .upload
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
    assert_eq!(
        f.server
            .prune_encrypted_images(prune.grace, 128)
            .await
            .unwrap(),
        0
    );
    let record = head_record(&f.peer, a).await;
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
        let result = e2ee_http::worker("encrypted_tail_http::tests::process_worker")
            .env("AVEN_TAIL_ROOT", f.root.path())
            .env("AVEN_TAIL_ORIGIN", &f.origin)
            .env("AVEN_TAIL_CRASH", stage)
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
        let store = isolated_store(&db, &f.root.path().join("peer-keys")).await;
        let client = Client::new(&f.origin).unwrap();
        let resumed = client
            .round(&store, &db, &f.root.path().join("peer-blobs"))
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
            .round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
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
    assert_eq!(
        f.server
            .prune_encrypted_images(std::time::Duration::ZERO, 128)
            .await
            .unwrap(),
        0
    );
    f.peer.delete_task_attachment(&w, &reference).await.unwrap();
    client
        .round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
        .await
        .unwrap();
    assert_eq!(
        f.server
            .prune_encrypted_images(std::time::Duration::ZERO, 128)
            .await
            .unwrap(),
        1
    );
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
        .round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
        .await
        .unwrap();
    assert_eq!(result.images, ImageTransfer::Complete);
    f.peer.delete_task_attachment(&w, &reference).await.unwrap();
    c.round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
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
async fn pending_image_head_and_later_task_edit_converge_and_missing_source_still_pulls() {
    let f = fixture().await;
    converge(&f).await;
    let reference = add_image(&f).await;
    let (task, sha): (String, String) =
        sqlx::query_as("SELECT task_id,sha256 FROM task_attachments WHERE attachment_id=?")
            .bind(&reference)
            .fetch_one(&mut *aven_core::test_support::acquire(&f.peer).await.unwrap())
            .await
            .unwrap();
    let w = f.peer.list_workspaces().await.unwrap().remove(0);
    f.peer
        .update_task(
            &w,
            &task.parse().unwrap(),
            TaskUpdate {
                title: Some("edited behind pending image".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let pending: Vec<String> = sqlx::query_scalar(
        "SELECT op_type FROM changes WHERE server_seq IS NULL
         ORDER BY local_seq, created_at, change_id",
    )
    .fetch_all(&mut *aven_core::test_support::acquire(&f.peer).await.unwrap())
    .await
    .unwrap();
    assert_eq!(pending, ["attachment_add", "set_field"]);
    let c = Client::new(&f.origin).unwrap();
    // An unavailable local source keeps the ordered head pending but still pulls.
    let source = f.root.path().join("peer-blobs/objects/sha256").join(&sha);
    let bytes = std::fs::read(&source).unwrap();
    std::fs::remove_file(&source).unwrap();
    let remote = f
        .seed
        .create_task(
            &f.seed.list_workspaces().await.unwrap().remove(0),
            draft("remote while image source is missing"),
        )
        .await
        .unwrap()
        .task;
    drain(&c, &f.seed_store, &f.seed).await;
    let blocked = c
        .round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
        .await
        .unwrap();
    assert_eq!(blocked.images, ImageTransfer::Failed);
    assert!(!blocked.metadata_caught_up);
    assert_eq!(
        title(&f.peer, remote.id.as_str()).await,
        "remote while image source is missing"
    );
    assert_eq!(
        scalar(&f.peer, "SELECT count(*) FROM local_e2ee_outbox").await,
        0
    );
    assert_eq!(
        scalar(
            &f.peer,
            "SELECT count(*) FROM changes WHERE server_seq IS NULL"
        )
        .await,
        2
    );
    std::fs::write(&source, bytes).unwrap();
    let mut rounds = 0;
    while !c
        .round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
        .await
        .unwrap()
        .metadata_caught_up
    {
        rounds += 1;
        assert!(rounds < 4, "pending head budget");
    }
    assert_eq!(
        scalar(
            &f.peer,
            "SELECT count(*) FROM changes WHERE server_seq IS NULL"
        )
        .await,
        0
    );
    let mut rounds = 0;
    loop {
        let round = c
            .round(&f.seed_store, &f.seed, f.root.path())
            .await
            .unwrap();
        if round.metadata_caught_up && round.images == ImageTransfer::Complete {
            break;
        }
        rounds += 1;
        assert!(rounds < 4, "receiver budget");
    }
    assert_eq!(title(&f.seed, &task).await, "edited behind pending image");
    assert_eq!(
        std::fs::read(f.root.path().join("objects/sha256").join(&sha)).unwrap(),
        std::fs::read(f.root.path().join("peer-blobs/objects/sha256").join(&sha)).unwrap()
    );
}

#[tokio::test]
async fn shared_refs_count_once_and_last_unref_starts_grace() {
    let f = fixture().await;
    converge(&f).await;
    let c = Client::new(&f.origin).unwrap();
    let first = add_image(&f).await;
    c.round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
        .await
        .unwrap();
    let second = add_image(&f).await;
    c.round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
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
    c.round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
        .await
        .unwrap();
    assert_eq!(scalar_after_prune_pass(&f.server,"SELECT count(*) FROM server_e2ee_images WHERE bootstrap IS NULL AND unreferenced_at IS NULL").await,1);
    f.peer.delete_task_attachment(&w, &second).await.unwrap();
    c.round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
        .await
        .unwrap();
    assert_eq!(scalar_after_prune_pass(&f.server,"SELECT count(*) FROM server_e2ee_images WHERE bootstrap IS NULL AND unreferenced_at IS NOT NULL").await,1);
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
            .round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
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
            .round(&f.seed_store, &receiver, f.root.path())
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
    // Observing pending demand never consumes a selection turn.
    let selector = f.seed.meta("e2ee_image_download_after").await.unwrap();
    let inputs = f.seed_store.tail_inputs(&f.seed, &f.origin).await.unwrap();
    for _ in 0..2 {
        let state = f
            .seed
            .encrypted_round_state(&inputs.authority)
            .await
            .unwrap();
        assert!(state.downloads.unwrap().pending);
    }
    drop(inputs);
    assert_eq!(
        f.seed.meta("e2ee_image_download_after").await.unwrap(),
        selector
    );
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

#[tokio::test]
async fn cli_drain_stops_promptly_behind_missing_local_image_and_still_pulls() {
    let f = fixture().await;
    converge(&f).await;
    let reference = add_image(&f).await;
    let (task, sha): (String, String) =
        sqlx::query_as("SELECT task_id,sha256 FROM task_attachments WHERE attachment_id=?")
            .bind(&reference)
            .fetch_one(&mut *aven_core::test_support::acquire(&f.peer).await.unwrap())
            .await
            .unwrap();
    let w = f.peer.list_workspaces().await.unwrap().remove(0);
    f.peer
        .update_task(
            &w,
            &task.parse().unwrap(),
            TaskUpdate {
                title: Some("edited behind missing image".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    std::fs::remove_file(f.root.path().join("peer-blobs/objects/sha256").join(&sha)).unwrap();
    let c = Client::new(&f.origin).unwrap();
    let remote = f
        .seed
        .create_task(
            &f.seed.list_workspaces().await.unwrap().remove(0),
            draft("remote behind missing image"),
        )
        .await
        .unwrap()
        .task;
    drain(&c, &f.seed_store, &f.seed).await;
    let outcome = crate::sync::encrypted::drain(
        &c,
        &f.peer_store,
        &f.peer,
        &f.root.path().join("peer-blobs"),
        crate::sync::encrypted::ROUND_LIMIT,
    )
    .await
    .unwrap();
    assert_eq!(outcome.rounds, 16);
    assert!(!outcome.metadata_caught_up);
    assert_eq!(outcome.images, "failed");
    assert_eq!(
        title(&f.peer, remote.id.as_str()).await,
        "remote behind missing image"
    );
    let pending: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT entity_type, op_type, field FROM changes
         WHERE server_seq IS NULL ORDER BY local_seq",
    )
    .fetch_all(&mut *aven_core::test_support::acquire(&f.peer).await.unwrap())
    .await
    .unwrap();
    assert_eq!(
        pending,
        vec![
            (
                "task".into(),
                "attachment_add".into(),
                Some("attachments".into())
            ),
            ("task".into(), "set_field".into(), Some("title".into())),
            ("device".into(), "publish_device_label".into(), None),
        ]
    );
}
