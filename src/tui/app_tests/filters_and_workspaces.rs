use super::*;

#[tokio::test]
async fn back_shortcut_restores_filter_and_project_navigation() {
    let mut app = test_app().await;
    app.store
        .create_project("Mobile App".to_string())
        .await
        .unwrap();
    app.show_scope(TaskScopeTarget::Project("mobile-app".to_string()))
        .await
        .unwrap();
    app.submit_filter_priority(vec!["urgent".to_string()])
        .await
        .unwrap();

    assert_eq!(
        app.store.view_state.scope,
        TaskScope::Project("mobile-app".to_string())
    );
    assert_eq!(
        app.store.view_state.filter_modifiers.priority.as_deref(),
        Some("urgent")
    );

    app.handle_normal_key(KeyCode::Char('g')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('[')).await.unwrap();

    assert_eq!(
        app.store.view_state.scope,
        TaskScope::Project("mobile-app".to_string())
    );
    assert_eq!(app.store.view_state.filter_modifiers.priority, None);
    assert_eq!(toast_message(&app), None);

    app.handle_normal_key(KeyCode::Char('g')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('[')).await.unwrap();

    assert_eq!(app.store.view_state.scope, TaskScope::Workspace);
}

#[tokio::test]
async fn back_shortcut_reports_empty_history() {
    let mut app = test_app().await;

    app.handle_normal_key(KeyCode::Char('g')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('[')).await.unwrap();

    assert_eq!(
        toast_message(&app).as_deref(),
        Some("no previous navigation state")
    );
}

#[tokio::test]
async fn back_and_forward_shortcuts_round_trip_navigation_state() {
    let mut app = test_app().await;
    app.store
        .create_project("Mobile App".to_string())
        .await
        .unwrap();
    app.show_scope(TaskScopeTarget::Project("mobile-app".to_string()))
        .await
        .unwrap();
    app.submit_filter_priority(vec!["urgent".to_string()])
        .await
        .unwrap();

    app.handle_normal_key(KeyCode::Char('g')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('[')).await.unwrap();
    assert_eq!(app.store.view_state.filter_modifiers.priority, None);

    app.handle_normal_key(KeyCode::Char('g')).await.unwrap();
    app.handle_normal_key(KeyCode::Char(']')).await.unwrap();
    assert_eq!(
        app.store.view_state.scope,
        TaskScope::Project("mobile-app".to_string())
    );
    assert_eq!(
        app.store.view_state.filter_modifiers.priority.as_deref(),
        Some("urgent")
    );

    app.handle_normal_key(KeyCode::Char('g')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('[')).await.unwrap();
    assert_eq!(app.store.view_state.filter_modifiers.priority, None);
}

#[tokio::test]
async fn back_restores_list_selection() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("First")).await;
    let second = create_and_select_task(&mut app, test_task_draft("Second")).await;
    let second_id = app.store.tasks[second].task.id.clone();
    app.list.select_task(Some(second));

    app.submit_filter_priority(vec!["urgent".to_string()])
        .await
        .unwrap();
    assert!(app.list.selected_task().is_none());
    app.go_back().await.unwrap();

    assert_eq!(
        app.store.tasks[app.list.selected_task().unwrap()].task.id,
        second_id
    );
}

#[tokio::test]
async fn fresh_navigation_after_back_clears_forward_history() {
    let mut app = test_app().await;
    app.submit_filter_priority(vec!["urgent".to_string()])
        .await
        .unwrap();
    app.handle_normal_key(KeyCode::Char('g')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('[')).await.unwrap();

    app.submit_filter_priority(vec!["high".to_string()])
        .await
        .unwrap();
    app.handle_normal_key(KeyCode::Char('g')).await.unwrap();
    app.handle_normal_key(KeyCode::Char(']')).await.unwrap();

    assert_eq!(
        app.store.view_state.filter_modifiers.priority.as_deref(),
        Some("high")
    );
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("no next navigation state")
    );
}

#[tokio::test]
async fn forward_shortcut_reports_empty_history() {
    let mut app = test_app().await;

    app.handle_normal_key(KeyCode::Char('g')).await.unwrap();
    app.handle_normal_key(KeyCode::Char(']')).await.unwrap();

    assert_eq!(
        toast_message(&app).as_deref(),
        Some("no next navigation state")
    );
}

#[tokio::test]
async fn scope_project_shortcut_opens_project_picker() {
    let mut app = test_app().await;
    app.store
        .create_project("Mobile App".to_string())
        .await
        .unwrap();

    app.handle_normal_key(KeyCode::Char('g')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('p')).await.unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Picker(PickerState { title, .. })) if title == SCOPE_PROJECT_TITLE
    ));
}

#[tokio::test]
async fn scope_project_escape_cancels_from_filter() {
    let mut app = test_app().await;
    app.store
        .create_project("Mobile App".to_string())
        .await
        .unwrap();

    app.begin_scope_project();
    type_chars(&mut app, "mobile").await;
    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();

    assert!(app.overlay.is_none());
    assert_eq!(app.store.view_state.scope, TaskScope::Workspace);
}

