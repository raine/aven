use super::*;

#[tokio::test]
async fn create_task_refreshes_and_selects_visible_task() {
    let mut store = test_store().await;
    store
        .create_label("needs-review".to_string())
        .await
        .unwrap();
    let (message, selected) = store
        .create_task(
            TaskDraft {
                title: "Write docs".to_string(),
                description: "details".to_string(),
                priority: "high".to_string(),
                labels: vec!["needs-review".to_string()],
                ..task_draft("")
            },
            None,
        )
        .await
        .unwrap();

    let selected = selected.unwrap();
    assert_eq!(
        message,
        format!("created task {}", store.tasks[selected].display_ref)
    );
    let task = &store.tasks[selected];
    assert_eq!(task.task.title, "Write docs");
    assert_eq!(task.task.priority, TaskPriority::High);
    assert!(task.labels.iter().any(|label| label == "needs-review"));
}

#[tokio::test]
async fn create_task_assigns_tui_source() {
    let (_dir, pool, mut store) = test_store_with_pool().await;
    let (_, selected) = store
        .create_task(task_draft("TUI source"), None)
        .await
        .unwrap();
    let task_id = &store.tasks[selected.unwrap()].task.id;
    let source: String = sqlx::query_scalar("SELECT source FROM tasks WHERE id = ?")
        .bind(task_id)
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(source, "tui");
}

#[tokio::test]
async fn create_task_reports_hidden_by_filters() {
    let mut store = test_store().await;
    store.show_view(TaskView::Todo).await.unwrap();
    let (message, selected) = store
        .create_task(task_draft("Inbox task"), None)
        .await
        .unwrap();

    assert!(selected.is_none());
    assert!(message.contains("hidden by current filters"));
}

#[tokio::test]
async fn create_task_preserves_previous_selection_when_hidden() {
    let mut store = test_store().await;
    let (_, first_selected) = store
        .create_task(task_draft("Todo task"), None)
        .await
        .unwrap();
    let first_selected = first_selected.unwrap();
    let task_id = store.tasks[first_selected].task.id.clone();
    store
        .update_status(Some(first_selected), "todo")
        .await
        .unwrap();
    store.show_view(TaskView::Todo).await.unwrap();
    let current_index = store.refresh(Some(&task_id)).await.unwrap();

    let (_, selected) = store
        .create_task(task_draft("Hidden inbox task"), current_index)
        .await
        .unwrap();

    assert_eq!(selected, current_index);
    assert_eq!(store.tasks.len(), 1);
    assert_eq!(store.tasks[0].task.title, "Todo task");
}

#[tokio::test]
async fn queue_status_change_keeps_selection_at_ranked_position() {
    let mut store = test_store().await;
    for title in ["First", "Selected", "Third"] {
        create_selected_task(&mut store, title).await;
    }
    store.show_view(TaskView::Queue).await.unwrap();
    let selected = 1;
    let selected_id = store.tasks[selected].task.id.clone();

    let result = store
        .update_status(Some(selected), "active")
        .await
        .unwrap()
        .unwrap();

    assert_eq!(result.selected, Some(selected));
    assert_ne!(store.tasks[selected].task.id, selected_id);
    assert_eq!(
        store
            .tasks
            .iter()
            .position(|item| item.task.id == selected_id),
        Some(0)
    );
}

#[tokio::test]
async fn filtered_status_change_selects_successor_then_previous_at_end() {
    let mut store = test_store().await;
    for title in ["First", "Second", "Third"] {
        let (_, selected) = create_selected_task(&mut store, title).await;
        store.update_status(Some(selected), "todo").await.unwrap();
    }
    store.show_view(TaskView::Todo).await.unwrap();
    let second = 1;
    let predecessor_id = store.tasks[second - 1].task.id.clone();
    let successor_id = store.tasks[second + 1].task.id.clone();

    let result = store
        .update_status(Some(second), "done")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.selected, Some(second));
    assert_eq!(store.tasks[second].task.id, successor_id);

    let result = store
        .update_status(result.selected, "done")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.selected, Some(0));
    assert_eq!(store.tasks[0].task.id, predecessor_id);
}

#[tokio::test]
async fn unchanged_status_keeps_selected_task_at_its_position() {
    let mut store = test_store().await;
    for title in ["First", "Selected", "Third"] {
        create_selected_task(&mut store, title).await;
    }
    store.show_view(TaskView::Queue).await.unwrap();
    let selected = 1;
    let selected_id = store.tasks[selected].task.id.clone();

    let result = store
        .update_status(Some(selected), "inbox")
        .await
        .unwrap()
        .unwrap();

    assert_eq!(result.selected, Some(selected));
    assert_eq!(store.tasks[selected].task.id, selected_id);
}

#[tokio::test]
async fn marked_status_change_keeps_selection_at_ranked_position() {
    let mut store = test_store().await;
    for title in ["First", "Selected", "Third", "Fourth"] {
        create_selected_task(&mut store, title).await;
    }
    store.show_view(TaskView::Queue).await.unwrap();
    let selected = 2;
    let targets = vec![
        store.tasks[0].task.id.clone(),
        store.tasks[selected].task.id.clone(),
    ];

    let result = store
        .update_status_for_tasks(Some(selected), &targets, "active")
        .await
        .unwrap()
        .unwrap();

    assert_eq!(result.selected, Some(selected));
    assert_eq!(store.tasks[selected].task.status, TaskStatus::Inbox);
}

#[tokio::test]
async fn preserving_status_change_follows_task_after_queue_reorder() {
    let mut store = test_store().await;
    for title in ["First", "Selected", "Third"] {
        create_selected_task(&mut store, title).await;
    }
    store.show_view(TaskView::Queue).await.unwrap();
    let selected = 1;
    let selected_id = store.tasks[selected].task.id.clone();

    let result = store
        .update_status_preserving_task(Some(selected), "active")
        .await
        .unwrap()
        .unwrap();

    assert_eq!(result.selected, Some(0));
    assert_eq!(store.tasks[0].task.id, selected_id);
}

#[tokio::test]
async fn update_status_preserving_task_keeps_done_item_in_filtered_view() {
    let mut store = test_store().await;
    let _ = store
        .create_task(
            TaskDraft {
                title: "Next target".to_string(),
                status: "todo".to_string(),
                ..task_draft("")
            },
            None,
        )
        .await
        .unwrap();
    let (_, selected) = store
        .create_task(
            TaskDraft {
                title: "Done target".to_string(),
                status: "todo".to_string(),
                ..task_draft("")
            },
            None,
        )
        .await
        .unwrap();
    let selected = selected.unwrap();
    let task_id = store.tasks[selected].task.id.clone();

    store.show_view(TaskView::Todo).await.unwrap();
    let selected = store
        .tasks
        .iter()
        .position(|item| item.task.id == task_id)
        .unwrap();

    let result = store
        .update_status_preserving_task(Some(selected), "done")
        .await
        .unwrap()
        .unwrap();

    assert_eq!(result.selected, Some(selected));
    assert_eq!(store.tasks[selected].task.id, task_id);
    assert_eq!(store.tasks[selected].task.status, TaskStatus::Done);
    assert_eq!(store.counts.done, 1);
}
