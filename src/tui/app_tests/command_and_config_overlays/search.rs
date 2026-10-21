use super::*;

#[tokio::test]
async fn search_overlay_shows_live_results_and_navigation() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("needle first")).await;
    create_and_select_task(&mut app, test_task_draft("needle second")).await;

    app.begin_search();
    type_chars(&mut app, "needle").await;
    settle_search_preview(&mut app).await;

    let Some(OverlayState::Search(state)) = &app.overlay else {
        panic!("expected search overlay");
    };
    assert_eq!(state.results.len(), 2);
    assert_eq!(state.selected, 0);
    assert!(
        state
            .results
            .iter()
            .any(|result| result.title == "needle first")
    );
    assert!(
        state
            .results
            .iter()
            .any(|result| result.title == "needle second")
    );

    for (event, selected) in [
        (key(KeyCode::Down), 1),
        (key(KeyCode::Up), 0),
        (ctrl_n(), 1),
        (ctrl_p(), 0),
    ] {
        app.handle_overlay_key(event).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Search(state)) if state.selected == selected
        ));
    }
}

#[tokio::test]
async fn search_overlay_allows_j_and_k_text_input() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("keyboard needle")).await;

    app.begin_search();
    type_chars(&mut app, "jk").await;

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Search(state)) if state.input.as_str() == "jk"
    ));
}

#[tokio::test]
async fn search_overlay_refreshes_results_after_paste() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("pasted needle")).await;

    app.begin_search();
    app.dispatch_paste("needle").await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Search(state)) if state.input.as_str() == "needle"
    ));
    assert!(app.search_preview_work_pending());
    settle_search_preview(&mut app).await;

    let Some(OverlayState::Search(state)) = &app.overlay else {
        panic!("expected search overlay");
    };
    assert_eq!(state.input.as_str(), "needle");
    assert!(
        state
            .results
            .iter()
            .any(|result| result.title == "pasted needle")
    );
}

#[tokio::test]
async fn search_overlay_tab_opens_results_list() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("list needle")).await;

    app.begin_search();
    type_chars(&mut app, "needle").await;
    settle_search_preview(&mut app).await;
    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();

    assert!(app.overlay.is_none());
    assert_eq!(app.store.tasks.len(), 1);
    assert_eq!(app.store.tasks[0].task.title, "list needle");
}

#[tokio::test]
async fn search_overlay_runs_latest_preview_after_input_changes() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("needle first")).await;

    app.begin_search();
    app.handle_overlay_key(key(KeyCode::Char('n')))
        .await
        .unwrap();
    let Some(OverlayState::Search(state)) = &app.overlay else {
        panic!("expected search overlay");
    };
    assert_eq!(state.input.as_str(), "n");
    assert!(state.results.is_empty());
    assert!(state.results_query.is_none());
    assert!(!state.results_are_current());
    assert!(matches!(
        app.search.view(),
        SearchControllerView::Running { query: "n" }
    ));

    app.handle_overlay_key(key(KeyCode::Char('e')))
        .await
        .unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Search(state)) if state.input.as_str() == "ne"
    ));
    assert!(matches!(
        app.search.view(),
        SearchControllerView::Running { query: "ne" }
    ));

    app.handle_overlay_key(key(KeyCode::Char('e')))
        .await
        .unwrap();

    assert!(matches!(
        app.search.view(),
        SearchControllerView::Running { query: "nee" }
    ));

    settle_search_preview(&mut app).await;

    let Some(OverlayState::Search(state)) = &app.overlay else {
        panic!("expected search overlay");
    };
    assert_eq!(state.input.as_str(), "nee");
    assert_eq!(state.results_query.as_deref(), Some("nee"));
}

#[tokio::test]
async fn search_overlay_ignores_stale_preview_results() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("old needle")).await;
    create_and_select_task(&mut app, test_task_draft("new needle")).await;

    app.begin_search();
    app.handle_overlay_key(key(KeyCode::Char('o')))
        .await
        .unwrap();

    if let Some(OverlayState::Search(state)) = &mut app.overlay {
        state.input.text = "new".to_string();
    }
    settle_search_preview(&mut app).await;

    let Some(OverlayState::Search(state)) = &app.overlay else {
        panic!("expected search overlay");
    };
    assert_eq!(state.input.as_str(), "new");
    assert!(state.results_query.is_none());
    assert!(state.results.is_empty());
}

