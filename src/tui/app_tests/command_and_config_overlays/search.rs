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

    app.handle_overlay_key(key(KeyCode::Down)).await.unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Search(state)) if state.selected == 1
    ));
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
async fn search_overlay_ctrl_n_and_ctrl_p_select_results() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("needle first")).await;
    create_and_select_task(&mut app, test_task_draft("needle second")).await;

    app.begin_search();
    type_chars(&mut app, "needle").await;
    settle_search_preview(&mut app).await;
    app.handle_overlay_key(ctrl_n()).await.unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Search(state)) if state.selected == 1
    ));

    app.handle_overlay_key(ctrl_p()).await.unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Search(state)) if state.selected == 0
    ));
}

#[tokio::test]
async fn search_overlay_refreshes_results_after_paste() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("pasted needle")).await;

    app.begin_search();
    app.dispatch_paste("needle").await.unwrap();
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
async fn search_overlay_enter_opens_selected_task_detail() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("detail needle")).await;

    app.begin_search();
    type_chars(&mut app, "needle").await;
    settle_search_preview(&mut app).await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    let selected = app.list.selected_task().unwrap();
    assert_eq!(app.store.tasks[selected].task.title, "detail needle");
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
async fn search_overlay_keeps_input_immediate_while_preview_runs() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("needle first")).await;

    app.begin_search();
    app.handle_overlay_key(key(KeyCode::Char('n')))
        .await
        .unwrap();
    app.handle_overlay_key(key(KeyCode::Char('e')))
        .await
        .unwrap();

    let Some(OverlayState::Search(state)) = &app.overlay else {
        panic!("expected search overlay");
    };
    assert_eq!(state.input.as_str(), "ne");
    assert!(matches!(
        app.search.view(),
        SearchControllerView::Running { query: "ne" }
    ));
}

#[tokio::test]
async fn search_overlay_marks_first_preview_as_stale_while_running() {
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
}

#[tokio::test]
async fn search_overlay_runs_latest_preview_after_input_changes() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("needle first")).await;

    app.begin_search();
    app.handle_overlay_key(key(KeyCode::Char('n')))
        .await
        .unwrap();
    app.handle_overlay_key(key(KeyCode::Char('e')))
        .await
        .unwrap();
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
    assert_ne!(state.results_query.as_deref(), Some("o"));
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
async fn search_overlay_paste_keeps_input_immediate_while_preview_runs() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("pasted needle")).await;

    app.begin_search();
    app.dispatch_paste("needle").await.unwrap();

    let Some(OverlayState::Search(state)) = &app.overlay else {
        panic!("expected search overlay");
    };
    assert_eq!(state.input.as_str(), "needle");
    assert!(app.search_preview_work_pending());
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
