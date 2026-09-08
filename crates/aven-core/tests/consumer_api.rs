use std::path::Path;

use aven_core::api::{
    ConflictField, ConflictResolution, CreateRecurrenceSeries, CreateTask, ErrorCode,
    IosQueueMutation, IosQueueMutationKind, IosTaskCapture, MetadataInput, OptionalDateUpdate,
    OptionalLocalTimeUpdate, QueueBand, QueueDate, QueueDateKind, QueueReason, RecurrenceDuePolicy,
    RecurrenceFrequency, RecurrenceHistoryKind, RecurrenceOutcome, RecurrenceProjectionState,
    RecurrenceRule, RecurrenceScheduleInput, RecurrenceSeriesState, Store,
    UpdateRecurrenceTemplate, UpdateTask,
};
use aven_core::choices::{TaskPriority, TaskStatus};
use aven_core::db::Database;
use aven_core::ids::{TaskId, WorkspaceId};
use aven_core::recurrence::RecurrenceSeriesId;
use aven_core::sync::wire::{
    MAX_PULL_BATCH, MAX_PUSH_BATCH, SYNC_PROTOCOL_VERSION, SyncRequest, SyncResponse,
};
use aven_core::sync::{ApplySyncPage, ServerSyncPage};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection, SqliteConnection};

