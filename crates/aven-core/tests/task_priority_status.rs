use aven_core::choices::{TaskPriority, TaskSource, TaskStatus};
use aven_core::db::Database;
use aven_core::operations::{TaskDraft, TaskUpdate};

fn draft(title: &str, project: &str, status: TaskStatus, priority: TaskPriority) -> TaskDraft {
    TaskDraft {
        title: title.to_string(),
        description: String::new(),
        project: Some(project.to_string()),
        status: status.to_string(),
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
async fn creation_promotes_only_inbox_tasks_with_actionable_priority() {
    let directory = tempfile::tempdir().unwrap();
    let database = Database::open(&directory.path().join("aven.sqlite"))
        .await
        .unwrap();
    let workspace = database.list_workspaces().await.unwrap().remove(0);
    let project = database
        .create_project(&workspace, "Core")
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
                draft(priority.as_str(), &project.key, TaskStatus::Inbox, priority),
            )
            .await
            .unwrap();
        assert_eq!(created.task.status, TaskStatus::Todo);
    }

    let low = database
        .create_task(
            &workspace,
            draft(
                "low inbox",
                &project.key,
                TaskStatus::Inbox,
                TaskPriority::Low,
            ),
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
                TaskStatus::Backlog,
                TaskPriority::Urgent,
            ),
        )
        .await
        .unwrap();
    assert_eq!(backlog.task.status, TaskStatus::Backlog);
}

#[tokio::test]
async fn batch_priority_edit_promotes_only_current_inbox_tasks() {
    let directory = tempfile::tempdir().unwrap();
    let database = Database::open(&directory.path().join("aven.sqlite"))
        .await
        .unwrap();
    let workspace = database.list_workspaces().await.unwrap().remove(0);
    let project = database
        .create_project(&workspace, "Core")
        .await
        .unwrap()
        .project;
    let inbox = database
        .create_task(
            &workspace,
            draft("inbox", &project.key, TaskStatus::Inbox, TaskPriority::None),
        )
        .await
        .unwrap();
    let backlog = database
        .create_task(
            &workspace,
            draft(
                "backlog",
                &project.key,
                TaskStatus::Backlog,
                TaskPriority::None,
            ),
        )
        .await
        .unwrap();

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
