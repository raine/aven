use super::*;

#[tokio::test]
async fn add_note_from_detail_returns_to_detail() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Note target")).await;
    app.show_detail(0);

    app.dispatch_key(key(KeyCode::Char('n')), (80, 24).into())
        .await
        .unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::MultilineInput(state)) if state.title == ADD_NOTE_TITLE
    ));
    assert!(app.view().detail_underlay);

    type_chars(&mut app, "Important detail").await;
    app.handle_overlay_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL))
        .await
        .unwrap();

    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    let selected = app.list.selected_task().unwrap();
    assert_eq!(app.store.tasks[selected].notes.len(), 1);
}

#[tokio::test]
async fn availability_editor_sets_task_from_task_list() {
    let mut app = test_app().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Defer from list")).await;
    let task_id = app.store.tasks[selected].task.id.clone();

    app.dispatch_key(key(KeyCode::Char('e')), (100, 30).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char('a')), (100, 30).into())
        .await
        .unwrap();

    let rendered = render_app_text(&mut app, 100, 30);
    assert!(rendered.contains(EDIT_AVAILABILITY_TITLE));
    assert!(rendered.contains("Try tomorrow · in 2 weeks · next monday at 9am"));
    assert!(rendered.contains("Local dates/times · empty or now = immediate"));
    type_chars(&mut app, "tomorrow").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    app.show_view(TaskQuery::Upcoming).await.unwrap();
    let task = app
        .store
        .tasks
        .iter()
        .find(|item| item.task.id == task_id)
        .unwrap();
    assert!(task.task.available_at.is_some());
}

#[tokio::test]
async fn availability_editor_keeps_invalid_input_open() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Invalid availability")).await;
    app.begin_edit_availability();
    type_chars(&mut app, "someday maybe").await;

    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::TextInput(state))
            if matches!(state.intent, TextIntent::EditAvailability { .. })
                && state.input.text == "someday maybe"
    ));
    assert_eq!(toast_severity(&app), Some(ToastSeverity::Warning));
    assert!(toast_message(&app).is_some_and(|message| message.contains("use today, tomorrow")));
}

#[tokio::test]
async fn due_editor_sets_clears_and_undoes_deadline() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Due from list")).await;

    app.dispatch_key(key(KeyCode::Char('e')), (100, 30).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char('u')), (100, 30).into())
        .await
        .unwrap();

    let rendered = render_app_text(&mut app, 100, 30);
    assert!(rendered.contains(EDIT_DUE_TITLE));
    assert!(rendered.contains("Try today · tomorrow · in 2 weeks · next monday"));
    type_chars(&mut app, "2099-01-01").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    let selected = app.list.selected_task().unwrap();
    assert_eq!(
        app.store.tasks[selected].task.due_on.as_deref(),
        Some("2099-01-01")
    );

    app.begin_edit_due();
    app.handle_overlay_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL))
        .await
        .unwrap();
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    let selected = app.list.selected_task().unwrap();
    assert!(app.store.tasks[selected].task.due_on.is_none());

    app.handle_normal_key(KeyCode::Char('u')).await.unwrap();
    let selected = app.list.selected_task().unwrap();
    assert_eq!(
        app.store.tasks[selected].task.due_on.as_deref(),
        Some("2099-01-01")
    );
}

#[tokio::test]
async fn detail_availability_editor_reschedules_and_clears_task() {
    let mut app = test_app().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Detail availability")).await;
    let task_id = app.store.tasks[selected].task.id.clone();
    app.show_detail(2);

    for value in ["2099-01-01", "2099-02-01"] {
        let expected = crate::time_input::parse_available_at_input(value).unwrap();
        app.dispatch_key(key(KeyCode::Char('e')), (100, 30).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('a')), (100, 30).into())
            .await
            .unwrap();
        app.handle_overlay_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL))
            .await
            .unwrap();
        type_chars(&mut app, value).await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
        let selected = app.list.selected_task().unwrap();
        assert_eq!(
            app.store.tasks[selected].task.available_at.as_deref(),
            Some(expected.as_str())
        );

        app.refresh().await.unwrap();
        let selected = app.list.selected_task().unwrap();
        assert_eq!(app.store.tasks[selected].task.id, task_id);
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
    }

    app.dispatch_key(key(KeyCode::Char('t')), (100, 30).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char('e')), (100, 30).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char('a')), (100, 30).into())
        .await
        .unwrap();
    app.handle_overlay_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL))
        .await
        .unwrap();
    type_chars(&mut app, "now").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    let selected = app.list.selected_task().unwrap();
    assert!(app.store.tasks[selected].task.available_at.is_none());
}

