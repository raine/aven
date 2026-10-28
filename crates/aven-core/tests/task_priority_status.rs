use aven_core::choices::{TaskPriority, TaskSource, TaskStatus};
use aven_core::db::Database;
use aven_core::operations::{TaskDraft, TaskUpdate};

fn draft(
    title: &str,
    project: &str,
    status: Option<TaskStatus>,
    priority: TaskPriority,
) -> TaskDraft {
    TaskDraft {
        title: title.to_string(),
        description: String::new(),
        project: Some(project.to_string()),
        status: status.map(|status| status.to_string()),
        priority: priority.to_string(),
        source: TaskSource::Unknown,
        labels: Vec::new(),
        metadata: Vec::new(),
        available_at: None,
        due_on: None,
        is_epic: false,
    }
}

#[tokio::test]
async fn creation_promotes_only_omitted_status_with_actionable_priority() {
    let directory = tempfile::tempdir().unwrap();
    let database = Database::open(&directory.path().join("aven.sqlite"))
        .await
        .unwrap();
    let workspace = database.list_workspaces().await.unwrap().remove(0);
    let project = database
        .create_project(&workspace, "Core", None)
        .await
        .unwrap()
        .project;

    for priority in [
        TaskPriority::Medium,
        TaskPriority::High,
        TaskPriority::Urgent,
    ] {
        let created = database
            .create_task(
                &workspace,
                draft(priority.as_str(), &project.key, None, priority),
            )
            .await
            .unwrap();
        assert_eq!(created.task.status, TaskStatus::Todo);
    }

    for priority in [
        TaskPriority::Medium,
        TaskPriority::High,
        TaskPriority::Urgent,
    ] {
        let created = database
            .create_task(
                &workspace,
                draft(
                    "explicit inbox",
                    &project.key,
                    Some(TaskStatus::Inbox),
                    priority,
                ),
            )
            .await
            .unwrap();
        assert_eq!(created.task.status, TaskStatus::Inbox);
    }

    let low = database
        .create_task(
            &workspace,
            draft("low inbox", &project.key, None, TaskPriority::Low),
        )
        .await
        .unwrap();
    assert_eq!(low.task.status, TaskStatus::Inbox);

    let backlog = database
        .create_task(
            &workspace,
            draft(
                "urgent backlog",
                &project.key,
                Some(TaskStatus::Backlog),
                TaskPriority::Urgent,
            ),
        )
        .await
        .unwrap();
    assert_eq!(backlog.task.status, TaskStatus::Backlog);

    // Persisted and exported changes carry the resolved status, never an optional input.
    let export = database
        .export_data("2026-10-18T00:00:00Z".into())
        .await
        .unwrap();
    for task in &export.tables.tasks {
        let change = export
            .tables
            .changes
            .iter()
            .find(|change| change.entity_id == task.id.as_str() && change.op_type == "create_task")
            .unwrap();
        let payload: serde_json::Value = serde_json::from_str(&change.payload).unwrap();
        assert_eq!(payload["status"], task.status);
    }
}

#[tokio::test]
async fn batch_priority_edit_promotes_only_current_inbox_tasks() {
    let directory = tempfile::tempdir().unwrap();
    let database = Database::open(&directory.path().join("aven.sqlite"))
        .await
        .unwrap();
    let workspace = database.list_workspaces().await.unwrap().remove(0);
    let project = database
        .create_project(&workspace, "Core", None)
        .await
        .unwrap()
        .project;
    let inbox = database
        .create_task(
            &workspace,
            draft(
                "inbox",
                &project.key,
                Some(TaskStatus::Inbox),
                TaskPriority::None,
            ),
        )
        .await
        .unwrap();
    let backlog = database
        .create_task(
            &workspace,
            draft(
                "backlog",
                &project.key,
                Some(TaskStatus::Backlog),
                TaskPriority::None,
            ),
        )
        .await
        .unwrap();

    let low = database
        .update_task(
            &workspace,
            &inbox.task.id,
            TaskUpdate {
                priority: Some(TaskPriority::Low.to_string()),
                ..TaskUpdate::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(low.task.status, TaskStatus::Inbox);
    assert_eq!(low.task.priority, TaskPriority::Low);

    let explicit = database
        .update_task(
            &workspace,
            &inbox.task.id,
            TaskUpdate {
                status: Some("inbox".into()),
                priority: Some("high".into()),
                ..TaskUpdate::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(explicit.task.status, TaskStatus::Inbox);
    assert_eq!(explicit.task.priority, TaskPriority::High);

    let priority_update = TaskUpdate {
        priority: Some(TaskPriority::High.to_string()),
        ..TaskUpdate::default()
    };
    let outcomes = database
        .update_tasks(
            &workspace,
            vec![
                (inbox.task.id, priority_update.clone()),
                (backlog.task.id, priority_update),
            ],
        )
        .await
        .unwrap();
    assert_eq!(outcomes[0].task.status, TaskStatus::Todo);
    assert_eq!(outcomes[0].task.priority, TaskPriority::High);
    assert_eq!(outcomes[1].task.status, TaskStatus::Backlog);
    assert_eq!(outcomes[1].task.priority, TaskPriority::High);
}
