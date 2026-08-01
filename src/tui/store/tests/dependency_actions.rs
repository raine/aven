use super::*;

#[tokio::test]
async fn dependency_add_rolls_back_when_undo_recording_fails() {
    let (_dir, pool, mut store) = test_store_with_pool().await;
    let (blocker_id, _) = create_selected_task(&mut store, "Atomic blocker").await;
    let (task_id, selected) = create_selected_task(&mut store, "Atomic blocked").await;
    reject_undo_inserts(&pool).await;

    let error = store
        .add_dependency(Some(selected), &blocker_id)
        .await
        .unwrap_err();

    assert!(error.to_string().contains("injected undo failure"));
    let persisted: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM task_dependencies
         WHERE task_id = ? AND depends_on_task_id = ?",
    )
    .bind(&task_id)
    .bind(&blocker_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(persisted, 0);
}

#[tokio::test]
async fn dependency_actions_add_remove_and_undo() {
    let mut store = test_store().await;
    let (blocker_id, _) = create_selected_task(&mut store, "Blocker").await;
    let (task_id, selected) = create_selected_task(&mut store, "Blocked").await;

    let add = store
        .add_dependency(Some(selected), &blocker_id)
        .await
        .unwrap()
        .unwrap();
    assert!(add.message.contains("added dependency"));
    let selected = add.selected.unwrap();
    assert_eq!(store.tasks[selected].depends_on.len(), 1);
    assert_eq!(store.tasks[selected].depends_on[0].task_id, blocker_id);
    assert_eq!(store.tasks[selected].unresolved_blocker_count, 1);

    store.undo_last(Some(selected)).await.unwrap().unwrap();
    let selected = store
        .tasks
        .iter()
        .position(|item| item.task.id == task_id)
        .unwrap();
    assert!(store.tasks[selected].depends_on.is_empty());

    let add2 = store
        .add_dependency(Some(selected), &blocker_id)
        .await
        .unwrap()
        .unwrap();
    let selected = add2.selected.unwrap();
    let remove = store
        .remove_dependency(Some(selected), &blocker_id)
        .await
        .unwrap()
        .unwrap();
    assert!(remove.message.contains("removed dependency"));
    let selected = remove.selected.unwrap();
    assert!(store.tasks[selected].depends_on.is_empty());

    store.undo_last(Some(selected)).await.unwrap().unwrap();
    let selected = store
        .tasks
        .iter()
        .position(|item| item.task.id == task_id)
        .unwrap();
    assert_eq!(store.tasks[selected].depends_on[0].task_id, blocker_id);
    assert_eq!(store.tasks[selected].unresolved_blocker_count, 1);
}
