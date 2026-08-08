use super::*;

#[tokio::test]
async fn label_administration_updates_task_and_series_references_and_undoes() {
    use aven_core::recurrence::{
        RecurrenceDuePolicy, RecurrenceRule, RecurrenceSchedule, TimeZoneId,
    };
    use chrono::Utc;

    let (_dir, pool, mut store) = test_store_with_pool().await;
    store
        .create_label("Needs Review".to_string())
        .await
        .unwrap();
    let (_, selected) = store
        .create_task(
            TaskDraft {
                metadata: Vec::new(),
                title: "Labeled task".to_string(),
                description: String::new(),
                project: None,
                status: "inbox".to_string(),
                priority: "none".to_string(),
                source: TaskSource::Unknown,
                labels: vec!["needs-review".to_string()],
                available_at: None,
                due_on: None,
                is_epic: false,
            },
            None,
        )
        .await
        .unwrap();
    let task_id = store.tasks[selected.unwrap()].task.id.clone();
    let schedule = RecurrenceSchedule::new(
        RecurrenceRule::daily(),
        "UTC".parse::<TimeZoneId>().unwrap(),
        Utc::now().date_naive(),
        None,
        RecurrenceDuePolicy::SameDay,
    );
    let (_, selected) = store
        .create_recurrence_series(
            recurrence_draft(
                "Labeled series".to_string(),
                String::new(),
                None,
                "none".to_string(),
                "todo".to_string(),
                vec!["needs-review".to_string()],
                schedule,
            ),
            None,
        )
        .await
        .unwrap();
    let series_id = store.tasks[selected.unwrap()]
        .recurrence
        .as_ref()
        .unwrap()
        .series_id
        .clone();

    store.view_state.filter_modifiers.label = Some("needs-review".to_string());
    let renamed = store
        .rename_label("needs-review", " Follow Up ".to_string())
        .await
        .unwrap();
    assert!(
        renamed
            .message
            .contains("on 2 tasks and 1 recurring series")
    );
    assert_eq!(
        store.view_state.filter_modifiers.label.as_deref(),
        Some("follow-up")
    );
    assert_eq!(
        store
            .database
            .task_labels(&store.active_workspace.id, &task_id)
            .await
            .unwrap(),
        vec!["follow-up"]
    );
    let mut conn = pool.acquire().await.unwrap();
    let series_labels: Vec<String> = sqlx::query_scalar(
        "SELECT label FROM recurrence_series_labels
         WHERE workspace_id = ? AND series_id = ? ORDER BY label",
    )
    .bind(&store.active_workspace.id)
    .bind(&series_id)
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    assert_eq!(series_labels, vec!["follow-up"]);
    drop(conn);

    store.undo_last(None).await.unwrap().unwrap();
    assert_eq!(
        store.view_state.filter_modifiers.label.as_deref(),
        Some("needs-review")
    );
    assert_eq!(
        store
            .database
            .task_labels(&store.active_workspace.id, &task_id)
            .await
            .unwrap(),
        vec!["needs-review"]
    );

    let deleted = store.delete_label("needs-review").await.unwrap();
    assert!(
        deleted
            .message
            .contains("from 2 tasks and 1 recurring series")
    );
    assert!(
        store
            .database
            .task_labels(&store.active_workspace.id, &task_id)
            .await
            .unwrap()
            .is_empty()
    );
    store.undo_last(None).await.unwrap().unwrap();
    assert_eq!(
        store
            .database
            .task_labels(&store.active_workspace.id, &task_id)
            .await
            .unwrap(),
        vec!["needs-review"]
    );
    let mut conn = pool.acquire().await.unwrap();
    let series_labels: Vec<String> = sqlx::query_scalar(
        "SELECT label FROM recurrence_series_labels
         WHERE workspace_id = ? AND series_id = ? ORDER BY label",
    )
    .bind(&store.active_workspace.id)
    .bind(&series_id)
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    assert_eq!(series_labels, vec!["needs-review"]);
}

#[tokio::test]
async fn label_administration_rolls_back_when_undo_cannot_be_recorded() {
    let (_dir, pool, mut store) = test_store_with_pool().await;
    store.create_label("stable".to_string()).await.unwrap();
    let (task_id, selected) = create_selected_task(&mut store, "Stable label").await;
    store
        .update_labels(Some(selected), vec!["stable".to_string()])
        .await
        .unwrap();
    reject_undo_inserts(&pool).await;

    let rename_error = store
        .rename_label("stable", "changed".to_string())
        .await
        .unwrap_err();
    assert!(rename_error.to_string().contains("injected undo failure"));
    assert_eq!(
        store
            .database
            .task_labels(&store.active_workspace.id, &task_id)
            .await
            .unwrap(),
        vec!["stable"]
    );

    let delete_error = store.delete_label("stable").await.unwrap_err();
    assert!(delete_error.to_string().contains("injected undo failure"));
    assert_eq!(
        store
            .database
            .task_labels(&store.active_workspace.id, &task_id)
            .await
            .unwrap(),
        vec!["stable"]
    );
    assert!(
        store
            .database
            .list_labels(&store.active_workspace.id, None)
            .await
            .unwrap()
            .contains(&"stable".to_string())
    );
}