#[tokio::test]
async fn leaving_detail_reconciles_deferred_task_with_active_view() {
    let mut app = test_app().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Leave detail")).await;
    let task_id = app.store.tasks[selected].task.id.clone();
    app.show_detail(0);

    app.dispatch_key(key(KeyCode::Char('e')), (100, 30).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char('a')), (100, 30).into())
        .await
        .unwrap();
    type_chars(&mut app, "2099-01-01").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(app.store.tasks.iter().any(|item| item.task.id == task_id));

    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();

    assert!(app.overlay.is_none());
    assert!(app.store.tasks.iter().all(|item| item.task.id != task_id));
}

#[tokio::test]
async fn task_list_availability_editor_clears_with_empty_input() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Clear from list")).await;
    app.store
        .update_availability(
            app.list.selected_task(),
            "2099-01-01T00:00:00Z".to_string(),
            true,
        )
        .await
        .unwrap();
    app.show_view(TaskQuery::Upcoming).await.unwrap();
    app.list.select_task(Some(0));

    app.begin_edit_availability();
    app.handle_overlay_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL))
        .await
        .unwrap();
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    app.show_view(TaskQuery::Queue).await.unwrap();
    assert!(
        app.store
            .tasks
            .iter()
            .any(|item| item.task.title == "Clear from list" && item.task.available_at.is_none())
    );
}

#[tokio::test]
async fn detail_mutation_targets_selected_task_when_tasks_are_marked() {
    let mut app = test_app().await;
    let marked_index = create_and_select_task(&mut app, test_task_draft("Marked task")).await;
    let marked_id = app.store.tasks[marked_index].task.id.clone();
    let selected_index = create_and_select_task(&mut app, test_task_draft("Detail target")).await;
    let selected_id = app.store.tasks[selected_index].task.id.clone();
    app.store.show_view(TaskQuery::All).await.unwrap();
    app.list.mark(marked_id.clone());
    app.list.mark(selected_id.clone());
    app.list.select_task(
        app.store
            .tasks
            .iter()
            .position(|item| item.task.id == selected_id),
    );
    app.show_detail(0);

    for code in [KeyCode::Char('e'), KeyCode::Char('t')] {
        app.dispatch_key(key(code), (80, 24).into()).await.unwrap();
    }

    let selection = match &app.overlay {
        Some(OverlayState::TextInput(state)) => match &state.intent {
            TextIntent::EditTitle { selection } => selection.clone(),
            intent => panic!("expected title edit intent, got {intent:?}"),
        },
        overlay => panic!("expected title editor, got {overlay:?}"),
    };
    assert_eq!(selection.len(), 1);
    assert_eq!(selection.single_id(), Some(&selected_id));

    app.handle_overlay_key(key(KeyCode::End)).await.unwrap();
    type_chars(&mut app, " updated").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert_eq!(
        app.store
            .tasks
            .iter()
            .find(|item| item.task.id == marked_id)
            .unwrap()
            .task
            .title,
        "Marked task"
    );
    assert_eq!(
        app.store
            .tasks
            .iter()
            .find(|item| item.task.id == selected_id)
            .unwrap()
            .task
            .title,
        "Detail target updated"
    );

    app.dispatch_key(key(KeyCode::Char('t')), (80, 24).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char('d')), (80, 24).into())
        .await
        .unwrap();

    assert_eq!(
        app.store
            .tasks
            .iter()
            .find(|item| item.task.id == marked_id)
            .unwrap()
            .task
            .status,
        TaskStatus::Inbox
    );
    assert_eq!(
        app.store
            .tasks
            .iter()
            .find(|item| item.task.id == selected_id)
            .unwrap()
            .task
            .status,
        TaskStatus::Done
    );
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
}

#[tokio::test]
async fn detail_delete_confirmation_names_selected_task_when_tasks_are_marked() {
    let mut app = test_app().await;
    let marked_index = create_and_select_task(&mut app, test_task_draft("Marked task")).await;
    let marked_id = app.store.tasks[marked_index].task.id.clone();
    let selected_index = create_and_select_task(&mut app, test_task_draft("Detail target")).await;
    let selected = app.store.tasks[selected_index].clone();
    app.list.mark(marked_id);
    app.list.mark(selected.task.id.clone());
    app.list.select_task(Some(selected_index));
    app.show_detail(0);

    app.begin_delete_task();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Confirm(state))
            if state.prompt == format!("Delete {} {}?", selected.display_ref, selected.task.title)
                && matches!(
                    &state.intent,
                    ConfirmIntent::DeleteTasks { selection }
                        if selection.len() == 1
                            && selection.single_id() == Some(&selected.task.id)
                )
    ));
}