#[tokio::test]
async fn upcoming_view_shortcut_selects_upcoming() {
    let mut app = test_app().await;

    app.handle_normal_key(KeyCode::Char('v')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('p')).await.unwrap();

    assert_eq!(app.store.view_state.query, TaskQuery::Upcoming);
}

#[tokio::test]
async fn done_view_shortcut_keeps_project_scope() {
    let mut app = test_app().await;
    app.store
        .create_project("Mobile App".to_string())
        .await
        .unwrap();
    app.store.create_project("Ops".to_string()).await.unwrap();
    for (title, project) in [("Mobile done", "mobile-app"), ("Ops done", "ops")] {
        let (_, selected) = app
            .store
            .create_task(
                TaskDraft {
                    metadata: Vec::new(),
                    title: title.to_string(),
                    description: String::new(),
                    project: Some(project.to_string()),
                    status: "inbox".to_string(),
                    priority: "none".to_string(),
                    source: TaskSource::Unknown,
                    labels: Vec::new(),
                    available_at: None,
                    due_on: None,
                    is_epic: false,
                },
                None,
            )
            .await
            .unwrap();
        app.store.update_status(selected, "done").await.unwrap();
    }
    let selected = app
        .store
        .show_scope(TaskScopeTarget::Project("mobile-app".to_string()))
        .await
        .unwrap();
    app.apply_filter_selection(selected);

    app.handle_normal_key(KeyCode::Char('v')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('d')).await.unwrap();

    assert_eq!(
        app.store.view_state.scope,
        TaskScope::Project("mobile-app".to_string())
    );
    assert_eq!(app.store.view_state.query, TaskQuery::Done);
    assert_eq!(
        app.store.view_state.filters().statuses,
        vec!["done", "canceled"]
    );
    assert_eq!(app.store.tasks.len(), 1);
    assert_eq!(app.store.tasks[0].task.title, "Mobile done");
    assert_eq!(toast_message(&app), None);
}

#[tokio::test]
async fn filter_shortcuts_apply_label_status_priority_and_deleted() {
    let mut app = test_app().await;
    app.store.create_label("backend".to_string()).await.unwrap();
    create_and_select_task(
        &mut app,
        TaskDraft {
            metadata: Vec::new(),
            title: "Filtered task".to_string(),
            description: String::new(),
            project: None,
            status: "inbox".to_string(),
            priority: "urgent".to_string(),
            source: TaskSource::Unknown,
            labels: vec!["backend".to_string()],
            available_at: None,
            due_on: None,
            is_epic: false,
        },
    )
    .await;

    app.handle_normal_key(KeyCode::Char('f')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('l')).await.unwrap();
    type_chars(&mut app, "backend").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert_eq!(
        app.store.view_state.filter_modifiers.label.as_deref(),
        Some("backend")
    );
    assert_eq!(toast_message(&app), None);
    app.handle_normal_key(KeyCode::Char('f')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('p')).await.unwrap();
    type_chars(&mut app, "urgent").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert_eq!(
        app.store.view_state.filter_modifiers.priority.as_deref(),
        Some("urgent")
    );
    assert_eq!(toast_message(&app), None);

    app.handle_normal_key(KeyCode::Char('f')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('x')).await.unwrap();
    assert!(app.store.view_state.filter_modifiers.include_deleted);
    assert!(!app.store.view_state.filter_modifiers.deleted_only);
    assert_eq!(toast_message(&app), None);

    app.handle_normal_key(KeyCode::Char('f')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('x')).await.unwrap();
    assert!(app.store.view_state.filter_modifiers.include_deleted);
    assert!(app.store.view_state.filter_modifiers.deleted_only);
    assert_eq!(toast_message(&app), None);
}

#[tokio::test]
async fn workspace_creation_validates_input_and_keeps_active_workspace() {
    let mut app = test_app().await;
    let active_id = app.store.active_workspace.id.clone();

    app.handle_normal_key(KeyCode::Char('W')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('n')).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::TextInput(TextInputState { title, .. }))
            if title == ADD_WORKSPACE_TITLE
    ));

    app.submit_add_workspace("---".to_string()).await.unwrap();
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("workspace name must contain an ASCII letter or number")
    );
    assert_eq!(toast_severity(&app), Some(ToastSeverity::Warning));
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::TextInput(TextInputState {
            intent: TextIntent::AddWorkspace,
            ..
        }))
    ));

    app.overlay = None;
    app.submit_add_workspace("  Client Work  ".to_string())
        .await
        .unwrap();

    assert_eq!(app.store.active_workspace.id, active_id);
    assert_eq!(app.store.active_workspace.key, "default");
    assert!(
        app.store
            .workspaces
            .iter()
            .any(|workspace| workspace.key == "client-work" && workspace.name == "Client Work")
    );
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("created workspace Client Work (client-work)")
    );
}

