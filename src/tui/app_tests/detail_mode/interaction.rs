use super::*;

#[tokio::test]
async fn task_gist_shortcut_requires_publication_confirmation() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Share detail")).await;
    let task_id = app.store.tasks[app.list.selected_task().unwrap()]
        .task
        .id
        .clone();
    app.show_detail(0);

    app.dispatch_key(key(KeyCode::Char('t')), (80, 24).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char('g')), (80, 24).into())
        .await
        .unwrap();

    assert!(matches!(
        app.overlay,
        Some(OverlayState::Confirm(ConfirmState {
            intent: ConfirmIntent::CreateTaskGist { task_id: ref confirmed },
            ..
        })) if confirmed == &task_id
    ));
}

#[tokio::test]
async fn gist_creation_failure_replaces_loading_notification() {
    let mut app = test_app().await;
    app.notification = Some(Notification::loading("creating secret gist"));
    app.gist.set_test_task(tokio::spawn(async {
        Err(anyhow::anyhow!("GitHub is unavailable"))
    }));
    tokio::task::yield_now().await;

    assert!(app.poll_gist_creation().await);
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("gist creation failed: GitHub is unavailable")
    );
    assert_eq!(toast_severity(&app), Some(ToastSeverity::Error));
    assert!(!app.gist.work_pending());
}

#[tokio::test]
async fn clicking_detail_markdown_link_opens_browser() {
    let mut app = test_app().await;
    let mut draft = test_task_draft("Linked detail");
    draft.description = "Read the [Aven guide](https://aven.raine.dev/guide/).".to_string();
    create_and_select_task(&mut app, draft).await;
    app.show_detail(0);
    let terminal_size: ratatui::layout::Size = (100, 30).into();
    let document = app.detail_document_for_query(terminal_size).unwrap();
    let (column, row) = (0..terminal_size.height)
        .flat_map(|row| (0..terminal_size.width).map(move |column| (column, row)))
        .find(|&(column, row)| document.link_at_position(column, row).is_some())
        .expect("rendered link target");

    app.dispatch_mouse(click_at(column, row), terminal_size)
        .await
        .unwrap();

    assert_eq!(
        crate::tui::platform::browser_url_for_test().as_deref(),
        Some("https://aven.raine.dev/guide/")
    );
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("opened link in browser")
    );
}

#[tokio::test]
async fn q_closes_detail_overlay() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Detail target")).await;
    app.show_detail(0);

    app.dispatch_key(key(KeyCode::Char('q')), (80, 24).into())
        .await
        .unwrap();

    assert!(app.overlay.is_none());
    assert!(!app.should_quit);
}

#[tokio::test]
async fn back_shortcut_closes_detail_overlay() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Detail target")).await;
    app.show_detail(0);

    app.dispatch_key(key(KeyCode::Char('g')), (80, 24).into())
        .await
        .unwrap();
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());

    app.dispatch_key(key(KeyCode::Char('[')), (80, 24).into())
        .await
        .unwrap();

    assert!(app.overlay.is_none());
    assert!(!app.detail.is_active());
    assert!(app.pending_shortcut.is_empty());
}

#[tokio::test]
async fn detail_routes_global_overlays_and_refresh() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Global detail commands")).await;
    app.show_detail(0);

    app.dispatch_key(key(KeyCode::Char(':')), (80, 24).into())
        .await
        .unwrap();
    assert!(matches!(app.overlay, Some(OverlayState::Command { .. })));
    app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
        .await
        .unwrap();
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());

    app.dispatch_key(key(KeyCode::Char('/')), (80, 24).into())
        .await
        .unwrap();
    assert!(matches!(app.overlay, Some(OverlayState::Search(_))));
    app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
        .await
        .unwrap();
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());

    app.dispatch_key(key(KeyCode::Char('r')), (80, 24).into())
        .await
        .unwrap();
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
}

#[tokio::test]
async fn help_key_opens_detail_help_from_detail_overlay() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Detail help target")).await;
    app.show_detail(0);

    app.dispatch_key(key(KeyCode::Char('?')), (80, 24).into())
        .await
        .unwrap();

    assert!(matches!(app.overlay, Some(OverlayState::DetailHelp { .. })));
    assert_eq!(app.list.focus(), Focus::Tasks);
    assert!(app.list.selected_task().is_some());
}