#[tokio::test]
async fn search_overlay_tab_submits_current_input_when_preview_lags() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("current needle")).await;

    app.begin_search();
    type_chars(&mut app, "current").await;
    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();

    assert!(app.overlay.is_none());
    assert_eq!(app.store.tasks[0].task.title, "current needle");
    assert!(!app.search_preview_work_pending());
}

#[tokio::test]
async fn search_overlay_keeps_stale_results_when_input_changes() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("alpha needle")).await;

    app.begin_search();
    type_chars(&mut app, "alpha").await;
    settle_search_preview(&mut app).await;

    let Some(OverlayState::Search(state)) = &app.overlay else {
        panic!("expected search overlay");
    };
    assert_eq!(state.results_query.as_deref(), Some("alpha"));
    assert!(!state.results.is_empty());

    app.handle_overlay_key(key(KeyCode::Char('x')))
        .await
        .unwrap();

    let Some(OverlayState::Search(state)) = &app.overlay else {
        panic!("expected search overlay");
    };
    assert_eq!(state.input.as_str(), "alphax");
    assert!(!state.results.is_empty());
    assert_eq!(state.results_query.as_deref(), Some("alpha"));
    assert!(!state.results_are_current());
    assert!(matches!(
        app.search.view(),
        SearchControllerView::Running { query: "alphax" }
    ));
}

#[tokio::test]
async fn search_overlay_enter_does_not_open_stale_selected_detail() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("alpha needle")).await;

    app.begin_search();
    type_chars(&mut app, "alpha").await;
    settle_search_preview(&mut app).await;
    app.handle_overlay_key(key(KeyCode::Char('x')))
        .await
        .unwrap();
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(app.overlay.is_none());
    assert!(app.detail.is_inactive());
    assert!(app.store.tasks.is_empty());
    assert_eq!(app.store.view_state.query, TaskQuery::Search);
    assert!(!app.search_preview_work_pending());
}

#[tokio::test]
async fn search_overlay_cancel_aborts_active_preview() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("needle first")).await;

    app.begin_search();
    app.handle_overlay_key(key(KeyCode::Char('n')))
        .await
        .unwrap();
    assert!(app.search_preview_work_pending());

    app.cancel_overlay();

    assert!(app.overlay.is_none());
    assert!(!app.search_preview_work_pending());
}

#[tokio::test]
async fn search_overlay_whitespace_paste_clears_results() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("pasted needle")).await;

    app.begin_search();
    app.dispatch_paste("needle").await.unwrap();
    settle_search_preview(&mut app).await;
    app.handle_overlay_key(ctrl_u()).await.unwrap();
    app.dispatch_paste("   ").await.unwrap();

    let Some(OverlayState::Search(state)) = &app.overlay else {
        panic!("expected search overlay");
    };
    assert!(state.results.is_empty());
    assert!(state.results_query.is_none());
    assert!(!app.search_preview_work_pending());
}

#[tokio::test]
async fn search_replaces_existing_overlay() {
    let mut app = test_app().await;
    app.overlay = Some(OverlayState::Help { scroll: 0 });
    app.begin_search();
    assert!(matches!(app.overlay, Some(OverlayState::Search(_))));
}

async fn scoped_search_app() -> App {
    let mut app = test_app().await;
    for (project, title) in [
        ("app", "needle local"),
        ("other", "needle remote"),
        ("other", "elsewhere only"),
    ] {
        let mut draft = test_task_draft(title);
        draft.project = Some(project.into());
        create_and_select_task(&mut app, draft).await;
    }
    app.show_scope(TaskScopeTarget::Project("app".into()))
        .await
        .unwrap();
    app
}

