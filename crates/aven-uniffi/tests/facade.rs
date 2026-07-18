use aven_uniffi::{
    AvenClient, AvenError, CreateTask, ErrorCode, OptionalDateUpdate, SyncHttpResponse,
    TaskPriority, TaskStatus, UpdateTask,
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
    assert!(session.prepare_request().unwrap().is_none());
    assert!(session.summary().unwrap().complete);
}