#[tokio::test]
async fn closing_detail_help_returns_to_detail_overlay() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Detail help target")).await;
    app.show_detail(0);
    app.overlay = Some(OverlayState::DetailHelp { scroll: 0 });

    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();

    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
}

#[tokio::test]
async fn second_help_key_returns_from_detail_help_to_detail_overlay() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Detail help target")).await;
    app.show_detail(0);
    app.overlay = Some(OverlayState::DetailHelp { scroll: 0 });

    app.dispatch_key(key(KeyCode::Char('?')), (80, 24).into())
        .await
        .unwrap();

    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
}

#[tokio::test]
async fn focused_note_edit_preserves_note_identity() {
    let mut app = test_app().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Note edit target")).await;
    let task_id = app.store.tasks[selected].task.id.clone();
    let note_id = app
        .store
        .add_note_to_task(&task_id, "original note".to_string())
        .await
        .unwrap();
    app.refresh().await.unwrap();
    app.show_detail(0);

    app.dispatch_key(key(KeyCode::Tab), (80, 24).into())
        .await
        .unwrap();
    assert_eq!(
        app.detail
            .state()
            .and_then(|detail| detail.focused_target()),
        Some(&crate::tui::app::DetailTargetId::Note {
            note_id: note_id.clone(),
        })
    );

    app.dispatch_key(key(KeyCode::Char('e')), (80, 24).into())
        .await
        .unwrap();
    let Some(OverlayState::MultilineInput(state)) = app.overlay.as_mut() else {
        panic!("expected note editor");
    };
    assert!(matches!(
        &state.intent,
        MultilineIntent::EditNote {
            task_id: intent_task_id,
            note_id: intent_note_id,
            ..
        } if intent_task_id == &task_id && intent_note_id == &note_id
    ));
    state.lines = vec!["corrected note".to_string()];
    state.row = 0;
    state.column = 14;
    app.handle_overlay_key(ctrl_s()).await.unwrap();

    let item = app.store.selected_task(app.list.selected_task()).unwrap();
    assert_eq!(item.notes.len(), 1);
    assert_eq!(item.notes[0].id, note_id);
    assert_eq!(item.notes[0].body, "corrected note");
    assert!(toast_message(&app).is_some_and(|message| message.starts_with("edited note ")));
}

#[tokio::test]
async fn focused_note_delete_requires_confirmation_and_honors_cancellation() {
    let mut app = test_app().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Note delete target")).await;
    let task_id = app.store.tasks[selected].task.id.clone();
    let note_id = app
        .store
        .add_note_to_task(&task_id, "keep until confirmed".to_string())
        .await
        .unwrap();
    app.refresh().await.unwrap();
    app.show_detail(0);
    app.dispatch_key(key(KeyCode::Tab), (80, 24).into())
        .await
        .unwrap();

    app.dispatch_key(key(KeyCode::Char('D')), (80, 24).into())
        .await
        .unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Confirm(ConfirmState {
            intent: ConfirmIntent::DeleteNote {
                task_id: intent_task_id,
                note_id: intent_note_id,
            },
            ..
        })) if intent_task_id == &task_id && intent_note_id == &note_id
    ));
    app.handle_overlay_key(key(KeyCode::Char('n')))
        .await
        .unwrap();
    assert_eq!(app.store.tasks[selected].notes[0].id, note_id);

    app.dispatch_key(key(KeyCode::Char('D')), (80, 24).into())
        .await
        .unwrap();
    app.handle_overlay_key(key(KeyCode::Char('y')))
        .await
        .unwrap();

    assert!(app.store.tasks[selected].notes.is_empty());
    assert!(
        app.detail
            .state()
            .and_then(|detail| detail.focused_target())
            .is_none()
    );
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("deleted note · u undo")
    );
}

