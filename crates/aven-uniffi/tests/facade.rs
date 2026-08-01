use std::io::Read;
use std::sync::Arc;

use aven_core::db::Database;
use aven_core::sync::ServerSyncPage;
use aven_core::sync::wire::{SYNC_PROTOCOL_VERSION, SyncRequest, SyncResponse};
use aven_uniffi::{
    AvenClient, AvenError, AvenSyncSession, CreateRecurrenceSeries, CreateTask, ErrorCode,
    OptionalDateUpdate, OptionalLocalTimeUpdate, RecurrenceDuePolicy, RecurrenceFrequency,
    RecurrenceHistoryKind, RecurrenceOutcome, RecurrenceProjectionState, RecurrenceRule,
    RecurrenceScheduleInput, RecurrenceSeriesState, SyncHttpResponse, TaskPriority, TaskStatus,
    UpdateRecurrenceTemplate, UpdateTask,
};

fn error_parts(error: AvenError) -> (ErrorCode, String) {
    match error {
        AvenError::Failure { code, message } => (code, message),
    }
}

#[test]
fn local_task_flow_uses_typed_values_and_validates_ids() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("local.sqlite");
    let client = AvenClient::open(database.to_string_lossy().into_owned()).unwrap();
    let storage = client.initialize_storage().unwrap();
    assert_eq!(storage.root, format!("{}.blobs", database.display()));
    assert_eq!(storage.staging, storage.objects);
    assert!(std::path::Path::new(&storage.objects).is_dir());
    assert!(std::path::Path::new(&storage.trash).is_dir());
    assert!(std::path::Path::new(&storage.previews).is_dir());
    let workspace = client.resolve_workspace("default".to_string()).unwrap();

    let task = client
        .create_task(
            workspace.id.clone(),
            CreateTask {
                title: "prove facade".to_string(),
                description: "exercise the narrow consumer surface".to_string(),
                project: "swift-proof".to_string(),
                status: TaskStatus::Todo,
                priority: TaskPriority::High,
                available_at: None,
                due_on: None,
            },
        )
        .unwrap();
    assert_eq!(task.status, TaskStatus::Todo);
    assert_eq!(task.priority, TaskPriority::High);
    assert_eq!(task.available_at, None);
    assert_eq!(task.due_on, None);
    let output = std::process::Command::new("sqlite3")
        .arg(&database)
        .arg(format!("SELECT source FROM tasks WHERE id = '{}'", task.id))
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), "api");

    let updated = client
        .update_task(
            workspace.id.clone(),
            task.id.clone(),
            UpdateTask {
                title: None,
                description: None,
                project: None,
                status: Some(TaskStatus::Active),
                priority: Some(TaskPriority::Urgent),
                available_at: OptionalDateUpdate::Set {
                    value: "2026-07-19T10:00:00Z".to_string(),
                },
                due_on: OptionalDateUpdate::Set {
                    value: "2026-07-20".to_string(),
                },
            },
        )
        .unwrap();
    assert!(updated.changed);
    assert_eq!(updated.task.status, TaskStatus::Active);
    assert_eq!(updated.task.priority, TaskPriority::Urgent);
    assert_eq!(
        updated.task.available_at.as_deref(),
        Some("2026-07-19T10:00:00Z")
    );
    assert_eq!(updated.task.due_on.as_deref(), Some("2026-07-20"));

    let listed = client.list_tasks(workspace.id.clone()).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0], updated.task);
    assert_eq!(
        client.fetch_task(workspace.id.clone(), task.id).unwrap(),
        updated.task
    );
    assert!(
        client
            .list_conflicts(workspace.id.clone())
            .unwrap()
            .is_empty()
    );

    let (code, message) = error_parts(client.list_tasks("bad".to_string()).unwrap_err());
    assert_eq!(code, ErrorCode::Validation);
    assert!(message.contains("invalid workspace id"));

    let (code, message) = error_parts(
        client
            .fetch_task(workspace.id, "0000000000000000".to_string())
            .unwrap_err(),
    );
    assert_eq!(code, ErrorCode::NotFound);
    assert!(!message.is_empty());
}

