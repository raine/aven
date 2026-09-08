use super::*;

async fn create_task_with_distinct_note_activity(
    store: &mut TuiStore,
    pool: &SqlitePool,
    title: &str,
) -> TaskId {
    sqlx::query("CREATE TRIGGER task_creation_activity AFTER INSERT ON tasks BEGIN UPDATE tasks SET queue_activity_at = '2000-01-01T00:00:00Z' WHERE id = NEW.id; END")
        .execute(pool)
        .await
        .unwrap();
    let (task_id, _) = create_selected_task(store, title).await;
    let snapshot = store
        .database
        .task_undo_snapshot(&store.active_workspace.id, &task_id)
        .await
        .unwrap();
    assert_eq!(snapshot.queue_activity_at, "2000-01-01T00:00:00Z");
    // Note writes retain fresh activity even when their data mutations are undone.
    sqlx::query("CREATE TRIGGER note_mutation_activity AFTER UPDATE OF queue_activity_at ON tasks WHEN NEW.queue_activity_at != '2000-01-02T00:00:00Z' BEGIN UPDATE tasks SET queue_activity_at = '2000-01-02T00:00:00Z' WHERE id = NEW.id; END")
        .execute(pool)
        .await
        .unwrap();
    task_id
}

async fn assert_note_activity_preserves_stale_task_creation_undo(
    store: &mut TuiStore,
    pool: &SqlitePool,
    task_id: &TaskId,
) {
    let workspace_id = store.active_workspace.id.clone();
    let before = store
        .database
        .task_undo_snapshot(&workspace_id, task_id)
        .await
        .unwrap();
    assert_eq!(before.queue_activity_at, "2000-01-02T00:00:00Z");
    assert!(!before.deleted);
    let changes: Vec<(String, String)> =
        sqlx::query_as("SELECT change_id, payload FROM changes ORDER BY change_id")
            .fetch_all(pool)
            .await
            .unwrap();
    let undo_entries: Vec<(String, String, Option<String>)> =
        sqlx::query_as("SELECT id, payload, undone_at FROM tui_undo_entries ORDER BY id")
            .fetch_all(pool)
            .await
            .unwrap();
    assert_eq!(
        undo_entries
            .iter()
            .filter(|entry| entry.2.is_none())
            .count(),
        1
    );

    for _ in 0..2 {
        let error = store.undo_last(None).await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains(&format!("undo-state-changed task_id={task_id} field=task"))
        );
        let after = store
            .database
            .task_undo_snapshot(&workspace_id, task_id)
            .await
            .unwrap();
        let changes_after: Vec<(String, String)> =
            sqlx::query_as("SELECT change_id, payload FROM changes ORDER BY change_id")
                .fetch_all(pool)
                .await
                .unwrap();
        let undo_entries_after: Vec<(String, String, Option<String>)> =
            sqlx::query_as("SELECT id, payload, undone_at FROM tui_undo_entries ORDER BY id")
                .fetch_all(pool)
                .await
                .unwrap();
        assert_eq!(after, before);
        assert_eq!(changes_after, changes);
        assert_eq!(undo_entries_after, undo_entries);
    }
}

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
    let task_id =
        create_task_with_distinct_note_activity(&mut store, &pool, "Note mutations").await;
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
    store.undo_last(None).await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM notes")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    assert_note_activity_preserves_stale_task_creation_undo(&mut store, &pool, &task_id).await;
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

