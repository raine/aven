use super::*;

#[tokio::test]
async fn exact_task_load_ignores_active_view_filters() {
    let mut store = test_store().await;
    let (task_id, index) = create_selected_task(&mut store, "Filtered detail").await;
    store.update_status(Some(index), "todo").await.unwrap();
    store.view_state.view = TaskView::Inbox;
    store.refresh(None).await.unwrap();
    assert!(store.tasks.iter().all(|item| item.task.id != task_id));

    let item = store.load_task_item(&task_id).await.unwrap().unwrap();

    assert_eq!(item.task.id, task_id);
    assert_eq!(item.task.status.as_str(), "todo");
}

#[tokio::test]
async fn exact_task_load_returns_none_for_missing_id() {
    let store = test_store().await;
    let missing = crate::test_support::task_id("missing-detail-task");

    assert!(store.load_task_item(&missing).await.unwrap().is_none());
}