fn daily_series(title: &str) -> CreateRecurrenceSeries {
    CreateRecurrenceSeries {
        title: title.to_string(),
        description: "Swift-safe recurrence".to_string(),
        project: "swift-proof".to_string(),
        priority: TaskPriority::High,
        initial_status: TaskStatus::Todo,
        labels: Vec::new(),
        schedule: RecurrenceScheduleInput {
            rule: RecurrenceRule {
                frequency: RecurrenceFrequency::Daily,
                interval: 1,
                weekdays: Vec::new(),
            },
            timezone: "UTC".to_string(),
            start_on: aven_core::ids::now()[..10].to_string(),
            available_local_time: Some("08:45".to_string()),
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
fn recurrence_facade_round_trips_monthly_rules() {
    let directory = tempfile::tempdir().unwrap();
    let client = AvenClient::open(
        directory
            .path()
            .join("monthly.sqlite")
            .to_string_lossy()
            .into_owned(),
    )
    .unwrap();
    let workspace = client.resolve_workspace("default".to_string()).unwrap();
    let created = client
        .create_recurrence_series(workspace.id.clone(), monthly_series("facade monthly"))
        .unwrap();

    assert_eq!(created.series.rule.frequency, RecurrenceFrequency::Monthly);
    assert_eq!(
        client
            .show_recurrence_series(workspace.id, created.series_ref)
            .unwrap()
            .series
            .rule
            .frequency,
        RecurrenceFrequency::Monthly
    );
}

#[test]
fn recurrence_facade_exposes_lifecycle_history_reports_and_typed_ingress() {
    let directory = tempfile::tempdir().unwrap();
    let client = AvenClient::open(
        directory
            .path()
            .join("recurrence.sqlite")
            .to_string_lossy()
            .into_owned(),
    )
    .unwrap();
    let workspace = client.resolve_workspace("default".to_string()).unwrap();
    let created = client
        .create_recurrence_series(workspace.id.clone(), daily_series("facade daily"))
        .unwrap();
    assert_eq!(created.series.state, RecurrenceSeriesState::Active);
    assert_eq!(
        created.occurrence.projection_state,
        RecurrenceProjectionState::Projected
    );
    assert_eq!(
        created.occurrence.task_id.as_deref(),
        Some(created.task.id.as_str())
    );

    let resolution = client
        .resolve_recurrence_ref(workspace.id.clone(), created.task.id.clone())
        .unwrap();
    assert_eq!(resolution.series_ref, created.series_ref);
    let shown = client
        .show_recurrence_series(workspace.id.clone(), created.series_ref.clone())
        .unwrap();
    assert_eq!(
        shown.current_occurrence.unwrap().task_id,
        Some(created.task.id.clone())
    );

    let (code, message) = error_parts(
        client
            .update_recurrence_template(
                workspace.id.clone(),
                created.series.id.clone(),
                UpdateRecurrenceTemplate {
                    title: None,
                    description: None,
                    project: None,
                    priority: None,
                    initial_status: Some(TaskStatus::Done),
                    labels: None,
                    available_local_time: OptionalLocalTimeUpdate::Unchanged,
                    due_policy: None,
                },
            )
            .unwrap_err(),
    );
    assert_eq!(code, ErrorCode::Validation);
    assert_eq!(
        message,
        "error recurrence-initial-status-terminal status=done"
    );

    let edited = client
        .update_recurrence_template(
            workspace.id.clone(),
            created.series.id.clone(),
            UpdateRecurrenceTemplate {
                title: Some("facade future".to_string()),
                description: None,
                project: None,
                priority: None,
                initial_status: None,
                labels: None,
                available_local_time: OptionalLocalTimeUpdate::Clear,
                due_policy: Some(RecurrenceDuePolicy::None),
            },
        )
        .unwrap();
    assert!(edited.changed);
    assert_eq!(edited.series.due_policy, RecurrenceDuePolicy::None);

    assert_eq!(
        client
            .pause_recurrence_series(workspace.id.clone(), created.series.id.clone())
            .unwrap()
            .series
            .state,
        RecurrenceSeriesState::Paused
    );
    assert!(
        client
            .recurrence_task_report(workspace.id.clone(), false)
            .unwrap()
            .is_empty()
    );
    client
        .resume_recurrence_series(workspace.id.clone(), created.series.id.clone())
        .unwrap();

    let first = client
        .complete_recurrence_occurrence(workspace.id.clone(), created.task.id.clone())
        .unwrap();
    assert_eq!(first.occurrence.outcome, Some(RecurrenceOutcome::Completed));
    assert_eq!(first.successor.as_ref().unwrap().title, "facade future");
    client
        .skip_recurrence_occurrence(workspace.id.clone(), first.successor.unwrap().id)
        .unwrap();

    let grouped = client
        .recurrence_task_report(workspace.id.clone(), false)
        .unwrap();
    let expanded = client
        .recurrence_task_report(workspace.id.clone(), true)
        .unwrap();
    assert_eq!(grouped.len(), 2);
    assert_eq!(expanded.len(), 3);
    let counts = &grouped
        .iter()
        .find_map(|item| item.recurrence_group.as_ref())
        .unwrap()
        .counts;
    assert_eq!(counts.completed, 1);
    assert_eq!(counts.skipped, 1);

    let history = client
        .recurrence_history(workspace.id.clone(), created.series_ref.clone(), 0, 100)
        .unwrap();
    assert!(
        history
            .items
            .iter()
            .any(|row| row.kind == RecurrenceHistoryKind::Paused)
    );
    assert!(history.items.iter().any(|row| {
        row.kind == RecurrenceHistoryKind::Completed && row.task_id.is_some() && row.openable
    }));

    let (code, message) = error_parts(
        client
            .pause_recurrence_series(workspace.id.clone(), "bad".to_string())
            .unwrap_err(),
    );
    assert_eq!(code, ErrorCode::Validation);
    assert!(message.contains("invalid recurrence series id"));

    let stopped = client
        .stop_recurrence_series(workspace.id, created.series.id, true)
        .unwrap();
    assert_eq!(stopped.series.state, RecurrenceSeriesState::Stopped);
    assert_eq!(
        stopped.occurrence.unwrap().outcome,
        Some(RecurrenceOutcome::Skipped)
    );
}

#[test]
fn sync_session_carries_opaque_context_and_accepts_response_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("sync.sqlite");
    let client = AvenClient::open(database.to_string_lossy().into_owned()).unwrap();
    let session = client
        .start_sync_session("https://sync.test".to_string(), None, None)
        .unwrap();

    let first = session.prepare_request().unwrap().unwrap();
    let retry = session.prepare_request().unwrap().unwrap();
    assert_eq!(first.method, "POST");
    assert_eq!(first.url, "https://sync.test/sync");
    assert_eq!(first.method, retry.method);
    assert_eq!(first.url, retry.url);
    assert_eq!(first.headers, retry.headers);
    assert_eq!(first.body, retry.body);

    let outcome = session
        .accept_response(
            first.context,
            SyncHttpResponse {
                status: 200,
                headers: Vec::new(),
                body: format!(
                    r#"{{"protocol_version":{},"cursor":0,"has_more":false,"push_acks":[],"changes":[]}}"#,
                    aven_core::sync::wire::SYNC_PROTOCOL_VERSION
                )
                .into_bytes(),
            },
        )
        .unwrap();
    assert!(outcome.complete);
    assert_eq!(outcome.page, 1);
    assert_eq!(outcome.blob_uploaded, 0);
    assert_eq!(outcome.blob_uploaded_bytes, 0);
    assert_eq!(outcome.blob_downloaded, 0);
    assert_eq!(outcome.blob_downloaded_bytes, 0);
    assert!(session.prepare_request().unwrap().is_none());
    let summary = session.summary().unwrap();
    assert!(summary.complete);
    assert_eq!(summary.blob_uploaded, 0);
    assert_eq!(summary.blob_uploaded_bytes, 0);
    assert_eq!(summary.blob_downloaded, 0);
    assert_eq!(summary.blob_downloaded_bytes, 0);
    assert_eq!(summary.blob_upload_remaining, 0);
    assert_eq!(summary.blob_upload_remaining_bytes, 0);
    assert_eq!(summary.blob_download_remaining, 0);
    assert_eq!(summary.blob_download_remaining_bytes, 0);
}

fn exchange_facade_session(
    session: &Arc<AvenSyncSession>,
    server: &Database,
    runtime: &tokio::runtime::Runtime,
) -> Vec<String> {
    let mut operations = Vec::new();
    while let Some(prepared) = session.prepare_request().unwrap() {
        assert_eq!(prepared.url, "https://sync.test/sync");
        let body = if prepared
            .headers
            .iter()
            .any(|header| header.name == "content-encoding" && header.value == "gzip")
        {
            let mut decoded = Vec::new();
            flate2::read::GzDecoder::new(prepared.body.as_slice())
                .read_to_end(&mut decoded)
                .unwrap();
            decoded
        } else {
            prepared.body.clone()
        };
        let request: SyncRequest = serde_json::from_slice(&body).unwrap();
        operations.extend(request.changes.iter().map(|change| change.op_type.clone()));
        let persisted = runtime
            .block_on(server.persist_server_sync_page(ServerSyncPage {
                request: request.clone(),
            }))
            .unwrap();
        let cursor = persisted
            .changes
            .last()
            .and_then(|change| change.server_seq)
            .unwrap_or(request.after);
        session
            .accept_response(
                prepared.context,
                SyncHttpResponse {
                    status: 200,
                    headers: Vec::new(),
                    body: serde_json::to_vec(&SyncResponse {
                        protocol_version: SYNC_PROTOCOL_VERSION,
                        cursor,
                        has_more: persisted.has_more,
                        push_acks: persisted.push_acks,
                        changes: persisted.changes,
                    })
                    .unwrap(),
                },
            )
            .unwrap();
    }
    operations
}

#[test]
fn recurrence_operations_round_trip_through_facade_sync_sessions() {
    let directory = tempfile::tempdir().unwrap();
    let first_path = directory.path().join("sync-first.sqlite");
    let second_path = directory.path().join("sync-second.sqlite");
    let server_path = directory.path().join("sync-server.sqlite");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let server = runtime.block_on(Database::open(&server_path)).unwrap();

    let first = AvenClient::open(first_path.to_string_lossy().into_owned()).unwrap();
    let workspace = first.resolve_workspace("default".to_string()).unwrap();
    let created = first
        .create_recurrence_series(workspace.id.clone(), daily_series("facade synced"))
        .unwrap();
    first
        .pause_recurrence_series(workspace.id.clone(), created.series.id.clone())
        .unwrap();
    let first_session = first
        .start_sync_session("https://sync.test".to_string(), None, None)
        .unwrap();
    let operations = exchange_facade_session(&first_session, &server, &runtime);
    assert!(
        operations
            .iter()
            .any(|operation| operation == "create_recurrence_series")
    );
    assert!(
        operations
            .iter()
            .any(|operation| operation == "open_recurrence_pause")
    );

    let second = AvenClient::open(second_path.to_string_lossy().into_owned()).unwrap();
    let second_session = second
        .start_sync_session("https://sync.test".to_string(), None, None)
        .unwrap();
    exchange_facade_session(&second_session, &server, &runtime);
    let series = second.list_recurrence_series(workspace.id).unwrap();
    assert_eq!(series.len(), 1);
    assert_eq!(series[0].series.id, created.series.id);
    assert_eq!(series[0].series.state, RecurrenceSeriesState::Paused);
}
