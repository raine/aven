use super::*;

async fn create_epic_child_pair(store: &mut TuiStore) -> (TaskId, TaskId, usize) {
    let (parent_id, _parent_index) = create_selected_task(store, "epic parent").await;
    let child_title = format!("child of {}", &parent_id[..4]);
    let (child_id, _) = create_selected_task(store, &child_title).await;

    let mut conn = aven_core::test_support::acquire(&store.database)
        .await
        .unwrap();
    crate::operations::add_task_to_epic(
        &mut conn,
        &crate::workspaces::Workspace::default(),
        &child_id,
        &parent_id,
    )
    .await
    .unwrap();
    drop(conn);

    store.view_state.view = TaskView::Epics;
    store.refresh(Some(&parent_id)).await.unwrap();
    let parent_index = store
        .tasks
        .iter()
        .position(|t| t.task.id == parent_id)
        .unwrap();
    (parent_id, child_id, parent_index)
}

#[tokio::test]
async fn epics_view_closed_filter_includes_and_isolates_closed_epics() {
    let mut store = test_store().await;
    let (_, open_selected) = store
        .create_task(
            TaskDraft {
                is_epic: true,
                ..task_draft("Open epic")
            },
            None,
        )
        .await
        .unwrap();
    let (_, closed_selected) = store
        .create_task(
            TaskDraft {
                is_epic: true,
                ..task_draft("Finished epic")
            },
            open_selected,
        )
        .await
        .unwrap();
    store.update_status(closed_selected, "done").await.unwrap();

    store.show_view(TaskView::Epics).await.unwrap();

    assert_eq!(store.counts.epics, 1);
    assert_eq!(
        store
            .tasks
            .iter()
            .map(|item| item.task.title.as_str())
            .collect::<Vec<_>>(),
        vec!["Open epic"]
    );

    store.toggle_closed_filter().await.unwrap();
    assert_eq!(
        store
            .tasks
            .iter()
            .map(|item| item.task.title.as_str())
            .collect::<Vec<_>>(),
        vec!["Open epic", "Finished epic"]
    );

    store.toggle_closed_filter().await.unwrap();
    assert_eq!(store.tasks.len(), 1);
    assert_eq!(store.tasks[0].task.title, "Finished epic");
    assert_eq!(store.tasks[0].task.status, TaskStatus::Done);

    store.toggle_closed_filter().await.unwrap();
    assert_eq!(store.tasks.len(), 1);
    assert_eq!(store.tasks[0].task.title, "Open epic");
}

#[tokio::test]
async fn epics_view_starts_with_parents_collapsed() {
    let mut store = test_store().await;
    let (parent_id, child_id, _) = create_epic_child_pair(&mut store).await;

    assert!(!store.view_state.expanded_epic_ids.contains(&parent_id));
    assert!(!store.tasks.iter().any(|task| task.task.id == child_id));
}

#[tokio::test]
async fn toggle_epic_expands_and_collapses_parent() {
    let mut store = test_store().await;
    let (parent_task_id, child_id, parent_index) = create_epic_child_pair(&mut store).await;

    assert!(!store.view_state.expanded_epic_ids.contains(&parent_task_id));

    store
        .toggle_selected_epic(Some(parent_index))
        .await
        .unwrap()
        .unwrap();
    assert!(store.view_state.expanded_epic_ids.contains(&parent_task_id));
    assert!(
        !store
            .view_state
            .collapsed_epic_ids
            .contains(&parent_task_id)
    );
    assert!(store.tasks.iter().any(|task| task.task.id == child_id));

    store.refresh(Some(&parent_task_id)).await.unwrap();
    assert!(store.view_state.expanded_epic_ids.contains(&parent_task_id));
    assert!(store.tasks.iter().any(|task| task.task.id == child_id));

    let parent_index = store
        .tasks
        .iter()
        .position(|task| task.task.id == parent_task_id)
        .unwrap();
    store
        .toggle_selected_epic(Some(parent_index))
        .await
        .unwrap()
        .unwrap();
    assert!(!store.view_state.expanded_epic_ids.contains(&parent_task_id));
    assert!(
        store
            .view_state
            .collapsed_epic_ids
            .contains(&parent_task_id)
    );
    assert!(!store.tasks.iter().any(|task| task.task.id == child_id));

    store.refresh(Some(&parent_task_id)).await.unwrap();
    assert!(!store.view_state.expanded_epic_ids.contains(&parent_task_id));
    assert!(!store.tasks.iter().any(|task| task.task.id == child_id));
}

#[tokio::test]
async fn status_change_from_collapsed_epic_selects_remaining_row() {
    let mut store = test_store().await;
    let (first_parent_id, _, _) = create_epic_child_pair(&mut store).await;
    store.show_view(TaskView::Queue).await.unwrap();
    let (second_parent_id, _, _) = create_epic_child_pair(&mut store).await;
    let first_parent = store
        .tasks
        .iter()
        .position(|task| task.task.id == first_parent_id)
        .unwrap();

    let result = store
        .update_status(Some(first_parent), "done")
        .await
        .unwrap()
        .unwrap();

    let selected = result.selected.expect("a remaining epic row");
    assert_eq!(selected, first_parent.min(store.tasks.len() - 1));
    assert_ne!(store.tasks[selected].task.id, first_parent_id);
    assert!(
        store
            .tasks
            .iter()
            .any(|task| task.task.id == second_parent_id)
    );
    assert!(
        !store
            .view_state
            .collapsed_epic_ids
            .contains(&first_parent_id)
    );
}

