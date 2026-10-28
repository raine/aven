use aven_core::api::{
    AgentAssetInput, AttachmentChangeKind, ChangeOp, ChangeTarget, CreateRecurrenceSeries,
    CreateTask, ErrorCode, ImageOptimizationPolicy, OptionalDateUpdate, RecurrenceDuePolicy,
    RecurrenceFrequency, RecurrenceRule, RecurrenceScheduleInput, StagedAgentAsset, Store,
    TaskCapture, TaskEdit,
};
use aven_core::choices::{TaskPriority, TaskSource, TaskStatus};
use aven_core::db::Database;
use aven_core::ids::{TaskId, WorkspaceId};
use aven_core::test_support::{encrypted_sync::EncryptedSyncServer, open_blank_database};
use sqlx::{Connection, SqliteConnection};

struct Fixture {
    _dir: tempfile::TempDir,
    db: Database,
    store: Store,
    workspace: WorkspaceId,
    task: TaskId,
    conn: SqliteConnection,
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("changes.sqlite");
        let db = open_blank_database(&path).await.unwrap();
        let store = Store::open(&path).await.unwrap();
        let workspace = store.resolve_workspace("default").await.unwrap().id;
        let task = store
            .create_task(
                &workspace,
                CreateTask {
                    title: "Original".into(),
                    description: "original body".into(),
                    project: "Core".into(),
                    status: TaskStatus::Inbox,
                    priority: TaskPriority::None,
                    metadata: vec![],
                    available_at: None,
                    due_on: None,
                },
            )
            .await
            .unwrap()
            .id;
        let conn = SqliteConnection::connect_with(
            &sqlx::sqlite::SqliteConnectOptions::new().filename(&path),
        )
        .await
        .unwrap();
        Self {
            _dir: dir,
            db,
            store,
            workspace,
            task,
            conn,
        }
    }

    async fn exec(&mut self, sql: &'static str) {
        sqlx::raw_sql(sql).execute(&mut self.conn).await.unwrap();
    }

    async fn count(&mut self, table: &str) -> i64 {
        sqlx::query_scalar(match table {
            "tasks" => "SELECT count(*) FROM tasks",
            "notes" => "SELECT count(*) FROM notes",
            "changes" => "SELECT count(*) FROM changes",
            "field_versions" => "SELECT count(*) FROM field_versions",
            "task_labels" => "SELECT count(*) FROM task_labels",
            "labels" => "SELECT count(*) FROM labels",
            "task_workspace_history" => "SELECT count(*) FROM task_workspace_history",
            "tui_undo_entries" => "SELECT count(*) FROM tui_undo_entries",
            "recurrence_occurrences" => "SELECT count(*) FROM recurrence_occurrences",
            _ => panic!("unexpected table"),
        })
        .fetch_one(&mut self.conn)
        .await
        .unwrap()
    }

    async fn counts(&mut self) -> Vec<i64> {
        let mut counts = Vec::new();
        for table in [
            "tasks",
            "notes",
            "changes",
            "field_versions",
            "task_labels",
            "labels",
            "task_workspace_history",
            "tui_undo_entries",
            "recurrence_occurrences",
        ] {
            counts.push(self.count(table).await);
        }
        counts
    }

    async fn note_bodies(&mut self, id: &TaskId) -> Vec<String> {
        sqlx::query_scalar("SELECT body FROM notes WHERE task_id = ? ORDER BY body")
            .bind(id)
            .fetch_all(&mut self.conn)
            .await
            .unwrap()
    }

    async fn snapshot(&self, id: &TaskId) -> aven_core::undo::TaskUndoSnapshot {
        self.db
            .task_undo_snapshot(&self.workspace, id)
            .await
            .unwrap()
    }

    async fn recurring(&self) -> TaskId {
        self.store
            .create_recurrence_series(
                &self.workspace,
                CreateRecurrenceSeries {
                    title: "Daily".into(),
                    description: "".into(),
                    project: "Core".into(),
                    priority: TaskPriority::None,
                    initial_status: TaskStatus::Todo,
                    labels: vec![],
                    metadata: vec![],
                    schedule: RecurrenceScheduleInput {
                        rule: RecurrenceRule {
                            frequency: RecurrenceFrequency::Daily,
                            interval: 1,
                            weekdays: vec![],
                        },
                        timezone: "UTC".into(),
                        start_on: chrono::Utc::now().date_naive().to_string(),
                        available_local_time: None,
                        due_policy: RecurrenceDuePolicy::SameDay,
                    },
                },
            )
            .await
            .unwrap()
            .task
            .id
    }
}

fn capture(title: &str) -> TaskCapture {
    TaskCapture {
        title: title.into(),
        description: "Captured".into(),
        project: Some("Core".into()),
        status: TaskStatus::Inbox,
        priority: TaskPriority::High,
        source: TaskSource::Ios,
        due_on: Some("2099-01-02".into()),
        labels: vec![],
    }
}
fn create(local: &str) -> ChangeOp {
    ChangeOp::Create {
        local_id: local.into(),
        fields: capture("Created"),
    }
}
fn edit(target: ChangeTarget, title: &str) -> ChangeOp {
    ChangeOp::Update {
        target,
        fields: TaskEdit {
            title: Some(title.into()),
            ..Default::default()
        },
    }
}
fn note(target: ChangeTarget, body: &str) -> ChangeOp {
    ChangeOp::AddNote {
        target,
        body: body.into(),
    }
}
fn png(width: u32) -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image::RgbaImage::new(width, 1))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}
fn attach(target: ChangeTarget, attachment_id: &str, pin: &StagedAgentAsset) -> ChangeOp {
    ChangeOp::AddAttachment {
        target,
        attachment_id: attachment_id.into(),
        pin_id: pin.pin_id.clone(),
        filename: Some("screenshot.png".into()),
        alt_text: Some("A screenshot".into()),
    }
}
fn local(id: &str) -> ChangeTarget {
    ChangeTarget::Local(id.into())
}
fn existing(id: &TaskId) -> ChangeTarget {
    ChangeTarget::Existing(id.clone())
}