#[tokio::test]
async fn detail_shortcuts_do_not_leave_detail_before_opening_overlay() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Detail target")).await;
    app.show_detail(0);

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
    assert!(app.view().detail_underlay);
}

#[tokio::test]
async fn edit_title_from_detail_renders_inline_cursor() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Detail title target")).await;
    app.show_detail(5);

    app.dispatch_key(key(KeyCode::Char('t')), (100, 30).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char('e')), (100, 30).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char('t')), (100, 30).into())
        .await
        .unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::TextInput(state)) if matches!(state.intent, TextIntent::EditTitle { .. })
    ));
    assert!(app.view().detail_underlay);
    assert_eq!(app.view().detail_underlay_scroll, 0);

    let buffer = render_app_buffer(&mut app, 100, 30);
    let rendered = buffer
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();

    assert!(rendered.contains("Detail title target"));
    assert!(!rendered.contains("Edit title"));
    assert!(!rendered.contains("Enter submit"));
    assert!(
        !buffer[(99, 10)]
            .modifier
            .contains(ratatui::style::Modifier::DIM)
    );
}

#[tokio::test]
async fn cancel_edit_title_from_detail_returns_to_detail() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Detail title target")).await;
    app.show_detail(0);

    app.dispatch_key(key(KeyCode::Char('t')), (100, 30).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char('e')), (100, 30).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char('t')), (100, 30).into())
        .await
        .unwrap();
    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();

    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    assert!(app.authoring.is_idle());
}

#[tokio::test]
async fn committed_title_edit_refresh_failure_does_not_reopen_editor() {
    let mut app = test_app().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Original title")).await;
    let task_id = app.store.tasks[selected].task.id.clone();
    app.begin_edit_title();
    app.handle_overlay_key(key(KeyCode::End)).await.unwrap();
    type_chars(&mut app, " changed").await;
    app.store.fail_next_refresh();

    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(!matches!(app.overlay, Some(OverlayState::TextInput(_))));
    assert!(toast_message(&app).is_some_and(|message| message.contains("mutation committed")));
    let persisted = app.store.load_task_item(&task_id).await.unwrap().unwrap();
    assert_eq!(persisted.task.title, "Original title changed");
}

#[tokio::test]
async fn submit_edit_title_from_detail_returns_to_detail() {
    let mut app = test_app().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Detail title target")).await;
    app.show_detail(0);

    app.dispatch_key(key(KeyCode::Char('t')), (100, 30).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char('e')), (100, 30).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char('t')), (100, 30).into())
        .await
        .unwrap();
    app.handle_overlay_key(key(KeyCode::End)).await.unwrap();
    type_chars(&mut app, " updated").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    assert_eq!(
        app.store.tasks[selected].task.title,
        "Detail title target updated"
    );
}

#[tokio::test]
async fn detail_edit_chords_open_advertised_editors() {
    let mut app = test_app().await;
    app.store
        .create_project("Mobile App".to_string())
        .await
        .unwrap();
    app.store.create_label("Bug".to_string()).await.unwrap();
    create_and_select_task(
        &mut app,
        TaskDraft {
            metadata: Vec::new(),
            title: "Detail target".to_string(),
            description: "existing description".to_string(),
            project: None,
            status: "inbox".to_string(),
            priority: "none".to_string(),
            source: TaskSource::Unknown,
            labels: vec!["bug".to_string()],
            available_at: None,
            due_on: None,
            is_epic: false,
        },
    )
    .await;

    for (events, expected_intent) in [
        (
            vec![key(KeyCode::Char('e')), key(KeyCode::Char('l'))],
            "labels",
        ),
        (vec![shift_key(KeyCode::Char('N'))], "note"),
        (vec![shift_key(KeyCode::Char('D'))], "delete"),
    ] {
        app.show_detail(4);
        app.footer_choice = None;
        app.dispatch_key(key(KeyCode::Char('t')), (80, 24).into())
            .await
            .unwrap();
        assert_pending(&app, &["t"]);
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());

        for event in events {
            app.dispatch_key(event, (80, 24).into()).await.unwrap();
        }
        let matches_intent = match (&app.overlay, expected_intent) {
            (Some(OverlayState::TagCombobox(state)), "labels") => {
                matches!(state.intent, TagComboboxIntent::EditLabels { .. })
            }
            (Some(OverlayState::MultilineInput(state)), "note") => {
                matches!(state.intent, MultilineIntent::AddNote { .. })
            }
            (Some(OverlayState::Confirm(state)), "delete") => {
                matches!(state.intent, ConfirmIntent::DeleteTasks { .. })
            }
            _ => false,
        };
        assert!(
            matches_intent,
            "expected {expected_intent}, got {:?}",
            app.overlay
        );
        assert_pending_empty(&app);
        assert!(app.view().detail_underlay);
        assert_eq!(app.view().detail_underlay_scroll, 4);
    }

    for (events, expected_mode) in [
        (vec![key(KeyCode::Char('s'))], FooterChoiceMode::Status),
        (
            vec![key(KeyCode::Char('e')), key(KeyCode::Char('p'))],
            FooterChoiceMode::Priority,
        ),
    ] {
        app.show_detail(4);
        app.footer_choice = None;
        app.dispatch_key(key(KeyCode::Char('t')), (80, 24).into())
            .await
            .unwrap();
        assert_pending(&app, &["t"]);
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());

        for event in events {
            app.dispatch_key(event, (80, 24).into()).await.unwrap();
        }
        assert_eq!(
            app.footer_choice.as_ref().map(|choice| choice.mode),
            Some(expected_mode)
        );
        assert_pending_empty(&app);
        assert!(app.view().detail_underlay);
        assert_eq!(app.view().detail_underlay_scroll, 4);
    }
}