#[tokio::test]
async fn workspace_rename_targets_picker_choice_and_preserves_errors() {
    let mut app = test_app().await;
    app.store
        .create_workspace("Client Work".to_string())
        .await
        .unwrap();

    app.handle_normal_key(KeyCode::Char('W')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('r')).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Picker(PickerState {
            intent: PickerIntent::RenameWorkspace,
            title,
            items,
            ..
        })) if title == RENAME_WORKSPACE_TITLE
            && items.iter().find(|item| item.value == "default").is_some_and(|item| item.selected)
            && items.iter().find(|item| item.value == "client-work").is_some_and(|item| !item.selected)
    ));

    app.submit_rename_workspace_picker(vec!["default".to_string()]);
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::TextInput(TextInputState {
            intent: TextIntent::RenameWorkspace { workspace },
            input,
            ..
        })) if workspace == "default" && input.as_str() == "default"
    ));

    app.submit_rename_workspace("default".to_string(), "Client Work".to_string())
        .await
        .unwrap();
    assert_eq!(app.store.active_workspace.key, "default");
    assert_eq!(toast_severity(&app), Some(ToastSeverity::Error));
    assert!(toast_message(&app).is_some_and(|message| message.contains("workspace-exists")));
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::TextInput(TextInputState {
            intent: TextIntent::RenameWorkspace { workspace },
            ..
        })) if workspace == "default"
    ));

    app.overlay = None;
    app.submit_rename_workspace("default".to_string(), "Personal".to_string())
        .await
        .unwrap();
    assert_eq!(app.store.active_workspace.key, "personal");
    assert_eq!(app.store.active_workspace.name, "Personal");
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("renamed active workspace to personal (Personal)")
    );
}

#[tokio::test]
async fn workspace_administration_cancellation_reports_the_canceled_flow() {
    let mut app = test_app().await;

    app.begin_add_workspace();
    app.cancel_overlay();
    assert!(app.overlay.is_none());
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("workspace creation canceled")
    );

    app.begin_rename_workspace();
    app.cancel_overlay();
    assert!(app.overlay.is_none());
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("workspace rename canceled")
    );
}

#[tokio::test]
async fn switch_workspace_accepts_direct_filtering_and_arrow_navigation() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Default only")).await;
    app.store.view_state.query = TaskQuery::Todo;
    app.store
        .create_workspace("Client Alpha".to_string())
        .await
        .unwrap();
    app.store
        .create_workspace("Client Beta".to_string())
        .await
        .unwrap();

    app.handle_normal_key(KeyCode::Char('g')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('w')).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Picker(state)) if state.title == SWITCH_WORKSPACE_TITLE && state.mode == PickerMode::Filter
    ));
    type_chars(&mut app, "client").await;

    let first = match &app.overlay {
        Some(OverlayState::Picker(state)) => {
            assert_eq!(state.mode, PickerMode::Filter);
            assert_eq!(state.filter.as_str(), "client");
            state.selected
        }
        _ => panic!("expected workspace picker"),
    };
    app.handle_overlay_key(key(KeyCode::Down)).await.unwrap();
    let expected = match &app.overlay {
        Some(OverlayState::Picker(state)) => {
            assert_ne!(state.selected, first);
            state.items[state.selected].value.clone()
        }
        _ => panic!("expected workspace picker"),
    };
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert_eq!(app.store.active_workspace.key, expected);
    assert_eq!(app.store.view_state.query, TaskQuery::Todo);
    assert!(app.store.tasks.is_empty());
    assert!(app.overlay.is_none());
    assert_eq!(toast_message(&app), None);
}

#[tokio::test]
async fn switch_workspace_escape_cancels_from_filter() {
    let mut app = test_app().await;
    app.store
        .create_workspace("Client Work".to_string())
        .await
        .unwrap();

    app.begin_switch_workspace().await.unwrap();
    type_chars(&mut app, "client").await;
    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();

    assert!(app.overlay.is_none());
    assert_eq!(app.store.active_workspace.key, "default");
}

#[tokio::test]
async fn switch_workspace_no_match_warns_and_reopens_filter() {
    let mut app = test_app().await;

    app.begin_switch_workspace().await.unwrap();
    type_chars(&mut app, "missing").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert_eq!(
        toast_message(&app).as_deref(),
        Some("no matching workspace")
    );
    assert_eq!(toast_severity(&app), Some(ToastSeverity::Warning));
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Picker(PickerState {
            intent: PickerIntent::SwitchWorkspace,
            mode: PickerMode::Filter,
            filter,
            ..
        })) if filter.as_str().is_empty()
    ));
}

#[tokio::test]
async fn refresh_reports_invalid_project_scope_fallback() {
    let mut app = test_app().await;
    app.store.view_state.scope = TaskScope::Project("missing".to_string());
    app.store.view_state.query = TaskQuery::Todo;

    app.refresh().await.unwrap();

    assert_eq!(app.store.view_state.scope, TaskScope::Workspace);
    assert_eq!(app.store.view_state.query, TaskQuery::Todo);
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("project scope missing is no longer available")
    );
    assert_eq!(toast_severity(&app), Some(ToastSeverity::Warning));
}