#[tokio::test]
async fn search_scope_toggle_is_live_reversible_and_cancel_does_not_change_view() {
    let mut app = scoped_search_app().await;
    app.begin_search();
    type_chars(&mut app, "needle").await;
    settle_search_preview(&mut app).await;
    let Some(OverlayState::Search(state)) = &app.overlay else {
        panic!()
    };
    assert_eq!(state.project.as_deref(), Some("app"));
    assert_eq!(state.results.len(), 1);
    assert_eq!(state.other_project_matches, None);

    let toggle = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
    app.handle_overlay_key(toggle).await.unwrap();
    settle_search_preview(&mut app).await;
    let Some(OverlayState::Search(state)) = &app.overlay else {
        panic!()
    };
    assert_eq!(state.project, None);
    assert_eq!(state.results.len(), 2);
    assert_eq!(app.store.view_state.scope, TaskScope::Project("app".into()));
    app.handle_overlay_key(toggle).await.unwrap();
    // Repeated toggles with identical text must cancel the old-scope request.
    app.handle_overlay_key(toggle).await.unwrap();
    app.handle_overlay_key(toggle).await.unwrap();
    settle_search_preview(&mut app).await;
    let Some(OverlayState::Search(state)) = &app.overlay else {
        panic!()
    };
    assert_eq!(state.results.len(), 1);
    assert_eq!(state.results[0].project_key, "app");
    app.handle_overlay_key(toggle).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
    assert_eq!(app.store.view_state.scope, TaskScope::Project("app".into()));
}

#[tokio::test]
async fn empty_scoped_search_counts_other_projects_and_accepts_selected_scope() {
    let mut app = scoped_search_app().await;
    app.begin_search();
    type_chars(&mut app, "elsewhere").await;
    settle_search_preview(&mut app).await;
    let Some(OverlayState::Search(state)) = &app.overlay else {
        panic!()
    };
    assert!(state.results.is_empty());
    assert_eq!(state.other_project_matches, Some(1));
    app.handle_overlay_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL))
        .await
        .unwrap();
    // Submit without waiting for the new preview.
    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
    assert_eq!(app.store.view_state.scope, TaskScope::Workspace);
    assert_eq!(app.store.tasks.len(), 1);
    assert_eq!(app.store.tasks[0].task.title, "elsewhere only");

    app.show_scope(TaskScopeTarget::Project("app".into()))
        .await
        .unwrap();
    app.begin_search();
    type_chars(&mut app, "needle").await;
    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
    assert_eq!(app.store.view_state.scope, TaskScope::Project("app".into()));
    assert_eq!(app.store.tasks.len(), 1);
    assert_eq!(app.store.tasks[0].task.title, "needle local");
}

#[tokio::test]
async fn relationship_search_stays_workspace_wide_in_project_scope() {
    let mut app = scoped_search_app().await;
    let selection =
        crate::tui::task_selection::TaskSelection::resolve_single(&app.store.tasks, Some(0))
            .unwrap();
    for intent in [
        SearchIntent::AddDependency {
            selection: selection.clone(),
            display_ref: "APP".into(),
        },
        SearchIntent::AddRelated {
            selection,
            display_ref: "APP".into(),
        },
    ] {
        app.overlay = Some(OverlayState::Search(SearchState::for_intent(intent)));
        type_chars(&mut app, "elsewhere").await;
        settle_search_preview(&mut app).await;
        let Some(OverlayState::Search(state)) = &app.overlay else {
            panic!()
        };
        assert_eq!(state.project, None);
        assert_eq!(state.original_project, None);
        assert_eq!(state.results.len(), 1);
        assert_eq!(state.results[0].project_key, "other");
        assert_eq!(state.other_project_matches, None);
        app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
    }
}

#[tokio::test]
async fn recurring_search_accepts_the_dialog_scope() {
    let mut app = scoped_search_app().await;
    app.show_view(TaskQuery::Recurring).await.unwrap();
    app.begin_search();
    type_chars(&mut app, "needle").await;
    app.handle_overlay_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL))
        .await
        .unwrap();
    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
    assert_eq!(app.store.view_state.scope, TaskScope::Workspace);
    assert_eq!(app.store.view_state.query, TaskQuery::Recurring);
    assert_eq!(
        app.store.view_state.recurring.search.as_deref(),
        Some("needle")
    );
}
