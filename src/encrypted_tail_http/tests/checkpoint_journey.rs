use super::*;
use crate::{
    peer_enrollment_http,
    protected_local_keys::{self, tests::isolated_store},
};
use aven_core::{
    choices::TaskSource,
    ids::TaskId,
    metadata::TaskMetadataInput,
    operations::{
        CreateRecurrenceSeriesParams, RecurrenceSeriesDraft, RecurrenceTemplateUpdate, TaskDraft,
        TaskUpdate, UpdateRecurrenceTemplateParams,
    },
    recurrence::{RecurrenceDuePolicy, RecurrenceRule, RecurrenceSchedule},
};
use chrono::Utc;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    process::Stdio,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

struct Node {
    db: Database,
    store: ProtectedLocalKeyStore,
    blobs: PathBuf,
}

struct Journey {
    root: tempfile::TempDir,
    origin: String,
    server: Database,
    server_task: tokio::task::JoinHandle<()>,
    a: Node,
    b: Node,
}

impl Drop for Journey {
    fn drop(&mut self) {
        self.server_task.abort();
    }
}

struct TailState {
    a_task: TaskId,
    b_task: TaskId,
    c_task: TaskId,
    epic_task: TaskId,
}

fn expiry() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3500
}

async fn setup() -> Journey {
    let root = tempfile::tempdir().unwrap();
    let (a_db, a_store, authority, _) = e2ee_http::fixture_with_domain(root.path(), true).await;
    let server = Database::open(&root.path().join("journey-server.sqlite"))
        .await
        .unwrap();
    let (origin, server_task) =
        e2ee_http::serve(e2ee_http::router(server.clone()), "127.0.0.1:0").await;
    e2ee_http::adopt(&origin, &a_db, &a_store, &authority).await;
    let a = Node {
        db: a_db,
        store: a_store,
        blobs: root.path().to_path_buf(),
    };
    let b = join_peer(&root, &origin, "b", &a).await;
    Journey {
        root,
        origin,
        server,
        server_task,
        a,
        b,
    }
}

async fn join_peer(root: &tempfile::TempDir, origin: &str, name: &str, inviter: &Node) -> Node {
    let db = Database::open(&root.path().join(format!("journey-{name}.sqlite")))
        .await
        .unwrap();
    let keys = isolated_store(&db, &root.path().join(format!("journey-{name}-keys"))).await;
    let blobs = root.path().join(format!("journey-{name}-blobs"));
    let enrollment = peer_enrollment_http::Client::new(origin).unwrap();
    enrollment
        .refresh(&inviter.store, &inviter.db)
        .await
        .unwrap();
    let invitation = enrollment
        .invite(&inviter.store, &inviter.db, expiry())
        .await
        .unwrap();
    enrollment
        .request(&keys, &db, Some(invitation))
        .await
        .unwrap();
    assert_eq!(
        keys.enrollment_readiness(&db).await.unwrap(),
        protected_local_keys::EnrollmentReadiness::Pending
    );
    assert!(enrollment.admit(&inviter.store, &inviter.db).await.unwrap());
    assert!(enrollment.complete(&keys, &db).await.unwrap());
    assert!(matches!(
        keys.enrollment_readiness(&db).await.unwrap(),
        protected_local_keys::EnrollmentReadiness::Enrolled { .. }
    ));
    // Protected enrollment is ready before the immutable snapshot is installed.
    let client = Client::new(origin).unwrap();
    assert!(client.round(&keys, &db, &blobs).await.is_err());
    enrollment.install(&keys, &db).await.unwrap();
    let node = Node {
        db,
        store: keys,
        blobs,
    };
    sync_images(origin, &node).await;
    node
}

async fn db_idle(origin: &str, node: &Node) -> bool {
    let Ok(inputs) = node.store.tail_inputs(&node.db, origin).await else {
        return false;
    };
    node.db
        .encrypted_round_state(&inputs.authority)
        .await
        .unwrap()
        .idle
}

