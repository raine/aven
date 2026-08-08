use super::*;

#[tokio::test]
async fn update_status_for_tasks_sets_status_on_each_marked_task() {
    let mut store = test_store().await;
    let (first_id, _) = create_selected_task(&mut store, "First").await;
    let (second_id, _) = create_selected_task(&mut store, "Second").await;
    let task_ids = vec![first_id.clone(), second_id.clone()];

    let outcome = store
        .update_status_for_tasks(None, &task_ids, "todo")
        .await
        .unwrap()
        .unwrap();

    assert_eq!(outcome.message, "set status on 2 tasks");
    for task_id in task_ids {
        let item = store
            .tasks
            .iter()
            .find(|item| item.task.id == task_id)
            .unwrap();
        assert_eq!(item.task.status, TaskStatus::Todo);
    }
}

#[tokio::test]
async fn set_exact_priority_for_tasks_sets_priority_on_each_marked_task() {
    let mut store = test_store().await;
    let (first_id, _) = create_selected_task(&mut store, "First").await;
    let (second_id, _) = create_selected_task(&mut store, "Second").await;
    let task_ids = vec![first_id.clone(), second_id.clone()];

    let outcome = store
        .set_exact_priority_for_tasks(None, &task_ids, "high")
        .await
        .unwrap()
        .unwrap();

    assert_eq!(outcome.message, "set priority on 2 tasks");
    for task_id in task_ids {
        let item = store
            .tasks
            .iter()
            .find(|item| item.task.id == task_id)
            .unwrap();
        assert_eq!(item.task.priority, TaskPriority::High);
    }
}

#[tokio::test]
async fn set_exact_priority_for_tasks_rolls_back_on_later_conflict() {
    let (_dir, pool, mut store) = test_store_with_pool().await;
    let (first_id, _) = create_selected_task(&mut store, "First atomic").await;
    let (second_id, _) = create_selected_task(&mut store, "Second atomic").await;
    seed_field_conflict_database(&store.database, &second_id, "priority").await;
    let undo_before = pending_undo_count(&pool, &store.active_workspace.id).await;

    let error = store
        .set_exact_priority_for_tasks(None, &[first_id.clone(), second_id], "high")
        .await
        .unwrap_err();

    assert!(error.to_string().contains("conflicted-field"));
    store.refresh(None).await.unwrap();
    let first = store
        .tasks
        .iter()
        .find(|item| item.task.id == first_id)
        .unwrap();
    assert_eq!(first.task.priority, TaskPriority::None);
    assert_eq!(
        pending_undo_count(&pool, &store.active_workspace.id).await,
        undo_before
    );
}

#[tokio::test]
async fn update_project_for_tasks_sets_project_on_each_marked_task() {
    let mut store = test_store().await;
    store
        .create_project("Mobile App".to_string())
        .await
        .unwrap();
    let (first_id, _) = create_selected_task(&mut store, "First").await;
    let (second_id, _) = create_selected_task(&mut store, "Second").await;
    let task_ids = vec![first_id.clone(), second_id.clone()];

    let outcome = store
        .update_project_for_tasks(None, &task_ids, "mobile-app".to_string())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(outcome.message, "set project on 2 tasks");
    for task_id in task_ids {
        let item = store
            .tasks
            .iter()
            .find(|item| item.task.id == task_id)
            .unwrap();
        assert_eq!(item.task.project_key, "mobile-app");
    }
}

#[tokio::test]
async fn batch_delete_hides_rows_and_restores_a_clamped_anchor() {
    let mut store = test_store().await;
    let (first_id, _) = create_selected_task(&mut store, "First").await;
    let (second_id, _) = create_selected_task(&mut store, "Second").await;
    create_selected_task(&mut store, "Third").await;
    let task_ids = vec![first_id, second_id];
    let anchor = store
        .tasks
        .iter()
        .position(|item| item.task.id == task_ids[1])
        .unwrap();

    let outcome = store
        .update_deleted_for_tasks(Some(anchor), &task_ids, true)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(outcome.message, "deleted 2 tasks");
    assert_eq!(store.tasks.len(), 1);
    assert!(
        store
            .tasks
            .iter()
            .all(|item| !task_ids.contains(&item.task.id))
    );
    assert_eq!(outcome.selected, Some(0));
    assert_eq!(store.counts.inbox, 1);
    let inbox = store
        .sidebar_entries
        .iter()
        .find(|entry| entry.label == "Inbox")
        .unwrap();
    assert_eq!(inbox.count, 1);

    let undone = store.undo_last(outcome.selected).await.unwrap().unwrap();
    assert_eq!(store.tasks.len(), 3);
    assert_eq!(store.counts.inbox, 3);
    assert!(task_ids.iter().all(|task_id| {
        store
            .tasks
            .iter()
            .any(|item| &item.task.id == task_id && !item.task.deleted)
    }));
    assert!(undone.selected.is_some());
}

