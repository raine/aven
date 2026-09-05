use super::*;
use crate::tui::overlay::metadata::MetadataFocus;
use aven_core::metadata::TaskMetadataInput;

fn seeded_draft(title: &str) -> TaskDraft {
    TaskDraft {
        metadata: vec![TaskMetadataInput {
            expected_field_id: None,
            key: "review".to_string(),
            value: "pending".to_string(),
        }],
        ..test_task_draft(title)
    }
}

async fn open_editor(app: &mut App) {
    app.dispatch_key(key(KeyCode::Char('e')), (80, 24).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char('m')), (80, 24).into())
        .await
        .unwrap();
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
}

#[tokio::test]
async fn metadata_blank_save_is_disabled_and_explicit_remove_supports_undo() {
    let mut app = test_app().await;
    let index = create_and_select_task(&mut app, seeded_draft("Metadata target")).await;
    let id = app.store.tasks[index].task.id.clone();
    app.show_detail(0);
    app.ensure_selected_task_detail().await.unwrap();
    for (width, height) in [(120, 40), (80, 24), (70, 18)] {
        let rendered = render_app_text(&mut app, width, height);
        assert!(
            rendered.contains("CUSTOM METADATA"),
            "{width}x{height}: {rendered}"
        );
    }
    open_editor(&mut app).await;
    for blank in ["", " \t", "\n"] {
        let Some(OverlayState::Metadata(state)) = &mut app.overlay else {
            panic!()
        };
        let editor = state.editor.as_mut().unwrap();
        editor.input.lines = blank.split('\n').map(str::to_string).collect();
        editor.input.row = 0;
        editor.input.column = 0;
        editor.focus = MetadataFocus::Save;
        for event in [
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL),
            key(KeyCode::Enter),
        ] {
            app.handle_overlay_key(event).await.unwrap();
            assert!(
                matches!(&app.overlay, Some(OverlayState::Metadata(state)) if state.editor.is_some())
            );
            assert_eq!(
                app.store.metadata_values(&id).await.unwrap()[0].value,
                "pending"
            );
        }
        let Some(OverlayState::Metadata(state)) = app.overlay.take() else {
            panic!()
        };
        let layout = crate::tui::overlay::metadata::metadata_layout(&state.view(), (80, 24).into());
        let outcome = crate::tui::overlay::metadata::handle_mouse(
            state,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: layout.actions[0].x,
                row: layout.actions[0].y,
                modifiers: KeyModifiers::NONE,
            },
            (80, 24).into(),
        );
        let crate::tui::overlay::OverlayOutcome::None(OverlayState::Metadata(mut state)) = outcome
        else {
            panic!("disabled Save submitted")
        };
        state.editor.as_mut().unwrap().focus = MetadataFocus::Input;
        app.overlay = Some(OverlayState::Metadata(state));
    }
    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
    let Some(OverlayState::Metadata(state)) = &mut app.overlay else {
        panic!()
    };
    assert_eq!(state.editor.as_ref().unwrap().focus, MetadataFocus::Remove);
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(app.store.metadata_values(&id).await.unwrap().is_empty());
    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
    assert!(app.detail.is_active());
    app.store.undo_last(app.list.selected_task()).await.unwrap();
    assert_eq!(
        app.store.metadata_values(&id).await.unwrap()[0].value,
        "pending"
    );
}

#[tokio::test]
async fn metadata_validation_keeps_exact_input_cursor_and_cancel_requires_confirmation() {
    let mut app = test_app().await;
    let i = create_and_select_task(&mut app, seeded_draft("Validation")).await;
    let id = app.store.tasks[i].task.id.clone();
    open_editor(&mut app).await;
    let input = "é".repeat(2049);
    let Some(OverlayState::Metadata(state)) = &mut app.overlay else {
        panic!()
    };
    let editor = state.editor.as_mut().unwrap();
    editor.input.lines = vec![input.clone()];
    editor.input.column = 12;
    app.handle_overlay_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL))
        .await
        .unwrap();
    let Some(OverlayState::Metadata(state)) = &app.overlay else {
        panic!()
    };
    assert!(state.error.as_ref().unwrap().contains("4096"));
    assert_eq!(state.editor.as_ref().unwrap().input.lines, vec![input]);
    assert_eq!(state.editor.as_ref().unwrap().input.column, 12);
    assert_eq!(
        app.store.metadata_values(&id).await.unwrap()[0].value,
        "pending"
    );
    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
    assert!(
        matches!(&app.overlay, Some(OverlayState::Metadata(state)) if state.editor.as_ref().unwrap().discard)
    );
    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
    assert!(
        matches!(&app.overlay, Some(OverlayState::Metadata(state)) if !state.editor.as_ref().unwrap().discard)
    );
}