#[tokio::test]
async fn ios_navigation_reads_project_and_search_rows_by_stable_identity() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ios-navigation.sqlite");
    let store = Store::open(&path).await.unwrap();
    let workspace = store.resolve_workspace("default").await.unwrap();
    let ios = store
        .create_task(
            &workspace.id,
            CreateTask {
                title: "Searchable navigation task".to_string(),
                description: "distinct local search phrase".to_string(),
                project: "iOS".to_string(),
                status: TaskStatus::Active,
                priority: TaskPriority::High,
                metadata: Vec::new(),
                available_at: None,
                due_on: Some("2099-09-03".to_string()),
            },
        )
        .await
        .unwrap();
    store
        .create_task(
            &workspace.id,
            CreateTask {
                title: "Other project task".to_string(),
                description: String::new(),
                project: "Docs".to_string(),
                status: TaskStatus::Todo,
                priority: TaskPriority::None,
                metadata: Vec::new(),
                available_at: None,
                due_on: None,
            },
        )
        .await
        .unwrap();
    let state = store.ios_queue_state().await.unwrap();
    let project = state
        .projects
        .iter()
        .find(|project| project.key == "ios")
        .unwrap();

    let project_rows = store
        .ios_project_tasks(&workspace.id, &project.id)
        .await
        .unwrap();
    assert_eq!(project_rows.len(), 1);
    assert_eq!(project_rows[0].id, ios.id);
    assert_eq!(project_rows[0].project_key, "ios");
    assert!(!project_rows[0].display_ref.is_empty());

    let results = store
        .ios_search_tasks(&workspace.id, "distinct local search phrase")
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].task.id, ios.id);
    assert_eq!(results[0].task.display_ref, project_rows[0].display_ref);
    assert!(
        store
            .ios_search_tasks(&workspace.id, "   ")
            .await
            .unwrap()
            .is_empty()
    );

    let database = Database::open(&path).await.unwrap();
    let other_workspace = database.create_workspace("Other").await.unwrap();
    let foreign_project = database
        .resolve_or_create_project(&other_workspace.id, "Foreign")
        .await
        .unwrap();
    let cross_workspace = store
        .ios_project_tasks(&workspace.id, &foreign_project.id)
        .await
        .unwrap_err();
    assert_eq!(cross_workspace.code, ErrorCode::NotFound);

    let mut writer = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(false),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE projects SET deleted = 1 WHERE workspace_id = ? AND id = ?")
        .bind(&workspace.id)
        .bind(&project.id)
        .execute(&mut writer)
        .await
        .unwrap();
    let unavailable = store
        .ios_project_tasks(&workspace.id, &project.id)
        .await
        .unwrap_err();
    assert_eq!(unavailable.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn ios_capture_is_local_workspace_scoped_and_reversible() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ios-capture.sqlite");
    let store = Store::open(&path).await.unwrap();
    let workspace = store.resolve_workspace("default").await.unwrap();
    store
        .create_task(
            &workspace.id,
            CreateTask {
                title: "Seed project".to_string(),
                description: String::new(),
                project: "ios".to_string(),
                status: TaskStatus::Done,
                priority: TaskPriority::None,
                metadata: Vec::new(),
                available_at: None,
                due_on: None,
            },
        )
        .await
        .unwrap();
    let database = Database::open(&path).await.unwrap();
    let internal_workspace = database.workspace_for_id(&workspace.id).await.unwrap();
    database
        .create_label(&internal_workspace, "capture")
        .await
        .unwrap();

    let initial = store.ios_queue_state().await.unwrap();
    assert_eq!(
        initial
            .projects
            .iter()
            .map(|value| value.key.as_str())
            .collect::<Vec<_>>(),
        ["ios"]
    );
    assert_eq!(initial.labels, ["capture"]);

    let missing_project = store
        .capture_ios_queue_task(
            &workspace.id,
            IosTaskCapture {
                description: String::new(),
                title: "Requires a selected project".to_string(),
                project: None,
                priority: TaskPriority::None,
                due_on: None,
                labels: Vec::new(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(missing_project.code, ErrorCode::Validation);
    let after_rejection = store.ios_queue_state().await.unwrap();
    assert_eq!(after_rejection.projects, initial.projects);
    assert_eq!(after_rejection.queue.tasks, initial.queue.tasks);

    let due_on = (chrono::Utc::now().date_naive() + chrono::Duration::days(7)).to_string();
    let captured = store
        .capture_ios_queue_task(
            &workspace.id,
            IosTaskCapture {
                description: "    code block\n\n- item\n".to_string(),
                title: "  Captured offline  ".to_string(),
                project: Some("ios".to_string()),
                priority: TaskPriority::High,
                due_on: Some(due_on.clone()),
                labels: vec!["capture".to_string()],
            },
        )
        .await
        .unwrap();
    let row = captured
        .state
        .as_ref()
        .unwrap()
        .queue
        .tasks
        .iter()
        .find(|task| task.id == captured.task_id)
        .unwrap();
    assert_eq!(
        captured.state.as_ref().unwrap().selected_workspace.id,
        workspace.id
    );
    assert_eq!(row.title, "Captured offline");
    let reopened = Store::open(&path).await.unwrap();
    let detail = reopened
        .ios_task_detail(&workspace.id, &captured.task_id)
        .await
        .unwrap();
    assert_eq!(detail.description, "    code block\n\n- item\n");
    assert_eq!(row.status, TaskStatus::Inbox);
    assert_eq!(row.priority, TaskPriority::High);
    assert_eq!(row.due_on.as_deref(), Some(due_on.as_str()));
    assert_eq!(
        row.label.as_ref().map(|label| label.first.as_str()),
        Some("capture")
    );
    assert_eq!(row.band, QueueBand::Triage);

    for input in [
        IosTaskCapture {
            description: String::new(),
            title: "   ".to_string(),
            project: None,
            priority: TaskPriority::None,
            due_on: None,
            labels: Vec::new(),
        },
        IosTaskCapture {
            description: String::new(),
            title: "Stale project".to_string(),
            project: Some("missing".to_string()),
            priority: TaskPriority::None,
            due_on: None,
            labels: Vec::new(),
        },
        IosTaskCapture {
            description: String::new(),
            title: "Stale label".to_string(),
            project: Some("ios".to_string()),
            priority: TaskPriority::None,
            due_on: None,
            labels: vec!["missing".to_string()],
        },
    ] {
        assert!(
            store
                .capture_ios_queue_task(&workspace.id, input)
                .await
                .is_err()
        );
    }

    let undone = store
        .undo_ios_queue_capture(&captured.undo_token)
        .await
        .unwrap();
    assert!(
        undone
            .as_ref()
            .unwrap()
            .queue
            .tasks
            .iter()
            .all(|task| task.id != captured.task_id)
    );
    assert!(
        store
            .undo_ios_queue_capture(&captured.undo_token)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn ios_queue_quick_actions_are_exact_local_and_state_checked() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ios-quick-actions.sqlite");
    let store = Store::open(&path).await.unwrap();
    let workspace = store.resolve_workspace("default").await.unwrap();
    let task = store
        .create_task(
            &workspace.id,
            CreateTask {
                title: "Quick action target".to_string(),
                description: String::new(),
                project: "ios".to_string(),
                status: TaskStatus::Todo,
                priority: TaskPriority::None,
                metadata: Vec::new(),
                available_at: None,
                due_on: None,
            },
        )
        .await
        .unwrap();
    let other = store
        .create_task(
            &workspace.id,
            CreateTask {
                title: "Stable neighbor".to_string(),
                description: String::new(),
                project: "ios".to_string(),
                status: TaskStatus::Todo,
                priority: TaskPriority::Low,
                metadata: Vec::new(),
                available_at: None,
                due_on: None,
            },
        )
        .await
        .unwrap();

    let started = store
        .mutate_ios_queue_task(
            &workspace.id,
            &task.id,
            IosQueueMutation {
                kind: IosQueueMutationKind::Start,
                priority: None,
                local_date: None,
                time_zone: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .ios_task_detail(&workspace.id, &task.id)
            .await
            .unwrap()
            .status,
        TaskStatus::Active
    );
    store
        .undo_ios_queue_mutation(&started.undo_token)
        .await
        .unwrap();

    let snoozed = store
        .mutate_ios_queue_task(
            &workspace.id,
            &task.id,
            IosQueueMutation {
                kind: IosQueueMutationKind::Snooze,
                priority: None,
                local_date: Some("2026-09-02".to_string()),
                time_zone: Some("America/New_York".to_string()),
            },
        )
        .await
        .unwrap();
    let detail = store
        .ios_task_detail(&workspace.id, &task.id)
        .await
        .unwrap();
    assert_eq!(detail.available_at.as_deref(), Some("2026-09-03T04:00:00Z"));
    store
        .undo_ios_queue_mutation(&snoozed.undo_token)
        .await
        .unwrap();

    let completed = store
        .mutate_ios_queue_task(
            &workspace.id,
            &task.id,
            IosQueueMutation {
                kind: IosQueueMutationKind::Done,
                priority: None,
                local_date: None,
                time_zone: None,
            },
        )
        .await
        .unwrap();
    assert!(
        completed
            .state
            .as_ref()
            .unwrap()
            .queue
            .tasks
            .iter()
            .all(|row| row.id != task.id)
    );
    let restored = store
        .undo_ios_queue_mutation(&completed.undo_token)
        .await
        .unwrap();
    assert!(
        restored
            .as_ref()
            .unwrap()
            .queue
            .tasks
            .iter()
            .any(|row| row.id == task.id)
    );

    let priority = store
        .mutate_ios_queue_task(
            &workspace.id,
            &task.id,
            IosQueueMutation {
                kind: IosQueueMutationKind::SetPriority,
                priority: Some(TaskPriority::Urgent),
                local_date: None,
                time_zone: None,
            },
        )
        .await
        .unwrap();
    store
        .update_task(
            &workspace.id,
            &task.id,
            UpdateTask {
                title: Some("Changed after quick action".to_string()),
                ..UpdateTask::default()
            },
        )
        .await
        .unwrap();
    assert!(
        store
            .undo_ios_queue_mutation(&priority.undo_token)
            .await
            .is_err()
    );
    assert_eq!(
        store
            .ios_task_detail(&workspace.id, &other.id)
            .await
            .unwrap()
            .priority,
        TaskPriority::Low
    );
}

async fn task_source(path: &Path, task_id: &TaskId) -> String {
    let mut connection = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(false),
    )
    .await
    .unwrap();
    sqlx::query_scalar("SELECT source FROM tasks WHERE id = ?")
        .bind(task_id)
        .fetch_one(&mut connection)
        .await
        .unwrap()
}

async fn exchange(client_path: &Path, server: &Database) {
    exchange_bounded(client_path, server, MAX_PULL_BATCH).await;
}

async fn exchange_bounded(client_path: &Path, server: &Database, pull_limit: u32) {
    let client = Database::open(client_path).await.unwrap();
    let page = client
        .prepare_client_sync_page("https://sync.test".to_string(), MAX_PUSH_BATCH, pull_limit)
        .await
        .unwrap();
    let server_request = SyncRequest {
        protocol_version: page.request.protocol_version,
        client_id: page.request.client_id.clone(),
        after: page.request.after,
        pull_limit: page.request.pull_limit,
        changes: page.request.changes.clone(),
    };
    let persisted = server
        .persist_server_sync_page(ServerSyncPage {
            request: server_request,
        })
        .await
        .unwrap();
    let cursor = persisted
        .changes
        .last()
        .and_then(|change| change.server_seq)
        .unwrap_or(page.request.after);
    client
        .apply_client_sync_page(ApplySyncPage {
            request: page.request,
            response: SyncResponse {
                protocol_version: SYNC_PROTOCOL_VERSION,
                cursor,
                has_more: persisted.has_more,
                push_acks: persisted.push_acks,
                changes: persisted.changes,
            },
            attempted_at: "2026-07-18T00:00:00Z".to_string(),
            previous_pushed: 0,
            previous_pulled: 0,
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn consumer_api_creation_and_sync_preserve_api_source() {
    let directory = tempfile::tempdir().unwrap();
    let first_path = directory.path().join("source-first.sqlite");
    let second_path = directory.path().join("source-second.sqlite");
    let server = Database::open(&directory.path().join("source-server.sqlite"))
        .await
        .unwrap();
    let first = Store::open(&first_path).await.unwrap();
    let workspace = first.resolve_workspace("default").await.unwrap();
    let created = first
        .create_task(
            &workspace.id,
            CreateTask {
                metadata: Vec::new(),
                title: "consumer source".to_string(),
                description: String::new(),
                project: "Core".to_string(),
                status: TaskStatus::Inbox,
                priority: TaskPriority::None,
                available_at: None,
                due_on: None,
            },
        )
        .await
        .unwrap();
    drop(first);

    assert_eq!(task_source(&first_path, &created.id).await, "api");
    exchange(&first_path, &server).await;
    exchange(&second_path, &server).await;
    assert_eq!(task_source(&second_path, &created.id).await, "api");
}

#[tokio::test]
async fn consumer_api_queue_report_ranks_open_tasks_and_reads_last_success() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("queue-report.sqlite");
    let server = Database::open(&directory.path().join("queue-server.sqlite"))
        .await
        .unwrap();
    let store = Store::open(&path).await.unwrap();
    let workspace = store.resolve_workspace("default").await.unwrap();

    for (title, status, priority) in [
        ("triage", TaskStatus::Inbox, TaskPriority::None),
        ("focus", TaskStatus::Todo, TaskPriority::High),
        ("needs", TaskStatus::Todo, TaskPriority::Urgent),
        ("done", TaskStatus::Done, TaskPriority::None),
        ("canceled", TaskStatus::Canceled, TaskPriority::None),
    ] {
        store
            .create_task(
                &workspace.id,
                CreateTask {
                    metadata: Vec::new(),
                    title: title.to_string(),
                    description: String::new(),
                    project: "Core".to_string(),
                    status,
                    priority,
                    available_at: None,
                    due_on: None,
                },
            )
            .await
            .unwrap();
    }

    let report = store.queue_report(&workspace.id).await.unwrap();
    assert_eq!(
        report.tasks.iter().map(|row| row.band).collect::<Vec<_>>(),
        vec![QueueBand::NeedsAction, QueueBand::Focus, QueueBand::Triage]
    );
    assert_eq!(
        report
            .tasks
            .iter()
            .map(|row| row.title.as_str())
            .collect::<Vec<_>>(),
        vec!["needs", "focus", "triage"]
    );
    assert!(
        report
            .tasks
            .iter()
            .all(|row| { row.status != TaskStatus::Done && row.status != TaskStatus::Canceled })
    );
    assert!(report.tasks.iter().all(|row| !row.display_ref.is_empty()));
    assert_eq!(report.last_success_at, None);

    let missing_id = WorkspaceId::new();
    let error = store.queue_report(&missing_id).await.unwrap_err();
    assert_eq!(error.code, ErrorCode::NotFound);
    assert_eq!(error.message, format!("workspace not found: {missing_id}"));

    drop(store);
    exchange(&path, &server).await;
    let reopened = Store::open(&path).await.unwrap();
    assert_eq!(
        reopened
            .queue_report(&workspace.id)
            .await
            .unwrap()
            .last_success_at
            .as_deref(),
        Some("2026-07-18T00:00:00Z")
    );
}

#[tokio::test]
async fn ios_queue_state_lists_counts_restores_selection_and_falls_back() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ios-queue-state.sqlite");
    let database = Database::open(&path).await.unwrap();
    let personal = database.create_workspace("Personal").await.unwrap();
    let store = Store::open(&path).await.unwrap();
    let default = store.resolve_workspace("default").await.unwrap();

    for (workspace_id, title, status) in [
        (&default.id, "default open", TaskStatus::Todo),
        (&personal.id, "personal open", TaskStatus::Active),
        (&personal.id, "personal done", TaskStatus::Done),
    ] {
        store
            .create_task(
                workspace_id,
                CreateTask {
                    metadata: Vec::new(),
                    title: title.to_string(),
                    description: String::new(),
                    project: "Core".to_string(),
                    status,
                    priority: TaskPriority::None,
                    available_at: None,
                    due_on: None,
                },
            )
            .await
            .unwrap();
    }

    let selected = store
        .select_ios_queue_workspace(&personal.id)
        .await
        .unwrap();
    assert_eq!(selected.selected_workspace.id, personal.id);
    assert_eq!(selected.queue.tasks.len(), 1);
    assert_eq!(
        selected
            .workspaces
            .iter()
            .map(|summary| (summary.workspace.key.as_str(), summary.open_task_count))
            .collect::<Vec<_>>(),
        vec![("default", 1), ("personal", 1)]
    );
    drop(store);

    let renamed = database.rename_workspace("personal", "Home").await.unwrap();
    let reopened = Store::open(&path).await.unwrap();
    let restored = reopened.ios_queue_state().await.unwrap();
    assert_eq!(restored.selected_workspace.id, renamed.id);
    assert_eq!(restored.selected_workspace.key, "home");
    drop(reopened);

    let mut writer = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(false),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE workspaces SET archived = 1 WHERE id = ?")
        .bind(&renamed.id)
        .execute(&mut writer)
        .await
        .unwrap();
    drop(writer);
    let fallback = Store::open(&path)
        .await
        .unwrap()
        .ios_queue_state()
        .await
        .unwrap();
    assert_eq!(fallback.selected_workspace.id, default.id);
}

#[tokio::test]
async fn consumer_api_queue_report_exposes_bounded_row_presentation_and_conflict_total() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("queue-presentation.sqlite");
    let store = Store::open(&path).await.unwrap();
    let workspace = store.resolve_workspace("default").await.unwrap();
    let task = store
        .create_task(
            &workspace.id,
            CreateTask {
                metadata: Vec::new(),
                title: "presented".to_string(),
                description: "must stay outside the summary".to_string(),
                project: "Core".to_string(),
                status: TaskStatus::Todo,
                priority: TaskPriority::Urgent,
                available_at: None,
                due_on: Some("2099-01-02".to_string()),
            },
        )
        .await
        .unwrap();
    let epic = store
        .create_task(
            &workspace.id,
            CreateTask {
                metadata: Vec::new(),
                title: "epic".to_string(),
                description: String::new(),
                project: "Core".to_string(),
                status: TaskStatus::Todo,
                priority: TaskPriority::None,
                available_at: None,
                due_on: None,
            },
        )
        .await
        .unwrap();

    let mut connection = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(false),
    )
    .await
    .unwrap();
    for label in ["alpha", "beta", "gamma"] {
        sqlx::query("INSERT INTO task_labels(workspace_id, task_id, label) VALUES (?, ?, ?)")
            .bind(&workspace.id)
            .bind(&task.id)
            .bind(label)
            .execute(&mut connection)
            .await
            .unwrap();
    }
    for (attachment_id, deleted) in [("0000000000000001", 0), ("0000000000000002", 1)] {
        sqlx::query(
            "INSERT INTO task_attachments(
                 workspace_id, attachment_id, task_id, sha256, byte_size, media_type,
                 width, height, created_at, deleted, deleted_at
             ) VALUES (?, ?, ?, ?, 1, 'image/png', 1, 1,
                       '2026-09-01T00:00:00Z', ?, ?)",
        )
        .bind(&workspace.id)
        .bind(attachment_id)
        .bind(&task.id)
        .bind(if deleted == 0 {
            "a".repeat(64)
        } else {
            "b".repeat(64)
        })
        .bind(deleted)
        .bind((deleted != 0).then_some("2026-09-02T00:00:00Z"))
        .execute(&mut connection)
        .await
        .unwrap();
    }
    for (field, remote_change_id) in [("title", "remote-title"), ("due_on", "remote-due")] {
        sqlx::query(
            "INSERT INTO conflicts(
                 workspace_id, entity_type, entity_id, task_id, field, local_value,
                 remote_value, remote_change_id, variant_a, variant_b, created_at
             ) VALUES (?, 'task', ?, ?, ?, 'local', 'remote', ?, 'a', 'b',
                       '2026-09-01T00:00:00Z')",
        )
        .bind(&workspace.id)
        .bind(&task.id)
        .bind(&task.id)
        .bind(field)
        .bind(remote_change_id)
        .execute(&mut connection)
        .await
        .unwrap();
    }
    sqlx::query("UPDATE tasks SET is_epic = 1 WHERE workspace_id = ? AND id = ?")
        .bind(&workspace.id)
        .bind(&epic.id)
        .execute(&mut connection)
        .await
        .unwrap();
    drop(connection);

    let report = store.queue_report(&workspace.id).await.unwrap();
    assert_eq!(report.unresolved_conflict_count, 2);
    let row = report.tasks.iter().find(|row| row.id == task.id).unwrap();
    assert_eq!(row.reason, Some(QueueReason::Conflict));
    assert_eq!(
        row.date,
        Some(QueueDate::Due {
            on: "2099-01-02".to_string(),
            kind: QueueDateKind::Due,
        })
    );
    assert_eq!(row.label.as_ref().unwrap().first, "alpha");
    assert_eq!(row.label.as_ref().unwrap().remaining_count, 2);
    assert_eq!(row.live_attachment_count, 1);
    assert!(!row.is_epic);
    let epic_row = report.tasks.iter().find(|row| row.id == epic.id).unwrap();
    assert_eq!(epic_row.band, QueueBand::Epics);
    assert!(epic_row.is_epic);
    assert_eq!(epic_row.reason, None);
}

#[tokio::test]
async fn consumer_api_conflict_records_cover_every_typed_task_field() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("typed-conflicts.sqlite");
    let store = Store::open(&path).await.unwrap();
    let workspace = store.resolve_workspace("default").await.unwrap();
    let task = store
        .create_task(
            &workspace.id,
            CreateTask {
                title: "typed".to_string(),
                description: "description".to_string(),
                project: "Core".to_string(),
                status: TaskStatus::Todo,
                priority: TaskPriority::High,
                available_at: None,
                due_on: None,
                metadata: Vec::new(),
            },
        )
        .await
        .unwrap();
    let mut connection = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(false),
    )
    .await
    .unwrap();
    let project_id: String =
        sqlx::query_scalar("SELECT project_id FROM tasks WHERE workspace_id = ? AND id = ?")
            .bind(&workspace.id)
            .bind(&task.id)
            .fetch_one(&mut connection)
            .await
            .unwrap();
    let fields = [
        ("title", "typed", "remote title"),
        ("description", "description", "remote description"),
        ("project", project_id.as_str(), project_id.as_str()),
        ("status", "todo", "active"),
        ("priority", "high", "urgent"),
        ("available_at", "", "2026-09-03T09:00:00Z"),
        ("due_on", "", "2026-09-04"),
        ("deleted", "0", "1"),
        ("is_epic", "0", "1"),
    ];
    for (index, (field, local, remote)) in fields.iter().enumerate() {
        sqlx::query(
            "INSERT INTO conflicts(
                 workspace_id, entity_type, entity_id, task_id, field, local_value,
                 remote_value, remote_change_id, variant_a, variant_b, created_at
             ) VALUES (?, 'task', ?, ?, ?, ?, ?, ?, ?, ?, '2026-09-01T00:00:00Z')",
        )
        .bind(&workspace.id)
        .bind(&task.id)
        .bind(&task.id)
        .bind(field)
        .bind(local)
        .bind(remote)
        .bind(format!("remote-{index}"))
        .bind(format!("a-{index}"))
        .bind(format!("b-{index}"))
        .execute(&mut connection)
        .await
        .unwrap();
    }
    drop(connection);

    let conflicts = store
        .inspect_conflicts(&workspace.id, &task.id)
        .await
        .unwrap();
    assert_eq!(
        conflicts
            .iter()
            .map(|value| value.field)
            .collect::<Vec<_>>(),
        vec![
            ConflictField::AvailableAt,
            ConflictField::Deleted,
            ConflictField::Description,
            ConflictField::DueOn,
            ConflictField::IsEpic,
            ConflictField::Priority,
            ConflictField::Project,
            ConflictField::Status,
            ConflictField::Title,
        ]
    );
    for conflict in conflicts {
        store
            .resolve_conflict(
                &workspace.id,
                &task.id,
                conflict.field,
                conflict.variant_a,
                conflict.variant_b,
                ConflictResolution::Local,
            )
            .await
            .unwrap();
    }
    assert!(
        store
            .list_conflicts(&workspace.id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn consumer_api_workspace_lookup_is_direct_and_preserves_missing_error() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("workspace-lookup.sqlite");
    let store = Store::open(&path).await.unwrap();
    let workspace = store.resolve_workspace("default").await.unwrap();
    let mut connection = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(false),
    )
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workspaces(id, name, key, created_at, updated_at) VALUES (?, ?, ?, ?, ?)",
    )
    .bind("invalid-workspace-id")
    .bind("Unrelated")
    .bind("unrelated")
    .bind("2026-08-08T00:00:00Z")
    .bind("2026-08-08T00:00:00Z")
    .execute(&mut connection)
    .await
    .unwrap();

    assert!(store.list_tasks(&workspace.id).await.unwrap().is_empty());

    let missing_id = WorkspaceId::new();
    let error = store.list_tasks(&missing_id).await.unwrap_err();
    assert_eq!(error.code, ErrorCode::NotFound);
    assert_eq!(error.message, format!("workspace not found: {missing_id}"));
}