#[tokio::test]
async fn clear_filters_shortcut_resets_default_view_without_notification() {
    let mut app = test_app().await;
    app.store.view_state.query = TaskQuery::Todo;
    app.submit_filter_priority(vec!["urgent".to_string()])
        .await
        .unwrap();

    app.handle_normal_key(KeyCode::Char('f')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('c')).await.unwrap();

    assert_eq!(app.store.view_state.query, TaskQuery::Todo);
    assert!(app.store.view_state.filter_modifiers.priority.is_none());
    assert_eq!(toast_message(&app), None);
}

#[tokio::test]
async fn go_conflicts_shortcut_sets_conflicts_view() {
    let mut app = test_app().await;

    app.handle_normal_key(KeyCode::Char('v')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('c')).await.unwrap();

    assert_eq!(app.store.view_state.query, TaskQuery::Conflicts);
}

#[tokio::test]
async fn header_click_opens_scope_menu_and_selects_scope() {
    let mut app = test_app().await;
    app.store
        .create_project("Mobile App".to_string())
        .await
        .unwrap();
    let selected = app
        .store
        .show_scope(TaskScopeTarget::Project("mobile-app".to_string()))
        .await
        .unwrap();
    app.apply_filter_selection(selected);

    let (scope_column, menu_column) = (0..140)
        .find_map(|column| {
            match crate::tui::ui::header_target_at(
                &app.theme,
                &app.store,
                None,
                ratatui::layout::Rect::new(0, 0, 140, 2),
                column,
                0,
            ) {
                Some(crate::tui::ui::HeaderTarget::Scope {
                    column: menu_column,
                }) => Some((column, menu_column)),
                _ => None,
            }
        })
        .unwrap();
    app.dispatch_mouse(header_click(scope_column), (140, 24).into())
        .await
        .unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::HeaderMenu(state))
            if state.column == menu_column
                && state.row == 0
                && state.items.iter().any(|item| item.label == "workspace")
                && state.items.iter().any(|item| item.label.contains("Mobile App"))
    ));

    app.dispatch_mouse(
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: menu_column.saturating_add(2),
            row: 2,
            modifiers: KeyModifiers::NONE,
        },
        (140, 24).into(),
    )
    .await
    .unwrap();
    assert_eq!(app.store.view_state.scope, TaskScope::Workspace);
    assert_eq!(toast_message(&app), None);
}

#[tokio::test]
async fn header_click_opens_view_menu_and_selects_view() {
    let mut app = test_app().await;

    let (view_column, menu_column) = (0..140)
        .find_map(|column| {
            match crate::tui::ui::header_target_at(
                &app.theme,
                &app.store,
                None,
                ratatui::layout::Rect::new(0, 0, 140, 2),
                column,
                0,
            ) {
                Some(crate::tui::ui::HeaderTarget::Query {
                    column: menu_column,
                }) => Some((column, menu_column)),
                _ => None,
            }
        })
        .unwrap();
    app.dispatch_mouse(header_click(view_column), (140, 24).into())
        .await
        .unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::HeaderMenu(state))
            if state.column == menu_column
                && state.row == 0
                && state.items.iter().any(|item| item.label == "ready")
                && state.items.iter().any(|item| item.label == "blocked")
                && state.items.iter().any(|item| item.label == "overdue")
                && state.items.iter().any(|item| item.label == "inbox")
    ));
    let inbox_row = match &app.overlay {
        Some(OverlayState::HeaderMenu(state)) => state
            .items
            .iter()
            .position(|item| item.label == "inbox")
            .map(|index| state.row.saturating_add(index as u16).saturating_add(2))
            .unwrap(),
        _ => unreachable!(),
    };

    app.dispatch_mouse(
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: menu_column.saturating_add(2),
            row: inbox_row,
            modifiers: KeyModifiers::NONE,
        },
        (140, 24).into(),
    )
    .await
    .unwrap();
    assert_eq!(app.store.view_state.query, TaskQuery::Inbox);
    assert!(app.overlay.is_none());
    assert_eq!(toast_message(&app), None);
}

#[tokio::test]
async fn header_click_switches_directly_when_two_workspaces_are_available() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    let mut conn = pool.acquire().await.unwrap();
    crate::workspaces::create_workspace(&mut conn, "Client Work")
        .await
        .unwrap();
    drop(conn);

    let workspace_column = (0..140)
        .find(|column| {
            matches!(
                crate::tui::ui::header_target_at(
                    &app.theme,
                    &app.store,
                    None,
                    ratatui::layout::Rect::new(0, 0, 140, 2),
                    *column,
                    0,
                ),
                Some(crate::tui::ui::HeaderTarget::Workspace { .. })
            )
        })
        .unwrap();
    app.dispatch_mouse(header_click(workspace_column), (140, 24).into())
        .await
        .unwrap();

    assert_eq!(app.store.active_workspace.key, "client-work");
    assert!(app.overlay.is_none());
    assert_eq!(toast_message(&app), None);

    reset_default_workspace(&pool).await;
}