#[tokio::test]
async fn canceling_dirty_note_edit_preserves_persisted_body() {
    let mut app = test_app().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Note cancel target")).await;
    let task_id = app.store.tasks[selected].task.id.clone();
    let note_id = app
        .store
        .add_note_to_task(&task_id, "original note".to_string())
        .await
        .unwrap();
    app.refresh().await.unwrap();
    app.show_detail(0);
    app.dispatch_key(key(KeyCode::Tab), (80, 24).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char('e')), (80, 24).into())
        .await
        .unwrap();
    type_chars(&mut app, " changed").await;

    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::MultilineInput(state))
            if matches!(state.intent, MultilineIntent::EditNote { .. })
                && state.mode == MultilineInputMode::ConfirmDiscard
    ));
    app.handle_overlay_key(key(KeyCode::Char('y')))
        .await
        .unwrap();

    assert!(app.overlay.is_none());
    let note = app.store.tasks[selected]
        .notes
        .iter()
        .find(|note| note.id == note_id)
        .unwrap();
    assert_eq!(note.body, "original note");
}

#[tokio::test]
async fn detail_tab_jumps_to_notes_below_viewport() {
    let mut app = test_app().await;
    let mut draft = test_task_draft("Hidden notes target");
    draft.description = (0..40)
        .map(|index| format!("line {index}"))
        .collect::<Vec<_>>()
        .join("\n");
    let selected = create_and_select_task(&mut app, draft).await;
    let expected =
        crate::tui::ui::detail_section_scroll_target(&app.store.tasks[selected], 0, 80, 24, false);
    app.show_detail(0);

    app.dispatch_key(key(KeyCode::Tab), (80, 24).into())
        .await
        .unwrap();

    assert_eq!(detail_scroll(&app), expected);
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    assert_eq!(app.list.focus(), Focus::Tasks);

    app.dispatch_key(key(KeyCode::Tab), (80, 24).into())
        .await
        .unwrap();
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());

    app.dispatch_key(key(KeyCode::BackTab), (80, 24).into())
        .await
        .unwrap();
    assert_eq!(detail_scroll(&app), expected);
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
}

#[tokio::test]
async fn detail_tab_remains_unchanged_when_notes_are_visible() {
    let mut app = test_app().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Visible notes target")).await;
    assert_eq!(
        crate::tui::ui::detail_section_scroll_target(&app.store.tasks[selected], 0, 80, 24, false,),
        0
    );
    app.show_detail(0);

    app.dispatch_key(key(KeyCode::Tab), (80, 24).into())
        .await
        .unwrap();

    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    assert_eq!(app.list.focus(), Focus::Tasks);
}

#[tokio::test]
async fn detail_scroll_keys_update_detail_offset() {
    let mut app = test_app().await;
    let mut draft = test_task_draft("Scroll target");
    draft.description = (0..100)
        .map(|index| format!("line {index}"))
        .collect::<Vec<_>>()
        .join("\n");
    create_and_select_task(&mut app, draft).await;
    app.show_detail(0);

    app.dispatch_key(ctrl_d(), (80, 24).into()).await.unwrap();
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());

    app.dispatch_key(key(KeyCode::PageDown), (80, 24).into())
        .await
        .unwrap();
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());

    app.dispatch_key(ctrl_u(), (80, 24).into()).await.unwrap();
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());

    app.dispatch_key(key(KeyCode::Char('k')), (80, 24).into())
        .await
        .unwrap();
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());

    app.dispatch_key(key(KeyCode::PageUp), (80, 24).into())
        .await
        .unwrap();
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
}

#[tokio::test]
async fn detail_scroll_resists_down_input_at_bottom() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Short detail")).await;
    app.show_detail(0);

    for _ in 0..10 {
        app.dispatch_key(key(KeyCode::Char('j')), (80, 24).into())
            .await
            .unwrap();
    }
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());

    app.dispatch_key(key(KeyCode::Char('k')), (80, 24).into())
        .await
        .unwrap();
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
}

#[tokio::test]
async fn mouse_wheel_scrolls_prefix_hint_overlay() {
    let mut app = test_app().await;
    app.dispatch_key(key(KeyCode::Char('t')), (80, 10).into())
        .await
        .unwrap();

    app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), (80, 10).into())
        .await
        .unwrap();

    assert_eq!(app.pending_shortcut.labels(), vec!["t".to_string()]);
    assert_eq!(app.pending_shortcut_scroll, 1);
}

