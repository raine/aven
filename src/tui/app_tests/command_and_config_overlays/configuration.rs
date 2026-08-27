use super::*;

#[tokio::test]
async fn toggle_help_closes_active_help_overlay() {
    let mut app = test_app().await;
    app.overlay = Some(OverlayState::Help { scroll: 0 });
    app.toggle_help_at_height(24);
    assert!(app.overlay.is_none());
}

#[tokio::test]
async fn help_key_opens_help_overlay() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('?')).await.unwrap();
    assert!(matches!(app.overlay, Some(OverlayState::Help { .. })));
}

#[tokio::test]
async fn config_info_opens_text_panel() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('C')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('c')).await.unwrap();

    let Some(OverlayState::TextPanel(panel)) = app.overlay else {
        panic!("expected text panel");
    };
    assert_eq!(panel.title, CONFIG_INFO_TITLE);
    assert!(panel.lines.iter().any(|line| line.contains("config path:")));
}

#[tokio::test]
async fn config_status_opens_sync_status() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('C')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('s')).await.unwrap();

    let Some(OverlayState::SyncStatus(state)) = &app.overlay else {
        panic!("expected sync status");
    };
    assert_eq!(*state, SyncStatusState::default());
    let view = app.view();
    let Some(OverlayView::SyncStatus(status)) = view.overlay else {
        panic!("expected sync status view");
    };
    assert_eq!(*status.status, app.store.sync_status);
}

#[tokio::test]
async fn sync_status_actions_keep_the_card_when_no_navigation_occurs() {
    let mut app = test_app().await;
    app.show_config_status().unwrap();

    app.handle_overlay_key(key(KeyCode::Char('d')))
        .await
        .unwrap();
    assert!(matches!(
        app.overlay,
        Some(OverlayState::SyncStatus(SyncStatusState {
            details: true,
            ..
        }))
    ));

    app.handle_overlay_key(key(KeyCode::Char('S')))
        .await
        .unwrap();
    assert!(matches!(app.overlay, Some(OverlayState::SyncStatus(_))));
    assert!(toast_message(&app).is_some_and(|message| message.starts_with("sync unavailable:")));

    app.handle_overlay_key(key(KeyCode::Char('c')))
        .await
        .unwrap();
    assert!(matches!(app.overlay, Some(OverlayState::SyncStatus(_))));
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("no unresolved conflicts")
    );
}

#[tokio::test]
async fn sync_status_conflict_action_opens_conflicts_view() {
    let mut app = test_app().await;
    app.store.sync_status.conflicts = 1;
    app.show_config_status().unwrap();

    app.handle_overlay_key(key(KeyCode::Char('c')))
        .await
        .unwrap();

    assert!(app.overlay.is_none());
    assert_eq!(app.store.view_state.query, TaskQuery::Conflicts);
}

#[tokio::test]
async fn config_paths_opens_text_panel() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('C')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('d')).await.unwrap();

    let Some(OverlayState::TextPanel(panel)) = app.overlay else {
        panic!("expected text panel");
    };
    assert_eq!(panel.title, CONFIG_PATHS_TITLE);
    assert!(
        panel
            .lines
            .iter()
            .any(|line| line.contains("effective database:"))
    );
}

#[tokio::test]
async fn database_stats_opens_text_panel() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Stats target")).await;
    let selected = app.list.selected_task();
    app.store.update_status(selected, "done").await.unwrap();
    create_and_select_task(
        &mut app,
        TaskDraft {
            metadata: Vec::new(),
            title: "Urgent task".to_string(),
            description: String::new(),
            project: None,
            status: "inbox".to_string(),
            priority: "urgent".to_string(),
            source: TaskSource::Unknown,
            labels: Vec::new(),
            available_at: None,
            due_on: None,
            is_epic: false,
        },
    )
    .await;
    app.store
        .update_deleted(app.list.selected_task(), true)
        .await
        .unwrap();

    app.handle_normal_key(KeyCode::Char('C')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('D')).await.unwrap();

    let Some(OverlayState::DatabaseStats { stats, scroll }) = app.overlay else {
        panic!("expected database stats");
    };
    assert_eq!(scroll, 0);
    assert_eq!(stats.total_tasks, 2);
    assert_eq!(stats.open_tasks, 0);
    assert_eq!(stats.deleted_tasks, 1);
    assert_eq!(stats.statuses.done, 1);
    assert_eq!(stats.priorities.urgent, 0);
    assert_eq!(stats.notes, 0);
    assert!(stats.sqlite_page_size > 0);
    assert!(stats.sqlite_page_count > 0);
}

#[tokio::test]
async fn command_panel_runs_database_stats() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Stats target")).await;

    app.begin_command().await;
    type_chars(&mut app, "database-stats").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(matches!(
        app.overlay,
        Some(OverlayState::DatabaseStats { .. })
    ));
}

#[tokio::test]
async fn config_init_requires_confirmation() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('C')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('i')).await.unwrap();

    assert!(matches!(
        app.overlay,
        Some(OverlayState::Confirm(ConfirmState {
            intent: ConfirmIntent::InitializeConfig { ref path },
            ref title,
            ..
        })) if title == CONFIG_INIT_TITLE && path == &crate::config::config_file_path().unwrap()
    ));
}

#[tokio::test]
async fn config_init_cancel_does_not_set_success_message() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('C')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('i')).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Char('n')))
        .await
        .unwrap();
    assert!(app.overlay.is_none());
    assert!(app.notification.is_none());
}

#[tokio::test]
async fn command_panel_runs_config_show() {
    let mut app = test_app().await;
    app.begin_command().await;
    type_chars(&mut app, "config-show").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(matches!(
        app.overlay,
        Some(OverlayState::TextPanel(TextPanelState { ref title, .. })) if title == CONFIG_INFO_TITLE
    ));
}

#[tokio::test]
async fn command_panel_runs_workspace_switch() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    let mut conn = pool.acquire().await.unwrap();
    crate::workspaces::create_workspace(&mut conn, "Client Work")
        .await
        .unwrap();
    drop(conn);

    app.begin_command().await;
    type_chars(&mut app, "workspace-switch").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Picker(PickerState { title, items, .. }))
            if title == SWITCH_WORKSPACE_TITLE
                && items.iter().any(|item| item.value == "client-work")
    ));

    reset_default_workspace(&pool).await;
}