#[tokio::test]
async fn detail_single_key_edit_shortcuts_still_work() {
    let mut app = test_app().await;
    app.store.create_label("Bug".to_string()).await.unwrap();
    create_and_select_task(&mut app, test_task_draft("Detail target")).await;
    app.show_detail(3);

    app.dispatch_key(key(KeyCode::Char('e')), (80, 24).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char('l')), (80, 24).into())
        .await
        .unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::TagCombobox(state)) if matches!(state.intent, TagComboboxIntent::EditLabels { .. })
    ));
    assert_pending_empty(&app);
    assert!(app.view().detail_underlay);
}

#[tokio::test]
async fn invalid_detail_prefix_stays_in_detail() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Detail target")).await;
    app.show_detail(5);

    app.dispatch_key(key(KeyCode::Char('t')), (80, 24).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char('z')), (80, 24).into())
        .await
        .unwrap();

    assert_pending_empty(&app);
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("invalid shortcut: t z")
    );
}

#[tokio::test]
async fn detail_prefix_hints_render_above_detail_overlay() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Detail target")).await;
    app.show_detail(0);

    app.dispatch_key(key(KeyCode::Char('t')), (100, 30).into())
        .await
        .unwrap();

    let rendered = render_app_text(&mut app, 100, 30);

    assert!(rendered.contains("t …"));
    assert!(rendered.contains(":edit-title"));
}

#[tokio::test]
async fn ignored_keys_stay_in_detail() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Detail target")).await;
    app.show_detail(0);

    app.dispatch_key(key(KeyCode::Char('a')), (80, 24).into())
        .await
        .unwrap();

    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    assert!(app.authoring.is_idle());
}

#[tokio::test]
async fn detail_copy_clicks_copy_displayed_values_and_show_toasts() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Copy target")).await;
    app.show_detail(4);
    let terminal_size: ratatui::layout::Size = (120, 40).into();

    for (column, row) in [(2, 5), (88, 25), (88, 28), (88, 31)] {
        let value = crate::tui::ui::detail_copy_target_at(
            app.store.selected_task(app.list.selected_task()).unwrap(),
            terminal_size.width,
            terminal_size.height,
            column,
            row,
        )
        .unwrap()
        .value;

        app.dispatch_mouse(left_click(column, row), terminal_size)
            .await
            .unwrap();

        assert_eq!(
            crate::tui::platform::clipboard_text_for_test(),
            Some(value.clone())
        );
        assert_eq!(
            toast_message(&app).as_deref(),
            Some(format!("copied {value}").as_str())
        );
        assert_eq!(toast_severity(&app), Some(ToastSeverity::Success));
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
    }
}

#[tokio::test]
async fn detail_toast_renders_above_detail_overlay() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Toast target")).await;
    app.show_detail(0);
    app.set_success("set APP-TEST status=done");

    let rendered = render_app_text(&mut app, 100, 30);

    assert!(rendered.contains("set APP-TEST status=done"));
}