#[tokio::test]
async fn arrows_scroll_prefix_hint_overlay() {
    let mut app = test_app().await;
    app.dispatch_key(key(KeyCode::Char('t')), (80, 10).into())
        .await
        .unwrap();

    app.dispatch_key(key(KeyCode::Down), (80, 10).into())
        .await
        .unwrap();
    assert_eq!(app.pending_shortcut.labels(), vec!["t".to_string()]);
    assert_eq!(app.pending_shortcut_scroll, 1);

    app.dispatch_key(key(KeyCode::Up), (80, 10).into())
        .await
        .unwrap();
    assert_eq!(app.pending_shortcut.labels(), vec!["t".to_string()]);
    assert_eq!(app.pending_shortcut_scroll, 0);
}

#[tokio::test]
async fn mouse_wheel_scrolls_help_overlay() {
    let mut app = test_app().await;
    app.overlay = Some(OverlayState::Help { scroll: 0 });

    app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), (80, 24).into())
        .await
        .unwrap();
    assert!(matches!(
        app.overlay,
        Some(OverlayState::Help { scroll: 1 })
    ));

    app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollUp), (80, 24).into())
        .await
        .unwrap();
    assert!(matches!(
        app.overlay,
        Some(OverlayState::Help { scroll: 0 })
    ));
}

#[tokio::test]
async fn mouse_wheel_clamps_help_overlay() {
    let mut app = test_app().await;
    let expected = crate::tui::ui::help_scroll_cap(24);
    app.overlay = Some(OverlayState::Help { scroll: 0 });

    for _ in 0..200 {
        app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), (80, 24).into())
            .await
            .unwrap();
    }

    assert!(matches!(app.overlay, Some(OverlayState::Help { scroll }) if scroll == expected));

    let changed = app
        .dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), (80, 24).into())
        .await
        .unwrap();
    assert!(!changed);
}

#[tokio::test]
async fn mouse_wheel_scrolls_detail_help_overlay() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Detail help target")).await;
    app.show_detail(0);
    app.overlay = Some(OverlayState::DetailHelp { scroll: 0 });

    app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), (80, 10).into())
        .await
        .unwrap();
    assert!(matches!(
        app.overlay,
        Some(OverlayState::DetailHelp { scroll: 1 })
    ));
}

#[tokio::test]
async fn modal_overlay_pointer_move_clears_stale_detail_hover() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Overlay hover target")).await;
    app.show_detail(0);
    app.detail
        .state_mut()
        .unwrap()
        .set_hovered_target(Some(DetailTargetId::Expand {
            section: DetailSection::DependsOn,
        }));
    app.overlay = Some(OverlayState::DetailHelp { scroll: 0 });

    let changed = app
        .dispatch_mouse(
            MouseEvent {
                kind: MouseEventKind::Moved,
                column: 4,
                row: 7,
                modifiers: KeyModifiers::NONE,
            },
            (80, 24).into(),
        )
        .await
        .unwrap();

    assert!(changed);
    assert!(
        app.detail
            .state()
            .and_then(|detail| detail.hovered_target())
            .is_none()
    );
    assert!(matches!(
        app.overlay,
        Some(OverlayState::DetailHelp { scroll: 0 })
    ));
}

#[tokio::test]
async fn stable_frame_mouse_movement_reuses_detail_document() {
    let mut app = test_app().await;
    let mut draft = test_task_draft("Stable detail projection");
    draft.description = (0..100)
        .map(|index| format!("line {index}"))
        .collect::<Vec<_>>()
        .join("\n");
    create_and_select_task(&mut app, draft).await;
    app.show_detail(0);
    let size: ratatui::layout::Size = (80, 24).into();
    render_app_buffer(&mut app, size.width, size.height);
    let document = std::rc::Rc::clone(
        app.widgets
            .detail_document
            .as_ref()
            .expect("rendered detail document"),
    );
    let projection_id = document.projection_id();
    let geometry_id = document.geometry_id();

    for (column, row) in [(4, 7), (18, 9)] {
        app.dispatch_mouse(
            MouseEvent {
                kind: MouseEventKind::Moved,
                column,
                row,
                modifiers: KeyModifiers::NONE,
            },
            size,
        )
        .await
        .unwrap();
        render_app_buffer(&mut app, size.width, size.height);
        let cached = app
            .widgets
            .detail_document
            .as_ref()
            .expect("stable detail document");
        assert_eq!(cached.projection_id(), projection_id);
        assert!(std::rc::Rc::ptr_eq(cached, &document));
    }

    app.dispatch_key(key(KeyCode::PageDown), size)
        .await
        .unwrap();
    render_app_buffer(&mut app, size.width, size.height);
    let scrolled = app
        .widgets
        .detail_document
        .as_ref()
        .expect("scrolled detail document");
    assert_eq!(scrolled.geometry_id(), geometry_id);
    assert_ne!(scrolled.projection_id(), projection_id);
    assert!(!std::rc::Rc::ptr_eq(scrolled, &document));
}