#[tokio::test]
async fn metadata_mouse_and_focus_use_narrow_rendered_geometry() {
    let mut app = test_app().await;
    let mut draft = seeded_draft("Mouse");
    draft.metadata.push(TaskMetadataInput {
        expected_field_id: None,
        key: "second".to_string(),
        value: String::new(),
    });
    create_and_select_task(&mut app, draft).await;
    app.begin_edit_metadata().await.unwrap();
    let Some(OverlayState::Metadata(state)) = &app.overlay else {
        panic!()
    };
    let layout = crate::tui::overlay::metadata::metadata_layout(&state.view(), (40, 12).into());
    let Some(overlay) = app.overlay.take() else {
        panic!()
    };
    let outcome = crate::tui::overlay::dispatch_overlay_mouse(
        overlay,
        MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: layout.body.x,
            row: layout.body.y,
            modifiers: KeyModifiers::NONE,
        },
        (40, 12).into(),
        crate::tui::overlay::OverlayMouseContext {
            add_task_only: false,
            detail_help_scroll_cap: 0,
        },
    );
    let crate::tui::overlay::OverlayMouseOutcome::Retained(overlay) = outcome else {
        panic!()
    };
    assert!(matches!(&overlay, OverlayState::Metadata(state) if state.selected == 1));
    let outcome = crate::tui::overlay::dispatch_overlay_mouse(
        overlay,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: layout.body.x,
            row: layout.body.y,
            modifiers: KeyModifiers::NONE,
        },
        (40, 12).into(),
        crate::tui::overlay::OverlayMouseContext {
            add_task_only: false,
            detail_help_scroll_cap: 0,
        },
    );
    let crate::tui::overlay::OverlayMouseOutcome::Retained(overlay) = outcome else {
        panic!()
    };
    app.overlay = Some(overlay);
    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
    assert!(
        matches!(&app.overlay, Some(OverlayState::Metadata(state)) if state.editor.as_ref().unwrap().focus == MetadataFocus::Save)
    );
    app.handle_overlay_key(key(KeyCode::BackTab)).await.unwrap();
    let rendered = render_app_text(&mut app, 40, 12);
    assert!(rendered.contains("save"));
    assert!(rendered.contains("remove"));
    assert!(rendered.contains("cancel"));
}

#[tokio::test]
async fn recurring_creation_inherits_metadata_and_committed_refresh_failure_closes_draft() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, seeded_draft("Field seed")).await;
    let field = app.store.metadata_fields().await.unwrap().remove(0);
    app.begin_add_task().await.unwrap();
    let Some(OverlayState::AddTask(state)) = &mut app.overlay else {
        panic!()
    };
    state.title = LineEdit::new("Recurring metadata".to_string());
    state.selected_project = Some("aven".to_string());
    state.set_repeat_rule("daily".to_string());
    state.custom_metadata = vec![TaskMetadataInput {
        expected_field_id: Some(field.id),
        key: field.key,
        value: String::new(),
    }];
    app.store.fail_next_refresh();
    let error = app
        .handle_overlay_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL))
        .await
        .unwrap_err();
    assert!(crate::tui::store::mutation_committed(&error));
    assert!(app.overlay.is_none());
    assert!(app.authoring.add_task_context().is_none());
    app.store.refresh(None).await.unwrap();
    let item = app
        .store
        .tasks
        .iter()
        .find(|item| item.task.title == "Recurring metadata")
        .unwrap();
    assert_eq!(
        app.store.metadata_values(&item.task.id).await.unwrap()[0].value,
        ""
    );
    let series = item.recurrence.as_ref().unwrap().series_id.clone();
    let detail = app
        .store
        .recurrence_detail_for_series(&series)
        .await
        .unwrap();
    assert_eq!(detail.metadata[0].value, "");
}