#[tokio::test]
async fn consumer_api_completes_local_task_and_conflict_flows() {
    let directory = tempfile::tempdir().unwrap();
    let first_path = directory.path().join("first.sqlite");
    let second_path = directory.path().join("second.sqlite");
    let server = Database::open(&directory.path().join("server.sqlite"))
        .await
        .unwrap();

    let first = Store::open(&first_path).await.unwrap();
    let storage = first.initialize_storage().unwrap();
    assert_eq!(storage.root, first_path.with_extension("sqlite.blobs"));
    assert_eq!(storage.staging, storage.objects);
    assert!(storage.objects.is_dir());
    assert!(storage.trash.is_dir());
    assert!(storage.previews.is_dir());
    let invalid_sync_server = match first
        .start_sync_session("ftp://sync.test".to_string(), None, None)
        .await
    {
        Ok(_) => panic!("unsupported sync server URL was accepted"),
        Err(error) => error,
    };
    assert_eq!(invalid_sync_server.code, ErrorCode::Validation);
    let workspaces = first.list_workspaces().await.unwrap();
    assert_eq!(workspaces.len(), 1);
    let workspace = first.resolve_workspace("default").await.unwrap();
    assert_eq!(workspace, workspaces[0]);

    let created = first
        .create_task(
            &workspace.id,
            CreateTask {
                metadata: vec![MetadataInput {
                    key: "legacy-id".to_string(),
                    value: "42".to_string(),
                }],
                title: "consumer task".to_string(),
                description: "created through the narrow API".to_string(),
                project: "Core".to_string(),
                status: TaskStatus::Inbox,
                priority: TaskPriority::None,
                available_at: None,
                due_on: Some("2026-08-01".to_string()),
            },
        )
        .await
        .unwrap();
    assert_eq!(created.available_at, None);
    assert_eq!(created.due_on.as_deref(), Some("2026-08-01"));
    assert_eq!(created.metadata.len(), 1);
    assert_eq!(created.metadata[0].key, "legacy-id");
    assert_eq!(created.metadata[0].value, "42");
    let field = first
        .list_metadata_fields(&workspace.id)
        .await
        .unwrap()
        .remove(0);
    let renamed = first
        .rename_metadata_field(&workspace.id, "legacy-id", "external-id")
        .await
        .unwrap();
    assert_eq!(renamed.id, field.id);

    let updated = first
        .update_task(
            &workspace.id,
            &created.id,
            UpdateTask {
                status: Some(TaskStatus::Active),
                priority: Some(TaskPriority::High),
                available_at: OptionalDateUpdate::Set("2026-07-20T00:00:00Z".to_string()),
                due_on: OptionalDateUpdate::Clear,
                set_metadata: vec![MetadataInput {
                    key: "external-id".to_string(),
                    value: String::new(),
                }],
                ..UpdateTask::default()
            },
        )
        .await
        .unwrap();
    assert!(updated.changed);
    assert_eq!(updated.task.status, TaskStatus::Active);
    assert_eq!(updated.task.priority, TaskPriority::High);
    assert_eq!(
        updated.task.available_at.as_deref(),
        Some("2026-07-20T00:00:00Z")
    );
    assert_eq!(updated.task.due_on, None);
    assert_eq!(updated.task.metadata.len(), 1);
    assert_eq!(updated.task.metadata[0].key, "external-id");
    assert_eq!(updated.task.metadata[0].value, "");
    assert_eq!(
        first.fetch_task(&workspace.id, &created.id).await.unwrap(),
        updated.task
    );
    let mut expected_summary = updated.task.clone();
    expected_summary.metadata.clear();
    assert_eq!(
        first.list_tasks(&workspace.id).await.unwrap(),
        vec![expected_summary]
    );

    let validation = first
        .update_task(
            &workspace.id,
            &created.id,
            UpdateTask {
                due_on: OptionalDateUpdate::Set("not-a-date".to_string()),
                ..UpdateTask::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(validation.code, ErrorCode::Validation);
    let empty_date = first
        .update_task(
            &workspace.id,
            &created.id,
            UpdateTask {
                available_at: OptionalDateUpdate::Set(String::new()),
                ..UpdateTask::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(empty_date.code, ErrorCode::Validation);

    let missing = TaskId::new();
    let not_found = first.fetch_task(&workspace.id, &missing).await.unwrap_err();
    assert_eq!(not_found.code, ErrorCode::NotFound);
    drop(first);

    exchange(&first_path, &server).await;
    exchange(&second_path, &server).await;

    let first = Store::open(&first_path).await.unwrap();
    first
        .update_task(
            &workspace.id,
            &created.id,
            UpdateTask {
                title: Some("first title".to_string()),
                ..UpdateTask::default()
            },
        )
        .await
        .unwrap();
    drop(first);

    let second = Store::open(&second_path).await.unwrap();
    second
        .update_task(
            &workspace.id,
            &created.id,
            UpdateTask {
                title: Some("second title".to_string()),
                ..UpdateTask::default()
            },
        )
        .await
        .unwrap();
    drop(second);

    exchange(&first_path, &server).await;
    exchange(&second_path, &server).await;

    let second = Store::open(&second_path).await.unwrap();
    let summaries = second.list_conflicts(&workspace.id).await.unwrap();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].task_id, created.id);
    assert_eq!(summaries[0].field, ConflictField::Title);

    let conflicts = second
        .inspect_conflicts(&workspace.id, &created.id)
        .await
        .unwrap();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].local_value, "second title");
    assert_eq!(conflicts[0].remote_value, "first title");
    let conflict_identity = (
        conflicts[0].variant_a.clone(),
        conflicts[0].variant_b.clone(),
    );

    let open_conflict = second
        .update_task(
            &workspace.id,
            &created.id,
            UpdateTask {
                title: Some("third title".to_string()),
                ..UpdateTask::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(open_conflict.code, ErrorCode::OpenConflict);

    let stale = second
        .resolve_conflict(
            &workspace.id,
            &created.id,
            ConflictField::Title,
            "stale-a".to_string(),
            "stale-b".to_string(),
            ConflictResolution::Explicit("chosen title".to_string()),
        )
        .await
        .unwrap_err();
    assert_eq!(stale.code, ErrorCode::GenerationConflict);
    assert_eq!(second.list_conflicts(&workspace.id).await.unwrap().len(), 1);

    let resolved = second
        .resolve_conflict(
            &workspace.id,
            &created.id,
            ConflictField::Title,
            conflict_identity.0.clone(),
            conflict_identity.1.clone(),
            ConflictResolution::Remote,
        )
        .await
        .unwrap();
    assert_eq!(resolved.title, "first title");
    assert!(
        second
            .list_conflicts(&workspace.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        second
            .inspect_conflicts(&workspace.id, &created.id)
            .await
            .unwrap()
            .is_empty()
    );
    let missing_conflict = second
        .resolve_conflict(
            &workspace.id,
            &created.id,
            ConflictField::Title,
            conflict_identity.0,
            conflict_identity.1,
            ConflictResolution::Remote,
        )
        .await
        .unwrap_err();
    assert_eq!(missing_conflict.code, ErrorCode::NotFound);
}

fn daily_series(title: &str) -> CreateRecurrenceSeries {
    CreateRecurrenceSeries {
        title: title.to_string(),
        description: "consumer recurrence".to_string(),
        project: "Core".to_string(),
        priority: TaskPriority::High,
        initial_status: TaskStatus::Todo,
        labels: Vec::new(),
        metadata: vec![MetadataInput {
            key: "legacy-id".to_string(),
            value: "recurring".to_string(),
        }],
        schedule: RecurrenceScheduleInput {
            rule: RecurrenceRule {
                frequency: RecurrenceFrequency::Daily,
                interval: 1,
                weekdays: Vec::new(),
            },
            timezone: "UTC".to_string(),
            start_on: chrono::Utc::now().date_naive().to_string(),
            available_local_time: Some("09:30".to_string()),
            due_policy: RecurrenceDuePolicy::SameDay,
        },
    }
}

fn monthly_series(title: &str) -> CreateRecurrenceSeries {
    let mut series = daily_series(title);
    series.schedule.rule.frequency = RecurrenceFrequency::Monthly;
    series
}

#[test]
fn consumer_api_reuses_pure_recurrence_enums() {
    let frequency: aven_core::recurrence::RecurrenceFrequency = RecurrenceFrequency::Weekly;
    let due_policy: aven_core::recurrence::RecurrenceDuePolicy = RecurrenceDuePolicy::SameDay;
    let state: aven_core::recurrence::RecurrenceSeriesState = RecurrenceSeriesState::Paused;
    let outcome: aven_core::recurrence::RecurrenceOutcome = RecurrenceOutcome::Skipped;
    let projection: aven_core::recurrence::RecurrenceProjectionState =
        RecurrenceProjectionState::Archived;
    let history_kind: aven_core::query::RecurrenceHistoryKind = RecurrenceHistoryKind::Missed;

    assert_eq!(
        frequency,
        aven_core::recurrence::RecurrenceFrequency::Weekly
    );
    assert_eq!(
        due_policy,
        aven_core::recurrence::RecurrenceDuePolicy::SameDay
    );
    assert_eq!(state, aven_core::recurrence::RecurrenceSeriesState::Paused);
    assert_eq!(outcome, aven_core::recurrence::RecurrenceOutcome::Skipped);
    assert_eq!(
        projection,
        aven_core::recurrence::RecurrenceProjectionState::Archived
    );
    assert_eq!(
        history_kind,
        aven_core::query::RecurrenceHistoryKind::Missed
    );
}

#[tokio::test]
async fn consumer_api_round_trips_monthly_and_yearly_recurrence_rules() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path().join("monthly.sqlite"))
        .await
        .unwrap();
    let workspace = store.resolve_workspace("default").await.unwrap();
    let created = store
        .create_recurrence_series(&workspace.id, monthly_series("monthly review"))
        .await
        .unwrap();

    assert_eq!(created.series.rule.frequency, RecurrenceFrequency::Monthly);
    assert_eq!(
        store
            .show_recurrence_series(&workspace.id, &created.series_ref)
            .await
            .unwrap()
            .series
            .rule
            .frequency,
        RecurrenceFrequency::Monthly
    );

    let mut yearly = daily_series("biennial review");
    yearly.schedule.rule.frequency = RecurrenceFrequency::Yearly;
    yearly.schedule.rule.interval = 2;
    let created = store
        .create_recurrence_series(&workspace.id, yearly)
        .await
        .unwrap();
    assert_eq!(created.series.rule.frequency, RecurrenceFrequency::Yearly);
    assert_eq!(created.series.rule.interval, 2);
}

#[tokio::test]
async fn consumer_api_owns_recurrence_lifecycle_reports_and_mutation_routing() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path().join("recurrence.sqlite"))
        .await
        .unwrap();
    let workspace = store.resolve_workspace("default").await.unwrap();
    let missing_series = store
        .pause_recurrence_series(&workspace.id, &RecurrenceSeriesId::new())
        .await
        .unwrap_err();
    assert_eq!(missing_series.code, ErrorCode::NotFound);
    let created = store
        .create_recurrence_series(&workspace.id, daily_series("daily review"))
        .await
        .unwrap();

    assert_eq!(created.series.state, RecurrenceSeriesState::Active);
    assert_eq!(created.task.status, TaskStatus::Todo);
    assert_eq!(created.task.metadata.len(), 1);
    assert_eq!(created.task.metadata[0].key, "legacy-id");
    assert_eq!(created.task.metadata[0].value, "recurring");
    assert_eq!(created.occurrence.task_id.as_ref(), Some(&created.task.id));
    assert!(created.series_ref.starts_with("RCR-"));

    let terminal_status = store
        .update_recurrence_template(
            &workspace.id,
            &created.series.id,
            UpdateRecurrenceTemplate {
                initial_status: Some(TaskStatus::Done),
                ..UpdateRecurrenceTemplate::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(terminal_status.code, ErrorCode::Validation);
    assert_eq!(
        terminal_status.message,
        "error recurrence-initial-status-terminal status=done"
    );

    assert_eq!(
        store
            .resolve_recurrence_ref(&workspace.id, created.task.id.as_str())
            .await
            .unwrap()
            .series_ref,
        created.series_ref
    );

    let listed = store.list_recurrence_series(&workspace.id).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert!(listed[0].current_task_ref.is_some());
    let shown = store
        .show_recurrence_series(&workspace.id, &created.series_ref)
        .await
        .unwrap();
    assert!(shown.labels.is_empty());
    assert_eq!(shown.metadata.len(), 1);
    assert_eq!(shown.metadata[0].key, "legacy-id");
    assert_eq!(shown.metadata[0].value, "recurring");
    assert_eq!(
        shown.current_occurrence.unwrap().task_id,
        Some(created.task.id.clone())
    );

    let edited = store
        .update_recurrence_template(
            &workspace.id,
            &created.series.id,
            UpdateRecurrenceTemplate {
                title: Some("future daily review".to_string()),
                available_local_time: OptionalLocalTimeUpdate::Clear,
                due_policy: Some(RecurrenceDuePolicy::None),
                ..UpdateRecurrenceTemplate::default()
            },
        )
        .await
        .unwrap();
    assert!(edited.changed);
    assert_eq!(
        store
            .fetch_task(&workspace.id, &created.task.id)
            .await
            .unwrap()
            .title,
        "daily review"
    );

    let paused = store
        .pause_recurrence_series(&workspace.id, &created.series.id)
        .await
        .unwrap();
    assert_eq!(paused.series.state, RecurrenceSeriesState::Paused);
    assert!(store.list_tasks(&workspace.id).await.unwrap().is_empty());
    assert!(
        store
            .recurrence_task_report(&workspace.id, false)
            .await
            .unwrap()
            .is_empty()
    );
    let resumed = store
        .resume_recurrence_series(&workspace.id, &created.series.id)
        .await
        .unwrap();
    assert_eq!(resumed.series.state, RecurrenceSeriesState::Active);
    assert_eq!(
        resumed.occurrence.unwrap().task_id,
        Some(created.task.id.clone())
    );

    let first = store
        .update_task(
            &workspace.id,
            &created.task.id,
            UpdateTask {
                status: Some(TaskStatus::Done),
                ..UpdateTask::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(first.task.status, TaskStatus::Done);
    let successor_ref = store.list_recurrence_series(&workspace.id).await.unwrap()[0]
        .current_task_ref
        .clone()
        .unwrap();
    let successor_series = store
        .resolve_recurrence_ref(&workspace.id, &successor_ref)
        .await
        .unwrap();
    assert_eq!(successor_series.series_id, created.series.id);
    let successor_task = store
        .show_recurrence_series(&workspace.id, &created.series_ref)
        .await
        .unwrap()
        .current_occurrence
        .unwrap()
        .task_id
        .unwrap();
    assert_eq!(
        store
            .fetch_task(&workspace.id, &successor_task)
            .await
            .unwrap()
            .title,
        "future daily review"
    );
    store
        .complete_recurrence_occurrence(&workspace.id, &successor_task)
        .await
        .unwrap();

    let grouped = store
        .recurrence_task_report(&workspace.id, false)
        .await
        .unwrap();
    let expanded = store
        .recurrence_task_report(&workspace.id, true)
        .await
        .unwrap();
    assert_eq!(grouped.len(), 2);
    assert_eq!(expanded.len(), 3);
    assert_eq!(
        grouped
            .iter()
            .find_map(|item| item.recurrence_group.as_ref())
            .unwrap()
            .counts
            .completed,
        2
    );

    let history = store
        .recurrence_history(&workspace.id, &created.series_ref, 0, 100)
        .await
        .unwrap();
    assert_eq!(history.series_ref, created.series_ref);
    assert_eq!(
        history
            .items
            .iter()
            .filter(|row| row.kind == RecurrenceHistoryKind::Completed)
            .count(),
        2
    );
    assert!(history.items.iter().any(|row| {
        row.kind == RecurrenceHistoryKind::Paused && row.interval_started_at.is_some()
    }));

    let stopped = store
        .stop_recurrence_series(&workspace.id, &created.series.id, true)
        .await
        .unwrap();
    assert_eq!(stopped.series.state, RecurrenceSeriesState::Stopped);
    assert_eq!(
        stopped.occurrence.unwrap().outcome,
        Some(RecurrenceOutcome::Skipped)
    );
}

#[tokio::test]
async fn consumer_recurrence_changes_survive_sync_round_trips() {
    let directory = tempfile::tempdir().unwrap();
    let first_path = directory.path().join("recurrence-first.sqlite");
    let second_path = directory.path().join("recurrence-second.sqlite");
    let server = Database::open(&directory.path().join("recurrence-server.sqlite"))
        .await
        .unwrap();
    let first = Store::open(&first_path).await.unwrap();
    let workspace = first.resolve_workspace("default").await.unwrap();
    let created = first
        .create_recurrence_series(&workspace.id, monthly_series("synced monthly"))
        .await
        .unwrap();
    first
        .pause_recurrence_series(&workspace.id, &created.series.id)
        .await
        .unwrap();
    drop(first);

    exchange(&first_path, &server).await;
    exchange(&second_path, &server).await;

    let second = Store::open(&second_path).await.unwrap();
    let series = second.list_recurrence_series(&workspace.id).await.unwrap();
    assert_eq!(series.len(), 1);
    assert_eq!(series[0].series.id, created.series.id);
    assert_eq!(
        series[0].series.rule.frequency,
        RecurrenceFrequency::Monthly
    );
    assert_eq!(series[0].series.state, RecurrenceSeriesState::Paused);
    let history = second
        .recurrence_history(&workspace.id, &series[0].series_ref, 0, 100)
        .await
        .unwrap();
    assert!(
        history
            .items
            .iter()
            .any(|row| row.kind == RecurrenceHistoryKind::Paused)
    );
}

#[tokio::test]
async fn consumer_api_exposes_symmetric_related_links_on_detail_records() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path().join("related.sqlite"))
        .await
        .unwrap();
    let workspace = store.resolve_workspace("default").await.unwrap();
    let create = |title: &str| CreateTask {
        metadata: Vec::new(),
        title: title.to_string(),
        description: String::new(),
        project: "Core".to_string(),
        status: TaskStatus::Inbox,
        priority: TaskPriority::None,
        available_at: None,
        due_on: None,
    };
    let first = store
        .create_task(&workspace.id, create("first"))
        .await
        .unwrap();
    let second = store
        .create_task(&workspace.id, create("second"))
        .await
        .unwrap();

    assert!(
        store
            .add_related_task(&workspace.id, &first.id, &second.id)
            .await
            .unwrap()
            .changed
    );
    let detail = store.fetch_task(&workspace.id, &second.id).await.unwrap();
    assert_eq!(detail.related.len(), 1);
    assert_eq!(detail.related[0].task_id, first.id);
    assert_eq!(detail.related[0].title, "first");
}

#[tokio::test]
async fn related_links_converge_across_remove_and_offline_remove_add_race() {
    let directory = tempfile::tempdir().unwrap();
    let first_path = directory.path().join("related-race-first.sqlite");
    let second_path = directory.path().join("related-race-second.sqlite");
    let server = Database::open(&directory.path().join("related-race-server.sqlite"))
        .await
        .unwrap();
    let first = Store::open(&first_path).await.unwrap();
    let workspace = first.resolve_workspace("default").await.unwrap();
    let create = |title: &str| CreateTask {
        metadata: Vec::new(),
        title: title.to_string(),
        description: String::new(),
        project: "Core".to_string(),
        status: TaskStatus::Inbox,
        priority: TaskPriority::None,
        available_at: None,
        due_on: None,
    };
    let task = first
        .create_task(&workspace.id, create("task"))
        .await
        .unwrap();
    let related = first
        .create_task(&workspace.id, create("related"))
        .await
        .unwrap();
    first
        .add_related_task(&workspace.id, &task.id, &related.id)
        .await
        .unwrap();
    exchange(&first_path, &server).await;
    exchange(&second_path, &server).await;
    let second = Store::open(&second_path).await.unwrap();

    first
        .remove_related_task(&workspace.id, &task.id, &related.id)
        .await
        .unwrap();
    second
        .remove_related_task(&workspace.id, &task.id, &related.id)
        .await
        .unwrap();
    second
        .add_related_task(&workspace.id, &task.id, &related.id)
        .await
        .unwrap();
    exchange(&first_path, &server).await;
    exchange(&second_path, &server).await;
    exchange(&first_path, &server).await;

    assert_eq!(
        first
            .fetch_task(&workspace.id, &task.id)
            .await
            .unwrap()
            .related
            .len(),
        1
    );
    assert_eq!(
        second
            .fetch_task(&workspace.id, &task.id)
            .await
            .unwrap()
            .related
            .len(),
        1
    );
}

#[tokio::test]
async fn consumer_base_list_matches_detail_selection_without_detail_tables() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("base-list.sqlite");
    let store = Store::open(&path).await.unwrap();
    let workspace = store.resolve_workspace("default").await.unwrap();
    for title in ["First", "Second", "Epic"] {
        store
            .create_task(
                &workspace.id,
                CreateTask {
                    metadata: Vec::new(),
                    title: title.to_string(),
                    description: "base description".to_string(),
                    project: "Core".to_string(),
                    status: TaskStatus::Todo,
                    priority: TaskPriority::High,
                    available_at: None,
                    due_on: None,
                },
            )
            .await
            .unwrap();
    }
    let mut connection = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(false),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE tasks SET is_epic = 1 WHERE title = 'Epic'")
        .execute(&mut connection)
        .await
        .unwrap();
    let database = Database::open(&path).await.unwrap();
    let expected = database
        .list_task_items(
            &workspace.id,
            aven_core::query::TaskFilters {
                exclude_epics: true,
                ..Default::default()
            },
            aven_core::query::TaskQueryMode::Flat,
            aven_core::query::TaskSort::Created,
            aven_core::query::SortDirection::Asc,
        )
        .await
        .unwrap()
        .into_iter()
        .map(|item| aven_core::api::TaskRecord::from(item.task))
        .collect::<Vec<_>>();
    assert_eq!(expected.len(), 2);
    assert_eq!(store.list_tasks(&workspace.id).await.unwrap(), expected);
    for statement in [
        "DROP TABLE notes",
        "DROP TABLE task_attachments",
        "DROP TABLE task_related_links",
        "DROP TABLE task_metadata",
        "DROP TABLE task_labels",
        "DROP TABLE task_dependencies",
        "DROP TABLE task_epic_links",
    ] {
        sqlx::query(statement)
            .execute(&mut connection)
            .await
            .unwrap();
    }
    assert_eq!(store.list_tasks(&workspace.id).await.unwrap(), expected);
}

