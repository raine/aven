use super::*;
use crate::tui::detail_session::DetailSession;

#[tokio::test]
async fn ctrl_c_quits_from_normal_mode() {
    let mut app = test_app().await;
    app.dispatch_key(ctrl_c(), (80, 24).into()).await.unwrap();
    assert!(app.should_quit);
}

#[tokio::test]
async fn ctrl_c_quits_while_overlay_captures_input() {
    let mut app = test_app().await;
    app.begin_search();
    app.dispatch_key(ctrl_c(), (80, 24).into()).await.unwrap();
    assert!(app.should_quit);
}

#[tokio::test]
async fn prefix_key_enters_prefix_mode() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
    assert_pending(&app, &["t"]);
}

#[tokio::test]
async fn add_task_alias_executes_immediately() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
    assert_pending_empty(&app);
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::AddTask(state)) if state.focus == AddTaskStep::Title
    ));
}

#[tokio::test]
async fn normal_dispatch_ignores_modified_shortcuts() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Priority target")).await;

    app.dispatch_key(ctrl_p(), (80, 24).into()).await.unwrap();

    assert!(app.overlay.is_none());
    assert_pending_empty(&app);

    app.dispatch_key(key(KeyCode::Char('e')), (80, 24).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char('p')), (80, 24).into())
        .await
        .unwrap();

    assert_eq!(
        app.footer_choice.as_ref().map(|choice| choice.mode),
        Some(FooterChoiceMode::Priority)
    );
}

#[tokio::test]
async fn prefix_is_inactive_while_overlay_captures_input() {
    let mut app = test_app().await;
    app.begin_search();
    app.handle_normal_key(KeyCode::Char('t')).await.unwrap();

    assert_pending_empty(&app);
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Search(state)) if state.input.as_str() == "t"
    ));
}

#[tokio::test]
async fn esc_cancels_prefix_before_overlay() {
    let mut app = test_app().await;
    app.show_detail(0);
    app.dispatch_key(key(KeyCode::Char('t')), (80, 24).into())
        .await
        .unwrap();
    assert_pending(&app, &["t"]);
    app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
        .await
        .unwrap();
    assert_pending_empty(&app);
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());

    app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
        .await
        .unwrap();
    assert!(app.overlay.is_none());
}

#[tokio::test]
async fn invalid_continuation_shows_message() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('z')).await.unwrap();
    assert_pending_empty(&app);
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("invalid shortcut: t z")
    );
}

#[tokio::test]
async fn valid_continuation_executes_and_clears() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
    assert_pending_empty(&app);
}

#[tokio::test]
async fn order_shortcut_sets_sort() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('o')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('p')).await.unwrap();
    assert_eq!(app.store.view_state.order, TaskOrder::Priority);
    assert_eq!(toast_message(&app).as_deref(), Some("order priority asc"));
}

#[tokio::test]
async fn created_order_shortcut_defaults_to_descending() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('o')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
    assert_eq!(app.store.view_state.order, TaskOrder::Created);
    assert_eq!(app.store.sort_direction_label(), "desc");
    assert_eq!(toast_message(&app).as_deref(), Some("order created desc"));
}

#[tokio::test]
async fn order_reverse_shortcut_toggles_direction() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('o')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('r')).await.unwrap();
    assert_eq!(app.store.sort_direction_label(), "desc");
    assert_eq!(toast_message(&app).as_deref(), Some("order created desc"));
}

#[tokio::test]
async fn due_order_shortcut_sets_sort() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('o')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('d')).await.unwrap();
    assert_eq!(app.store.view_state.order, TaskOrder::DueOn);
    assert_eq!(toast_message(&app).as_deref(), Some("order due asc"));
}

#[tokio::test]
async fn h_and_l_move_between_sidebar_and_tasks() {
    let mut app = test_app().await;
    app.list.focus_tasks();
    app.handle_normal_key(KeyCode::Char('h')).await.unwrap();
    assert_eq!(app.list.focus(), Focus::Sidebar);

    app.handle_normal_key(KeyCode::Char('l')).await.unwrap();
    assert_eq!(app.list.focus(), Focus::Tasks);
}