#[tokio::test]
async fn detail_bare_status_and_priority_shortcuts_keep_detail() {
    let mut app = test_app().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Quick detail actions")).await;
    let task_id = app.store.tasks[selected].task.id.clone();
    app.store.show_view(TaskQuery::All).await.unwrap();
    app.list.select_task(
        app.store
            .tasks
            .iter()
            .position(|item| item.task.id == task_id),
    );
    app.show_detail(0);

    app.dispatch_key(key(KeyCode::Char('d')), (80, 24).into())
        .await
        .unwrap();
    assert_eq!(app.store.tasks[selected].task.status, TaskStatus::Done);
    assert!(app.detail.is_active());
    assert!(app.overlay.is_none());

    app.dispatch_key(key(KeyCode::Char('x')), (80, 24).into())
        .await
        .unwrap();
    assert_eq!(app.store.tasks[selected].task.status, TaskStatus::Canceled);
    assert!(app.detail.is_active());
    assert!(app.overlay.is_none());

    app.dispatch_key(key(KeyCode::Char('t')), (80, 24).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char('h')), (80, 24).into())
        .await
        .unwrap();
    let task = app.store.load_task_item(&task_id).await.unwrap().unwrap();
    assert_eq!(task.task.id, task_id);
    assert_eq!(task.task.priority, TaskPriority::High);
    assert!(app.detail.is_active());
    assert!(app.overlay.is_none());
}

#[tokio::test]
async fn detail_restore_shortcut_restores_deleted_task() {
    let mut app = test_app().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Restore detail target")).await;
    let task_id = app.store.tasks[selected].task.id.clone();
    app.store
        .update_deleted(Some(selected), true)
        .await
        .unwrap();
    let deleted = app.store.load_task_item(&task_id).await.unwrap().unwrap();
    app.store.show_exact_task(deleted);
    app.list.select_task(Some(0));
    app.show_detail(0);
    assert!(render_app_text(&mut app, 100, 30).contains("t R"));

    app.dispatch_key(key(KeyCode::Char('t')), (80, 24).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char('R')), (80, 24).into())
        .await
        .unwrap();

    assert!(!app.store.tasks[0].task.deleted);
    assert!(app.detail.is_active());
    assert!(app.overlay.is_none());
}

#[tokio::test]
async fn detail_conflict_shortcut_opens_confirmation() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    let selected =
        create_and_select_task(&mut app, test_task_draft("Conflict detail target")).await;
    let task_id = app.store.tasks[selected].task.id.clone();
    insert_title_conflict_for_task_id(&pool, &mut app, &task_id, "local", "remote").await;
    app.list.select_task(Some(selected));
    app.show_detail(0);

    app.dispatch_key(key(KeyCode::Char('c')), (80, 24).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char('a')), (80, 24).into())
        .await
        .unwrap();

    let Some(OverlayState::Confirm(confirm)) = app.overlay else {
        panic!("expected conflict confirmation");
    };
    assert_eq!(confirm.title, CONFLICT_CONFIRM_LOCAL_TITLE);
    assert!(matches!(
        confirm.intent,
        ConfirmIntent::ResolveConflict { .. }
    ));
    assert!(app.detail.is_active());
}

#[tokio::test]
async fn conflict_navigation_closes_detail_with_non_default_session_state() {
    for action in [
        Action::BeginConflictList,
        Action::NextConflict,
        Action::PreviousConflict,
    ] {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        let first = create_and_select_task(&mut app, test_task_draft("First conflict")).await;
        let first_id = app.store.tasks[first].task.id.clone();
        let second = create_and_select_task(&mut app, test_task_draft("Second conflict")).await;
        let second_id = app.store.tasks[second].task.id.clone();
        insert_title_conflict_for_task_id(&pool, &mut app, &first_id, "first", "remote").await;
        insert_title_conflict_for_task_id(&pool, &mut app, &second_id, "second", "remote").await;
        let selected = app
            .store
            .tasks
            .iter()
            .position(|item| item.task.id == first_id)
            .unwrap();
        app.list.select_task(Some(selected));
        app.show_detail(7);
        let state = app.detail.state_mut().unwrap();
        state.focused_target = Some(DetailTargetId::Attachment {
            attachment_id: "focused".to_string(),
        });
        state.hovered_target = Some(DetailTargetId::Expand {
            section: DetailSection::Blocks,
        });
        state.expanded_sections.insert(DetailSection::DependsOn);
        state.begin_text_selection(
            first_id.clone(),
            80,
            crate::tui::detail_selection::TextCell { start: 0, end: 1 },
        );

        app.execute(action).await.unwrap();

        assert!(
            app.detail.is_inactive(),
            "{action:?} kept stale detail state"
        );
        assert!(app.pending_shortcut.is_empty());
        if action == Action::BeginConflictList {
            assert_eq!(app.store.view_state.query, TaskQuery::Conflicts);
        } else {
            assert_ne!(
                app.store.tasks[app.list.selected_task().unwrap()].task.id,
                first_id
            );
        }
    }
}