#[tokio::test]
async fn ios_task_detail_is_exact_bounded_and_attachment_aware() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ios-task-detail.sqlite");
    let store = Store::open(&path).await.unwrap();
    let workspace = store.resolve_workspace("default").await.unwrap();
    let database = Database::open(&path).await.unwrap();
    let internal_workspace = database.workspace_for_id(&workspace.id).await.unwrap();
    database
        .create_label(&internal_workspace, "security")
        .await
        .unwrap();

    let seed = store
        .create_task(
            &workspace.id,
            CreateTask {
                title: "Seed API project".to_string(),
                description: String::new(),
                project: "api".to_string(),
                status: TaskStatus::Done,
                priority: TaskPriority::None,
                metadata: Vec::new(),
                available_at: None,
                due_on: None,
            },
        )
        .await
        .unwrap();
    let blocker = store
        .create_task(
            &workspace.id,
            CreateTask {
                title: "Blocking task".to_string(),
                description: String::new(),
                project: "api".to_string(),
                status: TaskStatus::Todo,
                priority: TaskPriority::High,
                metadata: Vec::new(),
                available_at: None,
                due_on: None,
            },
        )
        .await
        .unwrap();
    let captured = store
        .capture_ios_queue_task(
            &workspace.id,
            IosTaskCapture {
                description: String::new(),
                title: "Detailed task".to_string(),
                project: Some("api".to_string()),
                priority: TaskPriority::High,
                due_on: Some("2026-09-03".to_string()),
                labels: vec!["security".to_string()],
            },
        )
        .await
        .unwrap();
    store
        .update_task(
            &workspace.id,
            &captured.task_id,
            UpdateTask {
                description: Some(
                    "## Goal\nUse `tasks:read` and the [guide](https://example.com).".to_string(),
                ),
                ..UpdateTask::default()
            },
        )
        .await
        .unwrap();

    let mut connection = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(false),
    )
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO notes(workspace_id, id, task_id, body, created_at, change_id)
         VALUES (?, 'DETAILNOTE000001', ?, 'Saved note body', '2026-09-01T10:00:00Z', 'detail-note-change')",
    )
    .bind(&workspace.id)
    .bind(&captured.task_id)
    .execute(&mut connection)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO task_dependencies(workspace_id, task_id, depends_on_task_id, created_at)
         VALUES (?, ?, ?, '2026-09-01T10:00:00Z')",
    )
    .bind(&workspace.id)
    .bind(&captured.task_id)
    .bind(&blocker.id)
    .execute(&mut connection)
    .await
    .unwrap();
    let present_sha = "1".repeat(64);
    let remote_sha = "2".repeat(64);
    for (attachment_id, sha, filename) in [
        ("ATTACHMENT000001", present_sha.as_str(), "present.png"),
        ("ATTACHMENT000002", remote_sha.as_str(), "remote.png"),
    ] {
        sqlx::query(
            "INSERT INTO task_attachments(
                workspace_id, attachment_id, task_id, sha256, byte_size, media_type,
                filename, alt_text, width, height, created_at, deleted
             ) VALUES (?, ?, ?, ?, 4, 'image/png', ?, 'diagram', 1, 1, '2026-09-01T10:00:00Z', 0)",
        )
        .bind(&workspace.id)
        .bind(attachment_id)
        .bind(&captured.task_id)
        .bind(sha)
        .bind(filename)
        .execute(&mut connection)
        .await
        .unwrap();
    }
    for (sha, available) in [(present_sha.as_str(), 1), (remote_sha.as_str(), 0)] {
        sqlx::query(
            "INSERT INTO blob_inventory(sha256, byte_size, media_type, available, first_seen_at)
             VALUES (?, 4, 'image/png', ?, '2026-09-01T10:00:00Z')",
        )
        .bind(sha)
        .bind(available)
        .execute(&mut connection)
        .await
        .unwrap();
    }
    drop(connection);

    let detail = store
        .ios_task_detail(&workspace.id, &captured.task_id)
        .await
        .unwrap();
    assert_eq!(detail.id, captured.task_id);
    assert_eq!(detail.workspace_id, workspace.id);
    assert_eq!(detail.title, "Detailed task");
    assert!(detail.description.contains("tasks:read"));
    assert_eq!(detail.labels, ["security"]);
    assert_eq!(detail.notes[0].body, "Saved note body");
    assert_eq!(detail.blocked_by.len(), 1);
    assert_eq!(detail.blocked_by[0].task_id, blocker.id);
    assert!(detail.blocked_by[0].unresolved);
    assert_eq!(detail.attachments.len(), 2);
    assert!(detail.attachments[0].has_blob);
    assert_eq!(
        detail.attachments[0].availability,
        aven_core::api::IosAttachmentAvailability::Present
    );
    assert!(!detail.attachments[1].has_blob);
    assert_eq!(
        detail.attachments[1].availability,
        aven_core::api::IosAttachmentAvailability::Unavailable
    );
    assert_ne!(detail.id, seed.id);

    let missing = store
        .ios_task_detail(&workspace.id, &TaskId::new())
        .await
        .unwrap_err();
    assert_eq!(missing.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn ios_capture_undo_rejects_activity_changed_by_inverse_status_mutation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("capture-activity.sqlite");
    let store = Store::open(&path).await.unwrap();
    let workspace = store.resolve_workspace("default").await.unwrap();
    let database = Database::open(&path).await.unwrap();
    database
        .resolve_or_create_project(&workspace.id, "Capture")
        .await
        .unwrap();
    let mut connection =
        SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(&path))
            .await
            .unwrap();
    // Distinct fixture timestamps avoid depending on second-precision wall-clock races.
    sqlx::query("CREATE TRIGGER capture_activity AFTER INSERT ON tasks BEGIN UPDATE tasks SET queue_activity_at = '2000-01-01T00:00:00Z' WHERE id = NEW.id; END")
        .execute(&mut connection).await.unwrap();
    sqlx::query("CREATE TRIGGER mutation_activity AFTER UPDATE OF status ON tasks BEGIN UPDATE tasks SET queue_activity_at = CASE NEW.status WHEN 'done' THEN '2000-01-02T00:00:00Z' ELSE '2000-01-03T00:00:00Z' END WHERE id = NEW.id; END")
        .execute(&mut connection).await.unwrap();
    let input = || IosTaskCapture {
        title: "State-checked capture".to_string(),
        description: String::new(),
        project: Some("capture".to_string()),
        priority: TaskPriority::None,
        due_on: None,
        labels: Vec::new(),
    };
    let captured = store
        .capture_ios_queue_task(&workspace.id, input())
        .await
        .unwrap();
    let completed = store
        .mutate_ios_queue_task(
            &workspace.id,
            &captured.task_id,
            IosQueueMutation {
                kind: IosQueueMutationKind::Done,
                priority: None,
                local_date: None,
                time_zone: None,
            },
        )
        .await
        .unwrap();
    store
        .undo_ios_queue_mutation(&completed.undo_token)
        .await
        .unwrap();
    let row: (String, bool, String) =
        sqlx::query_as("SELECT status, deleted, queue_activity_at FROM tasks WHERE id = ?")
            .bind(&captured.task_id)
            .fetch_one(&mut connection)
            .await
            .unwrap();
    assert_eq!(
        row,
        (
            "inbox".to_string(),
            false,
            "2000-01-03T00:00:00Z".to_string()
        )
    );
    let changes: Vec<(String, String)> =
        sqlx::query_as("SELECT change_id, payload FROM changes ORDER BY change_id")
            .fetch_all(&mut connection)
            .await
            .unwrap();
    let error = store
        .undo_ios_queue_capture(&captured.undo_token)
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::GenerationConflict);
    let after: (String, bool, String) =
        sqlx::query_as("SELECT status, deleted, queue_activity_at FROM tasks WHERE id = ?")
            .bind(&captured.task_id)
            .fetch_one(&mut connection)
            .await
            .unwrap();
    let changes_after: Vec<(String, String)> =
        sqlx::query_as("SELECT change_id, payload FROM changes ORDER BY change_id")
            .fetch_all(&mut connection)
            .await
            .unwrap();
    assert_eq!(row, after);
    assert_eq!(changes, changes_after);
    let fresh = store
        .capture_ios_queue_task(&workspace.id, input())
        .await
        .unwrap();
    store
        .undo_ios_queue_capture(&fresh.undo_token)
        .await
        .unwrap();
    let deleted: bool = sqlx::query_scalar("SELECT deleted FROM tasks WHERE id = ?")
        .bind(&fresh.task_id)
        .fetch_one(&mut connection)
        .await
        .unwrap();
    assert!(deleted);
}

