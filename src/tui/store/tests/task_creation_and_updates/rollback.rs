use super::*;

#[tokio::test]
async fn single_mutation_rolls_back_when_undo_recording_fails() {
    let (_dir, pool, mut store) = test_store_with_pool().await;
    let (task_id, selected) = create_selected_task(&mut store, "Single undo failure").await;
    let workspace_id = store.active_workspace.id.clone();
    let undo_before = pending_undo_count(&pool, &workspace_id).await;
    reject_undo_inserts(&pool).await;

    let error = store
        .update_status(Some(selected), "todo")
        .await
        .unwrap_err();

    assert!(error.to_string().contains("injected undo failure"));
    let persisted: String = sqlx::query_scalar("SELECT status FROM tasks WHERE id = ?")
        .bind(&task_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(persisted, "inbox");
    assert_eq!(store.tasks[selected].task.status, TaskStatus::Inbox);
    assert_eq!(pending_undo_count(&pool, &workspace_id).await, undo_before);
}

#[tokio::test]
async fn batch_mutation_rolls_back_when_undo_recording_fails() {
    let (_dir, pool, mut store) = test_store_with_pool().await;
    let (first_id, _) = create_selected_task(&mut store, "First undo failure").await;
    let (second_id, _) = create_selected_task(&mut store, "Second undo failure").await;
    let workspace_id = store.active_workspace.id.clone();
    let undo_before = pending_undo_count(&pool, &workspace_id).await;
    reject_undo_inserts(&pool).await;

    let error = store
        .set_exact_priority_for_tasks(None, &[first_id.clone(), second_id.clone()], "high")
        .await
        .unwrap_err();

    assert!(error.to_string().contains("injected undo failure"));
    for task_id in [&first_id, &second_id] {
        let persisted: String = sqlx::query_scalar("SELECT priority FROM tasks WHERE id = ?")
            .bind(task_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(persisted, "none");
        let cached = store
            .tasks
            .iter()
            .find(|item| &item.task.id == task_id)
            .unwrap();
        assert_eq!(cached.task.priority, TaskPriority::None);
    }
    assert_eq!(pending_undo_count(&pool, &workspace_id).await, undo_before);
}

#[tokio::test]
async fn deletion_rolls_back_when_undo_recording_fails() {
    let (_dir, pool, mut store) = test_store_with_pool().await;
    let (task_id, selected) = create_selected_task(&mut store, "Delete undo failure").await;
    reject_undo_inserts(&pool).await;

    let error = store
        .update_deleted(Some(selected), true)
        .await
        .unwrap_err();

    assert!(error.to_string().contains("injected undo failure"));
    let persisted: i64 = sqlx::query_scalar("SELECT deleted FROM tasks WHERE id = ?")
        .bind(&task_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(persisted, 0);
}

#[tokio::test]
async fn label_assignment_rolls_back_when_undo_recording_fails() {
    let (_dir, pool, mut store) = test_store_with_pool().await;
    store.create_label("atomic".to_string()).await.unwrap();
    let (task_id, selected) = create_selected_task(&mut store, "Label undo failure").await;
    reject_undo_inserts(&pool).await;

    let error = store
        .update_labels_for_tasks(
            Some(selected),
            std::slice::from_ref(&task_id),
            vec!["atomic".to_string()],
            Vec::new(),
        )
        .await
        .unwrap_err();

    assert!(error.to_string().contains("injected undo failure"));
    let persisted: i64 = sqlx::query_scalar("SELECT count(*) FROM task_labels WHERE task_id = ?")
        .bind(&task_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(persisted, 0);
}

#[tokio::test]
async fn new_label_assignment_rolls_back_when_undo_recording_fails() {
    let (_dir, pool, mut store) = test_store_with_pool().await;
    let (task_id, selected) = create_selected_task(&mut store, "New label undo failure").await;
    reject_undo_inserts(&pool).await;

    let error = store
        .update_labels_for_tasks(
            Some(selected),
            std::slice::from_ref(&task_id),
            vec!["created-during-failure".to_string()],
            Vec::new(),
        )
        .await
        .unwrap_err();

    assert!(error.to_string().contains("injected undo failure"));
    let label_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM labels WHERE name = 'created-during-failure'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(label_count, 0);
    let task_label_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM task_labels WHERE task_id = ?")
            .bind(&task_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(task_label_count, 0);
}

#[tokio::test]
async fn project_assignment_rolls_back_when_undo_recording_fails() {
    let (_dir, pool, mut store) = test_store_with_pool().await;
    store
        .create_project("Atomic Project".to_string())
        .await
        .unwrap();
    let (task_id, selected) = create_selected_task(&mut store, "Project undo failure").await;
    let before: String = sqlx::query_scalar("SELECT project_id FROM tasks WHERE id = ?")
        .bind(&task_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    reject_undo_inserts(&pool).await;

    let error = store
        .update_project_for_tasks(
            Some(selected),
            std::slice::from_ref(&task_id),
            "atomic-project".to_string(),
        )
        .await
        .unwrap_err();

    assert!(error.to_string().contains("injected undo failure"));
    let persisted: String = sqlx::query_scalar("SELECT project_id FROM tasks WHERE id = ?")
        .bind(&task_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(persisted, before);
}

#[tokio::test]
async fn date_mutation_rolls_back_when_undo_recording_fails() {
    let (_dir, pool, mut store) = test_store_with_pool().await;
    let (task_id, selected) = create_selected_task(&mut store, "Date undo failure").await;
    reject_undo_inserts(&pool).await;

    let error = store
        .update_due_for_tasks(
            Some(selected),
            std::slice::from_ref(&task_id),
            "2026-08-01".to_string(),
            false,
        )
        .await
        .unwrap_err();

    assert!(error.to_string().contains("injected undo failure"));
    let persisted: String = sqlx::query_scalar("SELECT due_on FROM tasks WHERE id = ?")
        .bind(&task_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(persisted, "");
}