#[tokio::test]
async fn focused_disclosure_reports_task_commands_as_unavailable() {
    let mut app = test_app().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Disclosure target")).await;
    app.store.tasks[selected].depends_on = (0..4)
        .map(|index| crate::query::TaskDependencyLink {
            task_id: crate::test_support::task_id(&format!("disclosure-child-{index}")),
            display_ref: format!("APP-{index}"),
            title: format!("Child {index}"),
            status: "todo".to_string(),
            priority: "none".to_string(),
            unresolved: true,
        })
        .collect();
    app.show_detail(0);
    app.detail
        .state_mut()
        .unwrap()
        .set_focused_target(Some(DetailTargetId::Expand {
            section: DetailSection::DependsOn,
        }));

    app.dispatch_key(key(KeyCode::Char('s')), (80, 24).into())
        .await
        .unwrap();

    assert!(app.footer_choice.is_none());
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("leave relationship disclosure focus before using that command")
    );
    assert!(matches!(
        app.detail
            .state()
            .and_then(|detail| detail.focused_target()),
        Some(DetailTargetId::Expand {
            section: DetailSection::DependsOn
        })
    ));
}

#[tokio::test]
async fn rendered_disclosure_rebuilds_focus_and_scroll_from_expanded_document() {
    let mut app = test_app().await;
    let mut draft = test_task_draft("Expanded projection");
    draft.description = (0..30)
        .map(|index| format!("description line {index}"))
        .collect::<Vec<_>>()
        .join("\n");
    let selected = create_and_select_task(&mut app, draft).await;
    let child_ids = (0..6)
        .map(|index| crate::test_support::task_id(&format!("expanded-child-{index}")))
        .collect::<Vec<_>>();
    app.store.tasks[selected].depends_on = child_ids
        .iter()
        .enumerate()
        .map(|(index, task_id)| crate::query::TaskDependencyLink {
            task_id: task_id.clone(),
            display_ref: format!("APP-{index}"),
            title: format!("Child {index}"),
            status: "todo".to_string(),
            priority: "none".to_string(),
            unresolved: true,
        })
        .collect();
    app.show_detail(0);
    app.detail.state_mut().unwrap().set_focused_target(Some(
        crate::tui::app::DetailTargetId::Expand {
            section: crate::tui::app::DetailSection::DependsOn,
        },
    ));
    let size: ratatui::layout::Size = (80, 24).into();
    render_app_buffer(&mut app, size.width, size.height);

    app.dispatch_key(key(KeyCode::Enter), size).await.unwrap();

    let focused = app
        .detail
        .state()
        .and_then(|detail| detail.focused_target())
        .cloned()
        .expect("first revealed dependency focused");
    assert_eq!(focused.task_id(), Some(&child_ids[3]));
    assert!(
        app.detail
            .state()
            .unwrap()
            .expanded_sections()
            .contains(&crate::tui::app::DetailSection::DependsOn)
    );
    let expected_scroll = app
        .detail_document_for_query(size)
        .and_then(|document| document.target_scroll_target(&focused, 0))
        .unwrap();
    assert_eq!(detail_scroll(&app), expected_scroll);
}

