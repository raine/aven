use super::*;

#[tokio::test]
async fn undo_returns_none_when_empty() {
    let mut store = test_store().await;
    assert!(store.undo_last(None).await.unwrap().is_none());
}

#[tokio::test]
async fn undo_title_edit_expires_on_store_restart() {
    let (dir, pool, mut store) = test_store_with_pool().await;
    let (task_id, selected) = create_selected_task(&mut store, "Before").await;
    store
        .update_title(Some(selected), "After".to_string())
        .await
        .unwrap();
    assert_eq!(store.tasks[selected].task.title, "After");

    let mut restarted = TuiStore::new(
        aven_core::db::Database::open(&dir.path().join("test.db"))
            .await
            .unwrap(),
        crate::workspaces::Workspace::default(),
    )
    .await
    .unwrap();
    assert!(restarted.undo_last(None).await.unwrap().is_none());
    let index = restarted
        .tasks
        .iter()
        .position(|item| item.task.id == task_id)
        .unwrap();
    assert_eq!(restarted.tasks[index].task.title, "After");
}

#[tokio::test]
async fn store_startup_clears_pending_undo_but_preserves_consumed_entries() {
    let (dir, pool, mut store) = test_store_with_pool().await;
    let (_, selected) = create_selected_task(&mut store, "Before").await;
    let workspace_id = store.active_workspace.id.clone();

    store
        .update_title(Some(selected), "After".to_string())
        .await
        .unwrap();
    store.undo_last(None).await.unwrap().unwrap();

    let consumed_before = consumed_undo_count(&pool, &workspace_id).await;
    assert_eq!(consumed_before, 1);
    assert_eq!(pending_undo_count(&pool, &workspace_id).await, 1);

    drop(store);
    let _restarted = TuiStore::new(
        aven_core::db::Database::open(&dir.path().join("test.db"))
            .await
            .unwrap(),
        crate::workspaces::Workspace::default(),
    )
    .await
    .unwrap();

    assert_eq!(pending_undo_count(&pool, &workspace_id).await, 0);
    assert_eq!(
        consumed_undo_count(&pool, &workspace_id).await,
        consumed_before
    );
}

#[tokio::test]
async fn undo_guard_blocks_stale_task_field() {
    let (dir, pool, mut store) = test_store_with_pool().await;
    let (task_id, selected) = create_selected_task(&mut store, "Before").await;
    store
        .update_title(Some(selected), "After".to_string())
        .await
        .unwrap();

    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("UPDATE tasks SET title = ? WHERE id = ?")
        .bind("Changed")
        .bind(&task_id)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);

    let error = store.undo_last(None).await.unwrap_err();
    assert!(error.to_string().contains("undo-state-changed"));
    store.refresh(Some(&task_id)).await.unwrap();
    let index = store
        .tasks
        .iter()
        .position(|item| item.task.id == task_id)
        .unwrap();
    assert_eq!(store.tasks[index].task.title, "Changed");
}

#[tokio::test]
async fn undo_cancel_restores_prior_queue_activity() {
    let (dir, pool, mut store) = test_store_with_pool().await;
    let (task_id, selected) = create_selected_task(&mut store, "Preserve idle").await;
    let prior_activity = "1970-01-01T00:00:00Z";
    set_task_timestamps(
        &pool,
        &store.active_workspace.id,
        &task_id,
        prior_activity,
        None,
    )
    .await;
    store.refresh(Some(&task_id)).await.unwrap();

    store
        .update_status(Some(selected), "canceled")
        .await
        .unwrap();

    store.undo_last(None).await.unwrap().unwrap();
    let task = store
        .tasks
        .iter()
        .find(|item| item.task.id == task_id)
        .unwrap();
    assert_eq!(task.task.status, TaskStatus::Inbox);
    assert_eq!(task.task.queue_activity_at, prior_activity);
}

#[tokio::test]
async fn undo_status_keeps_later_queue_activity() {
    let (dir, pool, mut store) = test_store_with_pool().await;
    let (task_id, selected) = create_selected_task(&mut store, "Keep later activity").await;
    set_task_timestamps(
        &pool,
        &store.active_workspace.id,
        &task_id,
        "1970-01-01T00:00:00Z",
        None,
    )
    .await;
    store.refresh(Some(&task_id)).await.unwrap();
    store.update_status(Some(selected), "todo").await.unwrap();

    let later_activity = "2999-01-01T00:00:00Z";
    set_task_timestamps(
        &pool,
        &store.active_workspace.id,
        &task_id,
        later_activity,
        None,
    )
    .await;

    store.undo_last(None).await.unwrap().unwrap();
    let task = store
        .tasks
        .iter()
        .find(|item| item.task.id == task_id)
        .unwrap();
    assert_eq!(task.task.status, TaskStatus::Inbox);
    assert_eq!(task.task.queue_activity_at, later_activity);
}