#[tokio::test]
async fn header_click_opens_workspace_menu_when_more_than_two_are_available() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    let mut conn = pool.acquire().await.unwrap();
    crate::workspaces::create_workspace(&mut conn, "Client Work")
        .await
        .unwrap();
    crate::workspaces::create_workspace(&mut conn, "Team Space")
        .await
        .unwrap();
    drop(conn);

    let (workspace_column, menu_column) = (0..140)
        .find_map(|column| {
            match crate::tui::ui::header_target_at(
                &app.theme,
                &app.store,
                None,
                ratatui::layout::Rect::new(0, 0, 140, 2),
                column,
                0,
            ) {
                Some(crate::tui::ui::HeaderTarget::Workspace {
                    column: menu_column,
                }) => Some((column, menu_column)),
                _ => None,
            }
        })
        .unwrap();
    app.dispatch_mouse(header_click(workspace_column), (140, 24).into())
        .await
        .unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::HeaderMenu(state))
            if state.column == menu_column
                && state.row == 0
                && state.items.iter().map(|item| item.label.as_str()).eq([
                    "Client Work (client-work)",
                    "default",
                    "Team Space (team-space)",
                ])
    ));

    reset_default_workspace(&pool).await;
}

#[tokio::test]
async fn header_metric_click_still_selects_view_directly() {
    let mut app = test_app().await;
    app.show_view(TaskQuery::Todo).await.unwrap();

    let queue_column = (0..180)
        .find(|column| {
            matches!(
                crate::tui::ui::header_target_at(
                    &app.theme,
                    &app.store,
                    None,
                    ratatui::layout::Rect::new(0, 0, 180, 2),
                    *column,
                    0,
                ),
                Some(crate::tui::ui::HeaderTarget::MetricView(TaskQuery::Queue))
            )
        })
        .unwrap();
    app.dispatch_mouse(header_click(queue_column), (180, 24).into())
        .await
        .unwrap();
    assert_eq!(app.store.view_state.query, TaskQuery::Queue);
    assert_eq!(toast_message(&app), None);

    let mut app = test_app().await;
    let inbox_column = (0..180)
        .find(|column| {
            matches!(
                crate::tui::ui::header_target_at(
                    &app.theme,
                    &app.store,
                    None,
                    ratatui::layout::Rect::new(0, 0, 180, 2),
                    *column,
                    0,
                ),
                Some(crate::tui::ui::HeaderTarget::MetricView(TaskQuery::Inbox))
            )
        })
        .unwrap();
    app.dispatch_mouse(header_click(inbox_column), (180, 24).into())
        .await
        .unwrap();
    assert_eq!(app.store.view_state.query, TaskQuery::Inbox);
}

#[tokio::test]
async fn header_click_opens_sync_dialog() {
    let mut app = test_app().await;

    app.dispatch_mouse(header_click(135), (140, 24).into())
        .await
        .unwrap();

    let Some(OverlayState::Sync(state)) = &app.overlay else {
        panic!("expected sync dialog");
    };
    assert_eq!(*state, SyncDialogState::default());
}

#[tokio::test]
async fn header_click_opens_order_menu_and_selects_order() {
    let mut app = test_app().await;

    let (order_column, menu_column) = (0..140)
        .find_map(|column| {
            match crate::tui::ui::header_target_at(
                &app.theme,
                &app.store,
                None,
                ratatui::layout::Rect::new(0, 0, 140, 2),
                column,
                0,
            ) {
                Some(crate::tui::ui::HeaderTarget::Order {
                    column: menu_column,
                }) => Some((column, menu_column)),
                _ => None,
            }
        })
        .unwrap();
    app.dispatch_mouse(header_click(order_column), (140, 24).into())
        .await
        .unwrap();
    assert!(matches!(
        app.overlay,
        Some(OverlayState::OrderMenu(state))
            if state.column == menu_column
                && state.row == 0
                && state.selected == TaskOrder::Created
    ));

    app.dispatch_mouse(
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: order_column,
            row: 6,
            modifiers: KeyModifiers::NONE,
        },
        (140, 24).into(),
    )
    .await
    .unwrap();

    assert_eq!(app.store.view_state.query, TaskQuery::Open);
    assert_eq!(app.store.view_state.order, TaskOrder::Project);
    assert!(app.overlay.is_none());
    assert_eq!(toast_message(&app), None);
}

#[tokio::test]
async fn header_click_ignores_capturing_overlay() {
    let mut app = test_app().await;
    app.begin_search();

    app.dispatch_mouse(header_click(45), (140, 24).into())
        .await
        .unwrap();

    assert_eq!(app.store.view_state.query, TaskQuery::Queue);
    assert!(matches!(app.overlay, Some(OverlayState::Search(_))));
}

#[tokio::test]
async fn header_home_click_closes_detail_overlay() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Detail target")).await;
    app.show_detail(3);

    app.dispatch_mouse(header_click(2), (140, 24).into())
        .await
        .unwrap();

    assert!(app.overlay.is_none());
    assert!(!app.detail.is_active());
    assert_eq!(app.list.selected_task(), Some(0));
}

