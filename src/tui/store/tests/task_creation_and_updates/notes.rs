use super::*;

#[tokio::test]
async fn add_note_to_task_writes_note() {
    let mut store = test_store().await;
    let (_, selected) = store
        .create_task(task_draft("Note target"), None)
        .await
        .unwrap();
    let task_id = store.tasks[selected.unwrap()].task.id.clone();
    let note_id = store
        .add_note_to_task(&task_id, "hello note".to_string())
        .await
        .unwrap();
    assert!(!note_id.is_empty());
}

#[tokio::test]
async fn note_edit_and_delete_target_stable_identity() {
    let (_dir, pool, mut store) = test_store_with_pool().await;
    let (task_id, _) = create_selected_task(&mut store, "Note mutations").await;
    let note_id = store
        .add_note_to_task(&task_id, "original".to_string())
        .await
        .unwrap();

    assert_eq!(
        store
            .edit_note(&task_id, &note_id, "corrected".to_string())
            .await
            .unwrap(),
        Some(true)
    );
    let persisted: (String, String) =
        sqlx::query_as("SELECT id, body FROM notes WHERE task_id = ?")
            .bind(&task_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(persisted, (note_id.clone(), "corrected".to_string()));

    assert!(store.delete_note(&task_id, &note_id).await.unwrap());
    let persisted: i64 = sqlx::query_scalar("SELECT count(*) FROM notes WHERE task_id = ?")
        .bind(&task_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(persisted, 0);

    store.undo_last(None).await.unwrap();
    let restored: (String, String) = sqlx::query_as("SELECT id, body FROM notes WHERE task_id = ?")
        .bind(&task_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(restored, (note_id.clone(), "corrected".to_string()));

    store.undo_last(None).await.unwrap();
    let restored_body: String =
        sqlx::query_scalar("SELECT body FROM notes WHERE task_id = ? AND id = ?")
            .bind(&task_id)
            .bind(&note_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(restored_body, "original");
}

#[tokio::test]
async fn note_edit_and_delete_roll_back_when_undo_recording_fails() {
    let (_dir, pool, mut store) = test_store_with_pool().await;
    let (task_id, _) = create_selected_task(&mut store, "Note undo failure").await;
    let note_id = store
        .add_note_to_task(&task_id, "original".to_string())
        .await
        .unwrap();
    reject_undo_inserts(&pool).await;

    let edit_error = store
        .edit_note(&task_id, &note_id, "corrected".to_string())
        .await
        .unwrap_err();
    assert!(edit_error.to_string().contains("injected undo failure"));
    let persisted: String =
        sqlx::query_scalar("SELECT body FROM notes WHERE task_id = ? AND id = ?")
            .bind(&task_id)
            .bind(&note_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(persisted, "original");

    let delete_error = store.delete_note(&task_id, &note_id).await.unwrap_err();
    assert!(delete_error.to_string().contains("injected undo failure"));
    let persisted: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notes WHERE task_id = ? AND id = ? AND body = 'original'",
    )
    .bind(&task_id)
    .bind(&note_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(persisted, 1);
}

#[tokio::test]
async fn note_creation_rolls_back_when_undo_recording_fails() {
    let (_dir, pool, mut store) = test_store_with_pool().await;
    let (task_id, _) = create_selected_task(&mut store, "Note undo failure").await;
    reject_undo_inserts(&pool).await;

    let error = store
        .add_note_to_task(&task_id, "atomic note".to_string())
        .await
        .unwrap_err();

    assert!(error.to_string().contains("injected undo failure"));
    let persisted: i64 = sqlx::query_scalar("SELECT count(*) FROM notes WHERE task_id = ?")
        .bind(&task_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(persisted, 0);
}
