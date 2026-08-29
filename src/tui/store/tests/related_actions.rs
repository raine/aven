use super::*;

#[tokio::test]
async fn related_actions_add_remove_and_undo() {
    let mut store = test_store().await;
    let (related_id, _) = create_selected_task(&mut store, "Related").await;
    let (task_id, selected) = create_selected_task(&mut store, "Subject").await;

    let add = store
        .add_related(Some(selected), &related_id)
        .await
        .unwrap()
        .unwrap();
    assert!(add.message.contains("added related link"));
    let selected = add.selected.unwrap();
    assert_eq!(store.tasks[selected].related[0].task_id, related_id);

    store.undo_last(Some(selected)).await.unwrap().unwrap();
    store
        .ensure_task_details(std::slice::from_ref(&task_id))
        .await
        .unwrap();
    let selected = store
        .tasks
        .iter()
        .position(|item| item.task.id == task_id)
        .unwrap();
    assert!(store.tasks[selected].related.is_empty());

    let added = store
        .add_related(Some(selected), &related_id)
        .await
        .unwrap()
        .unwrap();
    let removed = store
        .remove_related(added.selected, &related_id)
        .await
        .unwrap()
        .unwrap();
    assert!(removed.message.contains("removed related link"));
    store.undo_last(removed.selected).await.unwrap().unwrap();
    store
        .ensure_task_details(std::slice::from_ref(&task_id))
        .await
        .unwrap();
    let selected = store
        .tasks
        .iter()
        .position(|item| item.task.id == task_id)
        .unwrap();
    assert_eq!(store.tasks[selected].related[0].task_id, related_id);
}

#[tokio::test]
async fn related_removal_picker_includes_deleted_targets_with_marker() {
    let (_dir, pool, mut store) = test_store_with_pool().await;
    let (related_id, _) = create_selected_task(&mut store, "Deleted related").await;
    let (task_id, selected) = create_selected_task(&mut store, "Picker subject").await;
    store
        .add_related(Some(selected), &related_id)
        .await
        .unwrap();
    sqlx::query("UPDATE tasks SET deleted = 1 WHERE id = ?")
        .bind(&related_id)
        .execute(&pool)
        .await
        .unwrap();
    store.refresh(None).await.unwrap();
    let selected = store
        .tasks
        .iter()
        .position(|item| item.task.title == "Picker subject")
        .unwrap();
    store
        .ensure_task_details(std::slice::from_ref(&task_id))
        .await
        .unwrap();
    let items = crate::tui::store::related_picker_items(&store.tasks[selected]);
    assert_eq!(items.len(), 1);
    assert!(items[0].label.contains("[deleted]"));
    assert_eq!(items[0].value, related_id.to_string());
}

#[tokio::test]
async fn create_undo_soft_deletes_tasks_with_related_tombstones() {
    let (_dir, pool, mut store) = test_store_with_pool().await;
    let (related_id, _) = create_selected_task(&mut store, "Related guard").await;
    let (task_id, selected) = create_selected_task(&mut store, "Guarded subject").await;
    let added = store
        .add_related(Some(selected), &related_id)
        .await
        .unwrap()
        .unwrap();
    store.undo_last(added.selected).await.unwrap().unwrap();
    let selected = store
        .tasks
        .iter()
        .position(|item| item.task.id == task_id)
        .unwrap();
    store.undo_last(Some(selected)).await.unwrap().unwrap();

    let deleted: i64 = sqlx::query_scalar("SELECT deleted FROM tasks WHERE id = ?")
        .bind(&task_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(deleted, 1);
    let related_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM task_related_links WHERE task_a_id = ? OR task_b_id = ?",
    )
    .bind(&task_id)
    .bind(&task_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(related_rows, 1);
}

#[tokio::test]
async fn related_no_ops_do_not_wake_the_daemon() {
    let mut store = test_store().await;
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    socket
        .set_read_timeout(Some(std::time::Duration::from_millis(100)))
        .unwrap();
    let (related_id, _) = create_selected_task(&mut store, "Wake related").await;
    let (_task_id, selected) = create_selected_task(&mut store, "Wake subject").await;
    let mut config = crate::config::AppConfig::default();
    config.sync.enabled = true;
    config.daemon.wake_addr = Some(socket.local_addr().unwrap().to_string());
    store.set_config(config);
    let mut byte = [0_u8; 1];

    let added = store
        .add_related(Some(selected), &related_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(socket.recv(&mut byte).unwrap(), 1);
    store
        .add_related(added.selected, &related_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        socket.recv(&mut byte).unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );

    let removed = store
        .remove_related(added.selected, &related_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(socket.recv(&mut byte).unwrap(), 1);
    store
        .remove_related(removed.selected, &related_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        socket.recv(&mut byte).unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[tokio::test]
async fn related_undo_rejects_a_changed_pair_version() {
    let mut store = test_store().await;
    let (related_id, _) = create_selected_task(&mut store, "Version related").await;
    let (_task_id, selected) = create_selected_task(&mut store, "Version subject").await;
    let added = store
        .add_related(Some(selected), &related_id)
        .await
        .unwrap()
        .unwrap();
    let subject_id = store.tasks[added.selected.unwrap()].task.id.clone();
    store
        .database
        .remove_task_related_link(&store.active_workspace, &subject_id, &related_id)
        .await
        .unwrap();

    let error = store.undo_last(added.selected).await.unwrap_err();
    assert!(error.to_string().contains("undo-state-changed"));
}