#[tokio::test]
async fn header_home_click_closes_picker_and_preserves_detail_underlay() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Detail target")).await;
    app.detail = crate::tui::detail_session::DetailSession::open(0);
    app.overlay = Some(OverlayState::Picker(PickerState {
        intent: PickerIntent::FilterLabel,
        title: "Pick".to_string(),
        filter: LineEdit::blank(),
        items: vec![PickerItem {
            label: "One".to_string(),
            value: "one".to_string(),
            selected: false,
        }],
        selected: 0,
        scroll: 0,
        multi: false,
        mode: PickerMode::Navigate,
    }));

    app.dispatch_mouse(header_click(2), (140, 24).into())
        .await
        .unwrap();

    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    assert_eq!(app.list.selected_task(), Some(0));
}

#[tokio::test]
async fn mouse_movement_without_hover_state_skips_redraw() {
    let mut app = test_app().await;

    let changed = app
        .dispatch_mouse(
            MouseEvent {
                kind: MouseEventKind::Moved,
                column: 10,
                row: 6,
                modifiers: KeyModifiers::NONE,
            },
            (80, 24).into(),
        )
        .await
        .unwrap();

    assert!(!changed);
}

#[tokio::test]
async fn mouse_wheel_moves_task_selection_down_and_up() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("first")).await;
    create_and_select_task(&mut app, test_task_draft("second")).await;
    app.list.select_task(Some(0));

    let changed = app
        .dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), (80, 24).into())
        .await
        .unwrap();
    assert!(changed);
    assert_eq!(app.list.selected_task(), Some(1));

    let changed = app
        .dispatch_mouse(mouse_wheel(MouseEventKind::ScrollUp), (80, 24).into())
        .await
        .unwrap();
    assert!(changed);
    assert_eq!(app.list.selected_task(), Some(0));
}

#[tokio::test]
async fn mouse_wheel_stops_at_task_list_edges() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("first")).await;
    create_and_select_task(&mut app, test_task_draft("second")).await;
    app.list.select_task(Some(0));

    let changed = app
        .dispatch_mouse(mouse_wheel(MouseEventKind::ScrollUp), (80, 24).into())
        .await
        .unwrap();
    assert!(!changed);
    assert_eq!(app.list.selected_task(), Some(0));

    app.list.select_task(Some(1));
    let changed = app
        .dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), (80, 24).into())
        .await
        .unwrap();
    assert!(!changed);
    assert_eq!(app.list.selected_task(), Some(1));
}

#[tokio::test]
async fn mouse_wheel_ignored_with_overlay() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("first")).await;
    create_and_select_task(&mut app, test_task_draft("second")).await;
    app.list.select_task(Some(0));
    app.begin_search();
    let selected = app.list.selected_task();

    app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), (80, 24).into())
        .await
        .unwrap();

    assert_eq!(app.list.selected_task(), selected);
    assert!(matches!(app.overlay, Some(OverlayState::Search(_))));
}

#[tokio::test]
async fn modal_overlay_mouse_does_not_reach_detail_underlay() {
    let mut app = test_app().await;
    let _ = create_and_select_task(&mut app, test_task_draft("task")).await;
    app.detail = crate::tui::detail_session::DetailSession::open(0);
    app.overlay = Some(OverlayState::Help { scroll: 0 });

    app.dispatch_mouse(left_click(2, 0), (80, 24).into())
        .await
        .unwrap();

    assert!(app.detail.is_active());
    assert_eq!(app.overlay, Some(OverlayState::Help { scroll: 0 }));
}

#[tokio::test]
async fn mouse_wheel_outside_sidebar_ignored_in_sidebar_focus() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("first")).await;
    create_and_select_task(&mut app, test_task_draft("second")).await;
    app.list.select_task(Some(0));
    app.list.focus_sidebar();
    let sidebar = app.list.selected_sidebar();

    app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), (80, 24).into())
        .await
        .unwrap();

    assert_eq!(app.list.selected_task(), Some(0));
    assert_eq!(app.list.focus(), Focus::Sidebar);
    assert_eq!(app.list.selected_sidebar(), sidebar);
}

#[tokio::test]
async fn mouse_wheel_ignored_with_detail_underlay() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("first")).await;
    create_and_select_task(&mut app, test_task_draft("second")).await;
    app.list.select_task(Some(0));
    app.detail = crate::tui::detail_session::DetailSession::open(0);

    app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), (80, 24).into())
        .await
        .unwrap();

    assert_eq!(app.list.selected_task(), Some(0));
    assert!(app.detail.is_active());
}

#[tokio::test]
async fn mouse_wheel_ignored_for_small_terminal() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("first")).await;
    create_and_select_task(&mut app, test_task_draft("second")).await;
    app.list.select_task(Some(0));

    app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), (69, 24).into())
        .await
        .unwrap();
    assert_eq!(app.list.selected_task(), Some(0));

    app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), (80, 17).into())
        .await
        .unwrap();
    assert_eq!(app.list.selected_task(), Some(0));
}

#[tokio::test]
async fn picker_row_click_submits_clicked_row() {
    let mut app = test_app().await;
    let size = (100, 24).into();
    app.begin_filter_priority();

    app.dispatch_mouse(picker_row_click(&app, 2, size), size)
        .await
        .unwrap();

    assert_eq!(
        app.store.view_state.filter_modifiers.priority.as_deref(),
        Some("medium")
    );
    assert!(app.overlay.is_none());
}