#[tokio::test]
async fn detail_done_shortcut_keeps_detail_and_sets_message() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Next target")).await;
    let selected = create_and_select_task(&mut app, test_task_draft("Done target")).await;
    let selected_task_id = app.store.tasks[selected].task.id.clone();
    let display_ref = app.store.tasks[selected].display_ref.clone();
    app.store.show_view(TaskQuery::All).await.unwrap();
    app.list.select_task(
        app.store
            .tasks
            .iter()
            .position(|item| item.task.id == selected_task_id),
    );
    app.show_detail(7);

    app.dispatch_key(key(KeyCode::Char('t')), (80, 24).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char('d')), (80, 24).into())
        .await
        .unwrap();

    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    assert_eq!(
        toast_message(&app).as_deref(),
        Some(format!("set {display_ref} status=done · u undo").as_str())
    );
    let selected = app.list.selected_task().unwrap();
    assert_eq!(app.store.tasks[selected].task.id, selected_task_id);
    assert_eq!(app.store.tasks[selected].task.status, TaskStatus::Done);
}

#[tokio::test]
async fn detail_closes_when_status_change_removes_task_from_query() {
    let mut app = test_app().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Filtered target")).await;
    let task_id = app.store.tasks[selected].task.id.clone();
    app.show_detail(0);

    app.update_status(TaskStatus::Done).await.unwrap();

    assert!(app.detail.is_inactive());
    assert!(app.store.tasks.iter().all(|item| item.task.id != task_id));
}

#[tokio::test]
async fn queue_status_change_preserves_viewport_row_and_recalls_changed_task() {
    let mut app = test_app().await;
    for title in ["First", "Changed", "Third"] {
        create_and_select_task(&mut app, test_task_draft(title)).await;
    }
    app.show_view(TaskQuery::Queue).await.unwrap();
    let selected = 1;
    let changed_id = app.store.tasks[selected].task.id.clone();
    app.list.select_task(Some(selected));
    app.list.set_task_offset(1);

    app.update_status(TaskStatus::Active).await.unwrap();

    assert_eq!(app.list.selected_task(), Some(selected));
    assert_ne!(app.store.tasks[selected].task.id, changed_id);
    assert_eq!(app.list.task_offset(), 2);
    assert_eq!(app.list.last_changed_task_id(), Some(&changed_id));
    assert!(
        toast_message(&app)
            .unwrap()
            .ends_with(" · u undo · g . return")
    );

    app.execute(Action::ReturnToLastChange).await.unwrap();

    assert_eq!(
        app.store
            .selected_task(app.list.selected_task())
            .unwrap()
            .task
            .id,
        changed_id
    );
}

#[tokio::test]
async fn recalling_filtered_status_change_returns_from_detail_to_queue_anchor() {
    let mut app = test_app().await;
    for title in ["First", "Changed", "Third"] {
        create_and_select_task(&mut app, test_task_draft(title)).await;
    }
    app.show_view(TaskQuery::Queue).await.unwrap();
    let selected = 1;
    let changed_id = app.store.tasks[selected].task.id.clone();
    app.list.select_task(Some(selected));
    app.list.set_task_offset(1);

    app.update_status(TaskStatus::Done).await.unwrap();
    let replacement_id = app.store.tasks[selected].task.id.clone();
    let return_offset = app.list.task_offset();
    app.execute(Action::ReturnToLastChange).await.unwrap();

    assert_eq!(app.store.view_state.query, TaskQuery::Search);
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    assert_eq!(app.store.tasks.len(), 1);
    assert_eq!(app.store.tasks[0].task.id, changed_id);

    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();

    assert_eq!(app.store.view_state.query, TaskQuery::Queue);
    assert!(app.overlay.is_none());
    assert_eq!(app.list.task_offset(), return_offset);
    assert_eq!(
        app.store
            .selected_task(app.list.selected_task())
            .unwrap()
            .task
            .id,
        replacement_id
    );
}