async fn sync_metadata(origin: &str, node: &Node) {
    let client = Client::new(origin).unwrap();
    for _ in 0..80 {
        match client.round(&node.store, &node.db, &node.blobs).await {
            Ok(round) if round.metadata_caught_up => return,
            Ok(_) => {}
            Err(error) => panic!("metadata round {}: {error:#}", node.db.path().display()),
        }
    }
    panic!("metadata did not become idle");
}

async fn sync_images(origin: &str, node: &Node) {
    let client = Client::new(origin).unwrap();
    for _ in 0..100 {
        let result = client
            .round(&node.store, &node.db, &node.blobs)
            .await
            .unwrap();
        if result.metadata_caught_up
            && result.images == ImageTransfer::Complete
            && db_idle(origin, node).await
        {
            return;
        }
    }
    panic!("images did not become available");
}

async fn sync_all(origin: &str, nodes: &[&Node]) {
    for _ in 0..8 {
        for node in nodes {
            sync_metadata(origin, node).await;
        }
    }
}

fn draft(title: &str, labels: Vec<String>, metadata: Vec<TaskMetadataInput>) -> TaskDraft {
    TaskDraft {
        title: title.into(),
        description: format!("{title} description"),
        project: Some("app".into()),
        status: "todo".into(),
        priority: "none".into(),
        source: TaskSource::Cli,
        labels,
        metadata,
        available_at: None,
        due_on: None,
        is_epic: false,
    }
}

fn metadata(key: &str, value: &str) -> TaskMetadataInput {
    TaskMetadataInput {
        expected_field_id: None,
        key: key.into(),
        value: value.into(),
    }
}

async fn add_png(node: &Node, task: &TaskId, marker: u8) -> (String, Vec<u8>) {
    let mut image = ::image::RgbaImage::new(13, 9);
    for (index, byte) in image.as_mut().iter_mut().enumerate() {
        *byte = marker.wrapping_add(index as u8).rotate_left(1);
    }
    let mut bytes = std::io::Cursor::new(Vec::new());
    ::image::DynamicImage::ImageRgba8(image)
        .write_to(&mut bytes, ::image::ImageFormat::Png)
        .unwrap();
    let bytes = bytes.into_inner();
    let workspace = node.db.list_workspaces().await.unwrap().remove(0);
    node.db
        .add_task_attachment(
            &workspace,
            &node.blobs,
            Default::default(),
            task,
            aven_core::operations::AttachmentAddInput {
                filename: Some(format!("journey-{marker}.png")),
                alt_text: Some(format!("journey image {marker}")),
                declared_media_type: None,
                bytes: bytes.clone(),
                optimization_policy: aven_core::attachments::ImageOptimizationPolicy::Preserve,
                dedupe_existing: false,
            },
        )
        .await
        .unwrap();
    let hash: String = scalar_string(
        &node.db,
        &format!(
            "SELECT sha256 FROM task_attachments WHERE task_id='{}' AND deleted=0 ORDER BY created_at DESC LIMIT 1",
            task
        ),
    )
    .await;
    (hash, bytes)
}

async fn scalar_i64(db: &Database, sql: &str) -> i64 {
    let mut conn = aven_core::test_support::acquire(db).await.unwrap();
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
        .fetch_one(&mut *conn)
        .await
        .unwrap()
}

async fn scalar_string(db: &Database, sql: &str) -> String {
    let mut conn = aven_core::test_support::acquire(db).await.unwrap();
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
        .fetch_one(&mut *conn)
        .await
        .unwrap()
}

async fn task_value(db: &Database, id: &TaskId, column: &str) -> String {
    scalar_string(db, &format!("SELECT {column} FROM tasks WHERE id='{}'", id)).await
}

async fn metadata_value(db: &Database, task: &TaskId, key: &str) -> String {
    scalar_string(
        db,
        &format!(
            "SELECT tm.value FROM task_metadata tm JOIN metadata_fields mf ON mf.id=tm.field_id AND mf.workspace_id=tm.workspace_id WHERE tm.task_id='{}' AND mf.key='{key}'",
            task
        ),
    )
    .await
}

