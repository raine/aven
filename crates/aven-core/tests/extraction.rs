use aven_core::db::Database;
use aven_core::operations::{TaskDraft, TaskUpdate};
use aven_core::sync::SyncSession;

#[tokio::test]
async fn opens_migrates_and_mutates_through_owned_api() {
    let directory = tempfile::tempdir().unwrap();
    let database = Database::open(&directory.path().join("aven.sqlite"))
        .await
        .unwrap();

    let workspaces = database.list_workspaces().await.unwrap();
    assert_eq!(workspaces.len(), 1);
    let workspace = &workspaces[0];
    let project = database
        .create_project(workspace, "Core")
        .await
        .unwrap()
        .project;

    let created = database
        .create_task(
            workspace,
            TaskDraft {
                title: "core task".to_string(),
                description: String::new(),
                project: Some(project.key),
                status: "inbox".to_string(),
                priority: "none".to_string(),
                labels: Vec::new(),
                available_at: None,
                due_on: None,
                is_epic: false,
            },
        )
        .await
        .unwrap();
    let updated = database
        .update_task(
            workspace,
            &created.task.id,
            TaskUpdate {
                title: Some("updated through core".to_string()),
                ..TaskUpdate::default()
            },
        )
        .await
        .unwrap();
    assert!(updated.changed);

    let export = database
        .export_data("2026-07-18T00:00:00Z".to_string())
        .await
        .unwrap();
    assert_eq!(
        export.schema_version,
        Database::latest_schema_version().expect("core has embedded migrations")
    );
    let task = export
        .tables
        .tasks
        .iter()
        .find(|task| task.id == created.task.id)
        .unwrap();
    assert_eq!(task.title, "updated through core");

    let title_change = export
        .tables
        .changes
        .iter()
        .find(|change| {
            change.entity_id == created.task.id.as_str()
                && change.field.as_deref() == Some("title")
                && change.op_type == "set_field"
        })
        .unwrap();
    let title_version = export
        .tables
        .field_versions
        .iter()
        .find(|version| version.entity_id == created.task.id.as_str() && version.field == "title")
        .unwrap();
    assert_eq!(title_version.version, title_change.change_id);
}

#[tokio::test]
async fn invalid_sync_initialization_records_attempt_and_error() {
    let directory = tempfile::tempdir().unwrap();
    let database = Database::open(&directory.path().join("aven.sqlite"))
        .await
        .unwrap();

    let error =
        match SyncSession::start(database.clone(), "not a URL".to_string(), None, None).await {
            Ok(_) => panic!("invalid URL must fail sync initialization"),
            Err(error) => error,
        };

    assert_eq!(error.to_string(), "invalid sync server URL");
    let status = database.sync_persistence_status().await.unwrap();
    assert!(status.last_attempt.is_some());
    assert_eq!(
        status.last_error.as_deref(),
        Some("invalid sync server URL")
    );
}