#[tokio::test]
async fn detail_status_picker_done_refreshes_content_and_metadata() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Next target")).await;
    let selected = create_and_select_task(&mut app, test_task_draft("Done target")).await;
    let selected_task_id = app.store.tasks[selected].task.id.clone();
    app.store.show_view(TaskQuery::All).await.unwrap();
    app.list.select_task(
        app.store
            .tasks
            .iter()
            .position(|item| item.task.id == selected_task_id),
    );
    app.show_detail(4);
    render_app_buffer(&mut app, 120, 40);

    app.dispatch_key(key(KeyCode::Char('s')), (120, 40).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char('d')), (120, 40).into())
        .await
        .unwrap();

    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    assert_eq!(app.detail.state().unwrap().scroll(), 4);
    let selected = app.list.selected_task().unwrap();
    assert_eq!(app.store.tasks[selected].task.id, selected_task_id);
    assert_eq!(app.store.tasks[selected].task.status, TaskStatus::Done);

    let buffer = render_app_buffer(&mut app, 120, 40);
    let content = (2..12)
        .flat_map(|row| (0..86).map(move |column| (column, row)))
        .map(|(column, row)| buffer[(column, row)].symbol())
        .collect::<String>();
    let metadata = (2..12)
        .flat_map(|row| (86..120).map(move |column| (column, row)))
        .map(|(column, row)| buffer[(column, row)].symbol())
        .collect::<String>();
    assert!(content.contains("done"));
    assert!(!content.contains("inbox"));
    assert!(metadata.contains("done"));
    assert!(!metadata.contains("inbox"));
}

#[tokio::test]
async fn detail_status_mouse_click_opens_menu_and_returns_to_detail() {
    let mut app = test_app().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Status click target")).await;
    app.show_detail(3);

    app.dispatch_mouse(
        detail_metadata_click(crate::tui::ui::DetailMetadataTarget::Status),
        (120, 30).into(),
    )
    .await
    .unwrap();

    assert_eq!(
        app.footer_choice.as_ref().map(|choice| choice.mode),
        Some(FooterChoiceMode::Status)
    );
    assert!(app.view().detail_underlay);

    app.dispatch_key(key(KeyCode::Char('a')), (120, 30).into())
        .await
        .unwrap();

    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    assert_eq!(app.store.tasks[selected].task.status, TaskStatus::Active);
}

#[tokio::test]
async fn detail_status_menu_empty_click_returns_to_detail_without_selecting_task() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Hidden task")).await;
    let selected = create_and_select_task(&mut app, test_task_draft("Visible task")).await;
    app.show_detail(0);

    app.dispatch_mouse(
        detail_metadata_click(crate::tui::ui::DetailMetadataTarget::Status),
        (120, 30).into(),
    )
    .await
    .unwrap();
    app.dispatch_mouse(left_click(110, 20), (120, 30).into())
        .await
        .unwrap();

    assert_eq!(app.list.selected_task(), Some(selected));
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
}

#[tokio::test]
async fn detail_priority_mouse_click_opens_menu_and_returns_to_detail() {
    let mut app = test_app().await;
    let selected = create_and_select_task(
        &mut app,
        TaskDraft {
            metadata: Vec::new(),
            priority: "medium".to_string(),
            ..test_task_draft("Priority click target")
        },
    )
    .await;
    app.show_detail(3);

    app.dispatch_mouse(
        detail_metadata_click(crate::tui::ui::DetailMetadataTarget::Priority),
        (120, 30).into(),
    )
    .await
    .unwrap();

    assert_eq!(
        app.footer_choice.as_ref().map(|choice| choice.mode),
        Some(FooterChoiceMode::Priority)
    );
    assert!(app.view().detail_underlay);

    app.dispatch_key(key(KeyCode::Char('u')), (120, 30).into())
        .await
        .unwrap();

    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    assert_eq!(
        app.store.tasks[selected].task.priority,
        TaskPriority::Urgent
    );
}

#[tokio::test]
async fn detail_project_mouse_click_opens_established_picker() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Project click target")).await;
    app.show_detail(3);

    app.dispatch_mouse(
        detail_metadata_click(crate::tui::ui::DetailMetadataTarget::Project),
        (120, 40).into(),
    )
    .await
    .unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Picker(PickerState {
            intent: PickerIntent::EditProject { .. },
            ..
        }))
    ));
    assert!(app.view().detail_underlay);
}

#[tokio::test]
async fn detail_labels_mouse_click_opens_established_combobox_for_unset_labels() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Labels click target")).await;
    app.show_detail(3);

    app.dispatch_mouse(
        detail_metadata_click(crate::tui::ui::DetailMetadataTarget::Labels),
        (120, 40).into(),
    )
    .await
    .unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::TagCombobox(state))
            if matches!(state.intent, TagComboboxIntent::EditLabels { .. })
    ));
    assert!(app.view().detail_underlay);
}

