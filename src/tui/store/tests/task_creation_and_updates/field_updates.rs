use super::*;

#[tokio::test]
async fn update_task_fields_refresh_selected_task() {
    let mut store = test_store().await;
    let (_, selected) = store
        .create_task(
            TaskDraft {
                metadata: Vec::new(),
                title: "Old".to_string(),
                description: "old body".to_string(),
                ..task_draft("")
            },
            None,
        )
        .await
        .unwrap();
    let selected = selected.unwrap();
    let display_ref = store.tasks[selected].display_ref.clone();

    let title = store
        .update_title(Some(selected), "New".to_string())
        .await
        .unwrap()
        .unwrap();
    let description = store
        .update_description(Some(selected), "new body".to_string())
        .await
        .unwrap()
        .unwrap();
    let priority = store
        .set_exact_priority(Some(selected), "urgent")
        .await
        .unwrap()
        .unwrap();

    assert_eq!(title.message, format!("set {display_ref} title"));
    assert_eq!(
        description.message,
        format!("set {display_ref} description")
    );
    assert_eq!(
        priority.message,
        format!("set {display_ref} priority=urgent")
    );
    let task = &store.tasks[selected].task;
    assert_eq!(task.title, "New");
    assert_eq!(task.description, "new body");
    assert_eq!(task.priority, TaskPriority::Urgent);
}

#[tokio::test]
async fn availability_edit_refreshes_task_and_sidebar_counts() {
    let mut store = test_store().await;
    let (_, selected) = store
        .create_task(task_draft("Availability target"), None)
        .await
        .unwrap();
    let selected = selected.unwrap();

    let set = store
        .update_availability(Some(selected), "2099-01-01T00:00:00Z".to_string(), true)
        .await
        .unwrap()
        .unwrap();

    assert!(set.message.contains("set"));
    assert_eq!(store.counts.inbox, 0);
    assert_eq!(store.counts.upcoming, 1);
    let selected = set.selected.unwrap();
    assert_eq!(
        store.tasks[selected].task.available_at.as_deref(),
        Some("2099-01-01T00:00:00Z")
    );

    let cleared = store
        .update_availability(Some(selected), String::new(), false)
        .await
        .unwrap()
        .unwrap();

    assert!(cleared.message.contains("cleared"));
    assert_eq!(store.counts.inbox, 1);
    assert_eq!(store.counts.upcoming, 0);
    let selected = cleared.selected.unwrap();
    assert!(store.tasks[selected].task.available_at.is_none());
}

#[tokio::test]
async fn title_edit_keeps_queue_activity_timestamp() {
    let (dir, pool, mut store) = test_store_with_pool().await;
    let (task_id, selected) =
        create_selected_task_with_stale_queue_activity(&mut store, &pool, "Old").await;
    let old_activity = "1970-01-01T00:00:00Z";
    let old_updated = "1970-01-02T00:00:00Z";
    set_task_timestamps(
        &pool,
        &store.active_workspace.id,
        &task_id,
        old_activity,
        Some(old_updated),
    )
    .await;
    store.refresh(Some(&task_id)).await.unwrap();

    store
        .update_title(Some(selected), "New".to_string())
        .await
        .unwrap();

    let task = &store.tasks[selected].task;
    assert_eq!(task.title, "New");
    assert_ne!(task.updated_at, old_updated);
    assert_eq!(task.queue_activity_at, old_activity);
}

#[tokio::test]
async fn unchanged_title_edit_leaves_pending_change_count() {
    let (dir, pool, mut store) = test_store_with_pool().await;
    let (_task_id, selected) = create_selected_task(&mut store, "Stable").await;
    let pending_before = pending_change_count(&pool).await;
    let display_ref = store.tasks[selected].display_ref.clone();

    let outcome = store
        .update_title(Some(selected), "Stable".to_string())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(outcome.message, format!("unchanged {display_ref} title"));
    assert_eq!(pending_change_count(&pool).await, pending_before);
    assert_eq!(store.tasks[selected].task.title, "Stable");
}

#[tokio::test]
async fn unchanged_task_fields_leave_pending_change_count() {
    let (dir, pool, mut store) = test_store_with_pool().await;
    store.create_project("Side".to_string()).await.unwrap();
    let (task_id, _selected) = create_selected_task(&mut store, "Stable").await;
    let pending_before = pending_change_count(&pool).await;
    let mut conn = pool.acquire().await.unwrap();

    let outcome = crate::operations::update_task(
        &mut conn,
        &crate::workspaces::Workspace::default(),
        &task_id,
        TaskUpdate {
            title: Some("Stable".to_string()),
            description: Some(String::new()),
            project: Some("aven".to_string()),
            status: Some("inbox".to_string()),
            priority: Some("none".to_string()),
            ..TaskUpdate::default()
        },
    )
    .await
    .unwrap();
    let deleted = crate::operations::set_task_deleted(
        &mut conn,
        &crate::workspaces::Workspace::default(),
        &task_id,
        false,
    )
    .await
    .unwrap();
    drop(conn);

    assert!(!outcome.changed);
    assert_eq!(deleted.task.title, "Stable");
    assert_eq!(pending_change_count(&pool).await, pending_before);
}

