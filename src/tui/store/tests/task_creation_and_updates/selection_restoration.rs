use super::*;

#[tokio::test]
async fn unchanged_single_and_batch_edits_preserve_state_without_undo() {
    let (_dir, pool, mut store) = test_store_with_pool().await;
    let (task_id, selected) = create_selected_task(&mut store, "Unchanged edits").await;
    let workspace_id = store.active_workspace.id.clone();
    let undo_before = pending_undo_count(&pool, &workspace_id).await;
    let changes_before = pending_change_count(&pool).await;

    let availability = store
        .update_availability(Some(selected), String::new(), false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        availability.message,
        format!(
            "unchanged {} availability",
            store.tasks[selected].display_ref
        )
    );
    assert_selected_task(&store, &availability, &task_id);

    let status = store
        .update_status_for_tasks(Some(selected), std::slice::from_ref(&task_id), "inbox")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        status.message,
        format!(
            "unchanged {} status=inbox",
            store.tasks[selected].display_ref
        )
    );
    assert_selected_task(&store, &status, &task_id);

    let priority = store
        .set_exact_priority_for_tasks(status.selected, std::slice::from_ref(&task_id), "none")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        priority.message,
        format!(
            "unchanged {} priority=none",
            store.tasks[selected].display_ref
        )
    );
    assert_selected_task(&store, &priority, &task_id);

    let project = store
        .update_project_for_tasks(
            priority.selected,
            std::slice::from_ref(&task_id),
            "aven".to_string(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        project.message,
        format!("unchanged {} project", store.tasks[selected].display_ref)
    );
    assert_selected_task(&store, &project, &task_id);

    let labels = store
        .update_labels_for_tasks(
            project.selected,
            std::slice::from_ref(&task_id),
            Vec::new(),
            Vec::new(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        labels.message,
        format!("unchanged {} labels", store.tasks[selected].display_ref)
    );
    assert_selected_task(&store, &labels, &task_id);

    let availability = store
        .update_availability_for_tasks(
            labels.selected,
            std::slice::from_ref(&task_id),
            String::new(),
            false,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        availability.message,
        format!(
            "unchanged {} availability",
            store.tasks[selected].display_ref
        )
    );
    assert_selected_task(&store, &availability, &task_id);

    let due = store
        .update_due_for_tasks(
            availability.selected,
            std::slice::from_ref(&task_id),
            String::new(),
            false,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        due.message,
        format!("unchanged {} due date", store.tasks[selected].display_ref)
    );
    assert_selected_task(&store, &due, &task_id);

    let deleted = store
        .update_deleted_for_tasks(due.selected, std::slice::from_ref(&task_id), false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        deleted.message,
        format!("already restored {}", store.tasks[selected].display_ref)
    );
    assert_selected_task(&store, &deleted, &task_id);

    assert_eq!(pending_change_count(&pool).await, changes_before);
    assert_eq!(pending_undo_count(&pool, &workspace_id).await, undo_before);
}

#[tokio::test]
async fn mutation_families_restore_selection_after_refresh() {
    let mut store = test_store().await;
    store
        .create_project("Mobile App".to_string())
        .await
        .unwrap();
    store.create_label("bug".to_string()).await.unwrap();
    let (target_id, _) = create_selected_task(&mut store, "Mutation target").await;
    let (selected_id, _) = create_selected_task(&mut store, "Selection anchor").await;
    let selected = store
        .tasks
        .iter()
        .position(|item| item.task.id == selected_id)
        .unwrap();

    let status = store
        .update_status_for_tasks(Some(selected), std::slice::from_ref(&target_id), "todo")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status.selected, Some(selected));
    let selected = store
        .tasks
        .iter()
        .position(|item| item.task.id == selected_id)
        .unwrap();

    let priority = store
        .set_exact_priority_for_tasks(Some(selected), std::slice::from_ref(&target_id), "high")
        .await
        .unwrap()
        .unwrap();
    assert_selected_task(&store, &priority, &selected_id);

    let project = store
        .update_project_for_tasks(
            priority.selected,
            std::slice::from_ref(&target_id),
            "mobile-app".to_string(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_selected_task(&store, &project, &selected_id);

    let labels = store
        .update_labels_for_tasks(
            project.selected,
            std::slice::from_ref(&target_id),
            vec!["bug".to_string()],
            Vec::new(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_selected_task(&store, &labels, &selected_id);

    let availability = store
        .update_availability_for_tasks(
            labels.selected,
            std::slice::from_ref(&target_id),
            "2020-01-01T00:00:00Z".to_string(),
            false,
        )
        .await
        .unwrap()
        .unwrap();
    assert_selected_task(&store, &availability, &selected_id);

    let due = store
        .update_due_for_tasks(
            availability.selected,
            std::slice::from_ref(&target_id),
            "2099-01-01".to_string(),
            false,
        )
        .await
        .unwrap()
        .unwrap();
    assert_selected_task(&store, &due, &selected_id);

    let deleted = store
        .update_deleted_for_tasks(due.selected, std::slice::from_ref(&target_id), true)
        .await
        .unwrap()
        .unwrap();
    assert_selected_task(&store, &deleted, &selected_id);
}