#[tokio::test]
async fn single_task_undo_presentation_keeps_display_ref() {
    let mut store = test_store().await;
    let (_task_id, selected) = create_selected_task(&mut store, "Single summary").await;
    let display_ref = store.tasks[selected].display_ref.clone();

    store.update_status(Some(selected), "todo").await.unwrap();
    let undo = store.undo_last(None).await.unwrap().unwrap();

    assert_eq!(
        undo.message,
        format!("undid status change on {display_ref}")
    );
}

#[tokio::test]
async fn partially_unchanged_batch_undo_presentation_uses_changed_scope() {
    let mut store = test_store().await;
    let (first_id, first) = create_selected_task(&mut store, "Already changed").await;
    let (second_id, _) = create_selected_task(&mut store, "Needs change").await;
    store.update_status(Some(first), "todo").await.unwrap();

    store
        .update_status_for_tasks(None, &[first_id, second_id.clone()], "todo")
        .await
        .unwrap();
    let expected = store.latest_undo.as_ref().unwrap().phrase.clone();
    let undo = store.undo_last(None).await.unwrap().unwrap();

    assert_eq!(undo.message, format!("undid {expected}"));
}

#[tokio::test]
async fn undo_presentation_resolves_task_hidden_by_mutation() {
    let mut store = test_store().await;
    store.view_state.query = TaskQuery::Inbox;
    store.refresh(None).await.unwrap();
    let (_task_id, selected) = create_selected_task(&mut store, "Leaves inbox").await;
    let display_ref = store.tasks[selected].display_ref.clone();

    store.update_status(Some(selected), "todo").await.unwrap();

    assert!(
        store
            .tasks
            .iter()
            .all(|item| item.display_ref != display_ref)
    );
    let expected = format!("status change on {display_ref}");
    assert_eq!(
        store.latest_undo.as_ref().map(|undo| undo.phrase.as_str()),
        Some(expected.as_str())
    );
}

#[tokio::test]
async fn stacked_undo_reports_consumed_entry_and_exposes_next_entry() {
    let mut store = test_store().await;
    let (_task_id, selected) = create_selected_task(&mut store, "Stacked undo").await;
    let display_ref = store.tasks[selected].display_ref.clone();
    store.update_status(Some(selected), "todo").await.unwrap();
    store
        .set_exact_priority(Some(selected), "high")
        .await
        .unwrap();

    let undo = store.undo_last(None).await.unwrap().unwrap();

    assert_eq!(
        undo.message,
        format!("undid priority change on {display_ref}")
    );
    let next_phrase = format!("status change on {display_ref}");
    assert_eq!(
        store.latest_undo.as_ref().map(|undo| undo.phrase.as_str()),
        Some(next_phrase.as_str())
    );
}

#[tokio::test]
async fn undo_delete_restores_task() {
    let mut store = test_store().await;
    let (task_id, selected) = create_selected_task(&mut store, "Keep").await;
    let display_ref = store.tasks[selected].display_ref.clone();
    let delete = store
        .update_deleted(Some(selected), true)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(delete.message, format!("deleted {display_ref}"));
    assert!(!store.view_state.filter_modifiers.include_deleted);
    store.refresh(Some(&task_id)).await.unwrap();
    assert!(store.tasks.iter().all(|item| item.task.id != task_id));
    store.view_state.filter_modifiers.include_deleted = true;
    store.refresh(Some(&task_id)).await.unwrap();
    let index = store
        .tasks
        .iter()
        .position(|item| item.task.id == task_id)
        .unwrap();
    assert!(store.tasks[index].task.deleted);

    store.undo_last(None).await.unwrap();
    store.refresh(Some(&task_id)).await.unwrap();
    let index = store
        .tasks
        .iter()
        .position(|item| item.task.id == task_id)
        .unwrap();
    assert!(!store.tasks[index].task.deleted);
}