#[tokio::test]
async fn ios_attachment_bytes_are_scoped_bounded_and_leased() {
    use aven_core::api::IosAttachmentRead;
    use aven_core::attachments::{MAX_BLOB_BYTES, default_blob_dir, object_path, sha256_hex};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("images.sqlite");
    let store = Store::open(&path).await.unwrap();
    let workspace = store.resolve_workspace("default").await.unwrap();
    let database = Database::open(&path).await.unwrap();
    database
        .resolve_or_create_project(&workspace.id, "Images")
        .await
        .unwrap();
    let task = store
        .capture_ios_queue_task(
            &workspace.id,
            IosTaskCapture {
                title: "Images".into(),
                description: String::new(),
                project: Some("images".into()),
                priority: TaskPriority::None,
                due_on: None,
                labels: vec![],
            },
        )
        .await
        .unwrap();
    let bytes = b"bounded snapshot".to_vec();
    let sha = sha256_hex(&bytes);
    let object = object_path(&default_blob_dir(&path), &sha).unwrap();
    std::fs::create_dir_all(object.parent().unwrap()).unwrap();
    std::fs::write(&object, &bytes).unwrap();
    let mut conn = SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(&path))
        .await
        .unwrap();
    sqlx::query("INSERT INTO task_attachments(workspace_id, attachment_id, task_id, sha256, byte_size, media_type, width, height, created_at, deleted) VALUES (?, 'ATTACHMENT000001', ?, ?, ?, 'image/png', 1, 1, '2026-09-01T10:00:00Z', 0)")
        .bind(&workspace.id).bind(&task.task_id).bind(&sha).bind(bytes.len() as i64).execute(&mut conn).await.unwrap();
    sqlx::query("INSERT INTO blob_inventory(sha256, byte_size, media_type, available, first_seen_at) VALUES (?, ?, 'image/png', 1, '2026-09-01T10:00:00Z')")
        .bind(&sha).bind(bytes.len() as i64).execute(&mut conn).await.unwrap();
    let read = || store.ios_attachment_bytes(&workspace.id, &task.task_id, "ATTACHMENT000001");
    let snapshot = read().await.unwrap();
    assert_eq!(
        snapshot,
        IosAttachmentRead::Bytes {
            bytes: bytes.clone()
        }
    );
    assert_eq!(
        store
            .ios_attachment_bytes(&workspace.id, &TaskId::new(), "ATTACHMENT000001")
            .await
            .unwrap(),
        IosAttachmentRead::Invalidated
    );
    std::fs::write(&object, vec![0; bytes.len()]).unwrap();
    assert_eq!(read().await.unwrap(), IosAttachmentRead::Corrupt);
    std::fs::File::create(&object)
        .unwrap()
        .set_len(MAX_BLOB_BYTES as u64 + 1)
        .unwrap();
    assert_eq!(read().await.unwrap(), IosAttachmentRead::Corrupt);
    std::fs::remove_file(&object).unwrap();
    assert_eq!(read().await.unwrap(), IosAttachmentRead::Missing);
    assert_eq!(snapshot, IosAttachmentRead::Bytes { bytes });
    sqlx::query("UPDATE blob_inventory SET available = 0")
        .execute(&mut conn)
        .await
        .unwrap();
    assert_eq!(read().await.unwrap(), IosAttachmentRead::Unavailable);
    sqlx::query("UPDATE task_attachments SET deleted = 1, deleted_at = '2026-09-01T10:00:00Z'")
        .execute(&mut conn)
        .await
        .unwrap();
    assert_eq!(read().await.unwrap(), IosAttachmentRead::Invalidated);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM blob_leases")
        .fetch_one(&mut conn)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn ios_sync_facts_confirm_metadata_only_after_catch_up() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("facts.sqlite");
    let store = Store::open(&path).await.unwrap();
    let server = Database::open(&directory.path().join("server.sqlite"))
        .await
        .unwrap();
    let before = store.ios_sync_facts().await.unwrap();
    assert!(!before.metadata_caught_up);
    assert!(before.metadata_confirmed_at.is_none());
    exchange(&path, &server).await;
    let confirmed = store.ios_sync_facts().await.unwrap();
    assert_eq!(confirmed.pending_changes, 0);
    assert!(confirmed.metadata_caught_up);
    assert!(confirmed.metadata_confirmed_at.as_deref().unwrap() > "2026-07-18T00:00:00Z");
    let workspace = store.resolve_workspace("default").await.unwrap();
    store
        .create_task(
            &workspace.id,
            CreateTask {
                metadata: Vec::new(),
                title: "Pending".into(),
                description: String::new(),
                project: "Core".into(),
                status: TaskStatus::Inbox,
                priority: TaskPriority::None,
                available_at: None,
                due_on: None,
            },
        )
        .await
        .unwrap();
    let pending = store.ios_sync_facts().await.unwrap();
    assert!(pending.pending_changes > 0);
    assert_eq!(
        pending.metadata_confirmed_at,
        confirmed.metadata_confirmed_at
    );
    exchange_bounded(&path, &server, 1).await;
    let partial = store.ios_sync_facts().await.unwrap();
    assert_eq!(partial.pending_changes, 0);
    assert!(!partial.metadata_caught_up);
    assert_eq!(
        partial.metadata_confirmed_at,
        confirmed.metadata_confirmed_at
    );
    exchange(&path, &server).await;
    assert_eq!(store.ios_sync_facts().await.unwrap().pending_changes, 0);
}