#[tokio::test]
async fn detail_drag_selects_rendered_text_and_coexists_with_scrolling() {
    let mut app = test_app().await;
    let mut draft = test_task_draft("Select this title");
    draft.description = (0..40)
        .map(|index| format!("description line {index}"))
        .collect::<Vec<_>>()
        .join("\n");
    create_and_select_task(&mut app, draft).await;
    app.show_detail(0);
    let size = (80, 24).into();

    app.dispatch_mouse(left_click(0, 3), size).await.unwrap();
    app.dispatch_mouse(left_drag(7, 3), size).await.unwrap();
    app.dispatch_mouse(left_release(7, 3), size).await.unwrap();

    let selection = app
        .detail
        .state_mut()
        .unwrap()
        .text_selection
        .clone()
        .unwrap();
    let selected_text = {
        let item = app.store.selected_task(app.list.selected_task()).unwrap();
        crate::tui::ui::detail_selected_text(item, &selection)
    };
    assert_eq!(selected_text.as_deref(), Some("Select"));
    assert!(!app.detail.state_mut().unwrap().text_dragging);
    assert!(render_app_text(&mut app, 80, 24).contains("copy selection"));

    app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), size)
        .await
        .unwrap();
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    let item = app.store.selected_task(app.list.selected_task()).unwrap();
    assert_eq!(
        crate::tui::ui::detail_selected_text(item, &selection).as_deref(),
        Some("Select")
    );

    app.dispatch_key(key(KeyCode::Esc), size).await.unwrap();
    assert!(app.detail.state_mut().unwrap().text_selection.is_none());
    assert!(!render_app_text(&mut app, 80, 24).contains("copy selection"));
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
}

#[tokio::test]
async fn detail_mouse_wheel_updates_detail_offset() {
    let mut app = test_app().await;
    let mut draft = test_task_draft("Mouse detail scroll target");
    draft.description = (0..100)
        .map(|index| format!("line {index}"))
        .collect::<Vec<_>>()
        .join("\n");
    create_and_select_task(&mut app, draft).await;
    app.show_detail(0);

    app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), (80, 24).into())
        .await
        .unwrap();
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());

    app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollUp), (80, 24).into())
        .await
        .unwrap();
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
}

#[tokio::test]
async fn detail_mouse_wheel_clamps_detail_offset() {
    let mut app = test_app().await;
    let mut draft = test_task_draft("Mouse detail clamp target");
    draft.description = (0..100)
        .map(|index| format!("line {index}"))
        .collect::<Vec<_>>()
        .join("\n");
    let selected = create_and_select_task(&mut app, draft).await;
    let expected = crate::tui::ui::detail_scroll_cap(&app.store.tasks[selected], 80, 24);
    app.show_detail(0);

    for _ in 0..200 {
        app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), (80, 24).into())
            .await
            .unwrap();
    }

    assert_eq!(detail_scroll(&app), expected);
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
}

#[tokio::test]
async fn detail_mouse_wheel_scrolls_conflict_text_panel() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    let selected =
        create_and_select_task(&mut app, test_task_draft("Conflict mouse scroll target")).await;
    let task_id = app.store.tasks[selected].task.id.clone();
    for index in 0..20 {
        insert_conflict_for_task_id(
            &pool,
            &mut app,
            &task_id,
            &format!("field-{index}"),
            &format!("local value {index}"),
            &format!("remote value {index}"),
        )
        .await;
    }

    app.show_conflict_details().await.unwrap();
    let expected = match app.overlay.as_ref() {
        Some(OverlayState::TextPanel(panel)) => crate::tui::ui::text_panel_scroll_cap(&panel.lines),
        Some(overlay) => panic!("unexpected overlay for conflict details: {overlay:?}"),
        None => panic!("expected conflict details overlay"),
    };

    app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), (80, 24).into())
        .await
        .unwrap();
    assert!(matches!(
        app.overlay,
        Some(OverlayState::TextPanel(ref panel)) if panel.scroll == 1
    ));

    for _ in 0..200 {
        app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), (80, 24).into())
            .await
            .unwrap();
    }
    assert!(matches!(
        app.overlay,
        Some(OverlayState::TextPanel(ref panel)) if panel.scroll == expected
    ));

    app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollUp), (80, 24).into())
        .await
        .unwrap();
    assert!(matches!(
        app.overlay,
        Some(OverlayState::TextPanel(ref panel)) if panel.scroll == expected.saturating_sub(1)
    ));
}
