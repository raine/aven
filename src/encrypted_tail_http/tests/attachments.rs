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
    add_image_to_task(f, &w, &task.id, width).await
}

async fn add_image_to_task(
    f: &Fixture,
    workspace: &aven_core::workspaces::Workspace,
    task: &aven_core::ids::TaskId,
    width: u32,
) -> String {
    add_image_to_database(
        &f.peer,
        &f.root.path().join("peer-blobs"),
        workspace,
        task,
        width,
    )
    .await
}

async fn add_image_to_database(
    db: &Database,
    blob_dir: &std::path::Path,
    workspace: &aven_core::workspaces::Workspace,
    task: &aven_core::ids::TaskId,
    width: u32,
) -> String {
    let mut bytes = std::io::Cursor::new(Vec::new());
    ::image::DynamicImage::new_rgb8(width, 3)
        .write_to(&mut bytes, ::image::ImageFormat::Png)
        .unwrap();
    db.add_task_attachment(
        workspace,
        blob_dir,
        Default::default(),
        task,
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

async fn add_image_batch(f: &Fixture, count: usize) {
    let workspace = f.peer.list_workspaces().await.unwrap().remove(0);
    let task = f
        .peer
        .create_task(&workspace, draft("attachment batch parent"))
        .await
        .unwrap()
        .task;
    drain(&Client::new(&f.origin).unwrap(), &f.peer_store, &f.peer).await;
    for index in 0..count {
        add_image_to_task(f, &workspace, &task.id, 20 + index as u32).await;
    }
}
fn policy() -> aven_core::attachments::LifecyclePolicy {
    crate::server_config::AttachmentsConfig::default().policy()
}

async fn configured_quota_case() -> (Fixture, Client, String, String, String, i64) {
    let f = fixture().await;
    converge(&f).await;

    let workspace = f.seed.list_workspaces().await.unwrap().remove(0);
    let filler_task = f
        .seed
        .create_task(&workspace, draft("local quota filler"))
        .await
        .unwrap()
        .task;
    let filler_reference =
        add_image_to_database(&f.seed, f.root.path(), &workspace, &filler_task.id, 8).await;
    let (filler_hash, filler_bytes): (String, i64) =
        sqlx::query_as("SELECT sha256,byte_size FROM task_attachments WHERE attachment_id=?")
            .bind(&filler_reference)
            .fetch_one(&mut *aven_core::test_support::acquire(&f.seed).await.unwrap())
            .await
            .unwrap();
    let used_before: i64 = scalar(
        &f.seed,
        "SELECT COALESCE(SUM(byte_size),0) FROM blob_inventory WHERE available=1",
    )
    .await;
    assert!(used_before >= filler_bytes && filler_bytes > 1);

    let incoming_reference = add_image(&f).await;
    let client = Client::new(&f.origin).unwrap();
    let source_round = client
        .round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
        .await
        .unwrap();
    assert_eq!(source_round.images, ImageTransfer::Complete);
    let (incoming_hash, incoming_bytes): (String, i64) =
        sqlx::query_as("SELECT sha256,byte_size FROM task_attachments WHERE attachment_id=?")
            .bind(&incoming_reference)
            .fetch_one(&mut *aven_core::test_support::acquire(&f.peer).await.unwrap())
            .await
            .unwrap();
    let quota_bytes = used_before - 1;
    assert_ne!(filler_hash, incoming_hash);
    assert!(quota_bytes < crate::attachments::lifecycle::DEFAULT_ORIGINAL_QUOTA_BYTES);
    assert!(
        used_before + incoming_bytes < crate::attachments::lifecycle::DEFAULT_ORIGINAL_QUOTA_BYTES
    );

    (
        f,
        client,
        incoming_reference,
        filler_hash,
        incoming_hash,
        quota_bytes,
    )
}

fn config_with_local_quota(quota_bytes: i64) -> crate::config::AppConfig {
    let mut config = crate::config::AppConfig::default();
    config.local.attachment_lifecycle.quota_bytes = quota_bytes;
    config
}

async fn incoming_available(db: &Database, sha256: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM blob_inventory WHERE sha256=? AND available=1")
        .bind(sha256)
        .fetch_one(&mut *aven_core::test_support::acquire(db).await.unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn engine_uses_configured_quota_for_download_and_retry() {
    let (f, client, _reference, filler_hash, incoming_hash, quota_bytes) =
        configured_quota_case().await;
    let incoming_path = aven_core::attachments::object_path(f.root.path(), &incoming_hash).unwrap();
    let filler_path = aven_core::attachments::object_path(f.root.path(), &filler_hash).unwrap();

    let rejected = crate::sync::encrypted::drain_with_config(
        &client,
        &f.seed_store,
        &f.seed,
        f.root.path(),
        1,
        &config_with_local_quota(quota_bytes),
    )
    .await
    .unwrap();
    assert_eq!(rejected.images, ImageTransfer::Failed);
    assert!(filler_path.exists());
    assert!(!incoming_path.exists());
    assert_eq!(incoming_available(&f.seed, &incoming_hash).await, 0);

    // Raising only the configured local quota makes the same queued transfer
    // install successfully on a subsequent engine drain.
    let retried = crate::sync::encrypted::drain_with_config(
        &client,
        &f.seed_store,
        &f.seed,
        f.root.path(),
        1,
        &config_with_local_quota(crate::attachments::lifecycle::DEFAULT_ORIGINAL_QUOTA_BYTES),
    )
    .await
    .unwrap();
    assert_eq!(retried.images, ImageTransfer::Complete);
    assert!(incoming_path.exists());
    assert_eq!(incoming_available(&f.seed, &incoming_hash).await, 1);
}

#[tokio::test]
async fn engine_uses_configured_quota_for_cached_image_install() {
    let (f, client, incoming_reference, filler_hash, incoming_hash, quota_bytes) =
        configured_quota_case().await;
    let incoming_path = aven_core::attachments::object_path(f.root.path(), &incoming_hash).unwrap();
    let cached_path =
        aven_core::attachments::object_path(&f.root.path().join("peer-blobs"), &incoming_hash)
            .unwrap();
    std::fs::copy(cached_path, &incoming_path).unwrap();

    // Withheld server bytes make this succeed only via the local-cache install
    // path; an empty-cache fallback would report Unavailable instead.
    let object: Vec<u8> =
        sqlx::query_scalar("SELECT object FROM server_e2ee_image_references WHERE reference=?")
            .bind(&incoming_reference)
            .fetch_one(&mut *aven_core::test_support::acquire(&f.server).await.unwrap())
            .await
            .unwrap();
    sqlx::query("DELETE FROM server_e2ee_image_chunks WHERE object=?")
        .bind(&object)
        .execute(&mut *aven_core::test_support::acquire(&f.server).await.unwrap())
        .await
        .unwrap();
    sqlx::query("UPDATE server_e2ee_images SET complete=0 WHERE object=?")
        .bind(object)
        .execute(&mut *aven_core::test_support::acquire(&f.server).await.unwrap())
        .await
        .unwrap();

    let rejected = crate::sync::encrypted::drain_with_config(
        &client,
        &f.seed_store,
        &f.seed,
        f.root.path(),
        1,
        &config_with_local_quota(quota_bytes),
    )
    .await
    .unwrap();
    assert_eq!(rejected.images, ImageTransfer::Failed);
    assert!(incoming_path.exists(), "the preloaded cache source remains");
    assert!(
        aven_core::attachments::object_path(f.root.path(), &filler_hash)
            .unwrap()
            .exists(),
        "the referenced image remains under the configured quota"
    );
    assert_eq!(incoming_available(&f.seed, &incoming_hash).await, 0);

    let retried = crate::sync::encrypted::drain_with_config(
        &client,
        &f.seed_store,
        &f.seed,
        f.root.path(),
        1,
        &config_with_local_quota(crate::attachments::lifecycle::DEFAULT_ORIGINAL_QUOTA_BYTES),
    )
    .await
    .unwrap();
    assert_eq!(retried.images, ImageTransfer::Complete);
    assert_eq!(incoming_available(&f.seed, &incoming_hash).await, 1);
}

#[tokio::test]
async fn image_round_honors_configured_local_quota() {
    let f = fixture().await;
    converge(&f).await;

    let workspace = f.seed.list_workspaces().await.unwrap().remove(0);
    let filler_task = f
        .seed
        .create_task(&workspace, draft("local quota filler"))
        .await
        .unwrap()
        .task;
    let filler_reference =
        add_image_to_database(&f.seed, f.root.path(), &workspace, &filler_task.id, 8).await;
    let (filler_hash, filler_bytes): (String, i64) =
        sqlx::query_as("SELECT sha256,byte_size FROM task_attachments WHERE attachment_id=?")
            .bind(&filler_reference)
            .fetch_one(&mut *aven_core::test_support::acquire(&f.seed).await.unwrap())
            .await
            .unwrap();
    let used_before: i64 = scalar(
        &f.seed,
        "SELECT COALESCE(SUM(byte_size),0) FROM blob_inventory WHERE available=1",
    )
    .await;
    assert!(used_before >= filler_bytes && filler_bytes > 1);
    let quota_bytes = used_before - 1;

    let incoming_reference = add_image(&f).await;
    let client = Client::new(&f.origin).unwrap();
    let source_round = client
        .round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
        .await
        .unwrap();
    assert_eq!(source_round.images, ImageTransfer::Complete);
    let (incoming_hash, incoming_bytes): (String, i64) =
        sqlx::query_as("SELECT sha256,byte_size FROM task_attachments WHERE attachment_id=?")
            .bind(&incoming_reference)
            .fetch_one(&mut *aven_core::test_support::acquire(&f.peer).await.unwrap())
            .await
            .unwrap();
    let default_quota = crate::attachments::lifecycle::DEFAULT_ORIGINAL_QUOTA_BYTES;
    assert_ne!(filler_hash, incoming_hash);
    assert!(used_before > quota_bytes);
    assert!(quota_bytes < default_quota);
    assert!(used_before + incoming_bytes < default_quota);

    let local_policy = crate::config::AttachmentLifecycleConfig {
        quota_bytes,
        ..crate::config::AttachmentLifecycleConfig::default()
    }
    .policy();
    // Prefilled referenced storage exceeds the configured quota, while both
    // images together fit under the default; the normal download must reject
    // only the incoming image under this policy.
    let round = client
        .round_with_policy(&f.seed_store, &f.seed, f.root.path(), local_policy)
        .await
        .unwrap();
    let incoming_path = aven_core::attachments::object_path(f.root.path(), &incoming_hash).unwrap();
    let filler_path = aven_core::attachments::object_path(f.root.path(), &filler_hash).unwrap();

    assert_eq!(round.images, ImageTransfer::Failed);
    assert!(
        filler_path.exists(),
        "the referenced prefilled image remains"
    );
    assert!(
        !incoming_path.exists(),
        "the over-quota synced image is not retained"
    );
    let incoming_available: i64 =
        sqlx::query_scalar("SELECT count(*) FROM blob_inventory WHERE sha256=? AND available=1")
            .bind(&incoming_hash)
            .fetch_one(&mut *aven_core::test_support::acquire(&f.seed).await.unwrap())
            .await
            .unwrap();
    assert_eq!(incoming_available, 0);
}

async fn exec(db: &Database, sql: &str) {
    sqlx::query(sqlx::AssertSqlSafe(sql.to_owned()))
        .execute(&mut *aven_core::test_support::acquire(db).await.unwrap())
        .await
        .unwrap();
}

async fn task_exists(db: &Database, id: &str) -> bool {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tasks WHERE id=?)")
        .bind(id)
        .fetch_one(&mut *aven_core::test_support::acquire(db).await.unwrap())
        .await
        .unwrap()
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
async fn missing_registered_attachment_stops_drain_before_push_or_apply() {
    let f = fixture().await;
    converge(&f).await;
    let reference = add_image(&f).await;
    converge(&f).await;
    let workspace = f.peer.list_workspaces().await.unwrap().remove(0);
    let local = f
        .peer
        .create_task(&workspace, draft("must not push"))
        .await
        .unwrap()
        .task;
    let remote = f
        .seed
        .create_task(&workspace, draft("must not apply"))
        .await
        .unwrap()
        .task;
    drain(&Client::new(&f.origin).unwrap(), &f.seed_store, &f.seed).await;
    sqlx::query("DELETE FROM local_e2ee_image_references WHERE reference=?")
        .bind(&reference)
        .execute(&mut *aven_core::test_support::acquire(&f.peer).await.unwrap())
        .await
        .unwrap();
    let cursor = f.peer.meta("sync_cursor").await.unwrap();
    let server_records = scalar(&f.server, "SELECT count(*) FROM server_e2ee_tail").await;
    let error = Client::new(&f.origin)
        .unwrap()
        .round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("reinitialization-required"),
        "{error:#}"
    );
    assert_eq!(f.peer.meta("sync_cursor").await.unwrap(), cursor);
    assert_eq!(
        scalar(&f.server, "SELECT count(*) FROM server_e2ee_tail").await,
        server_records
    );
    let mut conn = aven_core::test_support::acquire(&f.peer).await.unwrap();
    let local_pending: bool = sqlx::query_scalar(
        "SELECT server_seq IS NULL FROM changes WHERE entity_id=? ORDER BY local_seq DESC LIMIT 1",
    )
    .bind(local.id.as_str())
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    let remote_present: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tasks WHERE id=?)")
        .bind(remote.id.as_str())
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert!(local_pending);
    assert!(!remote_present);
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
async fn upload_batch_stops_at_first_failure() {
    let f = fixture().await;
    converge(&f).await;
    add_image_batch(&f, 20).await;
    let before = scalar(
        &f.server,
        "SELECT count(*) FROM server_e2ee_image_references WHERE deleted=0",
    )
    .await;
    exec(
        &f.server,
        "CREATE TABLE image_puts_before_failure(remaining INTEGER NOT NULL)",
    )
    .await;
    exec(&f.server, "INSERT INTO image_puts_before_failure VALUES(5)").await;
    exec(
        &f.server,
        "CREATE TRIGGER fail_image_put_in_batch BEFORE INSERT ON server_e2ee_image_chunks
         BEGIN
           UPDATE image_puts_before_failure SET remaining=remaining-1;
           SELECT CASE WHEN (SELECT remaining FROM image_puts_before_failure) < 0
                       THEN RAISE(FAIL,'fault') END;
         END",
    )
    .await;

    let result = Client::new(&f.origin)
        .unwrap()
        .round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
        .await
        .unwrap();

    assert_eq!(result.images, ImageTransfer::Failed);
    assert_eq!(
        scalar(
            &f.server,
            "SELECT count(*) FROM server_e2ee_image_references WHERE deleted=0"
        )
        .await,
        before + 5
    );
    assert_eq!(
        scalar(
            &f.peer,
            "SELECT count(*) FROM changes
             WHERE op_type='attachment_add' AND server_seq IS NULL"
        )
        .await,
        15
    );
}

#[tokio::test]
async fn download_batch_stops_at_first_failure() {
    let f = fixture().await;
    converge(&f).await;
    add_image_batch(&f, 20).await;
    let client = Client::new(&f.origin).unwrap();
    drain(&client, &f.peer_store, &f.peer).await;
    let failed: Vec<u8> = sqlx::query_scalar(
        "SELECT object FROM server_e2ee_images
         WHERE bootstrap IS NULL ORDER BY object LIMIT 1 OFFSET 5",
    )
    .fetch_one(&mut *aven_core::test_support::acquire(&f.server).await.unwrap())
    .await
    .unwrap();
    sqlx::query("DELETE FROM server_e2ee_image_chunks WHERE object=?")
        .bind(failed)
        .execute(&mut *aven_core::test_support::acquire(&f.server).await.unwrap())
        .await
        .unwrap();

    let result = client
        .round(&f.seed_store, &f.seed, f.root.path())
        .await
        .unwrap();

    assert_eq!(result.images, ImageTransfer::Unavailable);
    assert_eq!(
        f.seed.encrypted_image_downloads_remaining().await.unwrap(),
        15
    );
}

#[tokio::test]
async fn image_only_round_finishes_with_a_fresh_pull() {
    let f = fixture().await;
    converge(&f).await;
    add_image_batch(&f, 20).await;
    let client = Client::new(&f.origin).unwrap();
    drain(&client, &f.peer_store, &f.peer).await;
    let mut peer_drain = client.start_drain(&f.seed_store, &f.seed).await.unwrap();
    let first = client
        .round_in_drain(&f.seed_store, &f.seed, f.root.path(), &mut peer_drain)
        .await
        .unwrap();
    assert_eq!(first.images, ImageTransfer::Pending);

    let workspace = f.peer.list_workspaces().await.unwrap().remove(0);
    let remote = f
        .peer
        .create_task(&workspace, draft("arrived during image-only work"))
        .await
        .unwrap()
        .task;
    client
        .round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
        .await
        .unwrap();

    let image_only = client
        .round_in_drain(&f.seed_store, &f.seed, f.root.path(), &mut peer_drain)
        .await
        .unwrap();
    assert_eq!(image_only.images, ImageTransfer::Complete);
    assert!(!image_only.metadata_caught_up);
    assert!(!task_exists(&f.seed, remote.id.as_str()).await);

    let final_pull = client
        .round_in_drain(&f.seed_store, &f.seed, f.root.path(), &mut peer_drain)
        .await
        .unwrap();
    assert_eq!(final_pull.images, ImageTransfer::Complete);
    assert!(final_pull.metadata_caught_up);
    assert_eq!(title(&f.seed, remote.id.as_str()).await, remote.title);
}

#[tokio::test]
async fn metadata_arriving_during_image_only_work_is_pulled_within_the_bound() {
    let f = fixture().await;
    converge(&f).await;
    add_image_batch(&f, 80).await;
    let client = Client::new(&f.origin).unwrap();
    drain(&client, &f.peer_store, &f.peer).await;
    let mut peer_drain = client.start_drain(&f.seed_store, &f.seed).await.unwrap();
    client
        .round_in_drain(&f.seed_store, &f.seed, f.root.path(), &mut peer_drain)
        .await
        .unwrap();

    let workspace = f.peer.list_workspaces().await.unwrap().remove(0);
    let remote = f
        .peer
        .create_task(&workspace, draft("bounded metadata refresh"))
        .await
        .unwrap()
        .task;
    client
        .round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
        .await
        .unwrap();

    let mut rounds = 0;
    while !task_exists(&f.seed, remote.id.as_str()).await {
        client
            .round_in_drain(&f.seed_store, &f.seed, f.root.path(), &mut peer_drain)
            .await
            .unwrap();
        rounds += 1;
        assert!(rounds <= 4, "metadata pull bound");
    }
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
    let f = fixture_with(FixtureOptions {
        unavailable_image: true,
        ..Default::default()
    })
    .await;
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
        if round == 1 {
            // A failed attempt leaves both images remaining.
            assert_eq!(
                f.seed.encrypted_image_downloads_remaining().await.unwrap(),
                2
            );
        }
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
    // Only the installed image stops counting as remaining.
    assert_eq!(
        f.seed.encrypted_image_downloads_remaining().await.unwrap(),
        1
    );
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
    assert_eq!(outcome.images, ImageTransfer::Failed);
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

#[tokio::test]
async fn workspace_move_routes_late_images_and_preserves_download_and_pruning() {
    let f = fixture().await;
    converge(&f).await;
    let source = f.seed.list_workspaces().await.unwrap().remove(0);
    let target = f.seed.create_workspace("Moved").await.unwrap();
    f.seed
        .create_project(&target, "Destination", None)
        .await
        .unwrap();
    converge(&f).await;
    let task = f
        .seed
        .export_data("now".into())
        .await
        .unwrap()
        .tables
        .task_attachments[0]
        .task_id
        .clone();
    let late_reference = add_image_to_database(&f.peer, &blobs(&f.peer), &source, &task, 13).await;
    f.seed
        .move_tasks(
            &source,
            aven_core::operations::MoveTasksInput {
                task_ids: vec![task.clone()],
                target_workspace: target.clone(),
                target_project: "Destination".into(),
            },
        )
        .await
        .unwrap();
    converge(&f).await;
    assert_quiescent(&[&f.seed, &f.peer]).await;
    for db in [&f.seed, &f.peer] {
        let export = db.export_data("now".into()).await.unwrap();
        assert!(
            export
                .tables
                .task_attachments
                .iter()
                .filter(|a| a.task_id == task)
                .all(|a| a.workspace_id == target.id)
        );
        assert!(
            export
                .tables
                .task_attachments
                .iter()
                .any(|a| a.attachment_id == late_reference)
        );
        assert_eq!(db.encrypted_image_downloads_remaining().await.unwrap(), 0);
        assert_eq!(
            scalar(db, "SELECT count(*) FROM conflicts WHERE resolved=0").await,
            0
        );
    }
    let client = Client::new(&f.origin).unwrap();
    let record = f
        .peer
        .export_data("now".into())
        .await
        .unwrap()
        .tables
        .task_attachments
        .into_iter()
        .find(|a| a.attachment_id == late_reference)
        .unwrap();
    let path = aven_core::attachments::object_path(&blobs(&f.seed), &record.sha256).unwrap();
    std::fs::remove_file(path).unwrap();
    sqlx::query("UPDATE blob_inventory SET available=0 WHERE sha256=?")
        .bind(&record.sha256)
        .execute(&mut *aven_core::test_support::acquire(&f.seed).await.unwrap())
        .await
        .unwrap();
    drain(&client, &f.seed_store, &f.seed).await;
    assert_eq!(
        f.seed.encrypted_image_downloads_remaining().await.unwrap(),
        0
    );
    f.seed
        .update_task(
            &target,
            &task,
            TaskUpdate {
                deleted: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    converge(&f).await;
    assert_eq!(
        scalar_after_prune_pass(
            &f.server,
            "SELECT count(*) FROM server_e2ee_images WHERE unreferenced_at IS NOT NULL"
        )
        .await,
        2
    );
    f.seed
        .update_task(
            &target,
            &task,
            TaskUpdate {
                deleted: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    converge(&f).await;
    assert_eq!(
        scalar_after_prune_pass(
            &f.server,
            "SELECT count(*) FROM server_e2ee_images WHERE unreferenced_at IS NOT NULL"
        )
        .await,
        0
    );
}