#[tokio::test]
async fn picker_row_click_toggles_multi_select_row() {
    let mut app = test_app().await;
    let size = (100, 24).into();
    app.store.create_label("bug".to_string()).await.unwrap();
    create_and_select_task(
        &mut app,
        TaskDraft {
            metadata: Vec::new(),
            labels: vec!["bug".to_string()],
            ..test_task_draft("Labeled target")
        },
    )
    .await;
    app.begin_edit_labels();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::TagCombobox(state))
            if state.options.iter().any(|item| item == "bug")
                && state.selected.iter().any(|item| item == "bug")
    ));

    app.dispatch_mouse(picker_row_click(&app, 0, size), size)
        .await
        .unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::TagCombobox(state))
            if state.options.iter().any(|item| item == "bug")
                && !state.selected.iter().any(|item| item == "bug")
    ));
}

#[tokio::test]
async fn text_panel_mouse_scrolls_and_closes_outside() {
    let mut app = test_app().await;
    let size = (100, 24).into();
    app.overlay = Some(OverlayState::TextPanel(TextPanelState::new(
        "Panel",
        (0..20).map(|index| format!("line {index}")).collect(),
    )));

    app.dispatch_mouse(wheel_down(50, 12), size).await.unwrap();

    assert!(matches!(
        app.overlay,
        Some(OverlayState::TextPanel(TextPanelState { scroll: 1, .. }))
    ));

    app.dispatch_mouse(left_click(0, 0), size).await.unwrap();

    assert!(app.overlay.is_none());
}

#[tokio::test]
async fn sidebar_workspaces_keep_key_order_counts_and_target_after_switching() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Default task")).await;
    for name in ["Zulu", "Alpha", "Empty"] {
        app.store.create_workspace(name.to_string()).await.unwrap();
    }
    let keys: Vec<_> = app
        .store
        .sidebar_entries
        .iter()
        .filter_map(|entry| match &entry.target {
            Some(SidebarEntryTarget::Workspace(key)) => Some(key.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(keys, ["alpha", "default", "empty", "zulu"]);
    app.submit_switch_workspace(vec!["alpha".to_string()])
        .await
        .unwrap();
    app.store.create_project("Extra".to_string()).await.unwrap();
    create_and_select_task(&mut app, test_task_draft("Alpha task")).await;
    let mut deferred = test_task_draft("Deferred");
    deferred.available_at = Some("2999-01-01T00:00:00Z".to_string());
    create_and_select_task(&mut app, deferred).await;
    app.show_scope(TaskScopeTarget::Project("extra".to_string()))
        .await
        .unwrap();
    app.submit_filter_priority(vec!["urgent".to_string()])
        .await
        .unwrap();
    let rows: Vec<_> = app
        .store
        .sidebar_entries
        .iter()
        .filter_map(|entry| {
            if let Some(SidebarEntryTarget::Workspace(key)) = &entry.target {
                Some((key.as_str(), entry.count))
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        rows,
        [("alpha", 1), ("default", 1), ("empty", 0), ("zulu", 0)]
    );
    assert!(app.store.tasks.is_empty());
    let target = SidebarEntryTarget::Workspace("default".to_string());
    app.apply_sidebar_target(Some(target.clone()))
        .await
        .unwrap();
    assert_eq!(app.store.active_workspace.key, "default");
    assert_eq!(app.store.view_state.scope, TaskScope::Workspace);
    assert_eq!(app.store.view_state.filter_modifiers, Default::default());
    assert_eq!(
        app.list.sidebar_entries()[app.list.selected_sidebar().unwrap()].target,
        Some(target)
    );
    assert!(
        app.store
            .projects
            .iter()
            .all(|project| project.key != "extra")
    );
    assert!(
        app.store
            .tasks
            .iter()
            .all(|task| task.task.title == "Default task")
    );
    app.go_back().await.unwrap();
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("no previous navigation state")
    );

    app.submit_filter_priority(vec!["high".to_string()])
        .await
        .unwrap();
    app.apply_sidebar_target(Some(SidebarEntryTarget::Workspace("default".to_string())))
        .await
        .unwrap();
    assert_eq!(
        app.store.view_state.filter_modifiers.priority.as_deref(),
        Some("high")
    );
}

#[tokio::test]
async fn workspace_deletion_requires_typed_key_and_falls_back_to_default() {
    let mut app = test_app().await;
    aven_core::test_support::set_established_protocol(&app.store.database(), 19)
        .await
        .unwrap();
    app.store
        .create_workspace("Side".to_string())
        .await
        .unwrap();
    app.store
        .switch_workspace("side".to_string())
        .await
        .unwrap();
    create_and_select_task(&mut app, test_task_draft("Side task")).await;

    app.execute(Action::BeginDeleteWorkspace).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Picker(PickerState {
            intent: PickerIntent::DeleteWorkspace,
            items,
            ..
        })) if items.iter().map(|item| item.value.as_str()).collect::<Vec<_>>() == ["side"]
    ));

    app.submit_delete_workspace_picker(vec!["side".to_string()])
        .await
        .unwrap();
    let Some(OverlayState::TextInput(TextInputState {
        intent:
            TextIntent::ConfirmDeleteWorkspace {
                workspace,
                warnings,
            },
        prompt,
        ..
    })) = app.overlay.take()
    else {
        panic!("expected workspace deletion confirmation");
    };
    assert_eq!(
        warnings,
        ["It still has 1 tasks (1 open), 0 recurring schedules and 0 unresolved conflicts."]
    );
    assert!(prompt.ends_with("Type side to delete workspace Side on every device:"));

    app.submit_delete_workspace_name(workspace.clone(), warnings.clone(), "Side".to_string())
        .await
        .unwrap();
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("workspace key does not match")
    );
    assert_eq!(app.store.active_workspace.key, "side");
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::TextInput(TextInputState {
            intent: TextIntent::ConfirmDeleteWorkspace { .. },
            ..
        }))
    ));

    app.overlay = None;
    app.submit_delete_workspace_name(workspace, warnings, "side".to_string())
        .await
        .unwrap();
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("deleted workspace side; switched to default")
    );
    assert_eq!(app.store.active_workspace.key, "default");
    assert!(
        app.store
            .workspaces
            .iter()
            .all(|workspace| workspace.key != "side")
    );
    assert!(app.store.tasks.is_empty());
    assert!(!app.report_deleted_active_workspace());
}

