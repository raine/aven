use aven_core::db::Database;
use aven_core::operations::{TaskDraft, TaskUpdate};

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
    assert_eq!(export.schema_version, 20260716182623);
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