#[tokio::test]
async fn sidebar_selection_survives_focus_changes() {
    let mut app = test_app().await;
    app.list.focus_tasks();

    app.handle_normal_key(KeyCode::Char('h')).await.unwrap();
    let initial = app.list.selected_sidebar();
    app.handle_normal_key(KeyCode::Char('j')).await.unwrap();
    let selected = app.list.selected_sidebar();
    assert_ne!(selected, initial);

    app.handle_normal_key(KeyCode::Char('l')).await.unwrap();
    assert_eq!(app.list.focus(), Focus::Tasks);
    app.handle_normal_key(KeyCode::Char('h')).await.unwrap();

    assert_eq!(app.list.focus(), Focus::Sidebar);
    assert_eq!(app.list.selected_sidebar(), selected);
}

#[tokio::test]
async fn sidebar_toggle_shortcut_expands_task_list_and_restores_sidebar_focus() {
    let mut app = test_app().await;
    app.list.focus_sidebar();

    app.handle_normal_key(KeyCode::Char('g')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('s')).await.unwrap();

    assert!(!app.list.sidebar_visible());
    assert_eq!(app.list.focus(), Focus::Tasks);
    assert_eq!(toast_message(&app).as_deref(), Some("task list expanded"));
    let hidden = app.view();
    assert!(!hidden.sidebar_visible);

    app.handle_normal_key(KeyCode::Char('g')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('s')).await.unwrap();

    assert!(app.list.sidebar_visible());
    assert_eq!(app.list.focus(), Focus::Sidebar);
    assert_eq!(toast_message(&app).as_deref(), Some("sidebar visible"));
}

#[tokio::test]
async fn command_panel_runs_sidebar_toggle() {
    let mut app = test_app().await;

    app.begin_command().await;
    type_chars(&mut app, "toggle-sidebar").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(!app.list.sidebar_visible());
    assert_eq!(app.list.focus(), Focus::Tasks);
    assert!(app.overlay.is_none());
}

#[tokio::test]
async fn hidden_sidebar_allows_wide_task_click_at_left_edge() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("task one")).await;
    create_and_select_task(&mut app, test_task_draft("task two")).await;
    app.list.select_task(Some(1));
    app.list.hide_sidebar();

    let area = ratatui::layout::Rect::new(0, 2, 140, 20);
    let mut click = None;
    'rows: for row in area.y..area.y.saturating_add(area.height) {
        for column in area.x..area.x.saturating_add(area.width) {
            if crate::tui::ui::task_at_position(
                &app.store,
                app.list.table_state(),
                area,
                column,
                row,
            )
            .is_some_and(|hit| hit.task_index == 0)
            {
                click = Some(task_row_click(column, row));
                break 'rows;
            }
        }
    }
    let click = click.expect("expected task hit target");

    app.dispatch_mouse(click, (140, 24).into()).await.unwrap();

    assert_eq!(app.list.selected_task(), Some(0));
    assert_eq!(app.list.focus(), Focus::Tasks);
}

#[tokio::test]
async fn hidden_sidebar_is_not_rendered_in_wide_layout() {
    let mut app = test_app().await;
    app.list.hide_sidebar();

    let text = render_app_text(&mut app, 140, 24);

    assert!(!text.contains("FILTERS"));
}

#[tokio::test]
async fn h_reveals_sidebar() {
    let mut app = test_app().await;
    app.list.hide_sidebar();

    app.handle_normal_key(KeyCode::Char('h')).await.unwrap();

    assert!(app.list.sidebar_visible());
    assert_eq!(app.list.focus(), Focus::Sidebar);
}