#[tokio::test]
async fn detail_availability_mouse_click_opens_established_editor_for_unset_date() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Availability click target")).await;
    app.show_detail(3);

    app.dispatch_mouse(
        detail_metadata_click(crate::tui::ui::DetailMetadataTarget::Availability),
        (120, 40).into(),
    )
    .await
    .unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::TextInput(state))
            if matches!(state.intent, TextIntent::EditAvailability { .. })
                && state.input.text.is_empty()
    ));
    assert!(app.view().detail_underlay);
}

#[tokio::test]
async fn detail_due_mouse_click_opens_established_editor_for_unset_date() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Due click target")).await;
    app.show_detail(3);

    app.dispatch_mouse(
        detail_metadata_click(crate::tui::ui::DetailMetadataTarget::Due),
        (120, 40).into(),
    )
    .await
    .unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::TextInput(state))
            if matches!(state.intent, TextIntent::EditDue { .. })
                && state.input.text.is_empty()
    ));
    assert!(app.view().detail_underlay);
}

#[tokio::test]
async fn detail_undo_shortcut_reverts_last_mutation() {
    let mut app = test_app().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Before")).await;
    app.store
        .update_title(Some(selected), "After".to_string())
        .await
        .unwrap();
    app.show_detail(5);

    app.dispatch_key(key(KeyCode::Char('u')), (80, 24).into())
        .await
        .unwrap();

    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    assert_eq!(app.store.tasks[selected].task.title, "Before");
    assert!(toast_message(&app).is_some_and(|message| message.contains("undid")));
}

#[tokio::test]
async fn detail_undo_after_status_menu_keeps_task_identity() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Other task")).await;
    let selected = create_and_select_task(&mut app, test_task_draft("Undo target")).await;
    let task_id = app.store.tasks[selected].task.id.clone();
    app.store.show_view(TaskQuery::All).await.unwrap();
    app.list.select_task(
        app.store
            .tasks
            .iter()
            .position(|item| item.task.id == task_id),
    );
    app.show_detail(0);

    app.dispatch_mouse(
        detail_metadata_click(crate::tui::ui::DetailMetadataTarget::Status),
        (120, 30).into(),
    )
    .await
    .unwrap();
    assert_eq!(
        app.footer_choice.as_ref().map(|choice| choice.mode),
        Some(FooterChoiceMode::Status)
    );
    app.dispatch_key(key(KeyCode::Char('a')), (120, 30).into())
        .await
        .unwrap();
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());

    app.dispatch_key(key(KeyCode::Char('u')), (120, 30).into())
        .await
        .unwrap();

    let selected = app.list.selected_task().unwrap();
    assert_eq!(app.store.tasks[selected].task.id, task_id);
    assert_eq!(app.store.tasks[selected].task.status, TaskStatus::Inbox);
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
}

#[tokio::test]
async fn populated_add_note_from_detail_returns_after_confirmed_discard() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Note target")).await;
    app.show_detail(0);

    app.dispatch_key(key(KeyCode::Char('t')), (80, 24).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char('N')), (80, 24).into())
        .await
        .unwrap();
    type_chars(&mut app, "detail draft").await;
    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::MultilineInput(state))
            if state.mode == MultilineInputMode::ConfirmDiscard
                && state.lines == ["detail draft"]
    ));
    assert!(app.detail.is_active());

    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::MultilineInput(state))
            if state.mode == MultilineInputMode::Compose
                && state.lines == ["detail draft"]
    ));
    assert!(app.detail.is_active());

    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Char('Y')))
        .await
        .unwrap();
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    assert!(app.authoring.is_idle());
}

#[tokio::test]
async fn cancel_add_note_from_detail_returns_to_detail() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Note target")).await;
    app.show_detail(0);

    app.dispatch_key(key(KeyCode::Char('t')), (80, 24).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char('N')), (80, 24).into())
        .await
        .unwrap();
    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();

    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    assert!(app.authoring.is_idle());
}

#[tokio::test]
async fn add_note_blank_body_from_detail_returns_to_detail() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Note target")).await;
    app.show_detail(0);

    app.dispatch_key(key(KeyCode::Char('t')), (80, 24).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char('N')), (80, 24).into())
        .await
        .unwrap();
    app.handle_overlay_key(ctrl_s()).await.unwrap();

    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("note body is required")
    );
}