#[tokio::test]
async fn public_note_round_trips_verbatim_and_has_conditional_undo() {
    let mut f = Fixture::new().await;
    // An old activity value makes queue restoration observable without sleeping.
    f.exec("UPDATE tasks SET queue_activity_at = '2020-01-01T00:00:00Z'")
        .await;
    let before = f.snapshot(&f.task).await;
    let task = f.task.clone();
    for body in ["  called\nwaiting for callback  ", ""] {
        let receipt = f
            .store
            .add_note(&f.workspace, &task, body.into())
            .await
            .unwrap();
        assert_eq!(f.note_bodies(&task).await, [body]);
        assert_eq!(f.count("tui_undo_entries").await, 0);
        f.store.undo_note(&receipt.undo_token).await.unwrap();
        assert!(f.note_bodies(&task).await.is_empty());
        assert_eq!(f.snapshot(&task).await, before);
        assert_eq!(
            f.store
                .undo_note(&receipt.undo_token)
                .await
                .unwrap_err()
                .code,
            ErrorCode::GenerationConflict
        );
    }
    assert_eq!(
        f.store.undo_note("{}").await.unwrap_err().code,
        ErrorCode::Validation
    );
    assert_eq!(
        f.store
            .add_note(&f.workspace, &TaskId::new(), "missing".into())
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
}

#[tokio::test]
async fn mixed_set_resolves_final_refs_and_reverses_repeated_and_local_mutations() {
    let mut f = Fixture::new().await;
    let workspace = f.db.resolve_workspace("default").await.unwrap();
    let project =
        f.db.resolve_or_create_project(&f.workspace, "Other")
            .await
            .unwrap();
    f.db.create_label(&workspace, "important").await.unwrap();
    f.exec("UPDATE tasks SET queue_activity_at = '2020-01-01T00:00:00Z'")
        .await;
    let before = f.snapshot(&f.task).await;
    let result = f
        .store
        .apply_change_set(
            &f.workspace,
            vec![
                create("first"),
                create("second"),
                ChangeOp::Update {
                    target: local("first"),
                    fields: TaskEdit {
                        title: Some("Final title".into()),
                        project_id: Some(project.id),
                        add_labels: vec!["important".into()],
                        due_on: OptionalDateUpdate::Clear,
                        ..Default::default()
                    },
                },
                note(local("first"), "local note"),
                edit(existing(&f.task), "intermediate"),
                note(existing(&f.task), "existing note"),
                ChangeOp::Update {
                    target: existing(&f.task),
                    fields: TaskEdit {
                        title: Some("final".into()),
                        status: Some(TaskStatus::Active),
                        priority: Some(TaskPriority::Urgent),
                        add_labels: vec!["important".into()],
                        ..Default::default()
                    },
                },
            ],
        )
        .await
        .unwrap();
    assert_eq!(result.created.len(), 2);
    let first = &result.created["first"];
    let resolved =
        f.db.resolve_task_ref(&workspace, &first.display_ref)
            .await
            .unwrap();
    assert_eq!(resolved.id, first.task_id);
    assert_eq!(resolved.project_key, "other");
    assert_eq!(resolved.title, "Final title");
    assert_eq!(resolved.status, TaskStatus::Todo); // Existing capture promotion.
    assert_eq!(resolved.source, TaskSource::Ios);
    assert_eq!(f.snapshot(&first.task_id).await.labels, ["important"]);
    assert!(resolved.due_on.is_none());
    assert_eq!(f.note_bodies(&first.task_id).await, ["local note"]);
    assert_eq!(f.snapshot(&f.task).await.title, "final");
    assert_eq!(f.count("tui_undo_entries").await, 0);
    f.store.undo_change_set(&result.undo_token).await.unwrap();
    assert_eq!(f.snapshot(&f.task).await, before);
    let task = f.task.clone();
    assert!(f.note_bodies(&task).await.is_empty());
    for created in result.created.values() {
        let restored = f.snapshot(&created.task_id).await;
        assert!(restored.deleted);
        assert_eq!(restored.title, "Created");
        assert_eq!(restored.project_key, "core");
        assert!(restored.labels.is_empty());
        assert!(f.note_bodies(&created.task_id).await.is_empty());
    }
    assert_eq!(
        f.store
            .undo_change_set(&result.undo_token)
            .await
            .unwrap_err()
            .code,
        ErrorCode::GenerationConflict
    );
}

#[tokio::test]
async fn creates_match_capture_and_empty_or_noop_sets_are_inert() {
    let mut f = Fixture::new().await;
    let input = capture("  parity  ");
    let single = f
        .store
        .capture_queue_task(&f.workspace, input.clone())
        .await
        .unwrap();
    let batch = f
        .store
        .apply_change_set(
            &f.workspace,
            vec![ChangeOp::Create {
                local_id: "x".into(),
                fields: input,
            }],
        )
        .await
        .unwrap();
    let mut expected = f.snapshot(&single.task_id).await;
    let actual = f.snapshot(&batch.created["x"].task_id).await;
    expected.queue_activity_at = actual.queue_activity_at.clone();
    assert_eq!(actual, expected);
    let before = f.counts().await;
    for ops in [
        vec![],
        vec![ChangeOp::Update {
            target: existing(&f.task),
            fields: TaskEdit::default(),
        }],
    ] {
        let result = f.store.apply_change_set(&f.workspace, ops).await.unwrap();
        assert!(result.created.is_empty());
        f.store.undo_change_set(&result.undo_token).await.unwrap();
        assert_eq!(f.counts().await, before);
    }
}

#[tokio::test]
async fn every_operation_is_preflighted_before_even_the_first_write() {
    let mut f = Fixture::new().await;
    let before = f.counts().await;
    f.exec("CREATE TRIGGER reject_write BEFORE INSERT ON tasks BEGIN SELECT RAISE(ABORT, 'write reached'); END;").await;
    let cases = vec![
        (vec![create("a"), create("a")], 1),
        (vec![create("a"), create(" ")], 1),
        (vec![create("a"), note(local("unknown"), "x")], 1),
        (
            vec![create("a"), note(local("future"), "x"), create("future")],
            1,
        ),
        (vec![create("a"), edit(existing(&TaskId::new()), "x")], 1),
        (
            vec![
                create("a"),
                ChangeOp::Create {
                    local_id: "b".into(),
                    fields: TaskCapture {
                        project: Some("missing".into()),
                        ..capture("x")
                    },
                },
            ],
            1,
        ),
        (
            vec![
                create("a"),
                ChangeOp::Create {
                    local_id: "b".into(),
                    fields: TaskCapture {
                        labels: vec![" !!! ".into()],
                        ..capture("x")
                    },
                },
            ],
            1,
        ),
        (
            vec![
                create("a"),
                ChangeOp::Update {
                    target: existing(&f.task),
                    fields: TaskEdit {
                        remove_labels: vec!["missing".into()],
                        ..Default::default()
                    },
                },
            ],
            1,
        ),
        (vec![create("a"), edit(existing(&f.task), "\n")], 1),
        (
            vec![
                create("a"),
                ChangeOp::Update {
                    target: existing(&f.task),
                    fields: TaskEdit {
                        due_on: OptionalDateUpdate::Set("invalid".into()),
                        ..Default::default()
                    },
                },
            ],
            1,
        ),
    ];
    for (ops, index) in cases {
        let error = f
            .store
            .apply_change_set(&f.workspace, ops)
            .await
            .unwrap_err();
        assert_eq!(error.operation_index, Some(index), "{error}");
        assert!(!error.error.message.contains("write reached"), "{error}");
        assert_eq!(f.counts().await, before);
    }
}

#[tokio::test]
async fn workspace_and_deleted_targets_are_rejected_before_writes() {
    let mut f = Fixture::new().await;
    let foreign = f.db.create_workspace("foreign").await.unwrap();
    assert_eq!(
        f.store
            .add_note(&foreign.id, &f.task, "no".into())
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    let project =
        f.db.resolve_or_create_project(&foreign.id, "foreign")
            .await
            .unwrap();
    let error = f
        .store
        .apply_change_set(
            &f.workspace,
            vec![
                create("a"),
                ChangeOp::Update {
                    target: existing(&f.task),
                    fields: TaskEdit {
                        project_id: Some(project.id),
                        ..Default::default()
                    },
                },
            ],
        )
        .await
        .unwrap_err();
    assert_eq!(error.operation_index, Some(1));
    assert_eq!(error.error.code, ErrorCode::NotFound);
    f.exec("UPDATE tasks SET deleted = 1").await;
    for op in [note(existing(&f.task), "no"), edit(existing(&f.task), "no")] {
        let error = f
            .store
            .apply_change_set(&f.workspace, vec![create("a"), op])
            .await
            .unwrap_err();
        assert_eq!(error.operation_index, Some(1));
        assert_eq!(error.error.code, ErrorCode::NotFound);
    }
    assert_eq!(f.count("tasks").await, 1);
}

#[tokio::test]
async fn late_storage_failure_rolls_back_tasks_notes_versions_and_sync_history() {
    let mut f = Fixture::new().await;
    let before = f.counts().await;
    let original = f.snapshot(&f.task).await;
    f.exec("CREATE TRIGGER reject_note BEFORE INSERT ON notes BEGIN SELECT RAISE(ABORT, 'injected note failure'); END;").await;
    let error = f
        .store
        .apply_change_set(
            &f.workspace,
            vec![
                create_with_labels("a", &["Created label"]),
                label_edit(existing(&f.task), &["Edited label"], &[], true),
                note(local("a"), "fail"),
            ],
        )
        .await
        .unwrap_err();
    assert_eq!(error.operation_index, Some(2));
    assert_eq!(error.error.code, ErrorCode::Database);
    assert!(error.error.message.contains("injected note failure"));
    assert_eq!(f.counts().await, before);
    assert_eq!(f.snapshot(&f.task).await, original);
}

#[tokio::test]
async fn inverse_storage_failure_rolls_back_prior_inverses_and_receipt_remains_usable() {
    let mut f = Fixture::new().await;
    let original = f.snapshot(&f.task).await;
    let result = f
        .store
        .apply_change_set(
            &f.workspace,
            vec![
                create("a"),
                edit(existing(&f.task), "changed"),
                note(existing(&f.task), "note"),
            ],
        )
        .await
        .unwrap();
    let applied = f.snapshot(&f.task).await;
    let counts = f.counts().await;
    f.exec("CREATE TRIGGER reject_delete BEFORE UPDATE OF deleted ON tasks BEGIN SELECT RAISE(ABORT, 'injected undo failure'); END;").await;
    let error = f
        .store
        .undo_change_set(&result.undo_token)
        .await
        .unwrap_err();
    assert!(error.message.contains("injected undo failure"));
    assert_eq!(f.snapshot(&f.task).await, applied);
    assert_eq!(f.counts().await, counts);
    let task = f.task.clone();
    assert_eq!(f.note_bodies(&task).await, ["note"]);
    f.exec("DROP TRIGGER reject_delete").await;
    f.store.undo_change_set(&result.undo_token).await.unwrap();
    assert_eq!(f.snapshot(&f.task).await, original);
}

#[tokio::test]
async fn edited_note_and_stale_task_reject_the_whole_undo() {
    let mut f = Fixture::new().await;
    let result = f
        .store
        .apply_change_set(
            &f.workspace,
            vec![create("a"), note(existing(&f.task), "note")],
        )
        .await
        .unwrap();
    // Note edits preserve creation lineage. Freeze activity to prove the body guard independently.
    f.exec("UPDATE notes SET body = 'edited'").await;
    let counts = f.counts().await;
    assert_eq!(
        f.store
            .undo_change_set(&result.undo_token)
            .await
            .unwrap_err()
            .code,
        ErrorCode::GenerationConflict
    );
    assert_eq!(f.counts().await, counts);
    assert!(!f.snapshot(&result.created["a"].task_id).await.deleted);
    f.exec("UPDATE notes SET body = 'note'").await;
    f.store
        .edit_task(
            &f.workspace,
            &result.created["a"].task_id,
            TaskEdit {
                title: Some("elsewhere".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        f.store
            .undo_change_set(&result.undo_token)
            .await
            .unwrap_err()
            .code,
        ErrorCode::GenerationConflict
    );
    let task = f.task.clone();
    assert_eq!(f.note_bodies(&task).await, ["note"]);
}

#[tokio::test]
async fn recurring_completion_and_later_notes_undo_as_one_aggregate() {
    let mut f = Fixture::new().await;
    let recurring = f.recurring().await;
    let original = f.snapshot(&recurring).await;
    let count = f.count("tasks").await;
    let result = f
        .store
        .apply_change_set(
            &f.workspace,
            vec![
                ChangeOp::Update {
                    target: existing(&recurring),
                    fields: TaskEdit {
                        status: Some(TaskStatus::Done),
                        ..Default::default()
                    },
                },
                note(existing(&recurring), "completed note"),
                edit(existing(&f.task), "changed"),
            ],
        )
        .await
        .unwrap();
    assert_eq!(f.count("tasks").await, count + 1);
    f.store.undo_change_set(&result.undo_token).await.unwrap();
    assert_eq!(f.count("tasks").await, count);
    assert_eq!(f.snapshot(&recurring).await, original);
    assert!(f.note_bodies(&recurring).await.is_empty());
    assert_eq!(f.snapshot(&f.task).await.title, "Original");
}

#[tokio::test]
async fn recurrence_validation_includes_noops_notes_and_sequential_terminal_state() {
    let mut f = Fixture::new().await;
    let recurring = f.recurring().await;
    f.exec("CREATE TRIGGER reject_write BEFORE INSERT ON tasks BEGIN SELECT RAISE(ABORT, 'write reached'); END;").await;
    let result = f
        .store
        .apply_change_set(
            &f.workspace,
            vec![
                create("a"),
                ChangeOp::Update {
                    target: existing(&recurring),
                    fields: TaskEdit {
                        status: Some(TaskStatus::Done),
                        ..Default::default()
                    },
                },
                ChangeOp::Update {
                    target: existing(&recurring),
                    fields: TaskEdit {
                        status: Some(TaskStatus::Todo),
                        ..Default::default()
                    },
                },
            ],
        )
        .await
        .unwrap_err();
    assert_eq!(result.operation_index, Some(2));
    assert_eq!(result.error.code, ErrorCode::Validation);
    assert!(result.error.message.contains("terminal-reopen"));
    for state in ["projected", "archived"] {
        sqlx::query(
            "UPDATE recurrence_occurrences SET slot_on = '2000-01-01', projection_state = ?, archived_at = ?",
        )
        .bind(state)
        .bind(if state == "archived" { "2020-01-01T00:00:00Z" } else { "" })
        .execute(&mut f.conn)
        .await
        .unwrap();
        for op in [
            ChangeOp::Update {
                target: existing(&recurring),
                fields: TaskEdit::default(),
            },
            note(existing(&recurring), "x"),
        ] {
            let result = f
                .store
                .apply_change_set(&f.workspace, vec![create("a"), op])
                .await
                .unwrap_err();
            assert_eq!(result.operation_index, Some(1));
            assert_eq!(result.error.code, ErrorCode::Validation);
            assert!(result.error.message.contains("occurrence-archived"));
        }
    }
}

#[tokio::test]
async fn conflicted_field_is_preflighted_but_unchanged_value_is_allowed() {
    let mut f = Fixture::new().await;
    sqlx::query("INSERT INTO conflicts(workspace_id, entity_type, entity_id, task_id, field, local_value, remote_value, remote_change_id, variant_a, variant_b, created_at) VALUES (?, 'task', ?, ?, 'title', 'Original', 'remote', 'remote-change', 'a', 'b', '2026-01-01T00:00:00Z')")
        .bind(&f.workspace).bind(&f.task).bind(&f.task).execute(&mut f.conn).await.unwrap();
    f.exec("CREATE TRIGGER reject_write BEFORE INSERT ON tasks BEGIN SELECT RAISE(ABORT, 'write reached'); END;").await;
    let result = f
        .store
        .apply_change_set(
            &f.workspace,
            vec![create("a"), edit(existing(&f.task), "different")],
        )
        .await
        .unwrap_err();
    assert_eq!(result.operation_index, Some(1));
    assert_eq!(result.error.code, ErrorCode::OpenConflict);
    let unchanged = f
        .store
        .apply_change_set(&f.workspace, vec![edit(existing(&f.task), "Original")])
        .await
        .unwrap();
    f.store
        .undo_change_set(&unchanged.undo_token)
        .await
        .unwrap();
}

#[tokio::test]
async fn applied_and_undone_sets_converge_through_encrypted_sync_and_fresh_replay() {
    let mut f = Fixture::new().await;
    let server = EncryptedSyncServer::new().await;
    server.sync(&f.db).await.unwrap();
    let other = open_blank_database(&f._dir.path().join("other.sqlite"))
        .await
        .unwrap();
    server.sync(&other).await.unwrap();
    let result = f
        .store
        .apply_change_set(
            &f.workspace,
            vec![
                create("a"),
                note(local("a"), "local"),
                edit(existing(&f.task), "changed"),
                note(existing(&f.task), "remote note"),
            ],
        )
        .await
        .unwrap();
    server.sync(&f.db).await.unwrap();
    server.sync(&other).await.unwrap();
    assert_eq!(
        other
            .task_undo_snapshot(&f.workspace, &f.task)
            .await
            .unwrap()
            .title,
        "changed"
    );
    f.store.undo_change_set(&result.undo_token).await.unwrap();
    server.sync(&f.db).await.unwrap();
    server.sync(&other).await.unwrap();
    let fresh = open_blank_database(&f._dir.path().join("fresh.sqlite"))
        .await
        .unwrap();
    server.sync(&fresh).await.unwrap();
    for db in [&other, &fresh] {
        assert_eq!(
            db.task_undo_snapshot(&f.workspace, &f.task)
                .await
                .unwrap()
                .title,
            "Original"
        );
        assert!(
            db.task_undo_snapshot(&f.workspace, &result.created["a"].task_id)
                .await
                .unwrap()
                .deleted
        );
        let mut conn = SqliteConnection::connect_with(
            &sqlx::sqlite::SqliteConnectOptions::new().filename(db.path()),
        )
        .await
        .unwrap();
        let notes: i64 = sqlx::query_scalar("SELECT count(*) FROM notes")
            .fetch_one(&mut conn)
            .await
            .unwrap();
        assert_eq!(notes, 0);
    }
    let task = f.task.clone();
    assert!(f.note_bodies(&task).await.is_empty());
}

#[tokio::test]
async fn changed_recurrence_successor_prevents_every_inverse() {
    let mut f = Fixture::new().await;
    let recurring = f.recurring().await;
    let result = f
        .store
        .apply_change_set(
            &f.workspace,
            vec![
                ChangeOp::Update {
                    target: existing(&recurring),
                    fields: TaskEdit {
                        status: Some(TaskStatus::Done),
                        ..Default::default()
                    },
                },
                edit(existing(&f.task), "batch title"),
            ],
        )
        .await
        .unwrap();
    let successor: TaskId = sqlx::query_scalar(
        "SELECT task_id FROM recurrence_occurrences WHERE projection_state = 'projected'",
    )
    .fetch_one(&mut f.conn)
    .await
    .unwrap();
    f.store
        .edit_task(
            &f.workspace,
            &successor,
            TaskEdit {
                title: Some("later successor edit".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let before = f.counts().await;
    assert!(f.store.undo_change_set(&result.undo_token).await.is_err());
    assert_eq!(f.snapshot(&f.task).await.title, "batch title");
    assert_eq!(f.snapshot(&recurring).await.status, "done");
    assert_eq!(f.snapshot(&successor).await.title, "later successor edit");
    assert_eq!(f.counts().await, before);
}

impl Fixture {
    async fn stage(&self, width: u32) -> StagedAgentAsset {
        self.store
            .stage_agent_asset(
                &self.workspace,
                AgentAssetInput {
                    bytes: png(width),
                    declared_media_type: Some("image/png".into()),
                    optimization_policy: ImageOptimizationPolicy::Preserve,
                },
            )
            .await
            .unwrap()
    }

    fn object(&self, sha256: &str) -> std::path::PathBuf {
        aven_core::attachments::object_path(
            &aven_core::attachments::default_blob_dir(self.db.path()),
            sha256,
        )
        .unwrap()
    }

    /// (attachment_id, deleted) rows of a task in creation order.
    async fn attachments(&mut self, task: &TaskId) -> Vec<(String, bool)> {
        sqlx::query_as(
            "SELECT attachment_id, deleted != 0 FROM task_attachments
             WHERE task_id = ? ORDER BY created_at, attachment_id",
        )
        .bind(task)
        .fetch_all(&mut self.conn)
        .await
        .unwrap()
    }

    async fn pin_count(&mut self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM local_agent_asset_pins")
            .fetch_one(&mut self.conn)
            .await
            .unwrap()
    }
}

fn attachment_id() -> String {
    aven_core::ids::new_id()
}

#[tokio::test]
async fn staged_asset_is_protected_until_released_or_swept() {
    let mut f = Fixture::new().await;
    let asset = f.stage(3).await;
    assert_eq!(
        (asset.media_type.as_str(), asset.width, asset.height),
        ("image/png", 3, 1)
    );
    assert!(f.object(&asset.sha256).exists());
    let aggressive = aven_core::attachments::LifecyclePolicy {
        grace: std::time::Duration::ZERO,
        ..Default::default()
    };
    let blob_dir = aven_core::attachments::default_blob_dir(f.db.path());
    f.db.prune_attachments(&blob_dir, aggressive, true)
        .await
        .unwrap();
    assert!(f.object(&asset.sha256).exists(), "pin protects from prune");

    assert!(f.store.release_agent_asset(&asset.pin_id).await.unwrap());
    assert!(!f.object(&asset.sha256).exists());
    assert!(!f.store.release_agent_asset(&asset.pin_id).await.unwrap());

    let old = f.stage(4).await;
    let fresh = f.stage(5).await;
    sqlx::query(
        "UPDATE local_agent_asset_pins SET created_at = '2020-01-01T00:00:00Z' WHERE pin_id = ?",
    )
    .bind(&old.pin_id)
    .execute(&mut f.conn)
    .await
    .unwrap();
    let day = std::time::Duration::from_secs(86_400);
    assert_eq!(f.store.sweep_agent_assets(day).await.unwrap(), 1);
    assert!(!f.object(&old.sha256).exists());
    assert!(f.object(&fresh.sha256).exists());
    assert_eq!(f.pin_count().await, 1);

    let invalid = f
        .store
        .stage_agent_asset(
            &f.workspace,
            AgentAssetInput {
                bytes: b"not an image".to_vec(),
                declared_media_type: None,
                optimization_policy: ImageOptimizationPolicy::Optimize,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(invalid.code, ErrorCode::Validation);
}

#[tokio::test]
async fn created_task_attachment_applies_and_undoes() {
    let mut f = Fixture::new().await;
    let asset = f.stage(3).await;
    let id = attachment_id();
    let result = f
        .store
        .apply_change_set(
            &f.workspace,
            vec![create("a"), attach(local("a"), &id, &asset)],
        )
        .await
        .unwrap();
    let task = result.created["a"].task_id.clone();
    assert_eq!(result.attachments.len(), 1);
    let change = &result.attachments[0];
    assert_eq!(change.operation_index, 1);
    assert_eq!(change.task_id, task);
    assert_eq!(change.attachment_id, id);
    assert_eq!(change.kind, AttachmentChangeKind::Added);
    let detail = f.store.task_detail(&f.workspace, &task).await.unwrap();
    assert_eq!(detail.attachments.len(), 1);
    assert_eq!(
        detail.attachments[0].alt_text.as_deref(),
        Some("A screenshot")
    );
    assert_eq!(
        detail.attachments[0].filename.as_deref(),
        Some("screenshot.png")
    );
    assert_eq!(detail.attachments[0].width, Some(3));

    f.store.undo_change_set(&result.undo_token).await.unwrap();
    assert_eq!(f.attachments(&task).await, [(id, true)]);
    assert!(f.snapshot(&task).await.deleted);
    // The session can still attach the same image elsewhere.
    assert_eq!(f.pin_count().await, 1);
    let again = f
        .store
        .apply_change_set(
            &f.workspace,
            vec![attach(existing(&f.task), &attachment_id(), &asset)],
        )
        .await
        .unwrap();
    assert_eq!(again.attachments[0].kind, AttachmentChangeKind::Added);
    // Releasing the pin keeps a blob that tasks reference.
    assert!(f.store.release_agent_asset(&asset.pin_id).await.unwrap());
    assert!(f.object(&asset.sha256).exists());
}

#[tokio::test]
async fn duplicate_on_same_task_is_already_present_and_undo_keeps_original() {
    let mut f = Fixture::new().await;
    let task = f.task.clone();
    let asset = f.stage(3).await;
    let original = attachment_id();
    let first = f
        .store
        .apply_change_set(
            &f.workspace,
            vec![attach(existing(&task), &original, &asset)],
        )
        .await
        .unwrap();
    // A retry with the same ID and a second request in one set both dedupe.
    let duplicate = f
        .store
        .apply_change_set(
            &f.workspace,
            vec![
                attach(existing(&task), &original, &asset),
                attach(existing(&task), &attachment_id(), &asset),
            ],
        )
        .await
        .unwrap();
    for change in &duplicate.attachments {
        assert_eq!(change.kind, AttachmentChangeKind::AlreadyPresent);
        assert_eq!(change.attachment_id, original);
    }
    f.store
        .undo_change_set(&duplicate.undo_token)
        .await
        .unwrap();
    assert_eq!(f.attachments(&task).await, [(original.clone(), false)]);

    // The user already removed the attachment; undo still succeeds.
    let workspace = f.db.resolve_workspace("default").await.unwrap();
    f.db.delete_task_attachment(&workspace, &original)
        .await
        .unwrap();
    f.store.undo_change_set(&first.undo_token).await.unwrap();
    assert_eq!(f.attachments(&task).await, [(original, true)]);
}

#[tokio::test]
async fn recurring_task_attachment_applies_and_archived_occurrence_is_rejected() {
    let mut f = Fixture::new().await;
    let recurring = f.recurring().await;
    let asset = f.stage(3).await;
    let id = attachment_id();
    let result = f
        .store
        .apply_change_set(
            &f.workspace,
            vec![attach(existing(&recurring), &id, &asset)],
        )
        .await
        .unwrap();
    assert_eq!(result.attachments[0].kind, AttachmentChangeKind::Added);
    f.store.undo_change_set(&result.undo_token).await.unwrap();
    assert_eq!(f.attachments(&recurring).await, [(id, true)]);

    f.exec("UPDATE recurrence_occurrences SET slot_on = '2000-01-01', projection_state = 'archived', archived_at = '2020-01-01T00:00:00Z'").await;
    let error = f
        .store
        .apply_change_set(
            &f.workspace,
            vec![
                create("a"),
                attach(existing(&recurring), &attachment_id(), &asset),
            ],
        )
        .await
        .unwrap_err();
    assert_eq!(error.operation_index, Some(1));
    assert_eq!(error.error.code, ErrorCode::Validation);
    assert!(error.error.message.contains("occurrence-archived"));
}

#[tokio::test]
async fn invalid_attachment_ops_are_rejected_at_their_index_before_writes() {
    let mut f = Fixture::new().await;
    let asset = f.stage(3).await;
    let other = f.stage(4).await;
    let used = attachment_id();
    f.store
        .apply_change_set(&f.workspace, vec![attach(existing(&f.task), &used, &asset)])
        .await
        .unwrap();
    let before = f.counts().await;
    let cases = vec![
        // The parent create was excluded from the set.
        (
            attach(local("excluded"), &attachment_id(), &asset),
            ErrorCode::Validation,
        ),
        (
            ChangeOp::AddAttachment {
                target: existing(&f.task),
                attachment_id: attachment_id(),
                pin_id: "missing".into(),
                filename: None,
                alt_text: None,
            },
            ErrorCode::NotFound,
        ),
        (
            ChangeOp::AddAttachment {
                target: existing(&f.task),
                attachment_id: attachment_id(),
                pin_id: asset.pin_id.clone(),
                filename: None,
                alt_text: Some("x".repeat(501)),
            },
            ErrorCode::Validation,
        ),
        (
            attach(existing(&f.task), "short", &asset),
            ErrorCode::Validation,
        ),
        // The ID belongs to a row holding different bytes.
        (
            attach(existing(&f.task), &used, &other),
            ErrorCode::Validation,
        ),
    ];
    for (op, code) in cases {
        let error = f
            .store
            .apply_change_set(&f.workspace, vec![create("a"), op])
            .await
            .unwrap_err();
        assert_eq!(error.operation_index, Some(1), "{error}");
        assert_eq!(error.error.code, code, "{error}");
        assert_eq!(f.counts().await, before);
    }
    // Pins belong to the workspace that staged them.
    let foreign = f.db.create_workspace("foreign").await.unwrap();
    f.db.resolve_or_create_project(&foreign.id, "Core")
        .await
        .unwrap();
    let error = f
        .store
        .apply_change_set(
            &foreign.id,
            vec![create("a"), attach(local("a"), &attachment_id(), &asset)],
        )
        .await
        .unwrap_err();
    assert_eq!(error.operation_index, Some(1), "{error}");
    assert_eq!(error.error.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn failed_apply_leaves_the_pin_intact_for_retry() {
    let mut f = Fixture::new().await;
    let task = f.task.clone();
    let asset = f.stage(3).await;
    let id = attachment_id();
    let ops = vec![
        attach(existing(&task), &id, &asset),
        note(existing(&task), "x"),
    ];
    f.exec("CREATE TRIGGER reject_note BEFORE INSERT ON notes BEGIN SELECT RAISE(ABORT, 'note rejected'); END;").await;
    let error = f
        .store
        .apply_change_set(&f.workspace, ops.clone())
        .await
        .unwrap_err();
    assert_eq!(error.operation_index, Some(1));
    assert!(f.attachments(&task).await.is_empty());
    assert_eq!(f.pin_count().await, 1);
    assert!(f.object(&asset.sha256).exists());

    f.exec("DROP TRIGGER reject_note").await;
    let result = f.store.apply_change_set(&f.workspace, ops).await.unwrap();
    assert_eq!(result.attachments[0].kind, AttachmentChangeKind::Added);
    assert_eq!(f.attachments(&task).await, [(id, false)]);
}

fn create_with_labels(local_id: &str, labels: &[&str]) -> ChangeOp {
    ChangeOp::Create {
        local_id: local_id.into(),
        fields: TaskCapture {
            labels: labels.iter().map(|name| (*name).into()).collect(),
            ..capture("Labelled")
        },
    }
}

fn label_fields(add: &[&str], remove: &[&str], create_missing_labels: bool) -> TaskEdit {
    TaskEdit {
        add_labels: add.iter().map(|name| (*name).into()).collect(),
        remove_labels: remove.iter().map(|name| (*name).into()).collect(),
        create_missing_labels,
        ..Default::default()
    }
}

fn label_edit(target: ChangeTarget, add: &[&str], remove: &[&str], create: bool) -> ChangeOp {
    ChangeOp::Update {
        target,
        fields: label_fields(add, remove, create),
    }
}

#[tokio::test]
async fn consumer_edit_label_creation_is_opt_in_normalized_scoped_and_atomic() {
    let mut f = Fixture::new().await;
    let foreign = f.db.create_workspace("foreign").await.unwrap();
    f.db.create_label(&foreign, "Fresh Idea").await.unwrap();
    let before = f.counts().await;
    let original = f.snapshot(&f.task).await;
    assert!(!TaskEdit::default().create_missing_labels);
    assert!(
        f.store
            .edit_task(
                &f.workspace,
                &f.task,
                TaskEdit {
                    add_labels: vec!["Fresh Idea".into()],
                    ..Default::default()
                }
            )
            .await
            .is_err()
    );
    assert_eq!(f.counts().await, before);
    // A foreign task must not create labels in the caller's workspace.
    assert!(
        f.store
            .edit_task(
                &foreign.id,
                &f.task,
                label_fields(&["Wrong workspace"], &[], true)
            )
            .await
            .is_err()
    );
    for fields in [
        label_fields(&["Fresh Idea", "!!!"], &[], true),
        label_fields(&["Fresh Idea"], &["Unknown removal"], true),
    ] {
        assert!(
            f.store
                .edit_task(&f.workspace, &f.task, fields)
                .await
                .is_err()
        );
        assert_eq!(f.counts().await, before);
        assert_eq!(f.snapshot(&f.task).await, original);
    }
    assert!(
        f.store
            .edit_task(
                &f.workspace,
                &f.task,
                label_fields(&[" Fresh IDEA ", "fresh--idea", "FRESH_IDEA"], &[], true)
            )
            .await
            .unwrap()
    );
    assert_eq!(f.snapshot(&f.task).await.labels, ["fresh-idea"]);
    assert_eq!(
        f.db.list_labels(&f.workspace, None).await.unwrap(),
        ["fresh-idea"]
    );
    assert_eq!(
        f.db.list_labels(&foreign.id, None).await.unwrap(),
        ["fresh-idea"]
    );
    let after = f.counts().await;
    assert!(
        !f.store
            .edit_task(
                &f.workspace,
                &f.task,
                label_fields(&["Fresh Idea"], &[], true)
            )
            .await
            .unwrap()
    );
    assert_eq!(f.counts().await, after);
    assert!(
        f.store
            .edit_task(
                &f.workspace,
                &f.task,
                label_fields(&[], &["FRESH IDEA"], false)
            )
            .await
            .unwrap()
    );
    assert!(f.snapshot(&f.task).await.labels.is_empty());
    assert_eq!(
        f.db.list_labels(&f.workspace, None).await.unwrap(),
        ["fresh-idea"]
    );
}

#[tokio::test]
async fn staged_labels_resolve_in_order_and_undo_retains_shared_definitions() {
    let mut f = Fixture::new().await;
    let original = f.snapshot(&f.task).await;
    let ops = vec![
        create_with_labels("a", &["Fresh Idea", "fresh--idea"]),
        label_edit(local("a"), &["Second Label"], &["FRESH IDEA"], true),
        // Default edits can use labels introduced by either earlier operation.
        label_edit(
            existing(&f.task),
            &["fresh_idea", "SECOND LABEL"],
            &[],
            false,
        ),
        label_edit(local("a"), &["Ephemeral"], &["ephemeral"], true),
    ];
    // Staging and discarding are plain data, not mutations.
    let before = f.counts().await;
    drop(ops.clone());
    assert_eq!(f.counts().await, before);
    assert!(
        f.db.list_labels(&f.workspace, None)
            .await
            .unwrap()
            .is_empty()
    );
    let result = f.store.apply_change_set(&f.workspace, ops).await.unwrap();
    let created = &result.created["a"].task_id;
    assert_eq!(f.snapshot(created).await.labels, ["second-label"]);
    assert_eq!(
        f.snapshot(&f.task).await.labels,
        ["fresh-idea", "second-label"]
    );
    // A separate consumer can reuse the shared label without invalidating undo.
    let reused = f
        .store
        .capture_queue_task(
            &f.workspace,
            TaskCapture {
                labels: vec!["Fresh Idea".into()],
                ..capture("Reuses label")
            },
        )
        .await
        .unwrap();
    f.store.undo_change_set(&result.undo_token).await.unwrap();
    assert_eq!(f.snapshot(&f.task).await, original);
    assert!(f.snapshot(created).await.deleted);
    assert_eq!(f.snapshot(&reused.task_id).await.labels, ["fresh-idea"]);
    assert_eq!(
        f.db.list_labels(&f.workspace, None).await.unwrap(),
        ["ephemeral", "fresh-idea", "second-label"]
    );
    f.store
        .undo_queue_capture(&reused.undo_token)
        .await
        .unwrap();
    assert_eq!(
        f.db.list_labels(&f.workspace, None).await.unwrap(),
        ["ephemeral", "fresh-idea", "second-label"]
    );
}

#[tokio::test]
async fn label_preflight_rejects_forward_references_removals_and_invalid_names_without_writes() {
    let mut f = Fixture::new().await;
    let before = f.counts().await;
    f.exec("CREATE TRIGGER reject_label_write BEFORE INSERT ON labels BEGIN SELECT RAISE(ABORT, 'label write reached'); END;").await;
    for (ops, index) in [
        (
            vec![
                label_edit(existing(&f.task), &["Future"], &[], false),
                create_with_labels("a", &["Future"]),
            ],
            0,
        ),
        (
            vec![
                create_with_labels("a", &["Valid"]),
                label_edit(local("a"), &["Another"], &["Missing"], true),
            ],
            1,
        ),
        (
            vec![
                create_with_labels("a", &["Valid"]),
                label_edit(local("a"), &["!!!"], &[], true),
            ],
            1,
        ),
    ] {
        let error = f
            .store
            .apply_change_set(&f.workspace, ops)
            .await
            .unwrap_err();
        assert_eq!(error.operation_index, Some(index));
        assert!(!error.error.message.contains("write reached"), "{error}");
        assert_eq!(f.counts().await, before);
    }
}

#[tokio::test]
async fn label_creation_rolls_back_on_edit_conflict_and_set_preflight_conflict() {
    let mut f = Fixture::new().await;
    sqlx::query("INSERT INTO conflicts(workspace_id, entity_type, entity_id, task_id, field, local_value, remote_value, remote_change_id, variant_a, variant_b, created_at) VALUES (?, 'task', ?, ?, 'title', 'Original', 'remote', 'remote-change', 'a', 'b', '2026-01-01T00:00:00Z')")
        .bind(&f.workspace).bind(&f.task).bind(&f.task).execute(&mut f.conn).await.unwrap();
    let before = f.counts().await;
    let fields = TaskEdit {
        title: Some("conflicted".into()),
        ..label_fields(&["Must roll back"], &[], true)
    };
    assert_eq!(
        f.store
            .edit_task(&f.workspace, &f.task, fields.clone())
            .await
            .unwrap_err()
            .code,
        ErrorCode::OpenConflict
    );
    assert_eq!(f.counts().await, before);
    let error = f
        .store
        .apply_change_set(
            &f.workspace,
            vec![
                create_with_labels("a", &["Also roll back"]),
                ChangeOp::Update {
                    target: existing(&f.task),
                    fields,
                },
            ],
        )
        .await
        .unwrap_err();
    assert_eq!(error.operation_index, Some(1));
    assert_eq!(error.error.code, ErrorCode::OpenConflict);
    assert_eq!(f.counts().await, before);
}

#[tokio::test]
async fn concurrent_task_label_edit_blocks_entire_change_set_undo() {
    let mut f = Fixture::new().await;
    let result = f
        .store
        .apply_change_set(
            &f.workspace,
            vec![
                create_with_labels("a", &["Shared"]),
                label_edit(existing(&f.task), &["Shared"], &[], false),
            ],
        )
        .await
        .unwrap();
    f.store
        .edit_task(
            &f.workspace,
            &f.task,
            label_fields(&["Concurrent"], &[], true),
        )
        .await
        .unwrap();
    let before = f.counts().await;
    assert_eq!(
        f.store
            .undo_change_set(&result.undo_token)
            .await
            .unwrap_err()
            .code,
        ErrorCode::GenerationConflict
    );
    assert_eq!(f.counts().await, before);
    assert!(!f.snapshot(&result.created["a"].task_id).await.deleted);
    assert_eq!(f.snapshot(&f.task).await.labels, ["concurrent", "shared"]);
}

#[tokio::test]
async fn consumer_created_labels_and_undo_converge_through_encrypted_sync() {
    let f = Fixture::new().await;
    let server = EncryptedSyncServer::new().await;
    server.sync(&f.db).await.unwrap();
    let other = open_blank_database(&f._dir.path().join("labels-other.sqlite"))
        .await
        .unwrap();
    server.sync(&other).await.unwrap();
    f.store
        .edit_task(
            &f.workspace,
            &f.task,
            label_fields(&["Ordinary Edit"], &[], true),
        )
        .await
        .unwrap();
    let result = f
        .store
        .apply_change_set(
            &f.workspace,
            vec![
                create_with_labels("a", &["Created label"]),
                label_edit(existing(&f.task), &["Updated label"], &[], true),
            ],
        )
        .await
        .unwrap();
    server.sync(&f.db).await.unwrap();
    server.sync(&other).await.unwrap();
    assert_eq!(
        other.list_labels(&f.workspace, None).await.unwrap(),
        ["created-label", "ordinary-edit", "updated-label"]
    );
    assert_eq!(
        other
            .task_undo_snapshot(&f.workspace, &f.task)
            .await
            .unwrap()
            .labels,
        ["ordinary-edit", "updated-label"]
    );
    assert_eq!(
        other
            .task_undo_snapshot(&f.workspace, &result.created["a"].task_id)
            .await
            .unwrap()
            .labels,
        ["created-label"]
    );
    f.store.undo_change_set(&result.undo_token).await.unwrap();
    server.sync(&f.db).await.unwrap();
    server.sync(&other).await.unwrap();
    let fresh = open_blank_database(&f._dir.path().join("labels-fresh.sqlite"))
        .await
        .unwrap();
    server.sync(&fresh).await.unwrap();
    for db in [&other, &fresh] {
        assert_eq!(
            db.list_labels(&f.workspace, None).await.unwrap(),
            ["created-label", "ordinary-edit", "updated-label"]
        );
        assert_eq!(
            db.task_undo_snapshot(&f.workspace, &f.task)
                .await
                .unwrap()
                .labels,
            ["ordinary-edit"]
        );
        assert!(
            db.task_undo_snapshot(&f.workspace, &result.created["a"].task_id)
                .await
                .unwrap()
                .deleted
        );
    }
}

#[tokio::test]
async fn staged_creation_reuses_intervening_labels_only_in_target_workspace() {
    let mut f = Fixture::new().await;
    let foreign = f.db.create_workspace("foreign").await.unwrap();
    f.db.create_label(&foreign, "Foreign Only").await.unwrap();
    let before = f.counts().await;
    assert!(
        f.store
            .apply_change_set(
                &f.workspace,
                vec![label_edit(existing(&f.task), &["Foreign Only"], &[], false),]
            )
            .await
            .is_err()
    );
    assert_eq!(f.counts().await, before);
    let ops = vec![
        create_with_labels("a", &["Shared Name", "Foreign Only"]),
        label_edit(
            existing(&f.task),
            &["SHARED_NAME", "foreign-only"],
            &[],
            true,
        ),
    ];
    // Another writer creates a staged name before Apply; reuse its definition.
    let workspace = f.db.resolve_workspace("default").await.unwrap();
    let shared = f.db.create_label(&workspace, "shared--name").await.unwrap();
    let result = f.store.apply_change_set(&f.workspace, ops).await.unwrap();
    let creations: Vec<(String, String)> = sqlx::query_as(
        "SELECT entity_id, change_id FROM changes WHERE json_extract(payload, '$.workspace_id') = ? AND op_type = 'create_label' ORDER BY entity_id",
    ).bind(&f.workspace).fetch_all(&mut f.conn).await.unwrap();
    assert_eq!(creations.len(), 2);
    assert_eq!(creations[0].0, "foreign-only");
    assert_eq!(
        creations[1],
        ("shared-name".into(), shared.change_id.unwrap())
    );
    let additions: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM changes WHERE json_extract(payload, '$.workspace_id') = ? AND entity_id = ? AND op_type = 'label_add'",
    ).bind(&f.workspace).bind(&f.task).fetch_one(&mut f.conn).await.unwrap();
    assert_eq!(additions, 2);
    assert_eq!(
        f.snapshot(&f.task).await.labels,
        ["foreign-only", "shared-name"]
    );
    f.store.undo_change_set(&result.undo_token).await.unwrap();
    assert_eq!(
        f.db.list_labels(&f.workspace, None).await.unwrap(),
        ["foreign-only", "shared-name"]
    );
    assert_eq!(
        f.db.list_labels(&foreign.id, None).await.unwrap(),
        ["foreign-only"]
    );
}