#[tokio::test]
async fn workspace_deletion_elsewhere_switches_to_default_on_refresh() {
    let mut app = test_app().await;
    let database = app.store.database();
    aven_core::test_support::set_established_protocol(&database, 19)
        .await
        .unwrap();
    app.store
        .create_workspace("Side".to_string())
        .await
        .unwrap();
    app.store
        .switch_workspace("side".to_string())
        .await
        .unwrap();
    let side = app.store.active_workspace.id.clone();

    database.delete_workspace(&side, &[], true).await.unwrap();
    app.store.refresh(None).await.unwrap();

    assert_eq!(app.store.active_workspace.key, "default");
    assert!(app.report_deleted_active_workspace());
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("workspace Side was deleted; switched to default")
    );
    assert_eq!(toast_severity(&app), Some(ToastSeverity::Warning));
    assert!(!app.report_deleted_active_workspace());
}

#[tokio::test]
async fn workspace_deletion_reports_unavailable_and_cancellation() {
    let mut app = test_app().await;
    app.store
        .create_workspace("Side".to_string())
        .await
        .unwrap();

    app.begin_delete_workspace();
    app.cancel_overlay();
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("workspace deletion canceled")
    );

    let workspace = app
        .store
        .workspaces
        .iter()
        .find(|workspace| workspace.key == "side")
        .cloned()
        .unwrap();
    app.submit_delete_workspace_name(workspace, Vec::new(), "side".to_string())
        .await
        .unwrap();
    assert_eq!(toast_severity(&app), Some(ToastSeverity::Error));
    assert!(
        toast_message(&app).is_some_and(|message| message.contains("workspace-delete-unavailable"))
    );
    assert!(
        app.store
            .workspaces
            .iter()
            .any(|workspace| workspace.key == "side")
    );
}

async fn app_in_deleted_side_workspace() -> App {
    let mut app = test_app().await;
    let database = app.store.database();
    aven_core::test_support::set_established_protocol(&database, 19)
        .await
        .unwrap();
    app.store
        .create_workspace("Side".to_string())
        .await
        .unwrap();
    app.store
        .switch_workspace("side".to_string())
        .await
        .unwrap();
    create_and_select_task(&mut app, test_task_draft("Side task")).await;
    app
}

#[tokio::test]
async fn workspace_fallback_does_not_restore_the_open_detail_task() {
    let mut app = app_in_deleted_side_workspace().await;
    let task_id = app.store.tasks[0].task.id.clone();
    app.open_detail_task(&task_id, 0).await;
    assert!(app.detail.is_active());

    let side = app.store.active_workspace.id.clone();
    app.store
        .database()
        .delete_workspace(&side, &[], true)
        .await
        .unwrap();
    app.refresh().await.unwrap();

    assert_eq!(app.store.active_workspace.key, "default");
    assert!(app.store.tasks.iter().all(|item| item.task.id != task_id));
}

#[tokio::test]
async fn workspace_fallback_does_not_restore_visible_deleted_rows() {
    let mut app = app_in_deleted_side_workspace().await;
    let task_id = app.store.tasks[0].task.id.clone();
    app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('D')).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Char('y')))
        .await
        .unwrap();
    assert!(app.store.tasks.iter().any(|item| item.task.id == task_id));

    let side = app.store.active_workspace.id.clone();
    app.store
        .database()
        .delete_workspace(&side, &[], true)
        .await
        .unwrap();
    app.store
        .refresh_preserving_visible_deleted(None)
        .await
        .unwrap();

    assert_eq!(app.store.active_workspace.key, "default");
    assert!(app.store.tasks.iter().all(|item| item.task.id != task_id));
}