#[tokio::test]
async fn repeated_delete_does_not_add_noop_undo_entry() {
    let (dir, pool, mut store) = test_store_with_pool().await;
    let (task_id, selected) = create_selected_task(&mut store, "Keep once").await;
    store.view_state.filter_modifiers.include_deleted = true;
    store.update_deleted(Some(selected), true).await.unwrap();
    let workspace_id = store.active_workspace.id.clone();
    let undo_count_after_delete = pending_undo_count(&pool, &workspace_id).await;
    let index = store
        .tasks
        .iter()
        .position(|item| item.task.id == task_id)
        .unwrap();
    store.update_deleted(Some(index), true).await.unwrap();

    assert!(store.new_undo_entry_id.is_none());
    assert_eq!(
        pending_undo_count(&pool, &workspace_id).await,
        undo_count_after_delete
    );
    store.undo_last(None).await.unwrap().unwrap();
    store.refresh(Some(&task_id)).await.unwrap();
    let index = store
        .tasks
        .iter()
        .position(|item| item.task.id == task_id)
        .unwrap();
    assert!(!store.tasks[index].task.deleted);
}

#[tokio::test]
async fn noop_task_field_updates_do_not_add_undo_entries() {
    let (dir, pool, mut store) = test_store_with_pool().await;
    store.create_project("Side".to_string()).await.unwrap();
    store.create_label("bug".to_string()).await.unwrap();
    let (task_id, selected) = create_selected_task(&mut store, "Noop fields").await;
    store
        .update_title(Some(selected), "Changed".to_string())
        .await
        .unwrap();
    store
        .update_description(Some(selected), "details".to_string())
        .await
        .unwrap();
    store
        .set_exact_priority(Some(selected), "high")
        .await
        .unwrap();
    store
        .update_project(Some(selected), "side".to_string())
        .await
        .unwrap();
    store
        .update_labels(Some(selected), vec!["bug".to_string()])
        .await
        .unwrap();
    store.update_status(Some(selected), "todo").await.unwrap();
    let workspace_id = store.active_workspace.id.clone();
    let undo_count_after_changes = pending_undo_count(&pool, &workspace_id).await;
    let index = store
        .tasks
        .iter()
        .position(|item| item.task.id == task_id)
        .unwrap();

    store.update_status(Some(index), "todo").await.unwrap();
    store.set_exact_priority(Some(index), "high").await.unwrap();
    store
        .update_title(Some(index), "Changed".to_string())
        .await
        .unwrap();
    store
        .update_description(Some(index), "details".to_string())
        .await
        .unwrap();
    store
        .update_project(Some(index), "side".to_string())
        .await
        .unwrap();
    store
        .update_labels(Some(index), vec!["bug".to_string()])
        .await
        .unwrap();

    assert_eq!(
        pending_undo_count(&pool, &workspace_id).await,
        undo_count_after_changes
    );
}

#[tokio::test]
async fn duplicate_project_and_label_do_not_add_undo_entries() {
    let (dir, pool, mut store) = test_store_with_pool().await;
    store.create_project("Side".to_string()).await.unwrap();
    store.create_label("bug".to_string()).await.unwrap();
    let workspace_id = store.active_workspace.id.clone();
    let undo_count_after_creates = pending_undo_count(&pool, &workspace_id).await;

    store.create_project("Side".to_string()).await.unwrap();
    store.create_label("bug".to_string()).await.unwrap();

    assert_eq!(
        pending_undo_count(&pool, &workspace_id).await,
        undo_count_after_creates
    );
}