async fn attachment_hashes(db: &Database) -> Vec<String> {
    let mut conn = aven_core::test_support::acquire(db).await.unwrap();
    sqlx::query_scalar("SELECT sha256 FROM task_attachments WHERE deleted=0 ORDER BY sha256")
        .fetch_all(&mut *conn)
        .await
        .unwrap()
}

async fn assert_images(node: &Node, expected: &BTreeMap<String, Vec<u8>>) {
    let actual = attachment_hashes(&node.db).await;
    let expected_hashes = expected.keys().cloned().collect::<BTreeSet<_>>();
    assert_eq!(
        actual.iter().cloned().collect::<BTreeSet<_>>(),
        expected_hashes
    );
    for (hash, bytes) in expected {
        let path = aven_core::attachments::object_path(&node.blobs, hash).unwrap();
        assert_eq!(std::fs::read(path).unwrap(), *bytes);
    }
}

async fn recurrence(
    db: &Database,
    workspace: &aven_core::workspaces::Workspace,
    title: &str,
    day_offset: i64,
) -> aven_core::operations::RecurrenceCreateOutcome {
    let at = Utc::now() + chrono::Duration::days(day_offset);
    db.create_recurrence_series(
        workspace,
        CreateRecurrenceSeriesParams::new(RecurrenceSeriesDraft {
            title: title.into(),
            description: format!("{title} template"),
            project: "app".into(),
            priority: "none".into(),
            initial_status: "todo".into(),
            labels: vec!["recurrence".into()],
            metadata: vec![metadata("cadence", title)],
            schedule: RecurrenceSchedule::new(
                RecurrenceRule::daily(),
                "UTC".parse().unwrap(),
                at.date_naive(),
                None,
                RecurrenceDuePolicy::SameDay,
            ),
        })
        .at(at)
        .with_create_missing_labels(),
    )
    .await
    .unwrap()
}

