use super::*;

#[tokio::test]
async fn conflict_list_shortcut_applies_conflicts_view() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('l')).await.unwrap();
    assert_eq!(app.store.view_state.view, TaskView::Conflicts);
    assert_eq!(app.store.view_state.view, TaskView::Conflicts);
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("no unresolved conflicts")
    );
}

#[tokio::test]
async fn conflict_show_opens_text_panel_and_esc_closes() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Conflict show")).await;
    insert_title_conflict(&pool, &mut app, selected, "local title", "remote title").await;

    app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('s')).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::TextPanel(state))
            if state.lines.iter().any(|line| line.contains("field=title"))
    ));

    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
    assert!(app.overlay.is_none());
}

#[tokio::test]
async fn conflict_next_selects_next_conflicted_task() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    create_and_select_task(&mut app, test_task_draft("First")).await;
    create_and_select_task(&mut app, test_task_draft("Second")).await;
    let first_id = app
        .store
        .tasks
        .iter()
        .find(|item| item.task.title == "First")
        .unwrap()
        .task
        .id
        .clone();
    let second_id = app
        .store
        .tasks
        .iter()
        .find(|item| item.task.title == "Second")
        .unwrap()
        .task
        .id
        .clone();
    insert_title_conflict_for_task_id(&pool, &mut app, &first_id, "local one", "remote one").await;
    insert_title_conflict_for_task_id(&pool, &mut app, &second_id, "local two", "remote two").await;
    let first = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == first_id)
        .unwrap();
    let second = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == second_id)
        .unwrap();
    app.list.select_task(Some(first));

    app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('n')).await.unwrap();
    assert_eq!(app.list.selected_task(), Some(second));
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("selected next conflict")
    );
}

#[tokio::test]
async fn accept_local_conflict_resolves_after_confirmation() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Before")).await;
    insert_title_conflict(&pool, &mut app, selected, "local title", "remote title").await;

    app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Confirm(state)) if state.title == CONFLICT_CONFIRM_LOCAL_TITLE
    ));

    app.handle_overlay_key(key(KeyCode::Char('y')))
        .await
        .unwrap();
    assert!(app.overlay.is_none());
    assert_eq!(app.store.tasks[selected].task.title, "local title");
    assert!(!app.store.tasks[selected].has_conflict);
    assert!(
        toast_message(&app)
            .is_some_and(|message| message.contains("resolved") && message.contains("field=title"))
    );
}

#[tokio::test]
async fn accept_remote_conflict_resolves_after_confirmation() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Before")).await;
    insert_title_conflict(&pool, &mut app, selected, "local title", "remote title").await;

    app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('r')).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Char('y')))
        .await
        .unwrap();

    assert_eq!(app.store.tasks[selected].task.title, "remote title");
    assert!(!app.store.tasks[selected].has_conflict);
}

#[tokio::test]
async fn manual_conflict_merge_resolves_with_submitted_value() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Before")).await;
    insert_title_conflict(&pool, &mut app, selected, "local title", "remote title").await;

    app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('m')).await.unwrap();
    type_chars(&mut app, " merged").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert_eq!(app.store.tasks[selected].task.title, "local title merged");
    assert!(!app.store.tasks[selected].has_conflict);
}

#[tokio::test]
async fn clean_manual_description_conflict_merge_cancels_without_confirmation() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Before")).await;
    let task_id = app.store.tasks[selected].task.id.clone();
    insert_conflict_for_task_id(
        &pool,
        &mut app,
        &task_id,
        "description",
        "local description",
        "remote description",
    )
    .await;

    app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('m')).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::MultilineInput(state))
            if state.mode == MultilineInputMode::Compose
                && state.lines == ["local description"]
    ));
    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();

    assert!(app.overlay.is_none());
    assert!(app.store.tasks[selected].has_conflict);
}

#[tokio::test]
async fn changed_manual_description_conflict_merge_requires_discard_confirmation() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Before")).await;
    let task_id = app.store.tasks[selected].task.id.clone();
    insert_conflict_for_task_id(
        &pool,
        &mut app,
        &task_id,
        "description",
        "local description",
        "remote description",
    )
    .await;

    app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('m')).await.unwrap();
    type_chars(&mut app, " updated").await;
    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::MultilineInput(state))
            if state.mode == MultilineInputMode::ConfirmDiscard
                && state.lines == ["local description updated"]
    ));
    app.handle_overlay_key(key(KeyCode::Char('y')))
        .await
        .unwrap();

    assert!(app.overlay.is_none());
    assert!(app.store.tasks[selected].has_conflict);
    assert!(app.store.tasks[selected].task.description.is_empty());
}

#[tokio::test]
async fn manual_conflict_retry_preserves_submitted_text_after_error() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Before")).await;
    let task_id = app.store.tasks[selected].task.id.clone();
    insert_title_conflict(&pool, &mut app, selected, "local title", "remote title").await;

    app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('m')).await.unwrap();
    type_chars(&mut app, " merged").await;

    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("DELETE FROM conflicts WHERE task_id = ? AND field = 'title'")
        .bind(&task_id)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);

    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(toast_message(&app).is_some_and(|message| message.contains("conflict-not-found")));
    assert_eq!(toast_severity(&app), Some(ToastSeverity::Error));
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::TextInput(state))
            if matches!(state.intent, TextIntent::ResolveConflictManually { .. })
                && state.input.as_str() == "local title merged"
    ));
}

#[tokio::test]
async fn conflict_resolution_without_selected_task_reports_message() {
    let mut app = test_app().await;
    app.list.select_task(None);

    app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("no selected task for conflict resolution")
    );
}

#[tokio::test]
async fn cancel_discards_conflict_intent() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Conflict")).await;
    insert_title_conflict(&pool, &mut app, selected, "local title", "remote title").await;

    app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
    assert!(matches!(
        app.overlay,
        Some(OverlayState::Confirm(ConfirmState {
            intent: ConfirmIntent::ResolveConflict { .. },
            ..
        }))
    ));
    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
    assert!(app.overlay.is_none());
}
