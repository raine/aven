use super::*;

#[tokio::test]
async fn project_creation_rolls_back_when_undo_recording_fails() {
    let (_dir, pool, mut store) = test_store_with_pool().await;
    reject_undo_inserts(&pool).await;

    let error = store
        .create_project("Atomic Domain".to_string())
        .await
        .unwrap_err();

    assert!(error.to_string().contains("injected undo failure"));
    let persisted: i64 = sqlx::query_scalar("SELECT count(*) FROM projects WHERE key = ?")
        .bind("atomic-domain")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(persisted, 0);
}

#[tokio::test]
async fn label_creation_rolls_back_when_undo_recording_fails() {
    let (_dir, pool, mut store) = test_store_with_pool().await;
    reject_undo_inserts(&pool).await;

    let error = store
        .create_label("atomic-domain".to_string())
        .await
        .unwrap_err();

    assert!(error.to_string().contains("injected undo failure"));
    let persisted: i64 = sqlx::query_scalar("SELECT count(*) FROM labels WHERE name = ?")
        .bind("atomic-domain")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(persisted, 0);
}

#[tokio::test]
async fn task_creation_rolls_back_new_labels_when_undo_recording_fails() {
    let (_dir, pool, mut store) = test_store_with_pool().await;
    reject_undo_inserts(&pool).await;
    let mut draft = task_draft("New label task failure");
    draft.labels = vec!["created-during-task-failure".to_string()];

    let error = store.create_task(draft, None).await.unwrap_err();

    assert!(error.to_string().contains("injected undo failure"));
    let label_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM labels WHERE name = 'created-during-task-failure'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(label_count, 0);
    let task_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM tasks WHERE title = 'New label task failure'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(task_count, 0);
}

#[tokio::test]
async fn project_rename_rolls_back_when_undo_recording_fails() {
    let (_dir, pool, mut store) = test_store_with_pool().await;
    store
        .create_project("Before Atomic Rename".to_string())
        .await
        .unwrap();
    reject_undo_inserts(&pool).await;

    let error = store
        .rename_project("before-atomic-rename", "After Atomic Rename".to_string())
        .await
        .unwrap_err();

    assert!(error.to_string().contains("injected undo failure"));
    let key: String = sqlx::query_scalar("SELECT key FROM projects WHERE name = ?")
        .bind("Before Atomic Rename")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(key, "before-atomic-rename");
}

#[tokio::test]
async fn delete_project_removes_unused_project() {
    let mut store = test_store().await;
    create_mobile_project(&mut store).await;

    let outcome = store.delete_project("mobile-app").await.unwrap();

    assert_eq!(outcome.message, "deleted project mobile-app");
    assert_project_hidden(&store, "mobile-app");
}

#[tokio::test]
async fn rename_project_updates_view_filters_and_tasks() {
    let mut store = test_store().await;
    store
        .create_project("Agent Offload".to_string())
        .await
        .unwrap();
    store
        .create_task(
            TaskDraft {
                title: "Rename keeps task".to_string(),
                project: Some("agent-offload".to_string()),
                ..task_draft("")
            },
            None,
        )
        .await
        .unwrap();
    store
        .show_scope(TaskScopeTarget::Project("agent-offload".to_string()))
        .await
        .unwrap();

    let outcome = store
        .rename_project("agent-offload", "sideagent".to_string())
        .await
        .unwrap();

    assert_eq!(outcome.message, "renamed project sideagent prefix=SDG");
    assert_eq!(
        store.view_state.scope,
        TaskScope::Project("sideagent".to_string())
    );
    assert!(store.projects.iter().any(|project| {
        project.key == "sideagent" && project.name == "sideagent" && project.prefix == "SDG"
    }));
    assert_eq!(store.tasks.len(), 1);
    assert_eq!(store.tasks[0].task.project_key, "sideagent");
}

#[tokio::test]
async fn undo_project_rename_restores_view_filters_and_tasks() {
    let mut store = test_store().await;
    store
        .create_project("Agent Offload".to_string())
        .await
        .unwrap();
    store
        .create_task(
            TaskDraft {
                title: "Undo rename keeps task".to_string(),
                project: Some("agent-offload".to_string()),
                ..task_draft("")
            },
            None,
        )
        .await
        .unwrap();
    store
        .show_scope(TaskScopeTarget::Project("agent-offload".to_string()))
        .await
        .unwrap();
    store
        .rename_project("agent-offload", "sideagent".to_string())
        .await
        .unwrap();

    store.undo_last(None).await.unwrap();

    assert_eq!(
        store.view_state.scope,
        TaskScope::Project("agent-offload".to_string())
    );
    assert!(store.projects.iter().any(|project| {
        project.key == "agent-offload" && project.name == "Agent Offload" && project.prefix == "AO"
    }));
    assert_eq!(store.tasks.len(), 1);
    assert_eq!(store.tasks[0].task.project_key, "agent-offload");
}

#[tokio::test]
async fn delete_project_hides_project_with_tasks() {
    let mut store = test_store().await;
    create_mobile_project(&mut store).await;
    create_task_in_project(&mut store, "Keep project", "mobile-app").await;

    let outcome = store.delete_project("mobile-app").await.unwrap();

    assert_eq!(outcome.message, "deleted project mobile-app");
    assert_project_hidden(&store, "mobile-app");
}

#[tokio::test]
async fn delete_project_hides_project_with_deleted_tasks() {
    let mut store = test_store().await;
    create_mobile_project(&mut store).await;
    let selected = create_task_in_project(&mut store, "Deleted project task", "mobile-app").await;
    store.update_deleted(Some(selected), true).await.unwrap();

    let outcome = store.delete_project("mobile-app").await.unwrap();

    assert_eq!(outcome.message, "deleted project mobile-app");
    assert!(
        !store
            .projects
            .iter()
            .any(|project| project.key == "mobile-app")
    );
}

#[tokio::test]
async fn project_picker_includes_inference_creation_and_existing_projects() {
    let mut store = test_store().await;
    create_mobile_project(&mut store).await;

    let items = store.project_picker_items(None);
    assert!(items[0].label.starts_with("Infer project"));
    assert!(items[0].selected);
    assert!(items.iter().any(|item| item.value == "mobile-app"));
    assert!(
        items
            .iter()
            .any(|item| item.value == CREATE_PROJECT_PICKER_VALUE_PREFIX)
    );
}

#[tokio::test]
async fn priority_picker_includes_all_priorities() {
    let store = test_store().await;
    let items = store.priority_picker_items("none");
    assert_eq!(items.len(), PRIORITIES.len());
    assert!(items[0].selected);
}

#[tokio::test]
async fn existing_project_picker_items_excludes_infer_project() {
    let mut store = test_store().await;
    create_mobile_project(&mut store).await;

    let items = store.existing_project_picker_items("mobile-app");
    assert!(!items.iter().any(|item| item.label == "Infer project"));
    assert!(items.iter().any(|item| item.value == "mobile-app"));
    assert!(items.iter().any(|item| item.selected));
}

#[tokio::test]
async fn edit_project_picker_items_includes_project_creation() {
    let mut store = test_store().await;
    create_mobile_project(&mut store).await;

    let items = store.edit_project_picker_items("mobile-app");
    assert!(items.iter().any(|item| item.value == "mobile-app"));
    assert!(
        items
            .iter()
            .any(|item| item.value == CREATE_PROJECT_PICKER_VALUE_PREFIX)
    );
}