#[tokio::test]
async fn tab_reveals_sidebar_when_task_list_is_expanded() {
    let mut app = test_app().await;
    app.list.hide_sidebar();

    app.handle_normal_key(KeyCode::Tab).await.unwrap();

    assert!(app.list.sidebar_visible());
    assert_eq!(app.list.focus(), Focus::Sidebar);
    assert_eq!(toast_message(&app).as_deref(), Some("sidebar visible"));
}

#[tokio::test]
async fn implemented_commands_execute_their_flows() {
    let mut app = test_app().await;

    app.handle_normal_key(KeyCode::Char('o')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('d')).await.unwrap();
    assert_eq!(app.store.view_state.order, TaskOrder::DueOn);
    assert_eq!(toast_message(&app).as_deref(), Some("order due asc"));

    app.begin_command().await;
    type_chars(&mut app, "add-project-path").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(matches!(
        app.overlay,
        Some(OverlayState::Picker(PickerState {
            intent: PickerIntent::AddProjectPath,
            ..
        }))
    ));
}

#[tokio::test]
async fn esc_closes_every_overlay_variant() {
    let overlays = vec![
        OverlayState::Onboarding {
            persist_on_exit: false,
        },
        OverlayState::Help { scroll: 0 },
        OverlayState::Detail,
        OverlayState::DetailHelp { scroll: 0 },
        OverlayState::Search(SearchState {
            input: LineEdit::new("q".to_string()),
            results: Vec::new(),
            selected: 0,
            total_matches: 0,
            results_query: None,
            intent: SearchIntent::Navigate,
        }),
        OverlayState::Command {
            state: CommandState::new(LineEdit::new("ref".to_string())),
        },
        OverlayState::TextInput(TextInputState::new(
            TextIntent::AddProject,
            "T",
            "P",
            "x".to_string(),
        )),
        OverlayState::MultilineInput(MultilineInputState::from_value(
            MultilineIntent::AddTaskNatural,
            "M",
            "P",
            "x".to_string(),
        )),
        OverlayState::Picker(PickerState {
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
        }),
        OverlayState::Confirm(ConfirmState {
            intent: ConfirmIntent::InitializeConfig {
                path: std::path::PathBuf::from("/tmp/config.toml"),
            },
            title: "C".to_string(),
            prompt: "?".to_string(),
        }),
        OverlayState::TextPanel(TextPanelState {
            title: "Panel".to_string(),
            lines: vec!["line".to_string()],
            scroll: 0,
        }),
        OverlayState::SyncStatus(Box::default()),
    ];

    for overlay in overlays {
        let detail_help = matches!(overlay, OverlayState::DetailHelp { .. });
        let mut app = test_app().await;
        if detail_help {
            app.detail = DetailSession::open(0);
        }
        app.overlay = Some(overlay);
        app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
            .await
            .unwrap();
        assert!(app.overlay.is_none());
        assert_eq!(app.detail.is_active(), detail_help);
        assert_pending_empty(&app);
    }
}

#[tokio::test]
async fn mark_shortcuts_update_task_marks() {
    let mut app = test_app().await;
    let first = create_and_select_task(&mut app, test_task_draft("first")).await;
    create_and_select_task(&mut app, test_task_draft("second")).await;
    app.list.select_task(Some(first));
    let first_id = app.store.tasks[first].task.id.clone();
    app.notification = None;

    app.handle_normal_key(KeyCode::Char(' ')).await.unwrap();
    assert!(app.list.marked_task_ids().contains(&first_id));
    assert!(toast_message(&app).is_none());

    app.handle_normal_key(KeyCode::Char(' ')).await.unwrap();
    assert!(!app.list.marked_task_ids().contains(&first_id));
    assert!(toast_message(&app).is_none());

    app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('V')).await.unwrap();
    assert_eq!(app.list.marked_task_ids().len(), 2);
    assert!(toast_message(&app).is_none());

    app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('V')).await.unwrap();
    assert!(app.list.marked_task_ids().is_empty());
    assert!(toast_message(&app).is_none());

    app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('V')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('C')).await.unwrap();
    assert!(app.list.marked_task_ids().is_empty());
    assert!(toast_message(&app).is_none());
}