#[tokio::test]
async fn unchanged_label_updates_leave_pending_change_count() {
    let (dir, pool, mut store) = test_store_with_pool().await;
    store.create_label("bug".to_string()).await.unwrap();
    store.create_label("missing".to_string()).await.unwrap();
    let (task_id, _selected) = create_selected_task(&mut store, "Labels").await;
    let mut conn = pool.acquire().await.unwrap();
    crate::operations::update_task(
        &mut conn,
        &crate::workspaces::Workspace::default(),
        &task_id,
        TaskUpdate {
            add_labels: vec!["bug".to_string()],
            ..TaskUpdate::default()
        },
    )
    .await
    .unwrap();
    drop(conn);
    let pending_before = pending_change_count(&pool).await;
    let mut conn = pool.acquire().await.unwrap();

    let outcome = crate::operations::update_task(
        &mut conn,
        &crate::workspaces::Workspace::default(),
        &task_id,
        TaskUpdate {
            add_labels: vec!["bug".to_string()],
            remove_labels: vec!["missing".to_string()],
            ..TaskUpdate::default()
        },
    )
    .await
    .unwrap();
    drop(conn);

    assert!(!outcome.changed);
    assert_eq!(pending_change_count(&pool).await, pending_before);
}

#[tokio::test]
async fn priority_edit_refreshes_queue_activity_timestamp() {
    let (dir, pool, mut store) = test_store_with_pool().await;
    let (_task_id, selected) =
        create_selected_task_with_stale_queue_activity(&mut store, &pool, "Old").await;
    let old_activity = "1970-01-01T00:00:00Z";

    store
        .set_exact_priority(Some(selected), "high")
        .await
        .unwrap();

    let task = &store.tasks[selected].task;
    assert_eq!(task.priority, TaskPriority::High);
    assert_ne!(task.queue_activity_at, old_activity);
}

#[tokio::test]
async fn text_mutation_rejects_multiple_targets() {
    let mut store = test_store().await;
    let (first_id, _) = create_selected_task(&mut store, "First").await;
    let (second_id, second) = create_selected_task(&mut store, "Second").await;
    let selection = crate::tui::task_selection::TaskSelection::from_ids(
        &store.tasks,
        &[first_id, second_id],
        Some(second),
    )
    .unwrap();

    let error = store
        .mutate_text_selection(&selection, TaskTextField::Title, "Unexpected".to_string())
        .await
        .unwrap_err();

    assert_eq!(error.to_string(), "text mutation requires exactly one task");
}

#[tokio::test]
async fn priority_cycle_uses_authoritative_value_after_selection_capture() {
    let mut store = test_store().await;
    let (task_id, selected) = create_selected_task(&mut store, "Priority race").await;
    let selection = crate::tui::task_selection::TaskSelection::from_ids(
        &store.tasks,
        std::slice::from_ref(&task_id),
        Some(selected),
    )
    .unwrap();
    store
        .database
        .update_task(
            &store.active_workspace,
            &task_id,
            TaskUpdate {
                priority: Some("high".to_string()),
                ..TaskUpdate::default()
            },
        )
        .await
        .unwrap();

    store
        .mutate_priority_selection(&selection, PriorityMutation::Cycle { reverse: false })
        .await
        .unwrap();

    assert_eq!(store.tasks[selected].task.priority, TaskPriority::Urgent);
}

#[tokio::test]
async fn label_selection_uses_authoritative_labels_after_capture() {
    let mut store = test_store().await;
    for label in ["old", "docs", "bug"] {
        store.create_label(label.to_string()).await.unwrap();
    }
    let (_, selected) = store
        .create_task(
            TaskDraft {
                metadata: Vec::new(),
                title: "Label race".to_string(),
                labels: vec!["old".to_string()],
                ..task_draft("")
            },
            None,
        )
        .await
        .unwrap();
    let selected = selected.unwrap();
    let task_id = store.tasks[selected].task.id.clone();
    let selection = crate::tui::task_selection::TaskSelection::from_ids(
        &store.tasks,
        std::slice::from_ref(&task_id),
        Some(selected),
    )
    .unwrap();
    store
        .database
        .update_task(
            &store.active_workspace,
            &task_id,
            TaskUpdate {
                add_labels: vec!["docs".to_string()],
                remove_labels: vec!["old".to_string()],
                ..TaskUpdate::default()
            },
        )
        .await
        .unwrap();

    store
        .mutate_labels_selection(&selection, vec!["bug".to_string()], Vec::new())
        .await
        .unwrap();

    assert_eq!(store.tasks[selected].labels, vec!["bug".to_string()]);
}

#[tokio::test]
async fn update_labels_adds_and_removes_labels() {
    let mut store = test_store().await;
    store.create_label("bug".to_string()).await.unwrap();
    store.create_label("docs".to_string()).await.unwrap();
    let (_, selected) = store
        .create_task(
            TaskDraft {
                metadata: Vec::new(),
                title: "Labels".to_string(),
                labels: vec!["bug".to_string()],
                ..task_draft("")
            },
            None,
        )
        .await
        .unwrap();
    let selected = selected.unwrap();
    let display_ref = store.tasks[selected].display_ref.clone();

    let outcome = store
        .update_labels(Some(selected), vec!["docs".to_string()])
        .await
        .unwrap()
        .unwrap();

    assert_eq!(outcome.message, format!("set {display_ref} labels"));
    assert_eq!(store.tasks[selected].labels, vec!["docs".to_string()]);
}