#[tokio::test]
async fn renamed_field_keeps_input_and_requires_confirmed_retry() {
    let mut app = test_app().await;
    let i = create_and_select_task(&mut app, seeded_draft("Rename")).await;
    let id = app.store.tasks[i].task.id.clone();
    open_editor(&mut app).await;
    let Some(OverlayState::Metadata(state)) = &mut app.overlay else {
        panic!()
    };
    state.paste(" edited");
    let database = aven_core::db::Database::open(
        &app._test_database_dir
            .as_ref()
            .unwrap()
            .path()
            .join("test.db"),
    )
    .await
    .unwrap();
    database
        .rename_metadata_field(&app.store.active_workspace, "review", "review-state")
        .await
        .unwrap();
    app.handle_overlay_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL))
        .await
        .unwrap();
    let Some(OverlayState::Metadata(state)) = &app.overlay else {
        panic!()
    };
    assert!(state.error.as_ref().unwrap().contains("Save again"));
    assert_eq!(
        state.editor.as_ref().unwrap().input.lines,
        vec!["pending edited"]
    );
    assert_eq!(
        app.store.metadata_values(&id).await.unwrap()[0].value,
        "pending"
    );
    app.handle_overlay_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL))
        .await
        .unwrap();
    assert_eq!(
        app.store.metadata_values(&id).await.unwrap()[0].value,
        "pending edited"
    );
    assert_eq!(app.store.metadata_fields().await.unwrap().len(), 1);
}

#[tokio::test]
async fn metadata_heading_activation_and_committed_failure_preserve_detail() {
    let mut app = test_app().await;
    let index = create_and_select_task(&mut app, seeded_draft("Committed metadata")).await;
    let id = app.store.tasks[index].task.id.clone();
    app.show_detail(0);
    app.ensure_selected_task_detail().await.unwrap();
    app.detail
        .state_mut()
        .unwrap()
        .set_focused_target(Some(DetailTargetId::CustomMetadata));
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(matches!(app.overlay, Some(OverlayState::Metadata(_))));
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    let Some(OverlayState::Metadata(state)) = &mut app.overlay else {
        panic!()
    };
    state.paste(" committed");
    app.store.fail_next_refresh();
    let error = app
        .handle_overlay_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL))
        .await
        .unwrap_err();
    assert!(crate::tui::store::mutation_committed(&error));
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
    assert_eq!(
        app.store.metadata_values(&id).await.unwrap()[0].value,
        "pending committed"
    );
}

#[tokio::test]
async fn metadata_heading_focus_keeps_advertised_edit_shortcut_available() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, seeded_draft("Focused metadata")).await;
    app.show_detail(0);
    app.ensure_selected_task_detail().await.unwrap();
    app.detail
        .state_mut()
        .unwrap()
        .set_focused_target(Some(DetailTargetId::CustomMetadata));
    assert!(
        app.detail_focus_targets((80, 24).into())
            .contains(&DetailTargetId::CustomMetadata)
    );
    assert_eq!(
        app.capture_command_session(None).situation(),
        crate::tui::event::CommandSituation::ParentDetail
    );
    app.dispatch_key(key(KeyCode::Char('e')), (80, 24).into())
        .await
        .unwrap();
    assert_eq!(app.pending_shortcut.labels(), vec!["e"]);
    assert!(render_app_text(&mut app, 100, 40).contains("view and edit custom metadata"));
    app.dispatch_key(key(KeyCode::Char('m')), (80, 24).into())
        .await
        .unwrap();
    assert!(matches!(app.overlay, Some(OverlayState::Metadata(_))));
}