#[tokio::test]
async fn ios_task_detail_activity_preserves_bounded_order_anchor_and_empty_history() {
    use aven_core::api::IosTaskActivityKind;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("activity.sqlite");
    let store = Store::open(&path).await.unwrap();
    let workspace = store.resolve_workspace("default").await.unwrap();
    let task = store
        .create_task(
            &workspace.id,
            CreateTask {
                title: "Activity".to_string(),
                description: String::new(),
                project: "ios".to_string(),
                status: TaskStatus::Todo,
                priority: TaskPriority::None,
                metadata: Vec::new(),
                available_at: None,
                due_on: None,
            },
        )
        .await
        .unwrap();
    let mut connection =
        SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(&path))
            .await
            .unwrap();
    sqlx::query("UPDATE tasks SET queue_activity_at = (SELECT created_at FROM changes WHERE entity_id = tasks.id AND op_type = 'create_task') WHERE id = ?")
        .bind(&task.id)
        .execute(&mut connection)
        .await
        .unwrap();
    let created = store
        .ios_task_detail(&workspace.id, &task.id)
        .await
        .unwrap();
    assert_eq!(created.activity.len(), 1);
    assert_eq!(created.activity[0].kind, IosTaskActivityKind::Created);
    assert_eq!(created.activity[0].summary, "created task");
    assert!(created.activity[0].anchors_queue_idle);

    store
        .update_task(
            &workspace.id,
            &task.id,
            UpdateTask {
                priority: Some(TaskPriority::High),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    for index in 0..12 {
        store
            .update_task(
                &workspace.id,
                &task.id,
                UpdateTask {
                    title: Some(format!("Title {index}")),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
    }
    sqlx::query("UPDATE changes SET created_at = '2026-01-01T12:00:00Z' WHERE entity_id = ?")
        .bind(&task.id)
        .execute(&mut connection)
        .await
        .unwrap();
    sqlx::query("UPDATE tasks SET queue_activity_at = '2026-01-01T12:00:00Z' WHERE id = ?")
        .bind(&task.id)
        .execute(&mut connection)
        .await
        .unwrap();
    let detail = store
        .ios_task_detail(&workspace.id, &task.id)
        .await
        .unwrap();
    assert_eq!(detail.activity.len(), 9);
    for (index, action) in detail.activity[..8].iter().enumerate() {
        assert_eq!(action.kind, IosTaskActivityKind::Title);
        assert_eq!(
            action.summary,
            format!("renamed task · Title {}", 11 - index)
        );
        assert_eq!(action.created_at, "2026-01-01T12:00:00Z");
        assert!(!action.anchors_queue_idle);
        assert!(!action.change_id.is_empty());
    }
    assert_eq!(detail.activity[8].kind, IosTaskActivityKind::Priority);
    assert!(detail.activity[8].anchors_queue_idle);
    assert_eq!(
        detail.activity,
        store
            .ios_task_detail(&workspace.id, &task.id)
            .await
            .unwrap()
            .activity
    );
    assert_eq!(
        store
            .ios_task_detail(&WorkspaceId::new(), &task.id)
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );

    sqlx::query("DELETE FROM changes WHERE entity_id = ?")
        .bind(&task.id)
        .execute(&mut connection)
        .await
        .unwrap();
    let empty = store
        .ios_task_detail(&workspace.id, &task.id)
        .await
        .unwrap();
    assert!(empty.activity.is_empty());
    assert_eq!(empty.title, "Title 11");
}

#[tokio::test]
async fn ios_detail_status_receipts_use_authoritative_state_and_reject_stale_undo() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("detail-status.sqlite");
    let store = Store::open(&path).await.unwrap();
    let workspace = store.resolve_workspace("default").await.unwrap();
    let task = store
        .create_task(
            &workspace.id,
            CreateTask {
                title: "Detail status".to_string(),
                description: String::new(),
                project: "ios".to_string(),
                status: TaskStatus::Todo,
                priority: TaskPriority::None,
                metadata: Vec::new(),
                available_at: None,
                due_on: None,
            },
        )
        .await
        .unwrap();
    // The detail's Todo baseline predates the accepted Backlog update.
    store
        .update_task(
            &workspace.id,
            &task.id,
            UpdateTask {
                status: Some(TaskStatus::Backlog),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let receipt = store
        .update_ios_detail_status(&workspace.id, &task.id, TaskStatus::Active)
        .await
        .unwrap();
    assert_eq!(receipt.status, TaskStatus::Active);
    store
        .undo_ios_detail_status(receipt.undo_token.as_deref().unwrap())
        .await
        .unwrap();
    assert_eq!(
        store
            .ios_task_detail(&workspace.id, &task.id)
            .await
            .unwrap()
            .status,
        TaskStatus::Backlog
    );

    let receipt = store
        .update_ios_detail_status(&workspace.id, &task.id, TaskStatus::Active)
        .await
        .unwrap();
    store
        .update_task(
            &workspace.id,
            &task.id,
            UpdateTask {
                status: Some(TaskStatus::Done),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .undo_ios_detail_status(receipt.undo_token.as_deref().unwrap())
            .await
            .unwrap_err()
            .code,
        ErrorCode::GenerationConflict
    );
    assert_eq!(
        store
            .ios_task_detail(&workspace.id, &task.id)
            .await
            .unwrap()
            .status,
        TaskStatus::Done
    );
    // Returning to the same status must not revive an older receipt.
    store
        .update_task(
            &workspace.id,
            &task.id,
            UpdateTask {
                status: Some(TaskStatus::Active),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .undo_ios_detail_status(receipt.undo_token.as_deref().unwrap())
            .await
            .unwrap_err()
            .code,
        ErrorCode::GenerationConflict
    );
    let noop = store
        .update_ios_detail_status(&workspace.id, &task.id, TaskStatus::Active)
        .await
        .unwrap();
    assert!(noop.undo_token.is_none());

    store
        .update_ios_detail_status(&workspace.id, &task.id, TaskStatus::Done)
        .await
        .unwrap();
    let reopen = store
        .update_ios_detail_status(&workspace.id, &task.id, TaskStatus::Todo)
        .await
        .unwrap();
    assert_eq!(reopen.status, TaskStatus::Todo);
    store
        .undo_ios_detail_status(reopen.undo_token.as_deref().unwrap())
        .await
        .unwrap();
    assert_eq!(
        store
            .ios_task_detail(&workspace.id, &task.id)
            .await
            .unwrap()
            .status,
        TaskStatus::Done
    );

    let mut connection =
        SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(&path))
            .await
            .unwrap();
    sqlx::query("CREATE TRIGGER reject_detail_write BEFORE INSERT ON changes BEGIN SELECT RAISE(ABORT, 'injected write failure'); END")
        .execute(&mut connection).await.unwrap();
    assert!(
        store
            .update_ios_detail_status(&workspace.id, &task.id, TaskStatus::Todo)
            .await
            .is_err()
    );
    assert_eq!(
        store
            .ios_task_detail(&workspace.id, &task.id)
            .await
            .unwrap()
            .status,
        TaskStatus::Done
    );
    sqlx::query("DROP TRIGGER reject_detail_write")
        .execute(&mut connection)
        .await
        .unwrap();
    let receipt = store
        .update_ios_detail_status(&workspace.id, &task.id, TaskStatus::Todo)
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER reject_detail_write BEFORE INSERT ON changes BEGIN SELECT RAISE(ABORT, 'injected write failure'); END")
        .execute(&mut connection).await.unwrap();
    assert!(
        store
            .undo_ios_detail_status(receipt.undo_token.as_deref().unwrap())
            .await
            .is_err()
    );
    assert_eq!(
        store
            .ios_task_detail(&workspace.id, &task.id)
            .await
            .unwrap()
            .status,
        TaskStatus::Todo
    );
    sqlx::query("DROP TRIGGER reject_detail_write")
        .execute(&mut connection)
        .await
        .unwrap();
    store
        .undo_ios_detail_status(receipt.undo_token.as_deref().unwrap())
        .await
        .unwrap();
}

#[tokio::test]
async fn ios_detail_status_receipts_preserve_recurrence_routing() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path().join("detail-recurrence.sqlite"))
        .await
        .unwrap();
    let workspace = store.resolve_workspace("default").await.unwrap();
    let created = store
        .create_recurrence_series(&workspace.id, daily_series("detail recurrence"))
        .await
        .unwrap();
    let started = store
        .update_ios_detail_status(&workspace.id, &created.task.id, TaskStatus::Active)
        .await
        .unwrap();
    store
        .undo_ios_detail_status(started.undo_token.as_deref().unwrap())
        .await
        .unwrap();
    assert_eq!(
        store
            .ios_task_detail(&workspace.id, &created.task.id)
            .await
            .unwrap()
            .status,
        TaskStatus::Todo
    );
    let receipt = store
        .update_ios_detail_status(&workspace.id, &created.task.id, TaskStatus::Done)
        .await
        .unwrap();
    assert!(
        store
            .recurrence_history(&workspace.id, &created.series_ref, 0, 10)
            .await
            .unwrap()
            .items
            .iter()
            .any(|item| item.task_id.as_ref() == Some(&created.task.id)
                && item.kind == RecurrenceHistoryKind::Completed)
    );
    // A terminal occurrence requires recurrence aggregate Undo, not a scalar reopen.
    assert!(
        store
            .undo_ios_detail_status(receipt.undo_token.as_deref().unwrap())
            .await
            .is_err()
    );
    assert!(
        store
            .update_ios_detail_status(&workspace.id, &created.task.id, TaskStatus::Todo)
            .await
            .is_err()
    );
    assert_eq!(
        store
            .ios_task_detail(&workspace.id, &created.task.id)
            .await
            .unwrap()
            .status,
        TaskStatus::Done
    );
}
