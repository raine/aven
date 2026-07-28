use std::path::Path;

use aven_core::api::{
    ConflictChoice, ConflictField, CreateTask, ErrorCode, OptionalDateUpdate, Store, UpdateTask,
};
use aven_core::choices::{TaskPriority, TaskStatus};
use aven_core::db::Database;
use aven_core::ids::TaskId;
use aven_core::sync::wire::{
    MAX_PULL_BATCH, MAX_PUSH_BATCH, SYNC_PROTOCOL_VERSION, SyncRequest, SyncResponse,
};
use aven_core::sync::{ApplySyncPage, ServerSyncPage};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection, SqliteConnection};

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
    let client = Database::open(client_path).await.unwrap();
    let page = client
        .prepare_client_sync_page(
            "https://sync.test".to_string(),
            MAX_PUSH_BATCH,
            MAX_PULL_BATCH,
        )
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

    let updated = first
        .update_task(
            &workspace.id,
            &created.id,
            UpdateTask {
                status: Some(TaskStatus::Active),
                priority: Some(TaskPriority::High),
                available_at: OptionalDateUpdate::Set("2026-07-20T00:00:00Z".to_string()),
                due_on: OptionalDateUpdate::Clear,
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
    assert_eq!(
        first.fetch_task(&workspace.id, &created.id).await.unwrap(),
        updated.task
    );
    assert_eq!(
        first.list_tasks(&workspace.id).await.unwrap(),
        vec![updated.task]
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

    let resolved = second
        .resolve_conflict(
            &workspace.id,
            &created.id,
            ConflictField::Title,
            ConflictChoice::Remote,
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
            ConflictChoice::Remote,
        )
        .await
        .unwrap_err();
    assert_eq!(missing_conflict.code, ErrorCode::NotFound);
}