#[tokio::test]
async fn metadata_single_line_enter_saves_and_external_editor_preserves_draft_baseline() {
    let mut app = test_app().await;
    let index = create_and_select_task(&mut app, seeded_draft("Single line")).await;
    let id = app.store.tasks[index].task.id.clone();
    open_editor(&mut app).await;
    app.handle_overlay_key(key(KeyCode::Char('!')))
        .await
        .unwrap();
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert_eq!(
        app.store.metadata_values(&id).await.unwrap()[0].value,
        "pending!"
    );
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    app.dispatch_key(
        KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
        (80, 24).into(),
    )
    .await
    .unwrap();
    app.dispatch_key(
        KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL),
        (80, 24).into(),
    )
    .await
    .unwrap();
    let Some(OverlayState::Metadata(state)) = &app.overlay else {
        panic!()
    };
    let editor = state.editor.as_ref().unwrap();
    assert_eq!(editor.input.lines.join("\n"), "pending! from editor");
    assert_eq!(editor.input.baseline_value(), "pending!");
    assert!(editor.input.is_dirty());
    assert_eq!(
        app.store.metadata_values(&id).await.unwrap()[0].value,
        "pending!"
    );
    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
    assert!(
        matches!(&app.overlay, Some(OverlayState::Metadata(state)) if state.editor.as_ref().unwrap().discard)
    );
}

#[tokio::test]
async fn metadata_multiline_paste_becomes_read_only_and_editor_action_keeps_exact_text() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, seeded_draft("Multiline preview")).await;
    open_editor(&mut app).await;
    let Some(OverlayState::Metadata(state)) = &mut app.overlay else {
        panic!()
    };
    state.paste("\r\n  next\n");
    let original = state.editor.as_ref().unwrap().input.lines.join("\n");
    assert_eq!(original, "pending\r\n  next\n");
    assert_eq!(
        state.editor.as_ref().unwrap().focus,
        MetadataFocus::ExternalEditor
    );
    app.handle_overlay_key(key(KeyCode::Char('x')))
        .await
        .unwrap();
    let Some(OverlayState::Metadata(state)) = &mut app.overlay else {
        panic!()
    };
    state.paste("ignored");
    assert_eq!(
        state.editor.as_ref().unwrap().input.lines.join("\n"),
        original
    );
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    let Some(OverlayState::Metadata(state)) = &app.overlay else {
        panic!()
    };
    let editor = state.editor.as_ref().unwrap();
    assert_eq!(
        editor.input.lines.join("\n"),
        format!("{original} from editor")
    );
    assert_eq!(editor.input.baseline_value(), "pending");
    assert_eq!(editor.focus, MetadataFocus::Save);
}

#[tokio::test]
async fn metadata_discard_uses_explicit_y_and_preserves_input_on_n_or_escape() {
    let mut app = test_app().await;
    let index = create_and_select_task(&mut app, seeded_draft("Discard metadata")).await;
    let id = app.store.tasks[index].task.id.clone();
    open_editor(&mut app).await;
    app.handle_overlay_key(key(KeyCode::Char('!')))
        .await
        .unwrap();
    for keep in [KeyCode::Esc, KeyCode::Char('n'), KeyCode::Char('N')] {
        app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
        let rendered = render_app_text(&mut app, 100, 30);
        assert!(rendered.contains("Discard metadata changes?"));
        assert!(rendered.contains("y discard"));
        assert!(rendered.contains("n keep editing"));
        assert!(!rendered.contains("pending!"));
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        app.handle_overlay_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL))
            .await
            .unwrap();
        assert!(
            matches!(&app.overlay, Some(OverlayState::Metadata(state)) if state.editor.as_ref().unwrap().discard)
        );
        app.handle_overlay_key(key(keep)).await.unwrap();
        let Some(OverlayState::Metadata(state)) = &app.overlay else {
            panic!()
        };
        let editor = state.editor.as_ref().unwrap();
        assert!(!editor.discard);
        assert_eq!(editor.input.lines.join("\n"), "pending!");
    }
    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Char('y')))
        .await
        .unwrap();
    assert!(matches!(&app.overlay, Some(OverlayState::Metadata(state)) if state.editor.is_none()));
    assert_eq!(
        app.store.metadata_values(&id).await.unwrap()[0].value,
        "pending"
    );
}
