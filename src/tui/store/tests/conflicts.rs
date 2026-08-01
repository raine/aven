use super::*;

#[test]
fn next_conflict_flag_index_wraps_forward() {
    let flags = vec![false, true, false, true];
    assert_eq!(
        TuiStore::next_conflict_flag_index(&flags, Some(1), 1),
        Some(3)
    );
    assert_eq!(
        TuiStore::next_conflict_flag_index(&flags, Some(3), 1),
        Some(1)
    );
}

#[test]
fn next_conflict_flag_index_wraps_backward() {
    let flags = vec![false, true, false, true];
    assert_eq!(
        TuiStore::next_conflict_flag_index(&flags, Some(3), -1),
        Some(1)
    );
    assert_eq!(
        TuiStore::next_conflict_flag_index(&flags, Some(1), -1),
        Some(3)
    );
}

#[test]
fn next_conflict_flag_index_returns_none_without_conflicts() {
    let flags = vec![false, false];
    assert!(TuiStore::next_conflict_flag_index(&flags, Some(0), 1).is_none());
}

#[test]
fn next_conflict_flag_index_keeps_single_conflict() {
    let flags = vec![false, true, false];
    assert_eq!(
        TuiStore::next_conflict_flag_index(&flags, Some(1), 1),
        Some(1)
    );
}

#[tokio::test]
async fn resolve_conflict_value_updates_task_and_clears_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::test_support::open_db(&dir.path().join("test.db"))
        .await
        .unwrap();
    let mut store = TuiStore::new(
        aven_core::db::Database::open(&dir.path().join("test.db"))
            .await
            .unwrap(),
        crate::workspaces::Workspace::default(),
    )
    .await
    .unwrap();
    let (_, selected) = store.create_task(task_draft("Before"), None).await.unwrap();
    let selected = selected.unwrap();
    let task_id = store.tasks[selected].task.id.clone();
    let display_ref = store.tasks[selected].display_ref.clone();

    seed_title_conflict(&pool, &task_id).await;
    store.refresh(Some(&task_id)).await.unwrap();

    let outcome = store
        .resolve_conflict_value(
            ConflictTarget {
                task_id,
                recurrence_series_id: None,
                display_ref: display_ref.clone(),
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

    assert_eq!(
        outcome.message,
        format!("resolved {display_ref} conflict field=title")
    );
    assert_eq!(store.tasks[selected].task.title, "local title");
    assert!(!store.tasks[selected].has_conflict);
}

#[tokio::test]
async fn conflict_resolution_rolls_back_when_undo_recording_fails() {
    let (_dir, pool, mut store) = test_store_with_pool().await;
    let (task_id, selected) = create_selected_task(&mut store, "Conflict undo failure").await;
    let display_ref = store.tasks[selected].display_ref.clone();
    seed_title_conflict(&pool, &task_id).await;
    reject_undo_inserts(&pool).await;

    let error = store
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
        .unwrap_err();

    assert!(error.to_string().contains("injected undo failure"));
    let title: String = sqlx::query_scalar("SELECT title FROM tasks WHERE id = ?")
        .bind(&task_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let resolved: i64 = sqlx::query_scalar("SELECT resolved FROM conflicts WHERE task_id = ?")
        .bind(&task_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(title, "Conflict undo failure");
    assert_eq!(resolved, 0);
}

#[tokio::test]
async fn resolve_missing_conflict_leaves_task_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::test_support::open_db(&dir.path().join("test.db"))
        .await
        .unwrap();
    let mut store = TuiStore::new(
        aven_core::db::Database::open(&dir.path().join("test.db"))
            .await
            .unwrap(),
        crate::workspaces::Workspace::default(),
    )
    .await
    .unwrap();
    let (_, selected) = store
        .create_task(task_draft("Stable title"), None)
        .await
        .unwrap();
    let selected = selected.unwrap();
    let task_id = store.tasks[selected].task.id.clone();

    let error = store
        .resolve_conflict_value(
            ConflictTarget {
                task_id,
                recurrence_series_id: None,
                display_ref: "APP-1".to_string(),
                field: "title".to_string(),
                variant_a: "a".to_string(),
                local_value: "local".to_string(),
                variant_b: "b".to_string(),
                remote_value: "remote".to_string(),
            },
            "local".to_string(),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("conflict-not-found"));
    assert_eq!(store.tasks[selected].task.title, "Stable title");
}

#[tokio::test]
async fn update_title_returns_conflicted_field_error() {
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::test_support::open_db(&dir.path().join("test.db"))
        .await
        .unwrap();
    let mut store = TuiStore::new(
        aven_core::db::Database::open(&dir.path().join("test.db"))
            .await
            .unwrap(),
        crate::workspaces::Workspace::default(),
    )
    .await
    .unwrap();
    let (_, selected) = store
        .create_task(task_draft("Conflict"), None)
        .await
        .unwrap();
    let selected = selected.unwrap();
    let task_id = store.tasks[selected].task.id.clone();

    let mut conn = pool.acquire().await.unwrap();
    sqlx::query(
        "INSERT INTO conflicts(task_id, field, base_version, local_value, remote_value,
         local_change_id, remote_change_id, variant_a, variant_b, created_at, resolved)
         VALUES (?, 'title', NULL, 'local', 'remote', NULL, ?, 'a', 'b', ?, 0)",
    )
    .bind(&task_id)
    .bind(crate::ids::new_id())
    .bind(crate::ids::now())
    .execute(&mut *conn)
    .await
    .unwrap();
    drop(conn);

    let error = store
        .update_title(Some(selected), "blocked".to_string())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("conflicted-field"));
}
