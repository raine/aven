use super::*;

#[tokio::test]
async fn repeat_entry_creation_reports_committed_refresh_failure_as_completion() {
    let (_dir, pool, mut store) = test_store_with_pool().await;
    store.fail_next_refresh();

    let completion = store
        .create_task_completion(task_draft("Committed repeat task"), None)
        .await
        .unwrap();

    assert!(completion.message.starts_with("created task "));
    assert!(completion.refresh_error.is_some());
    assert!(store.tasks.is_empty());
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM tasks WHERE title = 'Committed repeat task'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn committed_mutation_reports_refresh_failure_without_rolling_back() {
    let (_dir, pool, mut store) = test_store_with_pool().await;
    let (task_id, selected) = create_selected_task(&mut store, "Refresh failure").await;
    store.fail_next_refresh();

    let error = store
        .update_status(Some(selected), "todo")
        .await
        .unwrap_err();

    assert!(mutation_committed(&error));
    assert!(error.to_string().contains("injected refresh failure"));
    let persisted: String = sqlx::query_scalar("SELECT status FROM tasks WHERE id = ?")
        .bind(&task_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(persisted, "todo");
    assert_eq!(store.tasks[selected].task.status, TaskStatus::Inbox);
    assert_eq!(store.refresh_health(), RefreshHealth::Failed);
    assert!(store.available_undo().is_none());
    assert!(store.undo_last(None).await.unwrap().is_none());

    let recovered = store.refresh(Some(&task_id)).await.unwrap();
    assert_eq!(store.refresh_health(), RefreshHealth::Healthy);
    assert!(store.available_undo().is_some());
    assert_eq!(recovered, Some(selected));
    assert_eq!(store.tasks[selected].task.status, TaskStatus::Todo);
}

#[tokio::test]
async fn late_refresh_failure_preserves_view_and_cached_state() {
    let mut store = test_store().await;
    let (task_id, _) = create_selected_task(&mut store, "Late refresh failure").await;
    let original_view_state = store.view_state.clone();
    let original_task_ids = store
        .tasks
        .iter()
        .map(|item| item.task.id.clone())
        .collect::<Vec<_>>();
    let original_projects = store
        .projects
        .iter()
        .map(|project| project.key.clone())
        .collect::<Vec<_>>();
    let original_labels = store.labels.clone();
    let original_counts = (
        store.counts.open,
        store.counts.inbox,
        store.counts.todo,
        store.counts.done,
    );
    let original_sidebar = store
        .sidebar_entries
        .iter()
        .map(|entry| (entry.label.clone(), entry.count))
        .collect::<Vec<_>>();
    store.fail_next_refresh_at(RefreshFailureStage::Tasks);

    let error = store.show_view(TaskQuery::Todo).await.unwrap_err();

    assert!(error.to_string().contains("Tasks"));
    assert_eq!(store.refresh_health(), RefreshHealth::Failed);
    assert_eq!(store.view_state, original_view_state);
    assert_eq!(
        store
            .tasks
            .iter()
            .map(|item| item.task.id.clone())
            .collect::<Vec<_>>(),
        original_task_ids
    );
    assert_eq!(store.tasks[0].task.id, task_id);
    assert_eq!(
        store
            .projects
            .iter()
            .map(|project| project.key.clone())
            .collect::<Vec<_>>(),
        original_projects
    );
    assert_eq!(store.labels, original_labels);
    assert_eq!(
        (
            store.counts.open,
            store.counts.inbox,
            store.counts.todo,
            store.counts.done,
        ),
        original_counts
    );
    assert_eq!(
        store
            .sidebar_entries
            .iter()
            .map(|entry| (entry.label.clone(), entry.count))
            .collect::<Vec<_>>(),
        original_sidebar
    );

    store.show_view(TaskQuery::Todo).await.unwrap();
    assert_eq!(store.refresh_health(), RefreshHealth::Healthy);
    assert_eq!(store.view_state.query, TaskQuery::Todo);
    assert!(store.tasks.is_empty());
}

#[tokio::test]
async fn refresh_replacement_preserves_retained_state_without_cloning_projection() {
    let mut store = test_store().await;
    create_selected_task(&mut store, "Hydrated projection").await;
    store.app_config.sync.server_url = Some("https://sync.example.test".to_string());
    store.task_columns = vec![crate::config::TaskColumnConfig {
        name: "Work".to_string(),
        statuses: vec!["todo".to_string(), "active".to_string()],
    }];
    store.columns_preview_visible = false;
    store.db_stats.total_tasks = 42;
    let database_dir = store._test_database_dir.as_ref().unwrap().clone();
    let projection_clone_count = store.projection_clone_count();

    store.refresh(None).await.unwrap();

    assert_eq!(
        store.config().sync.server_url.as_deref(),
        Some("https://sync.example.test")
    );
    assert_eq!(store.task_columns.len(), 1);
    assert_eq!(store.task_columns[0].name, "Work");
    assert!(!store.columns_preview_visible);
    assert_eq!(store.db_stats.total_tasks, 42);
    assert!(std::sync::Arc::ptr_eq(
        store._test_database_dir.as_ref().unwrap(),
        &database_dir
    ));
    assert_eq!(
        projection_clone_count.load(std::sync::atomic::Ordering::Relaxed),
        0
    );
}