#[tokio::test]
async fn single_delete_preserves_row_index_counts_and_undo() {
    let mut store = test_store().await;
    create_selected_task(&mut store, "First").await;
    let (second_id, _) = create_selected_task(&mut store, "Second").await;
    create_selected_task(&mut store, "Third").await;
    let selected = store
        .tasks
        .iter()
        .position(|item| item.task.id == second_id)
        .unwrap();

    let outcome = store
        .update_deleted(Some(selected), true)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(outcome.selected, Some(selected));
    assert_eq!(store.tasks.len(), 3);
    assert_eq!(store.tasks[selected].task.id, second_id);
    assert!(store.tasks[selected].task.deleted);
    assert_eq!(store.counts.inbox, 2);
    let inbox = store
        .sidebar_entries
        .iter()
        .find(|entry| entry.label == "Inbox")
        .unwrap();
    assert_eq!(inbox.count, 2);

    let undone = store.undo_last(outcome.selected).await.unwrap().unwrap();
    assert_eq!(undone.selected, Some(selected));
    assert_eq!(store.tasks.len(), 3);
    assert_eq!(store.tasks[selected].task.id, second_id);
    assert!(!store.tasks[selected].task.deleted);
    assert_eq!(store.counts.inbox, 3);
}

#[tokio::test]
async fn successive_single_deletes_preserve_every_deleted_row() {
    let (_dir, pool, mut store) = test_store_with_pool().await;
    let (first_id, _) = create_selected_task(&mut store, "First").await;
    let (second_id, _) = create_selected_task(&mut store, "Second").await;

    let first_index = store
        .tasks
        .iter()
        .position(|item| item.task.id == first_id)
        .unwrap();
    store
        .update_deleted(Some(first_index), true)
        .await
        .unwrap()
        .unwrap();
    let second_index = store
        .tasks
        .iter()
        .position(|item| item.task.id == second_id)
        .unwrap();
    store
        .update_deleted(Some(second_index), true)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(store.tasks.len(), 2);
    assert!(store.tasks.iter().all(|item| item.task.deleted));
    let persisted: i64 =
        sqlx::query_scalar("SELECT count(*) FROM tasks WHERE id IN (?, ?) AND deleted = 1")
            .bind(&first_id)
            .bind(&second_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(persisted, 2);
}

#[tokio::test]
async fn update_labels_for_tasks_sets_labels_on_each_marked_task() {
    let mut store = test_store().await;
    store.create_label("bug".to_string()).await.unwrap();
    store.create_label("docs".to_string()).await.unwrap();
    let (_, first_selected) = store
        .create_task(
            TaskDraft {
                metadata: Vec::new(),
                title: "First".to_string(),
                labels: vec!["bug".to_string()],
                ..task_draft("")
            },
            None,
        )
        .await
        .unwrap();
    let first_id = store.tasks[first_selected.unwrap()].task.id.clone();
    let (_, second_selected) = store
        .create_task(
            TaskDraft {
                metadata: Vec::new(),
                title: "Second".to_string(),
                labels: vec!["docs".to_string()],
                ..task_draft("")
            },
            None,
        )
        .await
        .unwrap();
    let second_id = store.tasks[second_selected.unwrap()].task.id.clone();
    let task_ids = vec![first_id.clone(), second_id.clone()];

    let outcome = store
        .update_labels_for_tasks(
            None,
            &task_ids,
            vec!["bug".to_string(), "docs".to_string()],
            Vec::new(),
        )
        .await
        .unwrap()
        .unwrap();

    assert_eq!(outcome.message, "set labels on 2 tasks");
    for task_id in task_ids {
        let item = store
            .tasks
            .iter()
            .find(|item| item.task.id == task_id)
            .unwrap();
        assert_eq!(item.labels, vec!["bug".to_string(), "docs".to_string()]);
    }
}

#[tokio::test]
async fn update_labels_for_tasks_preserves_partial_membership() {
    let mut store = test_store().await;
    store.create_label("bug".to_string()).await.unwrap();
    store.create_label("docs".to_string()).await.unwrap();
    let (first_id, first) = create_selected_task(&mut store, "First labels").await;
    store
        .update_labels(Some(first), vec!["bug".to_string()])
        .await
        .unwrap();
    let (second_id, second) = create_selected_task(&mut store, "Second labels").await;
    store
        .update_labels(Some(second), vec!["docs".to_string()])
        .await
        .unwrap();

    store
        .update_labels_for_tasks(
            None,
            &[first_id.clone(), second_id.clone()],
            vec!["bug".to_string()],
            vec!["docs".to_string()],
        )
        .await
        .unwrap();

    let labels_for = |store: &TuiStore, task_id: &TaskId| {
        store
            .tasks
            .iter()
            .find(|item| &item.task.id == task_id)
            .unwrap()
            .labels
            .clone()
    };
    assert_eq!(labels_for(&store, &first_id), vec!["bug".to_string()]);
    assert_eq!(
        labels_for(&store, &second_id),
        vec!["bug".to_string(), "docs".to_string()]
    );
}

#[tokio::test]
async fn update_labels_for_tasks_reports_unchanged_batch() {
    let mut store = test_store().await;
    store.create_label("bug".to_string()).await.unwrap();
    let (task_id, selected) = create_selected_task(&mut store, "Stable labels").await;
    store
        .update_labels(Some(selected), vec!["bug".to_string()])
        .await
        .unwrap();

    let outcome = store
        .update_labels_for_tasks(None, &[task_id], vec!["bug".to_string()], Vec::new())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        outcome.message,
        format!("unchanged {} labels", store.tasks[selected].display_ref)
    );
}
