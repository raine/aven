use super::*;

#[tokio::test]
async fn command_overlay_executes_unique_lookup_and_keeps_overlay_on_errors() {
    let mut app = test_app().await;

    app.begin_command().await;
    for ch in "ref".chars() {
        app.handle_overlay_key(key(KeyCode::Char(ch)))
            .await
            .unwrap();
    }
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(app.overlay.is_none());

    app.begin_command().await;
    app.handle_overlay_key(key(KeyCode::Char('s')))
        .await
        .unwrap();
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(matches!(app.overlay, Some(OverlayState::Command { .. })));
    assert_eq!(toast_message(&app).as_deref(), Some("ambiguous command: s"));

    app.begin_command().await;
    for ch in "zzzz".chars() {
        app.handle_overlay_key(key(KeyCode::Char(ch)))
            .await
            .unwrap();
    }
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(matches!(app.overlay, Some(OverlayState::Command { .. })));
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("unknown command: zzzz")
    );
}

#[tokio::test]
async fn command_overlay_arrows_browse_and_enter_runs_selection() {
    let mut app = test_app().await;

    app.begin_command().await;
    type_chars(&mut app, "wel").await;
    app.handle_overlay_key(key(KeyCode::Down)).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Command { state })
            if state.input.text == "wel" && state.highlighted.as_deref() == Some("welcome")
    ));

    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(matches!(app.overlay, Some(OverlayState::Onboarding { .. })));
}

#[tokio::test]
async fn command_overlay_arrow_selection_wraps() {
    let mut app = test_app().await;

    app.begin_command().await;
    app.handle_overlay_key(key(KeyCode::Up)).await.unwrap();
    let last = match &app.overlay {
        Some(OverlayState::Command { state }) => state.highlighted.clone(),
        overlay => panic!("expected command overlay, got {overlay:?}"),
    };
    app.handle_overlay_key(key(KeyCode::Down)).await.unwrap();

    assert!(last.is_some());
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Command { state }) if state.highlighted.as_deref() == Some("quit")
    ));
}

#[tokio::test]
async fn command_palette_selects_upcoming_view() {
    let mut app = test_app().await;

    app.begin_command().await;
    type_chars(&mut app, "upcoming").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(app.overlay.is_none());
    assert_eq!(app.store.view_state.view, TaskView::Upcoming);
}

#[tokio::test]
async fn detail_command_overlay_limits_lookup_and_completion_to_detail_commands() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Detail command target")).await;
    app.show_detail(0);

    app.begin_command().await;
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Command { state })
            if state.context == crate::tui::event::CommandContext::Detail
    ));
    type_chars(&mut app, "view-tod").await;
    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("no command matches: view-tod")
    );
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(matches!(app.overlay, Some(OverlayState::Command { .. })));
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("unknown command: view-tod")
    );
    assert!(app.detail.is_active());
}

#[tokio::test]
async fn detail_command_overlay_routes_supported_focused_task_actions() {
    let mut app = test_app().await;
    let selected =
        create_and_select_task(&mut app, test_task_draft("Focused command target")).await;
    let selected_id = app.store.tasks[selected].task.id.clone();
    let linked = create_and_select_task(&mut app, test_task_draft("Linked task")).await;
    let linked = app.store.tasks[linked].clone();
    let selected = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == selected_id)
        .unwrap();
    app.store.tasks[selected].depends_on = vec![crate::query::TaskDependencyLink {
        task_id: linked.task.id.clone(),
        display_ref: linked.display_ref.clone(),
        title: linked.task.title.clone(),
        status: linked.task.status.as_str().to_string(),
        priority: linked.task.priority.as_str().to_string(),
        unresolved: true,
    }];
    app.list.select_task(Some(selected));
    app.show_detail(0);
    app.detail.state_mut().unwrap().focused_target = Some(DetailTargetId::Task {
        section: DetailSection::DependsOn,
        task_id: linked.task.id.clone(),
    });

    app.begin_command().await;
    type_chars(&mut app, "edit-title").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::TextInput(state))
            if matches!(
                &state.intent,
                TextIntent::EditTitle { selection }
                    if selection.single_id() == Some(&linked.task.id)
            )
    ));
    assert!(app.detail.is_active());

    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
    app.begin_command().await;
    type_chars(&mut app, "search").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(matches!(app.overlay, Some(OverlayState::Search(_))));
    assert!(app.detail.is_active());
}

#[tokio::test]
async fn command_overlay_tab_completes_unique_suffix_alias() {
    let mut app = test_app().await;

    app.begin_command().await;
    type_chars(&mut app, ":todo").await;
    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();

    assert!(matches!(
        app.overlay,
        Some(OverlayState::Command { state })
            if state.input.text == "status-todo" && state.input.cursor == "status-todo".len()
    ));
}

#[tokio::test]
async fn command_overlay_tab_selects_first_partial_command_match() {
    let mut app = test_app().await;

    app.begin_command().await;
    type_chars(&mut app, ":delet").await;
    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Command { state })
            if state.input.text == "delete"
                && state.input.cursor == "delete".len()
                && state.highlighted.as_deref() == Some("delete")
    ));
}

#[tokio::test]
async fn command_overlay_tab_cycles_from_exact_command_match() {
    let mut app = test_app().await;

    app.begin_command().await;
    type_chars(&mut app, ":delete").await;
    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Command { state })
            if state.input.text == "delete"
                && state.input.cursor == "delete".len()
                && state.highlighted.as_deref() == Some("delete")
    ));

    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Command { state })
            if state.input.text == "delete-label"
                && state.highlighted.as_deref() == Some("delete-label")
    ));

    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Command { state })
            if state.input.text == "delete-project"
                && state.highlighted.as_deref() == Some("delete-project")
    ));

    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Command { state })
            if state.input.text == "filter-deleted"
                && state.highlighted.as_deref() == Some("filter-deleted")
    ));
}

#[tokio::test]
async fn command_overlay_tab_cycles_ambiguous_matches() {
    let mut app = test_app().await;

    app.begin_command().await;
    type_chars(&mut app, "stat").await;
    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Command { state })
            if state.input.text == "status-picker"
                && state.input.cursor == "status-picker".len()
                && state.cycle_input.as_deref() == Some("stat")
                && state.highlighted.as_deref() == Some("status-picker")
    ));

    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Command { state })
            if state.input.text == "status-inbox"
                && state.input.cursor == "status-inbox".len()
                && state.cycle_input.as_deref() == Some("stat")
                && state.highlighted.as_deref() == Some("status-inbox")
    ));
}

#[tokio::test]
async fn command_overlay_tab_cycles_from_exact_alias_match() {
    let mut app = test_app().await;

    app.begin_command().await;
    type_chars(&mut app, ":todo").await;
    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Command { state })
            if state.input.text == "status-todo"
                && state.highlighted.as_deref() == Some("status-todo")
    ));

    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Command { state })
            if state.input.text == "view-todo"
                && state.highlighted.as_deref() == Some("view-todo")
    ));
}

#[tokio::test]
async fn command_overlay_edit_resets_completion_cycle() {
    let mut app = test_app().await;

    app.begin_command().await;
    type_chars(&mut app, "stat").await;
    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Backspace))
        .await
        .unwrap();

    assert!(matches!(
        app.overlay,
        Some(OverlayState::Command { state })
            if state.cycle_input.is_none() && state.highlighted.is_none()
    ));
}
