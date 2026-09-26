use super::*;
use crate::{protected_local_keys::tests::isolated_store, test_support::e2ee_http};
use aven_core::sync::encrypted_tail::Accepted;
use aven_core::{
    choices::TaskSource,
    operations::{TaskDraft, TaskUpdate},
};
use axum::{body::to_bytes, http::header, response::IntoResponse};
use serde::Serialize;
use std::path::{Path, PathBuf};
struct Fixture {
    root: tempfile::TempDir,
    seed: Database,
    peer: Database,
    server: Database,
    seed_store: ProtectedLocalKeyStore,
    peer_store: ProtectedLocalKeyStore,
    origin: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn serve(server: Database, address: &str) -> (String, tokio::task::JoinHandle<()>) {
    serve_with(server, address, false).await
}
async fn serve_with(
    server: Database,
    address: &str,
    count_http: bool,
) -> (String, tokio::task::JoinHandle<()>) {
    let tail = Router::new()
        .route(PATH, post(handle))
        .route(images::PATH, post(images::handle))
        .with_state(Arc::new(Server {
            db: server.clone(),
            gate: crate::http_admission::Admission::new(1),
            image_policy: crate::config::AttachmentLifecycleConfig::default().server_policy(),
        }));
    let mut app = e2ee_http::router_with_tail(server, tail).await;
    if count_http {
        app = app.layer(axum::middleware::from_fn(bench::count_http));
    }
    e2ee_http::serve(app, address).await
}
/// Seed content captured into the shared-state snapshot before the peer joins.
#[derive(Default)]
struct FixtureOptions {
    /// A second task references the seed's existing image bytes.
    shared_image: bool,
    /// A snapshot note; `Some(true)` also edits it after capture.
    note_after_capture: Option<bool>,
    /// A task with labels, a dependency, a related link and an epic parent.
    relations: bool,
    /// Removes the snapshot dependency after capture. Needs `relations`.
    dependency_after_capture: bool,
    /// The snapshot image's task and bytes are deleted before capture.
    unavailable_image: bool,
    /// A recurring task with an edited occurrence.
    recurrence: bool,
    /// Counts HTTP requests and body bytes for the benchmark.
    count_http: bool,
}
async fn fixture() -> Fixture {
    fixture_with(FixtureOptions::default()).await
}
fn with_relations() -> FixtureOptions {
    FixtureOptions {
        relations: true,
        ..Default::default()
    }
}
async fn fixture_with(options: FixtureOptions) -> Fixture {
    let FixtureOptions {
        shared_image: shared,
        note_after_capture,
        relations,
        dependency_after_capture,
        unavailable_image: unavailable,
        recurrence,
        count_http,
    } = options;
    let root = tempfile::tempdir().unwrap();
    let (seed, seed_store, authority, _) = e2ee_http::fixture(root.path()).await;
    if unavailable {
        let capture = seed
            .resume_local_shared_state_never_dispatched()
            .await
            .unwrap()
            .unwrap();
        seed.cancel_local_shared_state_never_dispatched(capture.candidate_id())
            .await
            .unwrap();
        let w = seed.list_workspaces().await.unwrap().remove(0);
        let task: aven_core::ids::TaskId =
            sqlx::query_scalar("SELECT task_id FROM task_attachments")
                .fetch_one(&mut *aven_core::test_support::acquire(&seed).await.unwrap())
                .await
                .unwrap();
        seed.update_task(
            &w,
            &task,
            TaskUpdate {
                deleted: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let reference: String = sqlx::query_scalar("SELECT attachment_id FROM task_attachments")
            .fetch_one(&mut *aven_core::test_support::acquire(&seed).await.unwrap())
            .await
            .unwrap();
        seed.delete_task_attachment(&w, &reference).await.unwrap();
        for entry in std::fs::read_dir(root.path().join("objects/sha256")).unwrap() {
            std::fs::remove_file(entry.unwrap().path()).unwrap();
        }
        sqlx::query("UPDATE blob_inventory SET available=0")
            .execute(&mut *aven_core::test_support::acquire(&seed).await.unwrap())
            .await
            .unwrap();
        seed.capture_local_shared_state_never_dispatched()
            .await
            .unwrap();
        seed_store
            .package_seed_capture(&seed, root.path(), [9; 32])
            .await
            .unwrap();
    }
    let mut snapshot_note = None;
    if shared || note_after_capture.is_some() || relations || recurrence {
        let capture = seed
            .resume_local_shared_state_never_dispatched()
            .await
            .unwrap()
            .unwrap();
        seed.cancel_local_shared_state_never_dispatched(capture.candidate_id())
            .await
            .unwrap();
        let workspace = seed.list_workspaces().await.unwrap().remove(0);
        if recurrence {
            let created = self::recurrence::create(&seed).await;
            seed.update_task(
                &workspace,
                &created.task.id,
                TaskUpdate {
                    title: Some("snapshot occurrence edit".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        }
        if shared {
            let task = seed
                .create_task(&workspace, draft("shared image parent"))
                .await
                .unwrap()
                .task;
            let bytes = files(&root.path().join("objects/sha256")).remove(0);
            seed.add_task_attachment(
                &workspace,
                root.path(),
                Default::default(),
                &task.id,
                aven_core::operations::AttachmentAddInput {
                    filename: Some("shared.png".into()),
                    alt_text: None,
                    declared_media_type: None,
                    bytes,
                    optimization_policy: aven_core::attachments::ImageOptimizationPolicy::Preserve,
                    dedupe_existing: false,
                },
            )
            .await
            .unwrap();
        }
        if relations {
            let mut owner = draft("snapshot relation owner");
            owner.labels = vec!["tag".into(), "untouched".into()];
            seed.create_label(&workspace, "tag").await.unwrap();
            seed.create_label(&workspace, "untouched").await.unwrap();
            let a = seed.create_task(&workspace, owner).await.unwrap().task;
            let b = seed
                .create_task(&workspace, draft("snapshot relation target"))
                .await
                .unwrap()
                .task;
            seed.add_task_dependency(&workspace, &a.id, &b.id)
                .await
                .unwrap();
            seed.add_task_related_link(&workspace, &a.id, &b.id)
                .await
                .unwrap();
            seed.add_task_to_epic(&workspace, &a.id, &b.id)
                .await
                .unwrap();
        }
        if note_after_capture.is_some() {
            let task = seed
                .create_task(&workspace, draft("snapshot note owner"))
                .await
                .unwrap()
                .task;
            let note = seed
                .add_note(&workspace, &task.id, "snapshot original".into())
                .await
                .unwrap();
            seed.edit_note(&workspace, &task.id, &note.note_id, "snapshot body".into())
                .await
                .unwrap();
            snapshot_note = Some((workspace, task.id, note.note_id));
        }
        seed.capture_local_shared_state_never_dispatched()
            .await
            .unwrap();
        seed_store
            .package_seed_capture(&seed, root.path(), [9; 32])
            .await
            .unwrap();
    }
    if dependency_after_capture {
        let workspace = seed.list_workspaces().await.unwrap().remove(0);
        let pair: (aven_core::ids::TaskId, aven_core::ids::TaskId) = {
            let mut c = aven_core::test_support::acquire(&seed).await.unwrap();
            sqlx::query_as("SELECT task_id, depends_on_task_id FROM task_dependencies")
                .fetch_one(&mut *c)
                .await
                .unwrap()
        };
        seed.remove_task_dependency(&workspace, &pair.0, &pair.1)
            .await
            .unwrap();
    }
    if note_after_capture == Some(true) {
        let (workspace, task, note) = snapshot_note.as_ref().unwrap();
        seed.edit_note(workspace, task, note, "source after capture".into())
            .await
            .unwrap();
    }
    let server = Database::open(&root.path().join("server.sqlite"))
        .await
        .unwrap();
    let (origin, task) = serve_with(server.clone(), "127.0.0.1:0", count_http).await;
    e2ee_http::adopt(&origin, &seed, &seed_store, &authority).await;
    let peer = Database::open(&root.path().join("peer.sqlite"))
        .await
        .unwrap();
    let peer_store = isolated_store(&peer, &root.path().join("peer-keys")).await;
    let enrollment = crate::peer_enrollment_http::Client::new(&origin).unwrap();
    let expiry = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3500;
    let invitation = enrollment.invite(&seed_store, &seed, expiry).await.unwrap();
    enrollment
        .request(&peer_store, &peer, Some(invitation))
        .await
        .unwrap();
    assert!(enrollment.admit(&seed_store, &seed).await.unwrap());
    assert!(enrollment.complete(&peer_store, &peer).await.unwrap());
    enrollment.install(&peer_store, &peer).await.unwrap();
    let initial = Client::new(&origin)
        .unwrap()
        .round(&peer_store, &peer, &root.path().join("peer-blobs"))
        .await
        .unwrap();
    assert!(initial.metadata_caught_up);
    assert_eq!(initial.images, ImageTransfer::Complete);
    assert_ne!(
        seed.meta("client_id").await.unwrap(),
        peer.meta("client_id").await.unwrap()
    );
    Fixture {
        root,
        seed,
        peer,
        server,
        seed_store,
        peer_store,
        origin,
        task,
    }
}
/// Fixture blob roots: the seed uses the fixture root, others `<name>-blobs`.
fn blobs(db: &Database) -> PathBuf {
    let path = db.path();
    let name = path.file_stem().unwrap().to_str().unwrap();
    let parent = path.parent().unwrap();
    if name == "client" {
        parent.to_path_buf()
    } else {
        parent.join(format!("{name}-blobs"))
    }
}
async fn head_record(db: &Database, a: &tail::Authority) -> Vec<u8> {
    db.prepare_encrypted_push(a, &blobs(db))
        .await
        .unwrap()
        .unwrap()
        .record
}
async fn drain(client: &Client, store: &ProtectedLocalKeyStore, db: &Database) {
    for _ in 0..100 {
        match client.round(store, db, &blobs(db)).await {
            Ok(round) if round.metadata_caught_up => return,
            Ok(_) => {}
            // Nonblocking installation exclusion can be briefly retained by a
            // concurrently spawned child until its close-on-exec descriptors close.
            Err(error)
                if matches!(
                    error.to_string().as_str(),
                    "error installation-busy" | "error enrollment-busy"
                ) =>
            {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            Err(error) => panic!("{error:#}"),
        }
    }
    panic!("round budget");
}
/// Asserts every change is accepted and no push is in flight.
async fn assert_quiescent(dbs: &[&Database]) {
    for db in dbs {
        assert_eq!(
            scalar(db, "SELECT count(*) FROM changes WHERE server_seq IS NULL").await,
            0
        );
        assert_eq!(
            scalar(db, "SELECT count(*) FROM local_e2ee_outbox").await,
            0
        );
    }
}
async fn converge(f: &Fixture) {
    let c = Client::new(&f.origin).unwrap();
    drain(&c, &f.seed_store, &f.seed).await;
    drain(&c, &f.peer_store, &f.peer).await;
    drain(&c, &f.seed_store, &f.seed).await;
}
fn draft(title: &str) -> TaskDraft {
    TaskDraft {
        title: title.into(),
        description: String::new(),
        project: Some("app".into()),
        status: "todo".into(),
        priority: "none".into(),
        source: TaskSource::Cli,
        labels: vec![],
        metadata: vec![],
        available_at: None,
        due_on: None,
        is_epic: false,
    }
}
async fn title(db: &Database, id: &str) -> String {
    let mut c = aven_core::test_support::acquire(db).await.unwrap();
    sqlx::query_scalar("SELECT title FROM tasks WHERE id=?")
        .bind(id)
        .fetch_one(&mut *c)
        .await
        .unwrap()
}
async fn scalar(db: &Database, sql: &str) -> i64 {
    let mut c = aven_core::test_support::acquire(db).await.unwrap();
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
        .fetch_one(&mut *c)
        .await
        .unwrap()
}
/// Runs a prune pass that deletes nothing, so unreferenced images get their
/// grace-period stamp, then evaluates `sql`.
async fn scalar_after_prune_pass(db: &Database, sql: &str) -> i64 {
    let pruned = db
        .prune_encrypted_images(std::time::Duration::from_secs(86400), 128)
        .await
        .unwrap();
    assert_eq!(pruned, 0);
    scalar(db, sql).await
}
fn files(path: &Path) -> Vec<Vec<u8>> {
    let mut bytes = std::fs::read_dir(path)
        .unwrap()
        .map(|p| std::fs::read(p.unwrap().path()).unwrap())
        .collect::<Vec<_>>();
    bytes.sort();
    bytes
}

#[tokio::test]
async fn restored_then_deleted_task_images_get_a_fresh_grace_period() {
    let f = fixture().await;
    converge(&f).await;
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    let export = f.seed.export_data("now".into()).await.unwrap();
    let image_task = export.tables.task_attachments[0].task_id.clone();
    let set_deleted = async |deleted: bool| {
        f.seed
            .update_task(
                &w,
                &image_task,
                TaskUpdate {
                    deleted: Some(deleted),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        converge(&f).await;
    };
    let stale = "SELECT count(*) FROM server_e2ee_images WHERE unreferenced_at=0";
    set_deleted(true).await;
    assert_eq!(
        scalar_after_prune_pass(
            &f.server,
            "SELECT count(*) FROM server_e2ee_images WHERE unreferenced_at IS NOT NULL"
        )
        .await,
        1
    );
    // Age the stamp far past any grace period.
    let mut c = aven_core::test_support::acquire(&f.server).await.unwrap();
    sqlx::query(
        "UPDATE server_e2ee_images SET unreferenced_at=0 WHERE unreferenced_at IS NOT NULL",
    )
    .execute(&mut *c)
    .await
    .unwrap();
    drop(c);
    assert_eq!(scalar(&f.server, stale).await, 1);
    // Restoring clears the stamp without waiting for a prune pass.
    set_deleted(false).await;
    assert_eq!(scalar(&f.server, stale).await, 0);
    set_deleted(true).await;
    let chunks = "SELECT count(*) FROM server_e2ee_image_chunks";
    let before = scalar(&f.server, chunks).await;
    assert!(before > 0);
    assert_eq!(
        f.server
            .prune_encrypted_images(std::time::Duration::from_secs(86400), 128)
            .await
            .unwrap(),
        0
    );
    assert_eq!(scalar(&f.server, stale).await, 0);
    assert_eq!(scalar(&f.server, chunks).await, before);
}

#[tokio::test]
async fn independent_clients_create_conflict_resolve_delete_restore_and_keep_images() {
    let f = fixture().await;
    converge(&f).await;
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    let seed_id = f.seed.meta("client_id").await.unwrap();
    let peer_id = f.peer.meta("client_id").await.unwrap();
    let image_bytes = files(&f.root.path().join("objects/sha256"));
    let task = f
        .seed
        .create_task(&w, draft("private fresh task"))
        .await
        .unwrap()
        .task;
    converge(&f).await;
    assert_eq!(title(&f.peer, task.id.as_str()).await, "private fresh task");
    f.seed
        .update_task(
            &w,
            &task.id,
            TaskUpdate {
                title: Some("offline seed".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    f.peer
        .update_task(
            &w,
            &task.id,
            TaskUpdate {
                title: Some("offline peer".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    converge(&f).await;
    assert_eq!(
        f.seed
            .task_conflicts(&w, &task.id, Some("title"))
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        f.peer
            .task_conflicts(&w, &task.id, Some("title"))
            .await
            .unwrap()
            .len(),
        1
    );
    f.seed
        .resolve_conflict(&w, &task.id, "title", "chosen title")
        .await
        .unwrap();
    f.peer
        .add_note(&w, &task.id, "private note".into())
        .await
        .unwrap();
    converge(&f).await;
    assert_eq!(title(&f.peer, task.id.as_str()).await, "chosen title");
    let export = f.seed.export_data("now".into()).await.unwrap();
    let image_task = export.tables.task_attachments[0].task_id.clone();
    f.seed
        .update_task(
            &w,
            &image_task,
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
        1
    );
    f.peer
        .update_task(
            &w,
            &image_task,
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
    assert_eq!(files(&f.root.path().join("objects/sha256")), image_bytes);
    assert_eq!(
        files(&f.root.path().join("peer-blobs/objects/sha256")),
        image_bytes
    );
    assert_eq!(scalar(&f.server, "SELECT count(*) FROM changes").await, 0);
    assert_eq!(f.seed.meta("client_id").await.unwrap(), seed_id);
    assert_eq!(f.peer.meta("client_id").await.unwrap(), peer_id);
    assert_eq!(
        scalar(
            &f.seed,
            "SELECT count(*) FROM changes WHERE server_seq IS NULL"
        )
        .await,
        0
    );
    assert_eq!(
        scalar(
            &f.peer,
            "SELECT count(*) FROM changes WHERE server_seq IS NULL"
        )
        .await,
        0
    );
}

#[tokio::test]
async fn frozen_restart_lost_ack_and_server_restart_keep_exact_identity() {
    let mut f = fixture().await;
    converge(&f).await;
    let w = f.peer.list_workspaces().await.unwrap().remove(0);
    f.peer
        .create_task(&w, draft("restart private"))
        .await
        .unwrap();
    let (record, context, bearer) = {
        let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
        (
            head_record(&f.peer, &inputs.authority).await,
            inputs.authority.context.clone(),
            Secret::new(*inputs.bearer.expose()),
        )
    };
    let reopened = Database::open(f.peer.path()).await.unwrap();
    let store = isolated_store(&reopened, &f.root.path().join("peer-keys")).await;
    {
        let inputs = store.tail_inputs(&reopened, &f.origin).await.unwrap();
        assert_eq!(head_record(&reopened, &inputs.authority).await, record);
    }
    let c = Client::new(&f.origin).unwrap();
    let Reply::Appended(first) = c
        .exchange(
            &context,
            &bearer,
            Operation::Append {
                ticket: None,
                record: record.clone(),
            },
        )
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(
        scalar(
            &f.peer,
            "SELECT count(*) FROM changes WHERE server_seq IS NULL"
        )
        .await,
        1
    );
    f.task.abort();
    let _ = (&mut f.task).await;
    let mut server_process = spawn_server(&f).await;
    let Reply::Appended(second) = c
        .exchange(
            &context,
            &bearer,
            Operation::Append {
                ticket: None,
                record: record.clone(),
            },
        )
        .await
        .unwrap()
    else {
        panic!()
    };
    assert!(first == second);
    let Reply::Found(accepted) = c
        .exchange(
            &context,
            &bearer,
            Operation::Lookup {
                operation_id: first.operation_id.clone(),
                expected: Some(first),
            },
        )
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(accepted.record, record);
    drain(&c, &store, &reopened).await;
    converge(&f).await;
    assert_eq!(
        scalar(&f.server, "SELECT count(*) FROM server_e2ee_tail").await,
        2
    );
    server_process.kill().await.unwrap();
    server_process.wait().await.unwrap();
}

#[tokio::test]
async fn offline_metadata_notes_and_relationships_converge() {
    let f = fixture().await;
    converge(&f).await;
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    let mut d = draft("metadata owner");
    d.project = Some("new-project".into());
    d.labels = vec!["new-label".into()];
    d.metadata = vec![aven_core::metadata::TaskMetadataInput {
        expected_field_id: None,
        key: "owner".into(),
        value: "initial".into(),
    }];
    let first = f
        .seed
        .create_task_with_options(
            &w,
            d,
            aven_core::operations::TaskCreationOptions::standalone(
                aven_core::operations::TaskCreationUndo::None,
            )
            .with_create_missing_labels(),
        )
        .await
        .unwrap()
        .task;
    let mut child = draft("related child");
    child.project = Some("new-project".into());
    let second = f.seed.create_task(&w, child).await.unwrap().task;
    converge(&f).await;
    for (db, value) in [(&f.seed, "seed-owner"), (&f.peer, "peer-owner")] {
        db.update_task(
            &w,
            &first.id,
            TaskUpdate {
                set_metadata: vec![aven_core::metadata::TaskMetadataInput {
                    expected_field_id: None,
                    key: "owner".into(),
                    value: value.into(),
                }],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }
    f.peer
        .add_task_related_link(&w, &first.id, &second.id)
        .await
        .unwrap();
    f.seed
        .add_task_dependency(&w, &first.id, &second.id)
        .await
        .unwrap();
    f.seed
        .add_task_to_epic(&w, &second.id, &first.id)
        .await
        .unwrap();
    f.peer
        .add_note(&w, &second.id, "peer offline note".into())
        .await
        .unwrap();
    converge(&f).await;
    let conflicts = f.seed.task_conflicts(&w, &first.id, None).await.unwrap();
    assert_eq!(conflicts.len(), 1);
    let mut conn = aven_core::test_support::acquire(&f.seed).await.unwrap();
    let field: String =
        sqlx::query_scalar("SELECT field FROM conflicts WHERE resolved=0 AND task_id=?")
            .bind(&first.id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    drop(conn);
    f.seed
        .resolve_conflict(&w, &first.id, &field, "resolved owner")
        .await
        .unwrap();
    converge(&f).await;
    for db in [&f.seed, &f.peer] {
        assert_eq!(
            scalar(db, "SELECT count(*) FROM task_related_links WHERE linked=1").await,
            1
        );
        assert_eq!(
            scalar(db, "SELECT count(*) FROM task_dependencies").await,
            1
        );
        assert_eq!(
            scalar(
                db,
                "SELECT count(*) FROM task_metadata WHERE value='resolved owner'"
            )
            .await,
            1
        );
        assert_eq!(
            scalar(db, "SELECT count(*) FROM conflicts WHERE resolved=0").await,
            0
        );
    }
}

#[tokio::test]
async fn current_auth_prefix_tamper_and_whole_page_rollback() {
    let f = fixture().await;
    converge(&f).await;
    let c = Client::new(&f.origin).unwrap();
    let w = f.peer.list_workspaces().await.unwrap().remove(0);
    f.peer
        .create_task(&w, draft("pending rollback"))
        .await
        .unwrap();
    let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
    let a = &inputs.authority;
    let frozen = head_record(&f.peer, a).await;
    for field in 0..5 {
        let mut wrong = a.context.clone();
        match field {
            0 => wrong.vault[0] ^= 1,
            1 => wrong.stream[0] ^= 1,
            2 => wrong.head[0] ^= 1,
            3 => wrong.device[0] ^= 1,
            _ => wrong.descriptor[0] ^= 1,
        };
        assert!(
            c.exchange(
                &wrong,
                &inputs.bearer,
                Operation::Append {
                    ticket: None,
                    record: frozen.clone()
                }
            )
            .await
            .is_err()
        );
    }
    assert!(
        c.exchange(
            &a.context,
            &Secret::new([0; 32]),
            Operation::Append {
                ticket: None,
                record: frozen.clone()
            }
        )
        .await
        .is_err()
    );
    assert!(
        c.exchange(
            &a.context,
            &inputs.bearer,
            Operation::Append {
                ticket: None,
                record: frozen[..20].to_vec()
            }
        )
        .await
        .is_err()
    );
    let prefix_id = {
        let mut conn = aven_core::test_support::acquire(&f.server).await.unwrap();
        sqlx::query_scalar::<_, String>("SELECT operation_id FROM server_bootstrap_prefix LIMIT 1")
            .fetch_one(&mut *conn)
            .await
            .unwrap()
    };
    // Operation ID is public framing at byte 156; server cannot authenticate AEAD.
    let mut collision = frozen.clone();
    collision[160..176].copy_from_slice(prefix_id.as_bytes());
    let error = c
        .exchange(
            &a.context,
            &inputs.bearer,
            Operation::Append {
                ticket: None,
                record: collision,
            },
        )
        .await
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("prefix-identity-collision"), "{error}");
    let Reply::Appended(mapping) = c
        .exchange(
            &a.context,
            &inputs.bearer,
            Operation::Append {
                ticket: None,
                record: frozen.clone(),
            },
        )
        .await
        .unwrap()
    else {
        panic!()
    };
    let before = f.peer.encrypted_round_state(a).await.unwrap().cursor;
    let Reply::Page(page) = c
        .exchange(
            &a.context,
            &inputs.bearer,
            Operation::Pull {
                after: before,
                limit: 16,
                watermark: None,
            },
        )
        .await
        .unwrap()
    else {
        panic!()
    };
    let mut bad = page.clone();
    let last = bad.records[0].record.len() - 1;
    bad.records[0].record[last] ^= 1;
    assert!(f.peer.apply_encrypted_tail_page(a, &bad).await.is_err());
    assert_eq!(
        f.peer.encrypted_round_state(a).await.unwrap().cursor,
        before
    );
    assert_eq!(
        scalar(
            &f.peer,
            "SELECT count(*) FROM changes WHERE server_seq IS NULL"
        )
        .await,
        1
    );
    // A real transaction failure after domain/rank work must also roll back.
    {
        let mut conn = aven_core::test_support::acquire(&f.peer).await.unwrap();
        sqlx::query("CREATE TRIGGER fail_tail_cursor BEFORE UPDATE ON meta WHEN NEW.key='sync_cursor' BEGIN SELECT RAISE(ABORT,'injected'); END").execute(&mut *conn).await.unwrap();
    }
    assert!(f.peer.apply_encrypted_tail_page(a, &page).await.is_err());
    assert_eq!(
        scalar(
            &f.peer,
            "SELECT count(*) FROM changes WHERE server_seq IS NULL"
        )
        .await,
        1
    );
    {
        let mut conn = aven_core::test_support::acquire(&f.peer).await.unwrap();
        sqlx::query("DROP TRIGGER fail_tail_cursor")
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    f.peer.apply_encrypted_tail_page(a, &page).await.unwrap();
    assert_eq!(
        scalar(
            &f.peer,
            "SELECT count(*) FROM changes WHERE server_seq IS NULL"
        )
        .await,
        0
    );
    let mut wrong = mapping;
    wrong.commitment[0] ^= 1;
    assert!(f.peer.observe_encrypted_tail(a, &wrong).await.is_err());
}

#[tokio::test]
#[ignore = "subprocess encrypted tail crash worker"]
async fn process_worker() {
    let root = std::path::PathBuf::from(std::env::var_os("AVEN_TAIL_ROOT").unwrap());
    let db = Database::open(&root.join("peer.sqlite")).await.unwrap();
    let store = isolated_store(&db, &root.join("peer-keys")).await;
    let client = Client::new(&std::env::var("AVEN_TAIL_ORIGIN").unwrap()).unwrap();
    client
        .round(&store, &db, &root.join("peer-blobs"))
        .await
        .unwrap();
}
#[tokio::test]
async fn process_exit_before_dispatch_after_acceptance_and_during_page_commit() {
    for stage in [
        "frozen",
        "after-append",
        "before-page-commit",
        "after-page-commit",
    ] {
        let f = fixture().await;
        converge(&f).await;
        let w = f.peer.list_workspaces().await.unwrap().remove(0);
        let local = f.peer.create_task(&w, draft(stage)).await.unwrap().task;
        let remote = f
            .seed
            .create_task(&w, draft("pull crash remote"))
            .await
            .unwrap()
            .task;
        let c = Client::new(&f.origin).unwrap();
        drain(&c, &f.seed_store, &f.seed).await;
        let before = f.peer.meta("sync_cursor").await.unwrap();
        let output = e2ee_http::worker("encrypted_tail_http::tests::process_worker")
            .env("AVEN_TAIL_ROOT", f.root.path())
            .env("AVEN_TAIL_ORIGIN", &f.origin)
            .env("AVEN_TAIL_CRASH", stage)
            .output()
            .await
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(84),
            "{stage}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        if stage != "after-page-commit" {
            assert_eq!(f.peer.meta("sync_cursor").await.unwrap(), before);
        }
        let frozen: Option<Vec<u8>> = {
            let mut conn = aven_core::test_support::acquire(&f.peer).await.unwrap();
            sqlx::query_scalar("SELECT record FROM local_e2ee_outbox")
                .fetch_optional(&mut *conn)
                .await
                .unwrap()
        };
        let reopened = Database::open(f.peer.path()).await.unwrap();
        let store = isolated_store(&reopened, &f.root.path().join("peer-keys")).await;
        if let Some(bytes) = frozen {
            let inputs = store.tail_inputs(&reopened, &f.origin).await.unwrap();
            assert_eq!(head_record(&reopened, &inputs.authority).await, bytes);
        }
        drain(&c, &store, &reopened).await;
        drain(&c, &f.seed_store, &f.seed).await;
        assert_eq!(title(&f.seed, local.id.as_str()).await, stage);
        assert_eq!(
            title(&reopened, remote.id.as_str()).await,
            "pull crash remote"
        );
        assert_eq!(
            scalar(&f.server, "SELECT count(*) FROM server_e2ee_tail").await,
            3
        );
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

#[tokio::test]
async fn same_id_different_ciphertext_requires_equal_domain_not_identity() {
    for divergent in [false, true] {
        let f = fixture().await;
        converge(&f).await;
        let w = f.seed.list_workspaces().await.unwrap().remove(0);
        let task = f
            .seed
            .create_task(&w, draft("shared identity target"))
            .await
            .unwrap()
            .task;
        converge(&f).await;
        f.seed
            .update_task(
                &w,
                &task.id,
                TaskUpdate {
                    title: Some("same meaning".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        f.peer
            .update_task(
                &w,
                &task.id,
                TaskUpdate {
                    title: Some(
                        if divergent {
                            "different meaning"
                        } else {
                            "same meaning"
                        }
                        .into(),
                    ),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        // Controlled duplicate identity fixture, not recurrence generation or server acceptance.
        let (id, at): (String, String) = {
            let mut conn = aven_core::test_support::acquire(&f.seed).await.unwrap();
            sqlx::query_as("SELECT change_id,created_at FROM changes WHERE server_seq IS NULL")
                .fetch_one(&mut *conn)
                .await
                .unwrap()
        };
        {
            let mut conn = aven_core::test_support::acquire(&f.peer).await.unwrap();
            let old: String =
                sqlx::query_scalar("SELECT change_id FROM changes WHERE server_seq IS NULL")
                    .fetch_one(&mut *conn)
                    .await
                    .unwrap();
            sqlx::query("UPDATE changes SET change_id=?,created_at=? WHERE change_id=?")
                .bind(&id)
                .bind(&at)
                .bind(&old)
                .execute(&mut *conn)
                .await
                .unwrap();
            sqlx::query("UPDATE field_versions SET version=? WHERE version=?")
                .bind(&id)
                .bind(&old)
                .execute(&mut *conn)
                .await
                .unwrap();
        }
        let peer_record = {
            let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
            head_record(&f.peer, &inputs.authority).await
        };
        let c = Client::new(&f.origin).unwrap();
        drain(&c, &f.seed_store, &f.seed).await;
        let accepted_record: Vec<u8> = {
            let mut conn = aven_core::test_support::acquire(&f.server).await.unwrap();
            sqlx::query_scalar("SELECT record FROM server_e2ee_tail WHERE operation_id=?")
                .bind(&id)
                .fetch_one(&mut *conn)
                .await
                .unwrap()
        };
        assert_ne!(peer_record, accepted_record);
        let before = f.peer.meta("sync_cursor").await.unwrap();
        if divergent {
            assert!(
                c.round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("same-id-divergence")
            );
            assert_eq!(f.peer.meta("sync_cursor").await.unwrap(), before);
            assert_eq!(
                scalar(
                    &f.peer,
                    "SELECT count(*) FROM changes WHERE server_seq IS NULL"
                )
                .await,
                1
            );
            assert_eq!(
                scalar(&f.peer, "SELECT blocked FROM local_e2ee_outbox").await,
                1
            );
            assert!(
                c.round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
                    .await
                    .is_err()
            );
            assert_eq!(title(&f.peer, task.id.as_str()).await, "different meaning");
        } else {
            drain(&c, &f.peer_store, &f.peer).await;
            assert_eq!(
                scalar(
                    &f.peer,
                    "SELECT count(*) FROM changes WHERE server_seq IS NULL"
                )
                .await,
                0
            );
            let local_origin: String = {
                let mut conn = aven_core::test_support::acquire(&f.peer).await.unwrap();
                sqlx::query_scalar("SELECT client_id FROM changes WHERE change_id=?")
                    .bind(&id)
                    .fetch_one(&mut *conn)
                    .await
                    .unwrap()
            };
            assert_eq!(Some(local_origin), f.peer.meta("client_id").await.unwrap());
        }
        assert_eq!(
            scalar(&f.server, "SELECT count(*) FROM server_e2ee_tail").await,
            3
        );
    }
}

#[tokio::test]
async fn attachment_add_transfer_and_explicit_delete_use_independent_clients() {
    let f = fixture().await;
    converge(&f).await;
    let w = f.peer.list_workspaces().await.unwrap().remove(0);
    let task = f
        .peer
        .create_task(&w, draft("image parent"))
        .await
        .unwrap()
        .task;
    let mut image_bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(3, 2)
        .write_to(&mut image_bytes, image::ImageFormat::Png)
        .unwrap();
    let bytes = image_bytes.into_inner();
    let added = f
        .peer
        .add_task_attachment(
            &w,
            &f.root.path().join("peer-blobs"),
            Default::default(),
            &task.id,
            aven_core::operations::AttachmentAddInput {
                filename: Some("local.png".into()),
                alt_text: None,
                declared_media_type: None,
                bytes: bytes.clone(),
                optimization_policy: aven_core::attachments::ImageOptimizationPolicy::Preserve,
                dedupe_existing: false,
            },
        )
        .await
        .unwrap();
    let c = Client::new(&f.origin).unwrap();
    for round in 0..4 {
        let result = c
            .round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
            .await
            .unwrap();
        if round == 0 {
            assert_eq!(result.images, ImageTransfer::Complete);
        }
        assert_ne!(result.images, ImageTransfer::Failed);
        if result.metadata_caught_up {
            break;
        }
    }
    assert!(
        c.round(&f.seed_store, &f.seed, f.root.path())
            .await
            .unwrap()
            .metadata_caught_up
    );
    assert_eq!(
        scalar(
            &f.seed,
            "SELECT count(*) FROM task_attachments WHERE deleted=0"
        )
        .await,
        2
    );
    assert_eq!(
        scalar(
            &f.server,
            "SELECT count(*) FROM server_e2ee_images WHERE origin IS NOT NULL AND complete=1"
        )
        .await,
        2
    );
    f.seed
        .delete_task_attachment(&w, &added.outcome.attachment.attachment_id)
        .await
        .unwrap();
    let result = c
        .round(&f.seed_store, &f.seed, f.root.path())
        .await
        .unwrap();
    assert!(result.metadata_caught_up);
    c.round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
        .await
        .unwrap();
    assert_eq!(
        scalar(
            &f.peer,
            "SELECT count(*) FROM task_attachments WHERE deleted=1"
        )
        .await,
        1
    );
}

#[tokio::test]
async fn deletion_conflict_force_and_exact_retry_keep_conservative_parent_protection() {
    let f = fixture().await;
    converge(&f).await;
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    let task = f
        .seed
        .export_data("now".into())
        .await
        .unwrap()
        .tables
        .task_attachments[0]
        .task_id
        .clone();
    f.seed
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
    let c = Client::new(&f.origin).unwrap();
    converge(&f).await;
    assert_eq!(
        scalar(&f.server, "SELECT protected FROM server_e2ee_image_parents").await,
        1
    );
    assert_eq!(
        scalar_after_prune_pass(
            &f.server,
            "SELECT count(*) FROM server_e2ee_images WHERE unreferenced_at IS NOT NULL"
        )
        .await,
        0
    );
    f.seed
        .resolve_conflict(&w, &task, "deleted", "1")
        .await
        .unwrap();
    converge(&f).await;
    assert_eq!(
        scalar(&f.server, "SELECT protected FROM server_e2ee_image_parents").await,
        1
    );
    let inputs = f.seed_store.tail_inputs(&f.seed, &f.origin).await.unwrap();
    let (record, before): (Vec<u8>, i64) = {
        let mut conn = aven_core::test_support::acquire(&f.server).await.unwrap();
        sqlx::query_as(
            "SELECT record,sequence FROM server_e2ee_tail ORDER BY sequence DESC LIMIT 1",
        )
        .fetch_one(&mut *conn)
        .await
        .unwrap()
    };
    let Reply::Appended(m) = c
        .exchange(
            &inputs.authority.context,
            &inputs.bearer,
            Operation::Append {
                ticket: None,
                record,
            },
        )
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(m.sequence, before);
    assert_eq!(
        scalar(&f.server, "SELECT high_water FROM server_e2ee_allocator").await,
        before
    );
    assert_eq!(
        scalar(&f.server, "SELECT protected FROM server_e2ee_image_parents").await,
        1
    );
}

#[tokio::test]
async fn missing_installation_or_protected_coverage_refuses_before_preparation() {
    for missing_key in [false, true] {
        let f = fixture().await;
        converge(&f).await;
        let w = f.peer.list_workspaces().await.unwrap().remove(0);
        f.peer
            .create_task(&w, draft("preserve local work"))
            .await
            .unwrap();
        if missing_key {
            let record = std::fs::read_dir(f.root.path().join("peer-keys"))
                .unwrap()
                .map(|p| p.unwrap().path())
                .find(|p| p.extension().is_some_and(|s| s == "peer-verified"))
                .unwrap();
            std::fs::remove_file(record).unwrap();
        } else {
            let mut conn = aven_core::test_support::acquire(&f.peer).await.unwrap();
            sqlx::query("DELETE FROM local_peer_snapshot_install")
                .execute(&mut *conn)
                .await
                .unwrap();
        }
        assert!(
            Client::new(&f.origin)
                .unwrap()
                .round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
                .await
                .is_err()
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
            1
        );
        assert_eq!(
            scalar(&f.server, "SELECT count(*) FROM server_e2ee_tail").await,
            1
        );
    }
}

#[tokio::test]
async fn note_and_relationship_removals_use_existing_domain_operations() {
    let f = fixture().await;
    converge(&f).await;
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    let a = f.seed.create_task(&w, draft("parent")).await.unwrap().task;
    let b = f.seed.create_task(&w, draft("child")).await.unwrap().task;
    f.seed.create_label(&w, "tag").await.unwrap();
    f.seed
        .update_task(
            &w,
            &b.id,
            TaskUpdate {
                add_labels: vec!["tag".into()],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let note = f
        .seed
        .add_note(&w, &b.id, "initial note".into())
        .await
        .unwrap();
    f.seed.add_task_to_epic(&w, &b.id, &a.id).await.unwrap();
    f.seed
        .add_task_related_link(&w, &a.id, &b.id)
        .await
        .unwrap();
    f.seed.add_task_dependency(&w, &a.id, &b.id).await.unwrap();
    converge(&f).await;
    f.peer
        .edit_note(&w, &b.id, &note.note_id, "edited note".into())
        .await
        .unwrap();
    converge(&f).await;
    f.peer.delete_note(&w, &b.id, &note.note_id).await.unwrap();
    f.peer
        .remove_task_from_epic(&w, &b.id, &a.id)
        .await
        .unwrap();
    f.peer
        .remove_task_related_link(&w, &a.id, &b.id)
        .await
        .unwrap();
    f.peer
        .remove_task_dependency(&w, &a.id, &b.id)
        .await
        .unwrap();
    f.peer
        .update_task(
            &w,
            &b.id,
            TaskUpdate {
                remove_labels: vec!["tag".into()],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    converge(&f).await;
    for db in [&f.seed, &f.peer] {
        assert_eq!(scalar(db, "SELECT count(*) FROM task_epic_links").await, 0);
        assert_eq!(
            scalar(db, "SELECT count(*) FROM task_related_links WHERE linked=1").await,
            0
        );
        assert_eq!(
            scalar(db, "SELECT count(*) FROM task_dependencies").await,
            0
        );
        assert_eq!(scalar(db, "SELECT count(*) FROM task_labels").await, 0);
    }
}

fn maximal_envelope<T: Serialize>(operation: T) -> usize {
    let context = Context {
        vault: [255; 32],
        genesis: [255; 32],
        device: [255; 32],
        head: [255; 32],
        stream: [255; 32],
        descriptor: [255; 32],
    };
    serde_json::to_vec(&Envelope {
        context,
        correlation: [255; 32],
        operation,
    })
    .unwrap()
    .len()
}

fn maximal_accepted(record: usize) -> Accepted {
    Accepted {
        mapping: tail::Mapping {
            // Every character escapes to six bytes.
            operation_id: "\u{1}".repeat(256),
            sequence: i64::MIN,
            commitment: [255; 32],
        },
        record: vec![255; record],
    }
}

#[test]
fn maximal_bodies_fit_their_transport_limits() {
    use aven_core::sync::encrypted_tail::attachments::{self as images, Operation as Op};
    let record = || vec![255; tail::RECORD_LIMIT];
    let append = Operation::Append {
        ticket: Some(images::Ticket {
            reservation: [255; 32],
        }),
        record: record(),
    };
    assert!(maximal_envelope(append) <= tail::APPEND_LIMIT);
    let found = Reply::Found(maximal_accepted(tail::RECORD_LIMIT));
    assert!(maximal_envelope(found) <= tail::APPEND_LIMIT);

    // The server stops a page at `PAGE_COUNT` records or `PAGE_BYTES` of
    // serialized records, whichever comes first; fill both.
    let framing = serde_json::to_vec(&maximal_accepted(0)).unwrap().len() + 1;
    let size = (tail::PAGE_BYTES / tail::PAGE_COUNT - framing) / 4 * 3;
    let records: Vec<_> = (0..tail::PAGE_COUNT)
        .map(|_| maximal_accepted(size))
        .collect();
    assert!(serde_json::to_vec(&records).unwrap().len() > tail::PAGE_BYTES - tail::PAGE_COUNT * 4);
    let page = Reply::Page(tail::Page {
        after: i64::MIN,
        watermark: i64::MIN,
        cursor: i64::MIN,
        has_more: false,
        records,
    });
    assert!(maximal_envelope(page) <= tail::RESPONSE_LIMIT);
    // A lone maximal record always fits a page, so every pull makes progress.
    assert!(
        serde_json::to_vec(&maximal_accepted(tail::RECORD_LIMIT))
            .unwrap()
            .len()
            < tail::PAGE_BYTES
    );

    let chunk = images::Reply::Chunk(vec![255; images::CHUNK_BYTES]);
    assert!(maximal_envelope(chunk) <= images::HTTP_LIMIT);
    let put = Op::Put {
        workspace: "\u{1}".repeat(256),
        object: [255; 32],
        descriptor_commitment: [255; 32],
        reservation: [255; 32],
        index: usize::MAX,
        record: vec![255; images::CHUNK_BYTES],
    };
    assert!(maximal_envelope(put) <= images::HTTP_LIMIT);
}

#[tokio::test]
async fn bounded_http_pull_keeps_watermark_and_makes_byte_limited_progress() {
    let f = fixture().await;
    converge(&f).await;
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    let count = 40;
    for i in 0..count {
        let mut d = draft(&format!("large {i}"));
        d.description = "x".repeat(65000);
        f.seed.create_task(&w, d).await.unwrap();
    }
    let c = Client::new(&f.origin).unwrap();
    drain(&c, &f.seed_store, &f.seed).await;
    let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
    let a = &inputs.authority;
    let mut after = f.peer.encrypted_round_state(a).await.unwrap().cursor;
    let mut watermark = None;
    let mut total = 0;
    let mut pages = 0;
    loop {
        let Reply::Page(page) = c
            .exchange(
                &a.context,
                &inputs.bearer,
                Operation::Pull {
                    after,
                    limit: tail::PAGE_COUNT,
                    watermark,
                },
            )
            .await
            .unwrap()
        else {
            panic!()
        };
        let raw = page.records.iter().map(|r| r.record.len()).sum::<usize>();
        let largest = page.records.iter().map(|r| r.record.len()).max().unwrap();
        assert!(serde_json::to_vec(&page.records).unwrap().len() <= tail::PAGE_BYTES);
        if pages == 0 {
            assert!(page.has_more);
            assert!(
                raw + largest <= tail::PAGE_BYTES,
                "the serialized size, not the record bytes, must shorten the first page"
            );
        }
        if let Some(mark) = watermark {
            assert_eq!(page.watermark, mark)
        } else {
            watermark = Some(page.watermark)
        }
        f.peer.apply_encrypted_tail_page(a, &page).await.unwrap();
        total += page.records.len();
        pages += 1;
        if !page.has_more {
            break;
        }
        assert!(page.cursor > after);
        after = page.cursor;
    }
    assert_eq!(total, count);
    assert_eq!(pages, 2);
}

async fn spawn_server(f: &Fixture) -> tokio::process::Child {
    let ready = f.root.path().join("tail-server-ready");
    let _ = std::fs::remove_file(&ready);
    let child = e2ee_http::worker("encrypted_tail_http::tests::server_worker")
        .env("AVEN_TAIL_ROOT", f.root.path())
        .env("AVEN_TAIL_ORIGIN", &f.origin)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        while !ready.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    child
}
#[tokio::test]
#[ignore = "subprocess encrypted tail server"]
async fn server_worker() {
    let root = std::path::PathBuf::from(std::env::var_os("AVEN_TAIL_ROOT").unwrap());
    let origin = std::env::var("AVEN_TAIL_ORIGIN").unwrap();
    let db = Database::open(&root.join("server.sqlite")).await.unwrap();
    let (_, task) = serve(db, origin.strip_prefix("http://").unwrap()).await;
    std::fs::write(root.join("tail-server-ready"), b"ready").unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn successful_tail_response_is_not_cacheable() {
    let f = fixture().await;
    let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
    let request = Request::builder()
        .method("POST")
        .uri(PATH)
        .header(header::CONTENT_TYPE, "application/json")
        .header(
            header::AUTHORIZATION,
            format!("Bearer {}", hex::encode(inputs.bearer.expose())),
        )
        .body(axum::body::Body::from(
            serde_json::to_vec(&Envelope {
                context: inputs.authority.context.clone(),
                correlation: [0; 32],
                operation: Operation::Pull {
                    after: inputs.authority.prefix,
                    limit: 1,
                    watermark: None,
                },
            })
            .unwrap(),
        ))
        .unwrap();
    let response = handle(
        State(Arc::new(Server {
            image_policy: crate::config::AttachmentLifecycleConfig::default().server_policy(),
            db: f.server.clone(),
            gate: crate::http_admission::Admission::new(2),
        })),
        request,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
}

#[tokio::test]
async fn client_retries_busy_but_not_dispatch_timeout() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let attempts = Arc::new(AtomicUsize::new(0));
    let state = attempts.clone();
    let app = Router::new().route(
        PATH,
        post(move || {
            let state = state.clone();
            async move {
                if state.fetch_add(1, Ordering::SeqCst) == 0 {
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        [(header::RETRY_AFTER, "0")],
                        "",
                    )
                        .into_response()
                } else {
                    crate::http_admission::refusal(
                        StatusCode::REQUEST_TIMEOUT,
                        "encrypted-tail-timeout",
                    )
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = Client::new(&origin).unwrap();
    let result = client
        .exchange(
            &Context {
                vault: [0; 32],
                genesis: [0; 32],
                device: [0; 32],
                head: [0; 32],
                stream: [0; 32],
                descriptor: [0; 32],
            },
            &Secret::new([0; 32]),
            Operation::Pull {
                after: 0,
                limit: 1,
                watermark: None,
            },
        )
        .await;
    let Err(error) = result else {
        panic!("dispatch timeout was accepted")
    };
    assert_eq!(
        error.to_string(),
        "error encrypted-tail-refused outcome-unknown"
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    task.abort();
}

#[tokio::test]
async fn client_reads_only_an_unauthorized_refusal_as_access_refusal() {
    use aven_core::sync::client::errors::{code, is_access_refusal};

    for (status, refusal, expected, refused) in [
        (
            StatusCode::FORBIDDEN,
            "encrypted-tail-unauthorized",
            "enrollment-unauthorized",
            true,
        ),
        (
            StatusCode::UNAUTHORIZED,
            "encrypted-tail-credential",
            "encrypted-tail-refused",
            false,
        ),
    ] {
        let app = Router::new().route(
            PATH,
            post(move || async move { crate::http_admission::refusal(status, refusal) }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let result = Client::new(&origin)
            .unwrap()
            .exchange(
                &Context {
                    vault: [0; 32],
                    genesis: [0; 32],
                    device: [0; 32],
                    head: [0; 32],
                    stream: [0; 32],
                    descriptor: [0; 32],
                },
                &Secret::new([0; 32]),
                Operation::Pull {
                    after: 0,
                    limit: 1,
                    watermark: None,
                },
            )
            .await;
        let Err(error) = result else {
            panic!("{refusal} was accepted")
        };
        assert_eq!(code(&error).as_deref(), Some(expected), "{refusal}");
        assert_eq!(is_access_refusal(&error), refused, "{refusal}");
        task.abort();
    }
}

#[tokio::test]
async fn concurrent_clients_exceeding_tail_permits_complete_drains() {
    let f = fixture().await;
    converge(&f).await;
    let workspace = f.seed.list_workspaces().await.unwrap().remove(0);
    f.seed
        .create_task(&workspace, draft("seed concurrent edit"))
        .await
        .unwrap();
    f.peer
        .create_task(&workspace, draft("peer concurrent edit"))
        .await
        .unwrap();
    let client = Client::new(&f.origin).unwrap();
    tokio::join!(
        drain(&client, &f.seed_store, &f.seed),
        drain(&client, &f.peer_store, &f.peer)
    );
    for db in [&f.seed, &f.peer] {
        assert_eq!(
            scalar(db, "SELECT count(*) FROM changes WHERE server_seq IS NULL").await,
            0
        );
    }
}

#[tokio::test]
async fn overlapping_rounds_repeated_offline_edits_and_reinstall() {
    let f = fixture().await;
    converge(&f).await;
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    let left = f.seed.create_task(&w, draft("left")).await.unwrap().task;
    let right = f.peer.create_task(&w, draft("right")).await.unwrap().task;
    converge(&f).await;
    for index in 0..12 {
        for (db, id, side) in [(&f.seed, &left.id, "left"), (&f.peer, &right.id, "right")] {
            db.update_task(
                &w,
                id,
                TaskUpdate {
                    title: Some(format!("{side}-{index}")),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        }
    }
    let client = Client::new(&f.origin).unwrap();
    tokio::join!(
        drain(&client, &f.seed_store, &f.seed),
        drain(&client, &f.peer_store, &f.peer)
    );
    converge(&f).await;
    let enrollment = crate::peer_enrollment_http::Client::new(&f.origin).unwrap();
    enrollment.install(&f.peer_store, &f.peer).await.unwrap();
    seed_bootstrap_http::Client::new(&f.origin)
        .unwrap()
        .resume(&f.seed_store, &f.seed)
        .await
        .unwrap();
    for db in [&f.seed, &f.peer] {
        assert_eq!(title(db, left.id.as_str()).await, "left-11");
        assert_eq!(title(db, right.id.as_str()).await, "right-11");
        assert_eq!(
            scalar(db, "SELECT count(*) FROM conflicts WHERE resolved=0").await,
            0
        );
        assert_eq!(
            scalar(db, "SELECT count(*) FROM changes WHERE server_seq IS NULL").await,
            0
        );
    }
}

#[tokio::test]
async fn creation_undo_pending_frozen_and_accepted() {
    use aven_core::operations::TaskCreationUndo;
    let f = fixture().await;
    converge(&f).await;
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    let pending = f
        .seed
        .create_task_with_undo(&w, draft("undo pending"), TaskCreationUndo::TuiTask)
        .await
        .unwrap()
        .task;
    f.seed.apply_latest_tui_undo(&w.id).await.unwrap().unwrap();
    assert_eq!(
        scalar(
            &f.seed,
            &format!("SELECT count(*) FROM tasks WHERE id='{}'", pending.id)
        )
        .await,
        0
    );
    let frozen = f
        .seed
        .create_task_with_undo(&w, draft("undo frozen"), TaskCreationUndo::TuiTask)
        .await
        .unwrap()
        .task;
    let record = {
        let inputs = f.seed_store.tail_inputs(&f.seed, &f.origin).await.unwrap();
        head_record(&f.seed, &inputs.authority).await
    };
    let error = f.seed.apply_latest_tui_undo(&w.id).await.err().unwrap();
    assert!(
        error.to_string().contains("encrypted-history-owned"),
        "{error:#}"
    );
    assert_eq!(title(&f.seed, frozen.id.as_str()).await, "undo frozen");
    {
        let inputs = f.seed_store.tail_inputs(&f.seed, &f.origin).await.unwrap();
        assert_eq!(head_record(&f.seed, &inputs.authority).await, record);
    }
    converge(&f).await;
    f.seed.apply_latest_tui_undo(&w.id).await.unwrap().unwrap();
    converge(&f).await;
    for db in [&f.seed, &f.peer] {
        assert_eq!(
            scalar(
                db,
                &format!("SELECT deleted FROM tasks WHERE id='{}'", frozen.id)
            )
            .await,
            1
        );
    }
}

#[tokio::test]
async fn overlapping_conflicts_and_frozen_pending_change() {
    let f = fixture().await;
    converge(&f).await;
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    let task = f
        .seed
        .create_task(&w, draft("contested"))
        .await
        .unwrap()
        .task;
    converge(&f).await;
    for index in 0..4 {
        for (db, side) in [(&f.seed, "seed"), (&f.peer, "peer")] {
            db.update_task(
                &w,
                &task.id,
                TaskUpdate {
                    title: Some(format!("{side}-{index}")),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        }
    }
    let client = Client::new(&f.origin).unwrap();
    tokio::join!(
        drain(&client, &f.seed_store, &f.seed),
        drain(&client, &f.peer_store, &f.peer)
    );
    converge(&f).await;
    assert!(
        !f.seed
            .task_conflicts(&w, &task.id, Some("title"))
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        !f.peer
            .task_conflicts(&w, &task.id, Some("title"))
            .await
            .unwrap()
            .is_empty()
    );
    f.peer
        .resolve_conflict(&w, &task.id, "title", "resolved checkpoint")
        .await
        .unwrap();
    let frozen = {
        let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
        head_record(&f.peer, &inputs.authority).await
    };
    f.peer
        .update_task(
            &w,
            &task.id,
            TaskUpdate {
                title: Some("later local edit".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    {
        let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
        assert_eq!(head_record(&f.peer, &inputs.authority).await, frozen);
    }
    converge(&f).await;
    for db in [&f.seed, &f.peer] {
        assert_eq!(title(db, task.id.as_str()).await, "later local edit");
        assert!(
            db.task_conflicts(&w, &task.id, Some("title"))
                .await
                .unwrap()
                .is_empty()
        );
    }
}

#[tokio::test]
async fn server_files_exclude_authored_content_and_client_secrets() {
    let f = fixture().await;
    converge(&f).await;
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    let marker = "checkpoint-private-content-91c3a8d7";
    let mut task = draft(marker);
    task.description = format!("description-{marker}");
    f.seed.create_task(&w, task).await.unwrap();
    converge(&f).await;
    let mut forbidden = vec![marker.as_bytes().to_vec()];
    for (store, db) in [(&f.seed_store, &f.seed), (&f.peer_store, &f.peer)] {
        let inputs = store.tail_inputs(db, &f.origin).await.unwrap();
        for secret in [
            inputs.bearer.expose(),
            inputs
                .authority
                .key(inputs.authority.generation())
                .unwrap()
                .protected_storage_bytes(),
        ] {
            forbidden.push(secret.to_vec());
            forbidden.push(hex::encode(secret).into_bytes());
        }
    }
    for suffix in ["", "-wal", "-shm"] {
        let path = f.root.path().join(format!("server.sqlite{suffix}"));
        if let Ok(bytes) = std::fs::read(path) {
            for secret in &forbidden {
                assert!(!bytes.windows(secret.len()).any(|window| window == secret));
            }
        }
    }
    assert_eq!(scalar(&f.server, "SELECT count(*) FROM changes").await, 0);
    assert_eq!(scalar(&f.server, "SELECT count(*) FROM tasks").await, 0);
}

#[tokio::test]
async fn observed_mapping_and_stale_page_contradictions() {
    let f = fixture().await;
    converge(&f).await;
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    f.seed
        .create_task(&w, draft("mapping checkpoint"))
        .await
        .unwrap();
    let inputs = f.seed_store.tail_inputs(&f.seed, &f.origin).await.unwrap();
    let a = &inputs.authority;
    let record = head_record(&f.seed, a).await;
    let client = Client::new(&f.origin).unwrap();
    let Reply::Appended(mapping) = client
        .exchange(
            &a.context,
            &inputs.bearer,
            Operation::Append {
                ticket: None,
                record: record.clone(),
            },
        )
        .await
        .unwrap()
    else {
        panic!("expected real acceptance");
    };
    f.seed.observe_encrypted_tail(a, &mapping).await.unwrap();
    // Fault injection changes only a received mapping, never server acceptance rows.
    for field in 0..2 {
        let mut bad = mapping.clone();
        if field == 0 {
            bad.sequence += 1;
        } else {
            bad.commitment[0] ^= 1;
        }
        assert!(f.seed.observe_encrypted_tail(a, &bad).await.is_err());
    }
    let after = f.seed.encrypted_round_state(a).await.unwrap().cursor;
    let Reply::Page(page) = client
        .exchange(
            &a.context,
            &inputs.bearer,
            Operation::Pull {
                after,
                limit: 16,
                watermark: None,
            },
        )
        .await
        .unwrap()
    else {
        panic!("expected real pull");
    };
    let mut bad = page.clone();
    bad.records[0].mapping.sequence += 1;
    bad.cursor += 1;
    bad.watermark += 1;
    assert!(f.seed.apply_encrypted_tail_page(a, &bad).await.is_err());
    assert_eq!(f.seed.encrypted_round_state(a).await.unwrap().cursor, after);
    assert_eq!(head_record(&f.seed, a).await, record);
    f.seed.apply_encrypted_tail_page(a, &page).await.unwrap();
    assert!(f.seed.apply_encrypted_tail_page(a, &page).await.is_err());
    assert_eq!(
        f.seed.encrypted_round_state(a).await.unwrap().cursor,
        page.cursor
    );
    assert!(
        f.seed
            .prepare_encrypted_push(a, &blobs(&f.seed))
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn snapshot_shared_image_survives_parent_delete_restore() {
    let f = fixture_with(FixtureOptions {
        shared_image: true,
        ..Default::default()
    })
    .await;
    converge(&f).await;
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    let export = f.seed.export_data("checkpoint".into()).await.unwrap();
    let refs = &export.tables.task_attachments;
    assert_eq!(refs.len(), 2);
    assert_eq!(refs[0].sha256, refs[1].sha256);
    assert_ne!(refs[0].task_id, refs[1].task_id);
    assert_eq!(
        scalar(&f.server, "SELECT count(*) FROM server_e2ee_images").await,
        1
    );
    assert_eq!(
        scalar(
            &f.server,
            "SELECT count(*) FROM server_e2ee_image_references"
        )
        .await,
        2
    );
    let before = files(&f.root.path().join("objects/sha256"));
    for (db, id, deleted, unreferenced) in [
        (&f.seed, &refs[0].task_id, true, 0),
        (&f.peer, &refs[1].task_id, true, 1),
        (&f.seed, &refs[0].task_id, false, 0),
    ] {
        db.update_task(
            &w,
            id,
            TaskUpdate {
                deleted: Some(deleted),
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
            unreferenced
        );
    }
    assert_eq!(files(&f.root.path().join("objects/sha256")), before);
    assert_eq!(
        files(&f.root.path().join("peer-blobs/objects/sha256")),
        before
    );
    assert_eq!(
        scalar(
            &f.server,
            "SELECT count(*) FROM server_e2ee_image_references WHERE deleted=0"
        )
        .await,
        2
    );
}

#[tokio::test]
async fn metadata_resolution_rejects_unsendable_value() {
    let f = fixture().await;
    converge(&f).await;
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    let mut d = draft("metadata resolution bound");
    d.metadata = vec![aven_core::metadata::TaskMetadataInput {
        expected_field_id: None,
        key: "owner".into(),
        value: "initial".into(),
    }];
    let task = f.seed.create_task(&w, d).await.unwrap().task;
    converge(&f).await;
    for (db, value) in [(&f.seed, "seed"), (&f.peer, "peer")] {
        db.update_task(
            &w,
            &task.id,
            TaskUpdate {
                set_metadata: vec![aven_core::metadata::TaskMetadataInput {
                    expected_field_id: None,
                    key: "owner".into(),
                    value: value.into(),
                }],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }
    converge(&f).await;
    let conflicts = f.seed.task_conflicts(&w, &task.id, None).await.unwrap();
    assert_eq!(conflicts.len(), 1);
    let oversized = "x".repeat(4097);
    assert!(
        f.seed
            .update_task(
                &w,
                &task.id,
                TaskUpdate {
                    set_metadata: vec![aven_core::metadata::TaskMetadataInput {
                        expected_field_id: None,
                        key: "owner".into(),
                        value: oversized.clone(),
                    }],
                    ..Default::default()
                },
            )
            .await
            .is_err()
    );
    let resolution = f
        .seed
        .resolve_conflict(&w, &task.id, &conflicts[0].field, &oversized)
        .await;
    let error = resolution
        .err()
        .expect("oversized resolution must refuse before commit");
    assert!(
        error.to_string().contains("metadata-value-too-large"),
        "{error:#}"
    );
    assert_eq!(
        f.seed
            .task_conflicts(&w, &task.id, None)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        scalar(
            &f.seed,
            "SELECT count(*) FROM task_metadata WHERE length(value)=4097"
        )
        .await,
        0
    );
    f.seed
        .resolve_conflict(&w, &task.id, &conflicts[0].field, &"x".repeat(4096))
        .await
        .unwrap();
    f.seed
        .create_task(&w, draft("later valid work"))
        .await
        .unwrap();
    converge(&f).await;
    for db in [&f.seed, &f.peer] {
        assert_eq!(
            scalar(db, "SELECT count(*) FROM changes WHERE server_seq IS NULL").await,
            0
        );
        assert_eq!(
            db.task_metadata(&w.id, &task.id).await.unwrap()[0].value,
            "x".repeat(4096)
        );
        assert!(
            db.task_conflicts(&w, &task.id, None)
                .await
                .unwrap()
                .is_empty()
        );
    }
}

async fn shared_note(
    f: &Fixture,
) -> (
    aven_core::workspaces::Workspace,
    aven_core::ids::TaskId,
    String,
) {
    converge(f).await;
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    let task = f
        .seed
        .create_task(&w, draft("shared note owner"))
        .await
        .unwrap()
        .task;
    let note = f
        .seed
        .add_note(&w, &task.id, "initial".into())
        .await
        .unwrap();
    converge(f).await;
    (w, task.id, note.note_id)
}

async fn note_body(db: &Database, note: &str) -> Option<String> {
    let mut conn = aven_core::test_support::acquire(db).await.unwrap();
    sqlx::query_scalar("SELECT body FROM notes WHERE id=?")
        .bind(note)
        .fetch_optional(&mut *conn)
        .await
        .unwrap()
}

async fn drain_note_order(f: &Fixture, seed_first: bool) {
    let c = Client::new(&f.origin).unwrap();
    if seed_first {
        converge(f).await;
    } else {
        drain(&c, &f.peer_store, &f.peer).await;
        drain(&c, &f.seed_store, &f.seed).await;
        drain(&c, &f.peer_store, &f.peer).await;
    }
    converge(f).await;
    assert_quiescent(&[&f.seed, &f.peer]).await;
}

#[tokio::test]
async fn concurrent_note_edits_follow_accepted_order() {
    for seed_first in [true, false] {
        let edits = 3;
        let f = fixture().await;
        let (w, task, note) = shared_note(&f).await;
        for i in 0..edits {
            f.seed
                .edit_note(&w, &task, &note, format!("seed offline edit {i}"))
                .await
                .unwrap();
            f.peer
                .edit_note(&w, &task, &note, format!("peer offline edit {i}"))
                .await
                .unwrap();
        }
        drain_note_order(&f, seed_first).await;
        let last = if seed_first { "peer" } else { "seed" };
        let expected = format!("{last} offline edit {}", edits - 1);
        for db in [&f.seed, &f.peer] {
            assert_eq!(
                note_body(db, &note).await.as_deref(),
                Some(expected.as_str())
            );
        }
    }
}

#[tokio::test]
async fn note_pending_edit_survives_frozen_acceptance_and_restart() {
    let f = fixture().await;
    let (w, task, note) = shared_note(&f).await;
    f.peer
        .edit_note(&w, &task, &note, "remote first".into())
        .await
        .unwrap();
    let client = Client::new(&f.origin).unwrap();
    drain(&client, &f.peer_store, &f.peer).await;
    f.seed
        .edit_note(&w, &task, &note, "frozen".into())
        .await
        .unwrap();
    let record = {
        let inputs = f.seed_store.tail_inputs(&f.seed, &f.origin).await.unwrap();
        head_record(&f.seed, &inputs.authority).await
    };
    f.seed
        .edit_note(&w, &task, &note, "later pending".into())
        .await
        .unwrap();
    {
        let inputs = f.seed_store.tail_inputs(&f.seed, &f.origin).await.unwrap();
        let a = &inputs.authority;
        assert_eq!(head_record(&f.seed, a).await, record);
        let Reply::Appended(mapping) = client
            .exchange(
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
            panic!("append");
        };
        f.seed.observe_encrypted_tail(a, &mapping).await.unwrap();
        let Reply::Found(accepted) = client
            .exchange(
                &a.context,
                &inputs.bearer,
                Operation::Lookup {
                    operation_id: mapping.operation_id.clone(),
                    expected: Some(mapping),
                },
            )
            .await
            .unwrap()
        else {
            panic!("lookup");
        };
        let cursor = f.seed.encrypted_round_state(a).await.unwrap().cursor;
        f.seed
            .verify_encrypted_tail_outcome(a, &accepted)
            .await
            .unwrap();
        assert_eq!(
            f.seed.encrypted_round_state(a).await.unwrap().cursor,
            cursor
        );
        assert_eq!(
            note_body(&f.seed, &note).await.as_deref(),
            Some("later pending")
        );
    }
    let reopened = Database::open(f.seed.path()).await.unwrap();
    {
        let inputs = f
            .seed_store
            .tail_inputs(&reopened, &f.origin)
            .await
            .unwrap();
        let a = &inputs.authority;
        let Reply::Page(page) = client
            .exchange(
                &a.context,
                &inputs.bearer,
                Operation::Pull {
                    after: reopened.encrypted_round_state(a).await.unwrap().cursor,
                    limit: 16,
                    watermark: None,
                },
            )
            .await
            .unwrap()
        else {
            panic!("pull");
        };
        reopened.apply_encrypted_tail_page(a, &page).await.unwrap();
        assert_eq!(
            note_body(&reopened, &note).await.as_deref(),
            Some("later pending")
        );
        assert_eq!(
            scalar(
                &reopened,
                "SELECT count(*) FROM changes WHERE server_seq IS NULL"
            )
            .await,
            1
        );
    }
    drain_note_order(&f, false).await;
    for db in [&f.seed, &f.peer] {
        assert_eq!(note_body(db, &note).await.as_deref(), Some("later pending"));
    }
}

#[tokio::test]
async fn note_delete_wins_over_edits_and_undo_restores() {
    for seed_first in [true, false] {
        let f = fixture().await;
        let (w, task, note) = shared_note(&f).await;
        f.seed
            .edit_note(&w, &task, &note, "offline edit".into())
            .await
            .unwrap();
        f.peer
            .delete_note_with_tui_undo(&w, &task, &note)
            .await
            .unwrap();
        drain_note_order(&f, seed_first).await;
        for db in [&f.seed, &f.peer] {
            assert_eq!(note_body(db, &note).await, None);
        }
        f.peer.apply_latest_tui_undo(&w.id).await.unwrap().unwrap();
        converge(&f).await;
        for db in [&f.seed, &f.peer] {
            assert_eq!(note_body(db, &note).await.as_deref(), Some("initial"));
        }
        f.seed
            .edit_note_with_tui_undo(&w, &task, &note, "undo edit".into())
            .await
            .unwrap();
        converge(&f).await;
        f.seed.apply_latest_tui_undo(&w.id).await.unwrap().unwrap();
        converge(&f).await;
        for db in [&f.seed, &f.peer] {
            assert_eq!(note_body(db, &note).await.as_deref(), Some("initial"));
        }
    }
}

#[tokio::test]
async fn note_undo_preserves_frozen_history_and_pending_restoration() {
    let f = fixture().await;
    let (w, task, note) = shared_note(&f).await;
    f.seed
        .edit_note_with_tui_undo(&w, &task, &note, "frozen edit".into())
        .await
        .unwrap();
    let record = {
        let inputs = f.seed_store.tail_inputs(&f.seed, &f.origin).await.unwrap();
        head_record(&f.seed, &inputs.authority).await
    };
    f.seed.apply_latest_tui_undo(&w.id).await.unwrap().unwrap();
    {
        let inputs = f.seed_store.tail_inputs(&f.seed, &f.origin).await.unwrap();
        assert_eq!(head_record(&f.seed, &inputs.authority).await, record);
    }
    converge(&f).await;
    assert_eq!(note_body(&f.peer, &note).await.as_deref(), Some("initial"));
    f.seed
        .delete_note_with_tui_undo(&w, &task, &note)
        .await
        .unwrap();
    let inputs = f.seed_store.tail_inputs(&f.seed, &f.origin).await.unwrap();
    let record = head_record(&f.seed, &inputs.authority).await;
    drop(inputs);
    f.seed.apply_latest_tui_undo(&w.id).await.unwrap().unwrap();
    f.seed
        .edit_note(&w, &task, &note, "restored pending edit".into())
        .await
        .unwrap();
    {
        let inputs = f.seed_store.tail_inputs(&f.seed, &f.origin).await.unwrap();
        assert_eq!(head_record(&f.seed, &inputs.authority).await, record);
    }
    converge(&f).await;
    for db in [&f.seed, &f.peer] {
        assert_eq!(
            note_body(db, &note).await.as_deref(),
            Some("restored pending edit")
        );
    }
}

#[tokio::test]
async fn note_creation_undo_respects_history_ownership() {
    let f = fixture().await;
    let (w, task, _) = shared_note(&f).await;
    let pending = f
        .seed
        .add_note_with_tui_undo(&w, &task, "pending add".into())
        .await
        .unwrap();
    f.seed.apply_latest_tui_undo(&w.id).await.unwrap().unwrap();
    assert_eq!(note_body(&f.seed, &pending.note_id).await, None);
    let frozen = f
        .seed
        .add_note_with_tui_undo(&w, &task, "frozen add".into())
        .await
        .unwrap();
    let record = {
        let inputs = f.seed_store.tail_inputs(&f.seed, &f.origin).await.unwrap();
        head_record(&f.seed, &inputs.authority).await
    };
    let error = f.seed.apply_latest_tui_undo(&w.id).await.err().unwrap();
    assert!(
        error.to_string().contains("encrypted-history-owned"),
        "{error:#}"
    );
    assert_eq!(
        note_body(&f.seed, &frozen.note_id).await.as_deref(),
        Some("frozen add")
    );
    {
        let inputs = f.seed_store.tail_inputs(&f.seed, &f.origin).await.unwrap();
        assert_eq!(head_record(&f.seed, &inputs.authority).await, record);
    }
    converge(&f).await;
    let error = f.seed.apply_latest_tui_undo(&w.id).await.err().unwrap();
    assert!(
        error.to_string().contains("undo-state-changed"),
        "{error:#}"
    );
    for db in [&f.seed, &f.peer] {
        assert_eq!(note_body(db, &pending.note_id).await, None);
        assert_eq!(
            note_body(db, &frozen.note_id).await.as_deref(),
            Some("frozen add")
        );
    }
}

#[tokio::test]
async fn round_pushes_every_creation_and_keeps_later_undo() {
    let f = fixture().await;
    converge(&f).await;
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    f.seed
        .create_task(&w, draft("first queued creation"))
        .await
        .unwrap();
    let second = f
        .seed
        .create_task_with_undo(
            &w,
            draft("second queued creation"),
            aven_core::operations::TaskCreationUndo::TuiTask,
        )
        .await
        .unwrap()
        .task;
    let client = Client::new(&f.origin).unwrap();
    assert!(
        client
            .round(&f.seed_store, &f.seed, f.root.path())
            .await
            .unwrap()
            .metadata_caught_up
    );
    assert_eq!(
        scalar(&f.seed, "SELECT count(*) FROM local_e2ee_outbox").await,
        0
    );
    assert_eq!(
        scalar(
            &f.seed,
            "SELECT count(*) FROM changes WHERE server_seq IS NULL"
        )
        .await,
        0
    );
    f.seed.apply_latest_tui_undo(&w.id).await.unwrap().unwrap();
    assert_eq!(
        scalar(
            &f.seed,
            &format!(
                "SELECT count(*) FROM tasks WHERE id='{}' AND deleted=1",
                second.id
            )
        )
        .await,
        1
    );
    assert_eq!(
        scalar(
            &f.seed,
            "SELECT count(*) FROM changes WHERE server_seq IS NULL"
        )
        .await,
        1
    );
    assert!(
        client
            .round(&f.seed_store, &f.seed, f.root.path())
            .await
            .unwrap()
            .metadata_caught_up
    );
}

#[tokio::test]
async fn idle_check_preserves_frozen_work_and_checks_authority() {
    let f = fixture().await;
    converge(&f).await;
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    f.seed
        .create_task(&w, draft("frozen idle check"))
        .await
        .unwrap();
    let mut inputs = f.seed_store.tail_inputs(&f.seed, &f.origin).await.unwrap();
    assert!(
        !f.seed
            .encrypted_round_state(&inputs.authority)
            .await
            .unwrap()
            .idle
    );
    assert_eq!(
        scalar(&f.seed, "SELECT count(*) FROM local_e2ee_outbox").await,
        0
    );
    let record = head_record(&f.seed, &inputs.authority).await;
    assert!(
        !f.seed
            .encrypted_round_state(&inputs.authority)
            .await
            .unwrap()
            .idle
    );
    assert_eq!(head_record(&f.seed, &inputs.authority).await, record);
    inputs.authority.sync_generation += 1;
    assert!(
        f.seed
            .encrypted_round_state(&inputs.authority)
            .await
            .is_err()
    );
}

async fn assert_snapshot_note_edits_converge(edit_after_capture: bool) {
    for seed_first in [true, false] {
        let f = fixture_with(FixtureOptions {
            note_after_capture: Some(edit_after_capture),
            ..Default::default()
        })
        .await;
        let w = f.seed.list_workspaces().await.unwrap().remove(0);
        let (task, note, created_at, add_change): (aven_core::ids::TaskId, String, String, String) = {
            let mut conn = aven_core::test_support::acquire(&f.seed).await.unwrap();
            sqlx::query_as("SELECT task_id, id, created_at, change_id FROM notes")
                .fetch_one(&mut *conn)
                .await
                .unwrap()
        };
        assert_eq!(
            note_body(&f.peer, &note).await.as_deref(),
            Some("snapshot body")
        );
        let seed_body = if edit_after_capture {
            "source after capture"
        } else {
            "seed offline edit"
        };
        if edit_after_capture {
            assert_eq!(note_body(&f.seed, &note).await.as_deref(), Some(seed_body));
            assert_eq!(
                scalar(
                    &f.seed,
                    "SELECT count(*) FROM changes WHERE server_seq IS NULL"
                )
                .await,
                1
            );
        } else {
            assert_eq!(
                note_body(&f.seed, &note).await.as_deref(),
                Some("snapshot body")
            );
            f.seed
                .edit_note(&w, &task, &note, seed_body.into())
                .await
                .unwrap();
        }
        for (db, keys) in [(&f.seed, &f.seed_store), (&f.peer, &f.peer_store)] {
            let inputs = keys.tail_inputs(db, &f.origin).await.unwrap();
            let mut conn = aven_core::test_support::acquire(db).await.unwrap();
            let rank: i64 = sqlx::query_scalar(
                "SELECT server_seq FROM changes WHERE change_id=? AND op_type='note_add'",
            )
            .bind(&add_change)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
            assert!(rank <= inputs.authority.prefix);
            let baseline: (String, String) =
                sqlx::query_as("SELECT created_at, change_id FROM notes WHERE id=?")
                    .bind(&note)
                    .fetch_one(&mut *conn)
                    .await
                    .unwrap();
            assert_eq!(baseline, (created_at.clone(), add_change.clone()));
        }
        f.peer
            .edit_note(&w, &task, &note, "peer offline edit".into())
            .await
            .unwrap();
        drain_note_order(&f, seed_first).await;
        let expected = if seed_first {
            "peer offline edit"
        } else {
            seed_body
        };
        for db in [&f.seed, &f.peer] {
            assert_eq!(note_body(db, &note).await.as_deref(), Some(expected));
            let mut conn = aven_core::test_support::acquire(db).await.unwrap();
            let baseline: (String, String) =
                sqlx::query_as("SELECT created_at, change_id FROM notes WHERE id=?")
                    .bind(&note)
                    .fetch_one(&mut *conn)
                    .await
                    .unwrap();
            assert_eq!(baseline, (created_at.clone(), add_change.clone()));
        }
    }
}

#[tokio::test]
async fn snapshot_note_concurrent_edits_follow_accepted_order() {
    assert_snapshot_note_edits_converge(false).await;
}

#[tokio::test]
async fn snapshot_note_keeps_source_edit_between_capture_and_adoption() {
    assert_snapshot_note_edits_converge(true).await;
}

#[tokio::test]
async fn drain_reuses_protected_tail_snapshot() {
    let f = fixture().await;
    converge(&f).await;
    let workspace = f.seed.list_workspaces().await.unwrap().remove(0);
    for index in 0..5 {
        f.seed
            .create_task(&workspace, draft(&format!("snapshot task {index}")))
            .await
            .unwrap();
    }
    let client = Client::new(&f.origin).unwrap();
    let (drain, setup_loads) = crate::protected_local_keys::tests::BACKEND_LOADS
        .measure(client.start_drain(&f.seed_store, &f.seed))
        .await;
    let mut drain = drain.unwrap();
    let (rounds, round_loads) = crate::protected_local_keys::tests::BACKEND_LOADS
        .measure(async {
            for round_number in 1..=16 {
                let round = client
                    .round_in_drain(&f.seed_store, &f.seed, &blobs(&f.seed), &mut drain)
                    .await
                    .unwrap();
                if round.metadata_caught_up && round.images == ImageTransfer::Complete {
                    return round_number;
                }
            }
            panic!("round budget")
        })
        .await;
    assert!(setup_loads > 0);
    assert_eq!(rounds, 1);
    assert_eq!(round_loads, 0);
}

#[tokio::test]
async fn one_round_pushes_an_offline_edit_backlog_and_peer_converges() {
    let f = fixture().await;
    converge(&f).await;
    let workspace = f.seed.list_workspaces().await.unwrap().remove(0);
    let task = f
        .seed
        .create_task(&workspace, draft("offline edit target"))
        .await
        .unwrap()
        .task;
    // Each change is one append, so the backlog spans many appends.
    const OFFLINE_EDITS: usize = 40;
    // Publish the task first so the measured backlog consists only of edits.
    let client = Client::new(&f.origin).unwrap();
    crate::sync::encrypted::drain(
        &client,
        &f.seed_store,
        &f.seed,
        &blobs(&f.seed),
        crate::sync::encrypted::ROUND_LIMIT,
    )
    .await
    .unwrap();
    crate::sync::encrypted::drain(
        &client,
        &f.peer_store,
        &f.peer,
        &blobs(&f.peer),
        crate::sync::encrypted::ROUND_LIMIT,
    )
    .await
    .unwrap();

    let before = scalar(&f.seed, "SELECT count(*) FROM changes").await;
    for index in 0..OFFLINE_EDITS {
        f.seed
            .update_task(
                &workspace,
                &task.id,
                TaskUpdate {
                    title: Some(format!("offline edit {index}")),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
    }
    assert_eq!(
        scalar(&f.seed, "SELECT count(*) FROM changes").await - before,
        OFFLINE_EDITS as i64
    );

    let mut seed_drain = client.start_drain(&f.seed_store, &f.seed).await.unwrap();
    let mut seed_rounds = 1;
    let mut round = client
        .round_in_drain(&f.seed_store, &f.seed, &blobs(&f.seed), &mut seed_drain)
        .await
        .unwrap();
    assert_eq!(
        scalar(
            &f.seed,
            "SELECT count(*) FROM changes WHERE server_seq IS NULL"
        )
        .await,
        0,
        "the first round must empty the offline-edit outbox"
    );
    while !round.metadata_caught_up || round.images != ImageTransfer::Complete {
        assert!(seed_rounds < crate::sync::encrypted::ROUND_LIMIT);
        round = client
            .round_in_drain(&f.seed_store, &f.seed, &blobs(&f.seed), &mut seed_drain)
            .await
            .unwrap();
        seed_rounds += 1;
    }
    let peer = crate::sync::encrypted::drain(
        &client,
        &f.peer_store,
        &f.peer,
        &blobs(&f.peer),
        crate::sync::encrypted::ROUND_LIMIT,
    )
    .await
    .unwrap();
    assert!(peer.metadata_caught_up);
    assert_eq!(
        title(&f.peer, &task.id).await,
        format!("offline edit {}", OFFLINE_EDITS - 1)
    );
}

mod administration;
mod relations;

mod dependencies;
mod metadata_limits;

mod attachments;

mod recurrence;

mod bench;
mod faults;
mod journey;
mod membership;