#[tokio::test]
async fn note_creation_undo_follows_deletion_restoration() {
    let (_dir, pool, mut store) = test_store_with_pool().await;
    let task_id = create_task_with_distinct_note_activity(&mut store, &pool, "Lineage").await;
    let note_id = store
        .add_note_to_task(&task_id, "original".into())
        .await
        .unwrap();
    let original: (String, String, String, String) =
        sqlx::query_as("SELECT id, body, created_at, change_id FROM notes WHERE id = ?")
            .bind(&note_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    store.delete_note(&task_id, &note_id).await.unwrap();
    let history: Vec<(String, String)> =
        sqlx::query_as("SELECT change_id, payload FROM changes ORDER BY change_id")
            .fetch_all(&pool)
            .await
            .unwrap();
    store.undo_last(None).await.unwrap();
    let restored: (String, String, String, String) =
        sqlx::query_as("SELECT id, body, created_at, change_id FROM notes WHERE id = ?")
            .bind(&note_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        (&original.0, &original.1, &original.2),
        (&restored.0, &restored.1, &restored.2)
    );
    assert_ne!(original.3, restored.3);
    store.undo_last(None).await.unwrap();
    let retained: Vec<(String, String)> =
        sqlx::query_as("SELECT change_id, payload FROM changes ORDER BY change_id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(history, retained);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM notes")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    assert_note_activity_preserves_stale_task_creation_undo(&mut store, &pool, &task_id).await;
}

#[tokio::test]
async fn note_restoration_preserves_sync_guards_and_rejects_replacement() {
    for case in [
        "original-synced",
        "restoration-synced",
        "replacement-before-delete",
        "replacement-after-restore",
    ] {
        let (_dir, pool, mut store) = test_store_with_pool().await;
        let (task_id, _) = create_selected_task(&mut store, "Safety").await;
        let note_id = store
            .add_note_to_task(&task_id, "original".into())
            .await
            .unwrap();
        let original: String = sqlx::query_scalar("SELECT change_id FROM notes WHERE id = ?")
            .bind(&note_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        if case == "replacement-before-delete" {
            sqlx::query("UPDATE notes SET change_id = 'unrelated' WHERE id = ?")
                .bind(&note_id)
                .execute(&pool)
                .await
                .unwrap();
        }
        store.delete_note(&task_id, &note_id).await.unwrap();
        store.undo_last(None).await.unwrap();
        let restored: String = sqlx::query_scalar("SELECT change_id FROM notes WHERE id = ?")
            .bind(&note_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_ne!(original, restored);
        if case.ends_with("synced") {
            let synced = if case == "original-synced" {
                &original
            } else {
                &restored
            };
            sqlx::query("UPDATE changes SET server_seq = 1 WHERE change_id = ?")
                .bind(synced)
                .execute(&pool)
                .await
                .unwrap();
        } else if case == "replacement-after-restore" {
            sqlx::query("UPDATE notes SET change_id = 'unrelated' WHERE id = ?")
                .bind(&note_id)
                .execute(&pool)
                .await
                .unwrap();
        }
        let before: i64 = sqlx::query_scalar("SELECT count(*) FROM changes")
            .fetch_one(&pool)
            .await
            .unwrap();
        for _ in 0..2 {
            let error = store.undo_last(None).await.unwrap_err();
            assert!(
                error.to_string().contains("undo-state-changed"),
                "{case}: {error}"
            );
        }
        let after: i64 = sqlx::query_scalar("SELECT count(*) FROM changes")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(before, after, "{case}");
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM notes WHERE id = ?")
            .bind(&note_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1, "{case}");
    }
}

#[tokio::test]
async fn note_lineage_update_failure_rolls_back_restoration() {
    let (_dir, pool, mut store) = test_store_with_pool().await;
    let (task_id, _) = create_selected_task(&mut store, "Atomic lineage").await;
    let note_id = store
        .add_note_to_task(&task_id, "original".into())
        .await
        .unwrap();
    store.delete_note(&task_id, &note_id).await.unwrap();
    sqlx::query("CREATE TRIGGER reject_lineage BEFORE UPDATE OF payload ON tui_undo_entries BEGIN SELECT RAISE(FAIL, 'lineage failure'); END")
        .execute(&pool).await.unwrap();
    let count_before: i64 = sqlx::query_scalar("SELECT count(*) FROM changes")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        store
            .undo_last(None)
            .await
            .unwrap_err()
            .to_string()
            .contains("lineage failure")
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM notes")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    let count_after: i64 = sqlx::query_scalar("SELECT count(*) FROM changes")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count_before, count_after);
    sqlx::query("DROP TRIGGER reject_lineage")
        .execute(&pool)
        .await
        .unwrap();
    store.undo_last(None).await.unwrap();
    // Multiple delete/restore cycles must chain from the last authorized add.
    store.delete_note(&task_id, &note_id).await.unwrap();
    store.undo_last(None).await.unwrap();
    store.undo_last(None).await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM notes")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}
