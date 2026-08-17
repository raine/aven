use std::path::Path;

use aven_core::api::{
    ConflictChoice, ConflictField, CreateRecurrenceSeries, CreateTask, ErrorCode, MetadataInput,
    OptionalDateUpdate, OptionalLocalTimeUpdate, RecurrenceDuePolicy, RecurrenceFrequency,
    RecurrenceHistoryKind, RecurrenceOutcome, RecurrenceProjectionState, RecurrenceRule,
    RecurrenceScheduleInput, RecurrenceSeriesState, Store, UpdateRecurrenceTemplate, UpdateTask,
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