#[tokio::test]
async fn undo_restore_redeletes_task() {
    let mut store = test_store().await;
    let (task_id, selected) = create_selected_task(&mut store, "Gone").await;
    store.update_deleted(Some(selected), true).await.unwrap();
    store.view_state.filter_modifiers.include_deleted = true;
    store.refresh(Some(&task_id)).await.unwrap();
    let index = store
        .tasks
        .iter()
        .position(|item| item.task.id == task_id)
        .unwrap();
    let display_ref = store.tasks[index].display_ref.clone();
    let restore = store
        .update_deleted(Some(index), false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(restore.message, format!("restored {display_ref}"));

    store.undo_last(None).await.unwrap();
    store.view_state.filter_modifiers.include_deleted = true;
    store.refresh(Some(&task_id)).await.unwrap();
    let index = store
        .tasks
        .iter()
        .position(|item| item.task.id == task_id)
        .unwrap();
    assert!(store.tasks[index].task.deleted);
}

#[tokio::test]
async fn repeated_task_creations_undo_independently() {
    let (_dir, pool, mut store) = test_store_with_pool().await;
    let workspace_id = store.active_workspace.id.clone();
    store
        .create_task(task_draft("First rapid task"), None)
        .await
        .unwrap();
    store
        .create_task(task_draft("Second rapid task"), None)
        .await
        .unwrap();
    assert_eq!(pending_undo_count(&pool, &workspace_id).await, 2);

    store.undo_last(None).await.unwrap();
    let first_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM tasks WHERE title = 'First rapid task'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let second_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM tasks WHERE title = 'Second rapid task'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((first_count, second_count), (1, 0));

    store.undo_last(None).await.unwrap();
    let first_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM tasks WHERE title = 'First rapid task'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(first_count, 0);
}

#[tokio::test]
async fn undo_create_task_removes_local_unsynced_task() {
    let (dir, pool, mut store) = test_store_with_pool().await;
    let (task_id, _) = create_selected_task(&mut store, "Temporary").await;
    store.undo_last(None).await.unwrap();

    let mut conn = pool.acquire().await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM tasks WHERE id = ?")
        .bind(&task_id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn undo_create_task_removes_created_labels() {
    let (_dir, pool, mut store) = test_store_with_pool().await;
    let mut draft = task_draft("Temporary labeled task");
    draft.labels = vec!["undo-created-task-label".to_string()];

    let (_, selected) = store.create_task(draft, None).await.unwrap();
    let task_id = store.tasks[selected.unwrap()].task.id.clone();

    store.undo_last(None).await.unwrap();

    let task_count: i64 = sqlx::query_scalar("SELECT count(*) FROM tasks WHERE id = ?")
        .bind(&task_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let label_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM labels WHERE name = 'undo-created-task-label'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let create_change_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM changes
         WHERE entity_type = 'label' AND entity_id = 'undo-created-task-label'
         AND op_type = 'create_label'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((task_count, label_count, create_change_count), (0, 0, 0));
}

#[tokio::test]
async fn undo_new_label_assignment_removes_unreferenced_label() {
    let (_dir, pool, mut store) = test_store_with_pool().await;
    let (task_id, selected) = create_selected_task(&mut store, "New label assignment").await;
    let label = "undo-created-assignment-label";

    store
        .update_labels_for_tasks(
            Some(selected),
            std::slice::from_ref(&task_id),
            vec![label.to_string()],
            Vec::new(),
        )
        .await
        .unwrap();
    store.undo_last(None).await.unwrap();

    let task_label_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM task_labels WHERE task_id = ? AND label = ?")
            .bind(&task_id)
            .bind(label)
            .fetch_one(&pool)
            .await
            .unwrap();
    let label_count: i64 = sqlx::query_scalar("SELECT count(*) FROM labels WHERE name = ?")
        .bind(label)
        .fetch_one(&pool)
        .await
        .unwrap();
    let create_change_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM changes
         WHERE entity_type = 'label' AND entity_id = ? AND op_type = 'create_label'",
    )
    .bind(label)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        (task_label_count, label_count, create_change_count),
        (0, 0, 0)
    );
}

#[tokio::test]
async fn undo_batch_new_label_assignment_removes_shared_label() {
    let (_dir, pool, mut store) = test_store_with_pool().await;
    let (first_id, _) = create_selected_task(&mut store, "First shared label").await;
    let (second_id, _) = create_selected_task(&mut store, "Second shared label").await;
    let label = "undo-created-batch-label";

    store
        .update_labels_for_tasks(
            None,
            &[first_id.clone(), second_id.clone()],
            vec![label.to_string()],
            Vec::new(),
        )
        .await
        .unwrap();
    store.undo_last(None).await.unwrap();

    let task_label_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM task_labels WHERE label = ?")
            .bind(label)
            .fetch_one(&pool)
            .await
            .unwrap();
    let label_count: i64 = sqlx::query_scalar("SELECT count(*) FROM labels WHERE name = ?")
        .bind(label)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!((task_label_count, label_count), (0, 0));
    assert!(store.tasks.iter().any(|item| item.task.id == first_id));
    assert!(store.tasks.iter().any(|item| item.task.id == second_id));
}

#[tokio::test]
async fn undo_atomic_attachment_task_removes_all_database_rows() {
    let (dir, pool, mut store) = test_store_with_pool().await;
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image::RgbaImage::new(2, 1))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    let pending = crate::tui::authoring::PendingTaskAttachment::new(
        "ATTACHMENT000001".to_string(),
        crate::operations::AttachmentAddInput {
            filename: Some("image.png".to_string()),
            alt_text: None,
            declared_media_type: Some("image/png".to_string()),
            bytes: bytes.into_inner(),
            optimization_policy:
                crate::attachments::optimization::ImageOptimizationPolicy::Preserve,
            dedupe_existing: false,
        },
    );
    store
        .create_task_with_attachments(
            task_draft("Temporary attachment task"),
            None,
            &dir.path().join("blobs"),
            crate::attachments::lifecycle::LifecyclePolicy::default(),
            vec![pending],
        )
        .await
        .unwrap();
    let task_id = store.tasks[0].task.id.clone();
    let mut conn = pool.acquire().await.unwrap();
    let undo_count: i64 = sqlx::query_scalar("SELECT count(*) FROM tui_undo_entries")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(undo_count, 1);
    drop(conn);

    store.undo_last(None).await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let task_count: i64 = sqlx::query_scalar("SELECT count(*) FROM tasks WHERE id = ?")
        .bind(&task_id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    let attachment_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM task_attachments WHERE task_id = ?")
            .bind(&task_id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    let change_count: i64 = sqlx::query_scalar("SELECT count(*) FROM changes WHERE entity_id = ?")
        .bind(&task_id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!((task_count, attachment_count, change_count), (0, 0, 0));
}

#[tokio::test]
async fn undo_labels_uses_set_comparison() {
    let mut store = test_store().await;
    store.create_label("bug".to_string()).await.unwrap();
    store.create_label("docs".to_string()).await.unwrap();
    let (task_id, selected) = create_selected_task(&mut store, "Labels").await;
    store
        .update_labels(Some(selected), vec!["bug".to_string()])
        .await
        .unwrap();
    store
        .update_labels(Some(selected), vec!["docs".to_string()])
        .await
        .unwrap();
    let index = store
        .tasks
        .iter()
        .position(|item| item.task.id == task_id)
        .unwrap();
    assert_eq!(store.tasks[index].labels, vec!["docs".to_string()]);

    store.undo_last(None).await.unwrap();
    store.refresh(Some(&task_id)).await.unwrap();
    let index = store
        .tasks
        .iter()
        .position(|item| item.task.id == task_id)
        .unwrap();
    assert_eq!(store.tasks[index].labels, vec!["bug".to_string()]);
}

#[tokio::test]
async fn undo_note_create_deletes_only_unsynced_note() {
    let (dir, pool, mut store) = test_store_with_pool().await;
    let (task_id, selected) = create_selected_task(&mut store, "Notes").await;
    let note_id = store
        .add_note_to_task(&task_id, "hello".to_string())
        .await
        .unwrap();
    store.undo_last(None).await.unwrap();

    let mut conn = pool.acquire().await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM notes WHERE id = ?")
        .bind(&note_id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(count, 0);
    drop(conn);
    store.refresh(Some(&task_id)).await.unwrap();
    assert_eq!(store.tasks[selected].task.title, "Notes");
}

#[tokio::test]
async fn undo_project_create_fails_when_referenced_or_synced() {
    let (dir, pool, mut store) = test_store_with_pool().await;
    store.create_project("Side".to_string()).await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let workspace_id = store.active_workspace.id.clone();
    let project_key = store
        .projects
        .iter()
        .find(|project| project.key == "side")
        .unwrap()
        .key
        .clone();
    sqlx::query(
        "INSERT INTO tasks(workspace_id, id, title, description, project_id, status, priority, created_at, updated_at)
         VALUES (?, ?, 'Uses project', '', (SELECT id FROM projects WHERE workspace_id = ? AND key = ?), 'inbox', 'none', ?, ?)",
    )
    .bind(&workspace_id)
    .bind(crate::ids::new_id())
    .bind(&workspace_id)
    .bind(&project_key)
    .bind(crate::ids::now())
    .bind(crate::ids::now())
    .execute(&mut *conn)
    .await
    .unwrap();
    drop(conn);

    let error = store.undo_last(None).await.unwrap_err();
    assert!(error.to_string().contains("undo-state-changed"));
    store.refresh(None).await.unwrap();
    assert!(store.projects.iter().any(|project| project.key == "side"));
}

#[tokio::test]
async fn undo_label_create_fails_when_referenced_or_synced() {
    let (dir, pool, mut store) = test_store_with_pool().await;
    store.create_label("shared".to_string()).await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let workspace_id = store.active_workspace.id.clone();
    sqlx::query("INSERT INTO task_labels(workspace_id, task_id, label) VALUES (?, ?, 'shared')")
        .bind(&workspace_id)
        .bind(crate::ids::new_id())
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);

    let error = store.undo_last(None).await.unwrap_err();
    assert!(error.to_string().contains("undo-state-changed"));
    let mut conn = pool.acquire().await.unwrap();
    store.labels = list_labels_in_workspace(&mut conn, &store.active_workspace.id, None)
        .await
        .unwrap();
    assert!(store.labels.iter().any(|label| label == "shared"));
}

#[tokio::test]
async fn undo_label_create_fails_when_referenced_by_recurrence() {
    let (_dir, pool, mut store) = test_store_with_pool().await;
    let label = "recurrence-shared";
    store.create_label(label.to_string()).await.unwrap();

    let mut conn = pool.acquire().await.unwrap();
    sqlx::query(
        "INSERT INTO recurrence_series_labels(workspace_id, series_id, label)
         VALUES (?, ?, ?)",
    )
    .bind(&store.active_workspace.id)
    .bind(crate::ids::new_id())
    .bind(label)
    .execute(&mut *conn)
    .await
    .unwrap();
    drop(conn);

    let error = store.undo_last(None).await.unwrap_err();
    assert!(error.to_string().contains("undo-state-changed"));
    let label_count: i64 = sqlx::query_scalar("SELECT count(*) FROM labels WHERE name = ?")
        .bind(label)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(label_count, 1);
}

#[tokio::test]
async fn undo_conflict_resolution_restores_unresolved_conflict() {
    let (dir, pool, mut store) = test_store_with_pool().await;
    let (task_id, selected) = create_selected_task(&mut store, "Before").await;
    let display_ref = store.tasks[selected].display_ref.clone();

    seed_title_conflict(&pool, &task_id).await;
    store.refresh(Some(&task_id)).await.unwrap();

    store
        .resolve_conflict_value(
            ConflictTarget {
                task_id: task_id.clone(),
                recurrence_series_id: None,
                display_ref,
                field: "title".to_string(),
                variant_a: "a".to_string(),
                local_value: "local title".to_string(),
                variant_b: "b".to_string(),
                remote_value: "remote title".to_string(),
            },
            "local title".to_string(),
        )
        .await
        .unwrap();
    assert_eq!(store.tasks[selected].task.title, "local title");
    assert!(!store.tasks[selected].has_conflict);

    store.undo_last(None).await.unwrap();
    store.refresh(Some(&task_id)).await.unwrap();
    assert_eq!(store.tasks[selected].task.title, "Before");
    assert!(store.tasks[selected].has_conflict);
}

#[tokio::test]
async fn undo_project_conflict_resolution_uses_project_ids() {
    let (dir, pool, mut store) = test_store_with_pool().await;
    store.create_project("Ops".to_string()).await.unwrap();
    let (task_id, selected) = create_selected_task(&mut store, "Before").await;
    let display_ref = store.tasks[selected].display_ref.clone();
    let workspace_id = store.active_workspace.id.clone();

    let mut conn = pool.acquire().await.unwrap();
    let app_id: String =
        sqlx::query_scalar("SELECT id FROM projects WHERE workspace_id = ? AND key = 'aven'")
            .bind(&workspace_id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    let ops_id: String =
        sqlx::query_scalar("SELECT id FROM projects WHERE workspace_id = ? AND key = 'ops'")
            .bind(&workspace_id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    sqlx::query(
        "INSERT INTO conflicts(workspace_id, task_id, field, base_version, local_value,
         remote_value, local_change_id, remote_change_id, variant_a, variant_b, created_at,
         resolved)
         VALUES (?, ?, 'project', NULL, ?, ?, NULL, ?, 'a', 'b', ?, 0)",
    )
    .bind(&workspace_id)
    .bind(&task_id)
    .bind(&app_id)
    .bind(&ops_id)
    .bind(crate::ids::new_id())
    .bind(crate::ids::now())
    .execute(&mut *conn)
    .await
    .unwrap();
    drop(conn);
    store.refresh(Some(&task_id)).await.unwrap();

    store
        .resolve_conflict_value(
            ConflictTarget {
                task_id: task_id.clone(),
                recurrence_series_id: None,
                display_ref,
                field: "project".to_string(),
                variant_a: "a".to_string(),
                local_value: app_id,
                variant_b: "b".to_string(),
                remote_value: ops_id,
            },
            "ops".to_string(),
        )
        .await
        .unwrap();
    assert_eq!(store.tasks[selected].task.project_key, "ops");
    assert!(!store.tasks[selected].has_conflict);

    store.undo_last(None).await.unwrap();
    store.refresh(Some(&task_id)).await.unwrap();
    assert_eq!(store.tasks[selected].task.project_key, "aven");
    assert!(store.tasks[selected].has_conflict);
}

#[tokio::test]
async fn undo_is_workspace_scoped_within_running_store() {
    let (dir, pool, mut store) = test_store_with_pool().await;
    let (task_id, selected) = create_selected_task(&mut store, "Scoped").await;
    store
        .update_title(Some(selected), "Changed".to_string())
        .await
        .unwrap();

    let mut conn = pool.acquire().await.unwrap();
    let other = crate::workspaces::create_workspace(&mut conn, "other")
        .await
        .unwrap();
    drop(conn);
    store.switch_workspace(other.key.clone()).await.unwrap();
    assert!(store.undo_last(None).await.unwrap().is_none());

    store.switch_workspace("default".to_string()).await.unwrap();
    store.undo_last(None).await.unwrap().unwrap();
    store.refresh(Some(&task_id)).await.unwrap();
    let index = store
        .tasks
        .iter()
        .position(|item| item.task.id == task_id)
        .unwrap();
    assert_eq!(store.tasks[index].task.title, "Scoped");
}

#[tokio::test]
async fn undo_consumes_entry_once() {
    let mut store = test_store().await;
    let (_, selected) = create_selected_task(&mut store, "Once").await;
    store
        .update_title(Some(selected), "Changed".to_string())
        .await
        .unwrap();
    store.undo_last(None).await.unwrap().unwrap();
    store.undo_last(None).await.unwrap().unwrap();
    assert!(store.undo_last(None).await.unwrap().is_none());
}

#[tokio::test]
async fn undo_skips_noop_status_before_previous_mutation() {
    let (dir, pool, mut store) = test_store_with_pool().await;
    let (task_id, selected) = create_selected_task(&mut store, "Noop status").await;
    store.update_status(Some(selected), "todo").await.unwrap();
    let workspace_id = store.active_workspace.id.clone();
    let undo_count_after_change = pending_undo_count(&pool, &workspace_id).await;
    store.update_status(Some(selected), "todo").await.unwrap();
    assert_eq!(
        pending_undo_count(&pool, &workspace_id).await,
        undo_count_after_change
    );

    store.undo_last(None).await.unwrap().unwrap();
    store.refresh(Some(&task_id)).await.unwrap();
    let index = store
        .tasks
        .iter()
        .position(|item| item.task.id == task_id)
        .unwrap();
    assert_eq!(store.tasks[index].task.status, TaskStatus::Inbox);
    assert_eq!(pending_undo_count(&pool, &workspace_id).await, 1);
}

#[tokio::test]
async fn update_labels_for_tasks_records_single_undo_payload() {
    let mut store = test_store().await;
    store.create_label("bug".to_string()).await.unwrap();
    store.create_label("docs".to_string()).await.unwrap();
    let (first_id, first) = create_selected_task(&mut store, "First").await;
    store
        .update_labels(Some(first), vec!["bug".to_string()])
        .await
        .unwrap();
    let (second_id, second) = create_selected_task(&mut store, "Second").await;
    store
        .update_labels(Some(second), vec!["docs".to_string()])
        .await
        .unwrap();

    store
        .update_labels_for_tasks(
            None,
            &[first_id.clone(), second_id.clone()],
            vec!["bug".to_string(), "docs".to_string()],
            Vec::new(),
        )
        .await
        .unwrap();

    store.undo_last(None).await.unwrap().unwrap();
    store.refresh(None).await.unwrap();
    let first = store
        .tasks
        .iter()
        .find(|item| item.task.id == first_id)
        .unwrap();
    let second = store
        .tasks
        .iter()
        .find(|item| item.task.id == second_id)
        .unwrap();
    assert_eq!(first.labels, vec!["bug".to_string()]);
    assert_eq!(second.labels, vec!["docs".to_string()]);
}