async fn create_tail_state(h: &Journey, c: &Node) -> (TailState, BTreeMap<String, Vec<u8>>) {
    let workspace = h.a.db.list_workspaces().await.unwrap().remove(0);
    for node in [&h.a, &h.b, c] {
        node.db.create_label(&workspace, "from-a").await.unwrap();
        node.db.create_label(&workspace, "from-b").await.unwrap();
        node.db.create_label(&workspace, "from-c").await.unwrap();
    }
    let mut a_draft = draft(
        "journey-a",
        vec!["from-a".into()],
        vec![metadata("owner", "a")],
    );
    a_draft.description = "E2EE-CHECKPOINT-PLAINTEXT-CANARY".into();
    let a_task =
        h.a.db
            .create_task(&workspace, a_draft)
            .await
            .unwrap()
            .task
            .id;
    let b_task =
        h.b.db
            .create_task(
                &workspace,
                draft(
                    "journey-b",
                    vec!["from-b".into()],
                    vec![metadata("owner", "b")],
                ),
            )
            .await
            .unwrap()
            .task
            .id;
    let c_task =
        c.db.create_task(
            &workspace,
            draft(
                "journey-c",
                vec!["from-c".into()],
                vec![metadata("owner", "c")],
            ),
        )
        .await
        .unwrap()
        .task
        .id;
    let a_note =
        h.a.db
            .add_note(&workspace, &a_task, "note from A".into())
            .await
            .unwrap();
    let b_note =
        h.b.db
            .add_note(&workspace, &b_task, "note from B".into())
            .await
            .unwrap();
    let c_note =
        c.db.add_note(&workspace, &c_task, "note from C".into())
            .await
            .unwrap();
    let a_series = recurrence(&h.a.db, &workspace, "series-a", 0).await;
    let b_series = recurrence(&h.b.db, &workspace, "series-b", 1).await;
    recurrence(&c.db, &workspace, "series-c", 2).await;
    sync_all(&h.origin, &[&h.a, &h.b, c]).await;

    h.b.db
        .edit_note(
            &workspace,
            &a_task,
            &a_note.note_id,
            "note A edited by B".into(),
        )
        .await
        .unwrap();
    c.db.edit_note(
        &workspace,
        &b_task,
        &b_note.note_id,
        "note B edited by C".into(),
    )
    .await
    .unwrap();
    h.a.db
        .edit_note(
            &workspace,
            &c_task,
            &c_note.note_id,
            "note C edited by A".into(),
        )
        .await
        .unwrap();
    h.b.db
        .update_task(
            &workspace,
            &a_task,
            TaskUpdate {
                set_metadata: vec![metadata("owner", "b-for-a")],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    c.db.update_task(
        &workspace,
        &b_task,
        TaskUpdate {
            set_metadata: vec![metadata("owner", "c-for-b")],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    h.a.db
        .update_task(
            &workspace,
            &c_task,
            TaskUpdate {
                set_metadata: vec![metadata("owner", "a-for-c")],
                ..Default::default()
            },
        )
        .await
        .unwrap();

    h.a.db
        .add_task_dependency(&workspace, &a_task, &b_task)
        .await
        .unwrap();
    h.a.db
        .add_task_related_link(&workspace, &a_task, &c_task)
        .await
        .unwrap();
    c.db.add_task_dependency(&workspace, &c_task, &a_task)
        .await
        .unwrap();
    c.db.add_task_related_link(&workspace, &b_task, &c_task)
        .await
        .unwrap();
    let epic =
        h.b.db
            .create_task(
                &workspace,
                TaskDraft {
                    is_epic: true,
                    ..draft("journey-epic", vec!["from-b".into()], Vec::new())
                },
            )
            .await
            .unwrap()
            .task;
    h.b.db
        .add_task_to_epic(&workspace, &c_task, &epic.id)
        .await
        .unwrap();
    h.b.db
        .update_recurrence_template(
            &workspace,
            &a_series.series.id,
            UpdateRecurrenceTemplateParams::new(RecurrenceTemplateUpdate {
                title: Some("series-a-from-b".into()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
    c.db.update_recurrence_template(
        &workspace,
        &b_series.series.id,
        UpdateRecurrenceTemplateParams::new(RecurrenceTemplateUpdate {
            title: Some("series-b-from-c".into()),
            ..Default::default()
        }),
    )
    .await
    .unwrap();
    c.db.update_task(
        &workspace,
        &a_series.task.id,
        TaskUpdate {
            status: Some("done".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let mut expected = BTreeMap::new();
    for (node, task, marker) in [(&h.a, &a_task, 11), (&h.b, &b_task, 37), (c, &c_task, 83)] {
        let (hash, bytes) = add_png(node, task, marker).await;
        expected.insert(hash, bytes);
    }
    for node in [&h.a, &h.b, c] {
        sync_images(&h.origin, node).await;
    }
    sync_all(&h.origin, &[&h.a, &h.b, c]).await;
    for node in [&h.a, &h.b, c] {
        sync_images(&h.origin, node).await;
    }
    (
        TailState {
            a_task,
            b_task,
            c_task,
            epic_task: epic.id,
        },
        expected,
    )
}

async fn related_link(db: &Database, left: &TaskId, right: &TaskId) -> i64 {
    let (a, b) = if left.as_str() < right.as_str() {
        (left.as_str(), right.as_str())
    } else {
        (right.as_str(), left.as_str())
    };
    scalar_i64(
        db,
        &format!(
            "SELECT count(*) FROM task_related_links WHERE task_a_id='{a}' AND task_b_id='{b}' AND linked=1"
        ),
    )
    .await
}

async fn assert_shared_state(nodes: &[&Node], state: &TailState) {
    for node in nodes {
        assert_eq!(
            task_value(&node.db, &state.a_task, "description").await,
            "E2EE-CHECKPOINT-PLAINTEXT-CANARY"
        );
        assert_eq!(
            task_value(&node.db, &state.b_task, "description").await,
            "journey-b description"
        );
        assert_eq!(
            task_value(&node.db, &state.c_task, "description").await,
            "journey-c description"
        );
        assert_eq!(
            scalar_i64(
                &node.db,
                &format!(
                    "SELECT count(*) FROM task_labels WHERE task_id='{}' AND label='from-a'",
                    state.a_task
                ),
            )
            .await,
            1
        );
        assert_eq!(
            scalar_i64(
                &node.db,
                &format!(
                    "SELECT count(*) FROM task_labels WHERE task_id='{}' AND label='from-b'",
                    state.b_task
                ),
            )
            .await,
            1
        );
        assert_eq!(
            scalar_i64(
                &node.db,
                &format!(
                    "SELECT count(*) FROM task_labels WHERE task_id='{}' AND label='from-c'",
                    state.c_task
                ),
            )
            .await,
            1
        );
        assert_eq!(
            metadata_value(&node.db, &state.a_task, "owner").await,
            "b-for-a"
        );
        assert_eq!(
            metadata_value(&node.db, &state.b_task, "owner").await,
            "c-for-b"
        );
        assert_eq!(
            metadata_value(&node.db, &state.c_task, "owner").await,
            "a-for-c"
        );
        assert_eq!(
            scalar_i64(
                &node.db,
                &format!(
                    "SELECT count(*) FROM task_dependencies WHERE task_id='{}' AND depends_on_task_id='{}'",
                    state.a_task, state.b_task
                ),
            )
            .await,
            1
        );
        assert_eq!(
            scalar_i64(
                &node.db,
                &format!(
                    "SELECT count(*) FROM task_dependencies WHERE task_id='{}' AND depends_on_task_id='{}'",
                    state.c_task, state.a_task
                ),
            )
            .await,
            1
        );
        assert_eq!(
            related_link(&node.db, &state.a_task, &state.c_task).await,
            1
        );
        assert_eq!(
            related_link(&node.db, &state.b_task, &state.c_task).await,
            1
        );
        assert_eq!(
            scalar_i64(
                &node.db,
                &format!(
                    "SELECT count(*) FROM task_epic_links WHERE epic_task_id='{}' AND child_task_id='{}'",
                    state.epic_task, state.c_task
                ),
            )
            .await,
            1
        );
        assert!(scalar_i64(&node.db, "SELECT count(*) FROM recurrence_series").await >= 4);
        assert_eq!(
            scalar_i64(
                &node.db,
                "SELECT count(*) FROM recurrence_series WHERE title IN ('series-a-from-b', 'series-b-from-c')",
            )
            .await,
            2
        );
        assert!(scalar_i64(&node.db, "SELECT count(*) FROM notes").await >= 3);
        for body in [
            "note A edited by B",
            "note B edited by C",
            "note C edited by A",
        ] {
            assert_eq!(
                scalar_i64(
                    &node.db,
                    &format!("SELECT count(*) FROM notes WHERE body='{body}'"),
                )
                .await,
                1
            );
        }
        assert!(
            scalar_i64(
                &node.db,
                "SELECT count(*) FROM recurrence_occurrences WHERE outcome='completed'"
            )
            .await
                >= 1
        );
        assert!(
            scalar_i64(
                &node.db,
                "SELECT count(*) FROM task_attachments WHERE deleted=0"
            )
            .await
                >= 4
        );
        assert!(
            scalar_i64(
                &node.db,
                "SELECT count(*) FROM changes WHERE server_seq IS NULL"
            )
            .await
                == 0
        );
    }
}

async fn report_quiescent_rounds(origin: &str, nodes: &[&Node], server: &Database) {
    let started = Instant::now();
    for _ in 0..3 {
        for node in nodes {
            assert!(
                Client::new(origin)
                    .unwrap()
                    .round(&node.store, &node.db, &node.blobs)
                    .await
                    .unwrap()
                    .metadata_caught_up
            );
        }
    }
    let elapsed = started.elapsed();
    let inputs = nodes[0]
        .store
        .active_inputs(&nodes[0].db, origin)
        .await
        .unwrap();
    let membership_sequence = inputs.membership.sequence();
    let device_count = inputs.membership.device_count();
    let prefix = inputs.membership.publication().binding().prefix_count;
    let tail_records = scalar_i64(server, "SELECT count(*) FROM server_e2ee_tail").await;
    let membership_transitions =
        scalar_i64(server, "SELECT count(*) FROM server_membership_transitions").await;
    eprintln!(
        "checkpoint journey timing: rounds=9 elapsed_ms={} prefix={} membership_sequence={} devices={} tail_records={} membership_transitions={}",
        elapsed.as_millis(),
        prefix,
        membership_sequence,
        device_count,
        tail_records,
        membership_transitions
    );
}

async fn assert_server_private(server: &Database, canaries: &[&str]) {
    for table in [
        "tasks",
        "notes",
        "task_metadata",
        "task_dependencies",
        "task_related_links",
        "task_epic_links",
    ] {
        assert_eq!(
            scalar_i64(server, &format!("SELECT count(*) FROM {table}")).await,
            0
        );
    }
    for path in [
        server.path().to_path_buf(),
        PathBuf::from(format!("{}-wal", server.path().display())),
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            for canary in canaries {
                assert!(
                    !bytes
                        .windows(canary.len())
                        .any(|window| window == canary.as_bytes())
                );
            }
        }
    }
}

async fn restart_server(journey: &mut Journey) {
    let address = journey.origin.strip_prefix("http://").unwrap().to_string();
    journey.server_task.abort();
    let _ = (&mut journey.server_task).await;
    let server = Database::open(journey.server.path()).await.unwrap();
    let (origin, server_task) = e2ee_http::serve(e2ee_http::router(server.clone()), &address).await;
    assert_eq!(origin, journey.origin);
    journey.server_task = server_task;
    let old = std::mem::replace(&mut journey.server, server);
    drop(old);
}

async fn run_client_worker(origin: &str, db: &Path, keys: &Path, blobs: &Path) {
    let database = Database::open(db).await.unwrap();
    let store = isolated_store(&database, keys).await;
    let client = Client::new(origin).unwrap();
    for _ in 0..100 {
        let round = client.round(&store, &database, blobs).await.unwrap();
        if round.metadata_caught_up && round.images == ImageTransfer::Complete {
            return;
        }
    }
    panic!("client worker did not catch up");
}

#[tokio::test]
#[ignore = "checkpoint journey client process worker"]
async fn client_worker() {
    run_client_worker(
        &std::env::var("E2EE_JOURNEY_ORIGIN").unwrap(),
        Path::new(&std::env::var("E2EE_JOURNEY_DB").unwrap()),
        Path::new(&std::env::var("E2EE_JOURNEY_KEYS").unwrap()),
        Path::new(&std::env::var("E2EE_JOURNEY_BLOBS").unwrap()),
    )
    .await;
}

#[tokio::test]
async fn normal_whole_engine_e2ee_journey() {
    let mut journey = setup().await;
    let c = join_peer(&journey.root, &journey.origin, "c", &journey.b).await;
    let (state, tail_images) = create_tail_state(&journey, &c).await;
    let initial_hash: String = scalar_string(
        &journey.a.db,
        "SELECT sha256 FROM task_attachments ORDER BY created_at LIMIT 1",
    )
    .await;
    let initial_bytes = std::fs::read(
        aven_core::attachments::object_path(&journey.a.blobs, &initial_hash).unwrap(),
    )
    .unwrap();
    let mut expected = tail_images;
    expected.insert(initial_hash.clone(), initial_bytes);
    for node in [&journey.a, &journey.b, &c] {
        assert_images(node, &expected).await;
    }
    assert_shared_state(&[&journey.a, &journey.b, &c], &state).await;
    report_quiescent_rounds(
        &journey.origin,
        &[&journey.a, &journey.b, &c],
        &journey.server,
    )
    .await;

    let w = journey.a.db.list_workspaces().await.unwrap().remove(0);
    let conflict_task = journey
        .a
        .db
        .create_task(
            &w,
            draft("conflict-base", vec!["from-a".into()], Vec::new()),
        )
        .await
        .unwrap()
        .task;
    sync_all(&journey.origin, &[&journey.a, &journey.b]).await;
    journey
        .a
        .db
        .update_task(
            &w,
            &conflict_task.id,
            TaskUpdate {
                title: Some("conflict-a".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    journey
        .b
        .db
        .update_task(
            &w,
            &conflict_task.id,
            TaskUpdate {
                title: Some("conflict-b".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    sync_all(&journey.origin, &[&journey.a, &journey.b]).await;
    assert!(
        !journey
            .a
            .db
            .task_conflicts(&w, &conflict_task.id, Some("title"))
            .await
            .unwrap()
            .is_empty()
    );
    journey
        .a
        .db
        .resolve_conflict(&w, &conflict_task.id, "title", "conflict-resolved")
        .await
        .unwrap();
    sync_all(&journey.origin, &[&journey.a, &journey.b]).await;
    for node in [&journey.a, &journey.b] {
        assert_eq!(
            task_value(&node.db, &conflict_task.id, "title").await,
            "conflict-resolved"
        );
        assert!(
            node.db
                .task_conflicts(&w, &conflict_task.id, Some("title"))
                .await
                .unwrap()
                .is_empty()
        );
    }

    journey
        .a
        .db
        .update_task(
            &w,
            &state.a_task,
            TaskUpdate {
                title: Some("offline-a-before-removal".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    journey
        .b
        .db
        .update_task(
            &w,
            &state.b_task,
            TaskUpdate {
                title: Some("offline-b-before-removal".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    c.db.update_task(
        &w,
        &state.c_task,
        TaskUpdate {
            title: Some("offline-c-removed".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        task_value(&journey.a.db, &state.a_task, "title").await,
        "offline-a-before-removal"
    );

    let c_device = c
        .store
        .active_inputs(&c.db, &journey.origin)
        .await
        .unwrap()
        .device();
    let enrollment = peer_enrollment_http::Client::new(&journey.origin).unwrap();
    assert_eq!(
        enrollment
            .remove_device(&journey.b.store, &journey.b.db, c_device)
            .await
            .unwrap(),
        peer_enrollment_http::RemovalStatus::Complete
    );
    assert!(
        !journey
            .b
            .store
            .active_inputs(&journey.b.db, &journey.origin)
            .await
            .unwrap()
            .membership
            .rotation_pending()
    );
    assert!(
        Client::new(&journey.origin)
            .unwrap()
            .round(&c.store, &c.db, &c.blobs)
            .await
            .is_err()
    );
    assert!(enrollment.refresh(&c.store, &c.db).await.is_err());

    let post_a = journey
        .a
        .db
        .create_task(
            &w,
            draft("post-rotation-a", vec!["from-a".into()], Vec::new()),
        )
        .await
        .unwrap()
        .task;
    let post_b = journey
        .b
        .db
        .create_task(
            &w,
            draft("post-rotation-b", vec!["from-b".into()], Vec::new()),
        )
        .await
        .unwrap()
        .task;
    let (a_hash, a_bytes) = add_png(&journey.a, &post_a.id, 101).await;
    let (b_hash, b_bytes) = add_png(&journey.b, &post_b.id, 151).await;
    expected.insert(a_hash, a_bytes);
    expected.insert(b_hash, b_bytes);
    for node in [&journey.a, &journey.b] {
        sync_images(&journey.origin, node).await;
    }
    sync_all(&journey.origin, &[&journey.a, &journey.b]).await;
    for node in [&journey.a, &journey.b] {
        sync_images(&journey.origin, node).await;
        assert_images(node, &expected).await;
    }
    restart_server(&mut journey).await;

    let d_db = Database::open(&journey.root.path().join("journey-d.sqlite"))
        .await
        .unwrap();
    let d_store = isolated_store(&d_db, &journey.root.path().join("journey-d-keys")).await;
    let d_blobs = journey.root.path().join("journey-d-blobs");
    let invitation = enrollment
        .invite(&journey.b.store, &journey.b.db, expiry())
        .await
        .unwrap();
    enrollment
        .request(&d_store, &d_db, Some(invitation))
        .await
        .unwrap();
    assert_eq!(
        d_store.enrollment_readiness(&d_db).await.unwrap(),
        protected_local_keys::EnrollmentReadiness::Pending
    );
    assert!(
        enrollment
            .admit(&journey.b.store, &journey.b.db)
            .await
            .unwrap()
    );
    assert!(enrollment.complete(&d_store, &d_db).await.unwrap());
    assert!(matches!(
        d_store.enrollment_readiness(&d_db).await.unwrap(),
        protected_local_keys::EnrollmentReadiness::Enrolled { .. }
    ));
    assert!(
        Client::new(&journey.origin)
            .unwrap()
            .round(&d_store, &d_db, &d_blobs)
            .await
            .is_err()
    );
    enrollment.install(&d_store, &d_db).await.unwrap();
    assert_eq!(
        scalar_i64(
            &d_db,
            &format!("SELECT count(*) FROM tasks WHERE id='{}'", state.a_task),
        )
        .await,
        0
    );
    let d_db_path = d_db.path().to_path_buf();
    let d_keys_path = journey.root.path().join("journey-d-keys");
    let d_blobs_path = d_blobs.clone();
    drop(d_db);
    drop(d_store);
    let log = journey
        .root
        .path()
        .join("checkpoint-journey-client-worker.log");
    let output = std::fs::File::create(log).unwrap();
    let status = e2ee_http::worker("encrypted_tail_http::tests::checkpoint_journey::client_worker")
        .env("E2EE_JOURNEY_ORIGIN", &journey.origin)
        .env("E2EE_JOURNEY_DB", &d_db_path)
        .env("E2EE_JOURNEY_KEYS", &d_keys_path)
        .env("E2EE_JOURNEY_BLOBS", &d_blobs_path)
        .stdout(Stdio::from(output.try_clone().unwrap()))
        .stderr(Stdio::from(output))
        .status()
        .await
        .unwrap();
    if !status.success() {
        panic!(
            "checkpoint worker failed: {}",
            std::fs::read_to_string(
                journey
                    .root
                    .path()
                    .join("checkpoint-journey-client-worker.log")
            )
            .unwrap_or_else(|error| format!("worker log unavailable: {error}"))
        );
    }
    let d_db = Database::open(&d_db_path).await.unwrap();
    let d = Node {
        store: isolated_store(&d_db, &d_keys_path).await,
        db: d_db,
        blobs: d_blobs_path,
    };
    assert_shared_state(&[&journey.a, &journey.b, &d], &state).await;
    for node in [&journey.a, &journey.b, &d] {
        assert_eq!(
            task_value(&node.db, &state.a_task, "title").await,
            "offline-a-before-removal"
        );
        assert_eq!(
            task_value(&node.db, &state.b_task, "title").await,
            "offline-b-before-removal"
        );
        assert_eq!(
            task_value(&node.db, &post_a.id, "title").await,
            "post-rotation-a"
        );
        assert_eq!(
            task_value(&node.db, &post_b.id, "title").await,
            "post-rotation-b"
        );
    }
    assert_images(&d, &expected).await;
    assert!(matches!(
        d.store.enrollment_readiness(&d.db).await.unwrap(),
        protected_local_keys::EnrollmentReadiness::Enrolled { .. }
    ));

    assert_server_private(
        &journey.server,
        &[
            "E2EE-CHECKPOINT-PLAINTEXT-CANARY",
            "offline-a-before-removal",
            "post-rotation-a",
        ],
    )
    .await;
}