#[tokio::test]
async fn toggle_epic_noop_when_no_selection() {
    let mut store = test_store().await;
    assert!(store.toggle_selected_epic(None).await.unwrap().is_none());
    assert!(
        store
            .toggle_selected_epic(Some(99))
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn remove_epic_child_uses_resolved_ids_and_records_undo() {
    let mut store = test_store().await;
    let (parent_id, child_id, parent_index) = create_epic_child_pair(&mut store).await;
    store
        .toggle_selected_epic(Some(parent_index))
        .await
        .unwrap()
        .unwrap();
    let child_index = store
        .tasks
        .iter()
        .position(|task| task.task.id == child_id)
        .unwrap();
    let target = store
        .resolve_epic_child_target(Some(child_index), None)
        .unwrap();

    let outcome = store.remove_epic_child(target).await.unwrap();

    assert!(outcome.changed);
    assert!(outcome.message.message.contains("Removed"));
    let parent_index = store
        .tasks
        .iter()
        .position(|task| task.task.id == parent_id)
        .unwrap();
    assert!(
        store.tasks[parent_index]
            .epic_children
            .iter()
            .all(|child| child.task_id != child_id)
    );

    store.undo_last(None).await.unwrap().unwrap();
    let parent = store
        .tasks
        .iter()
        .find(|task| task.task.id == parent_id)
        .unwrap();
    assert!(
        parent
            .epic_children
            .iter()
            .any(|child| child.task_id == child_id)
    );
}

#[tokio::test]
async fn epic_membership_rolls_back_when_undo_recording_fails() {
    let (_dir, pool, mut store) = test_store_with_pool().await;
    let (parent_id, parent_index) = create_selected_task(&mut store, "Atomic epic").await;
    let (child_id, _) = create_selected_task(&mut store, "Atomic child").await;
    let epic = EpicContext {
        epic_id: parent_id.clone(),
        display_ref: store.tasks[parent_index].display_ref.clone(),
        project_key: store.tasks[parent_index].task.project_key.clone(),
    };
    reject_undo_inserts(&pool).await;

    let error = store
        .add_epic_child(epic, child_id.clone())
        .await
        .unwrap_err();

    assert!(error.to_string().contains("injected undo failure"));
    let linked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM task_epic_links
         WHERE epic_task_id = ? AND child_task_id = ?",
    )
    .bind(&parent_id)
    .bind(&child_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(linked, 0);
    let is_epic: i64 = sqlx::query_scalar("SELECT is_epic FROM tasks WHERE id = ?")
        .bind(&parent_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(is_epic, 0);
}

#[tokio::test]
async fn add_epic_child_promotes_parent_and_undo_restores_both() {
    let (_dir, pool, mut store) = test_store_with_pool().await;
    let (parent_id, _) = create_selected_task(&mut store, "Ordinary parent").await;
    let (child_id, _) = create_selected_task(&mut store, "Child task").await;
    let parent_index = store
        .tasks
        .iter()
        .position(|item| item.task.id == parent_id)
        .expect("parent should remain visible");
    let epic = match store.resolve_add_epic_child_context(Some(parent_index)) {
        Some(AddEpicChildContext::Promote(epic)) => epic,
        context => panic!("expected promotion context, got {context:?}"),
    };

    let outcome = store.add_epic_child(epic, child_id.clone()).await.unwrap();

    assert!(outcome.changed);
    assert!(
        store
            .tasks
            .iter()
            .find(|item| item.task.id == parent_id)
            .is_some_and(|item| item.task.is_epic)
    );
    store.undo_last(None).await.unwrap().unwrap();
    let linked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM task_epic_links WHERE epic_task_id = ? AND child_task_id = ?",
    )
    .bind(&parent_id)
    .bind(&child_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(linked, 0);
    let is_epic: i64 = sqlx::query_scalar("SELECT is_epic FROM tasks WHERE id = ?")
        .bind(&parent_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(is_epic, 0);
}

#[tokio::test]
async fn epic_child_creation_rolls_back_task_when_link_validation_fails() {
    let (_dir, pool, store) = test_store_with_pool().await;
    let parent = store
        .database
        .create_task(
            &store.active_workspace,
            TaskDraft {
                project: Some("parent-project".to_string()),
                is_epic: true,
                ..task_draft("Parent epic")
            },
        )
        .await
        .unwrap();

    let error = store
        .database
        .create_task_for_epic(
            &store.active_workspace,
            TaskDraft {
                project: Some("child-project".to_string()),
                ..task_draft("Unexpected standalone child")
            },
            &parent.task.id,
        )
        .await
        .unwrap_err();

    assert!(format!("{error:#}").contains("epic-cross-project"));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM tasks WHERE title = ?")
        .bind("Unexpected standalone child")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn remove_completed_epic_child() {
    let mut store = test_store().await;
    let (_parent_id, child_id, parent_index) = create_epic_child_pair(&mut store).await;
    store
        .toggle_selected_epic(Some(parent_index))
        .await
        .unwrap()
        .unwrap();
    let child_index = store
        .tasks
        .iter()
        .position(|task| task.task.id == child_id)
        .unwrap();
    store
        .update_status(Some(child_index), "done")
        .await
        .unwrap();
    store.view_state.view = TaskView::Epics;
    store.refresh(None).await.unwrap();
    let child_index = store
        .tasks
        .iter()
        .position(|task| task.task.id == child_id)
        .unwrap();
    let target = store
        .resolve_epic_child_target(Some(child_index), None)
        .unwrap();

    let outcome = store.remove_epic_child(target).await.unwrap();

    assert!(outcome.changed);
}
