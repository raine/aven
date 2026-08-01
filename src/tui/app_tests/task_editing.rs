use super::*;

#[tokio::test]
async fn add_note_blank_body_is_rejected() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Note target")).await;

    app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('N')).await.unwrap();
    app.handle_overlay_key(ctrl_s()).await.unwrap();

    assert!(app.overlay.is_none());
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("note body is required")
    );
}

#[tokio::test]
async fn no_selected_mutating_shortcuts_report_failure() {
    let mut app = test_app().await;
    app.list.select_task(None);

    for sequence in [
        [KeyCode::Char('t'), KeyCode::Char('i')],
        [KeyCode::Char('t'), KeyCode::Char('h')],
        [KeyCode::Char('t'), KeyCode::Char('D')],
        [KeyCode::Char('t'), KeyCode::Char('R')],
    ] {
        app.notification = None;
        app.handle_normal_key(sequence[0]).await.unwrap();
        app.handle_normal_key(sequence[1]).await.unwrap();
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("no selected task to edit")
        );
    }
}

#[tokio::test]
async fn add_project_shortcut_opens_prompt_and_creates_project() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('p')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::TextInput(state)) if state.prompt == "project name:"
    ));

    for ch in "Mobile App".chars() {
        app.handle_overlay_key(key(KeyCode::Char(ch)))
            .await
            .unwrap();
    }
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(app.overlay.is_none());
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("created project mobile-app")
    );
    assert!(
        app.store
            .projects
            .iter()
            .any(|project| project.key == "mobile-app")
    );
    assert!(
        app.store
            .sidebar_entries
            .iter()
            .any(|entry| entry.label.contains("Mobile App"))
    );
}

#[tokio::test]
async fn add_label_shortcut_opens_prompt_and_creates_label() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('L')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('n')).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::TextInput(state)) if state.prompt == "label name:"
    ));

    for ch in "Needs Review".chars() {
        app.handle_overlay_key(key(KeyCode::Char(ch)))
            .await
            .unwrap();
    }
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(app.overlay.is_none());
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("created label needs-review")
    );
    assert!(app.store.labels.iter().any(|label| label == "needs-review"));
    assert!(
        app.store
            .label_picker_items()
            .iter()
            .any(|item| item.value == "needs-review")
    );
}

#[tokio::test]
async fn label_browser_renames_and_safely_deletes_used_labels() {
    let mut app = test_app().await;
    app.store
        .create_label("Bug Report".to_string())
        .await
        .unwrap();
    create_and_select_task(
        &mut app,
        TaskDraft {
            labels: vec!["bug-report".to_string()],
            ..test_task_draft("Labeled task")
        },
    )
    .await;

    app.execute(Action::BeginBrowseLabels).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Picker(state))
            if state.intent == PickerIntent::BrowseLabels
                && state.items.iter().any(|item| {
                    item.value == "bug-report" && item.label.contains("1 task")
                })
    ));
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Picker(state))
            if matches!(state.intent, PickerIntent::LabelActions { ref label } if label == "bug-report")
    ));

    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    app.handle_overlay_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL))
        .await
        .unwrap();
    type_chars(&mut app, "Customer Bug").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(app.store.labels.iter().any(|label| label == "customer-bug"));
    assert_eq!(app.store.tasks[0].labels, vec!["customer-bug"]);

    app.execute(Action::BeginDeleteLabel).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::TextInput(state))
            if matches!(state.intent, TextIntent::ConfirmDeleteLabel {
                ref label,
                task_count: 1,
                series_count: 0,
            } if label == "customer-bug")
    ));
    let Some(OverlayState::TextInput(state)) = &app.overlay else {
        panic!("delete label confirmation should remain open");
    };
    assert_eq!(
        state.prompt,
        "Type customer-bug to delete this label.\nUsed by: 1 task, 0 recurring series"
    );
    type_chars(&mut app, "wrong").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(app.store.labels.iter().any(|label| label == "customer-bug"));
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("label name does not match")
    );

    app.handle_overlay_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL))
        .await
        .unwrap();
    type_chars(&mut app, "customer-bug").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Confirm(state))
            if matches!(state.intent, ConfirmIntent::DeleteLabel { ref label } if label == "customer-bug")
    ));
    app.handle_overlay_key(key(KeyCode::Char('y')))
        .await
        .unwrap();
    assert!(!app.store.labels.iter().any(|label| label == "customer-bug"));
    assert!(app.store.tasks[0].labels.is_empty());
}

#[tokio::test]
async fn edit_title_shortcut_prefills_and_updates_title() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Old title")).await;

    app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('e')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::TextInput(state))
            if state.title == EDIT_TITLE_TITLE
                && state.prompt.is_empty()
                && state.input.as_str() == "Old title"
    ));

    app.handle_overlay_key(key(KeyCode::End)).await.unwrap();
    type_chars(&mut app, " updated").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    let selected = app.list.selected_task().unwrap();
    assert_eq!(app.store.tasks[selected].task.title, "Old title updated");
}

#[tokio::test]
async fn one_mark_edits_the_marked_title_and_preserves_cursor_selection() {
    let mut app = test_app().await;
    let marked = create_and_select_task(&mut app, test_task_draft("Marked title")).await;
    let marked_id = app.store.tasks[marked].task.id.clone();
    let selected = create_and_select_task(&mut app, test_task_draft("Cursor title")).await;
    let selected_id = app.store.tasks[selected].task.id.clone();
    app.list.mark(marked_id.clone());

    app.begin_edit_title();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::TextInput(state)) if state.input.as_str() == "Marked title"
    ));
    app.handle_overlay_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL))
        .await
        .unwrap();
    type_chars(&mut app, "Updated marked title").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    let marked = app
        .store
        .tasks
        .iter()
        .find(|item| item.task.id == marked_id)
        .unwrap();
    assert_eq!(marked.task.title, "Updated marked title");
    let selected = app.list.selected_task().unwrap();
    assert_eq!(app.store.tasks[selected].task.id, selected_id);
    assert!(app.list.marked_task_ids().contains(&marked_id));
}

#[tokio::test]
async fn multiple_marks_disable_title_editing() {
    let mut app = test_app().await;
    let first = create_and_select_task(&mut app, test_task_draft("First")).await;
    let first_id = app.store.tasks[first].task.id.clone();
    let second = create_and_select_task(&mut app, test_task_draft("Second")).await;
    let second_id = app.store.tasks[second].task.id.clone();
    app.list.mark(first_id);
    app.list.mark(second_id);

    app.begin_edit_title();

    assert!(app.overlay.is_none());
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("title requires one task · 2 tasks marked")
    );
}

#[tokio::test]
async fn edit_description_prefills_and_ctrl_s_updates() {
    let mut app = test_app().await;
    create_and_select_task(
        &mut app,
        TaskDraft {
            description: "first\nsecond".to_string(),
            ..test_task_draft("Description target")
        },
    )
    .await;

    app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('e')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('d')).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::MultilineInput(state))
            if state.title == EDIT_DESCRIPTION_TITLE
                && state.prompt.is_empty()
                && state.lines == vec!["first".to_string(), "second".to_string()]
    ));

    app.handle_overlay_key(key(KeyCode::End)).await.unwrap();
    type_chars(&mut app, " updated").await;
    app.handle_overlay_key(ctrl_s()).await.unwrap();

    let selected = app.list.selected_task().unwrap();
    assert_eq!(
        app.store.tasks[selected].task.description,
        "first\nsecond updated"
    );
}

#[tokio::test]
async fn clean_description_edit_cancels_without_confirmation() {
    let mut app = test_app().await;
    create_and_select_task(
        &mut app,
        TaskDraft {
            description: "existing description".to_string(),
            ..test_task_draft("Description target")
        },
    )
    .await;

    app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('e')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('d')).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();

    assert!(app.overlay.is_none());
    assert_eq!(
        app.store.tasks[app.list.selected_task().unwrap()]
            .task
            .description,
        "existing description"
    );
}

#[tokio::test]
async fn changed_description_edit_requires_discard_confirmation() {
    let mut app = test_app().await;
    create_and_select_task(
        &mut app,
        TaskDraft {
            description: "existing description".to_string(),
            ..test_task_draft("Description target")
        },
    )
    .await;

    app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('e')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('d')).await.unwrap();
    type_chars(&mut app, " updated").await;
    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::MultilineInput(state))
            if state.mode == MultilineInputMode::ConfirmDiscard
                && state.lines == ["existing description updated"]
    ));
    app.handle_overlay_key(key(KeyCode::Char('y')))
        .await
        .unwrap();
    assert!(app.overlay.is_none());
    assert_eq!(
        app.store.tasks[app.list.selected_task().unwrap()]
            .task
            .description,
        "existing description"
    );
}

#[tokio::test]
async fn edit_project_picker_creates_and_assigns_project() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Project target")).await;

    app.begin_edit_project();
    type_chars(&mut app, "Mobile App").await;
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Picker(PickerState { items, .. }))
            if items.iter().any(|item| {
                item.label == "+ Create project \"Mobile App\""
                    && crate::tui::store::create_project_picker_name(&item.value)
                        == Some("Mobile App")
            })
    ));
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    let selected = app.list.selected_task().unwrap();
    assert_eq!(app.store.tasks[selected].task.project_key, "mobile-app");
}

#[tokio::test]
async fn edit_project_creation_cancel_leaves_task_unchanged() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Project target")).await;
    let original_project = app.store.tasks[app.list.selected_task().unwrap()]
        .task
        .project_key
        .clone();

    app.begin_edit_project();
    type_chars(&mut app, "New Project").await;
    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();

    assert!(app.overlay.is_none());
    let selected = app.list.selected_task().unwrap();
    assert_eq!(app.store.tasks[selected].task.project_key, original_project);
}

#[tokio::test]
async fn edit_project_creation_error_keeps_picker_filter_open() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Project target")).await;

    app.begin_edit_project();
    type_chars(&mut app, "---").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Picker(PickerState { filter, items, .. }))
            if filter.as_str() == "---"
                && items.iter().any(|item| {
                    item.label == "+ Create project \"---\""
                        && crate::tui::store::create_project_picker_name(&item.value)
                            == Some("---")
                })
    ));
    assert!(toast_message(&app).is_some_and(|message| message.contains("invalid-project")));
}

#[tokio::test]
async fn edit_project_picker_excludes_project_inference() {
    let mut app = test_app().await;
    app.store
        .create_project("Mobile App".to_string())
        .await
        .unwrap();
    create_and_select_task(&mut app, test_task_draft("Project target")).await;

    app.handle_normal_key(KeyCode::Char('e')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('j')).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Picker(state))
            if state.title == EDIT_PROJECT_TITLE
                && !state.items.iter().any(|item| item.label == "Infer project")
    ));

    type_chars(&mut app, "mobile").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    let selected = app.list.selected_task().unwrap();
    assert_eq!(app.store.tasks[selected].task.project_key, "mobile-app");
}

#[tokio::test]
async fn edit_priority_picker_prefills_current_priority() {
    let mut app = test_app().await;
    create_and_select_task(
        &mut app,
        TaskDraft {
            priority: "high".to_string(),
            ..test_task_draft("Priority target")
        },
    )
    .await;

    app.handle_normal_key(KeyCode::Char('e')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('p')).await.unwrap();
    assert_eq!(
        app.footer_choice.as_ref().map(|choice| choice.mode),
        Some(FooterChoiceMode::Priority)
    );

    app.dispatch_key(key(KeyCode::Char('u')), (80, 24).into())
        .await
        .unwrap();
    let selected = app.list.selected_task().unwrap();
    assert_eq!(
        app.store.tasks[selected].task.priority,
        TaskPriority::Urgent
    );
}

#[tokio::test]
async fn edit_labels_picker_prefills_current_labels_and_removes_unselected() {
    let mut app = test_app().await;
    app.store.create_label("Bug".to_string()).await.unwrap();
    app.store.create_label("Docs".to_string()).await.unwrap();
    create_and_select_task(
        &mut app,
        TaskDraft {
            labels: vec!["bug".to_string()],
            ..test_task_draft("Label target")
        },
    )
    .await;

    app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('e')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('l')).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::TagCombobox(state))
            if state.title == EDIT_LABELS_TITLE
                && state.options.iter().any(|item| item == "bug")
                && state.selected.iter().any(|item| item == "bug")
    ));

    type_chars(&mut app, "bug").await;
    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
    type_chars(&mut app, "docs").await;
    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    let selected = app.list.selected_task().unwrap();
    assert_eq!(app.store.tasks[selected].labels, vec!["docs".to_string()]);
}

#[tokio::test]
async fn status_picker_alias_updates_selected_task() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Status alias")).await;

    app.handle_normal_key(KeyCode::Char('s')).await.unwrap();
    assert_eq!(
        app.footer_choice.as_ref().map(|choice| choice.mode),
        Some(FooterChoiceMode::Status)
    );
    app.dispatch_key(key(KeyCode::Char('t')), (80, 24).into())
        .await
        .unwrap();

    let selected = app.list.selected_task().unwrap();
    assert_eq!(app.store.tasks[selected].task.status, TaskStatus::Todo);
}

#[tokio::test]
async fn done_alias_keeps_selected_row_position() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("First")).await;
    create_and_select_task(&mut app, test_task_draft("Second")).await;
    create_and_select_task(&mut app, test_task_draft("Third")).await;
    let selected = 1;
    let next_title = app.store.tasks[selected + 1].task.title.clone();
    app.list.select_task(Some(selected));

    app.handle_normal_key(KeyCode::Char('d')).await.unwrap();

    assert_eq!(app.list.selected_task(), Some(selected));
    let selected = app.list.selected_task().unwrap();
    assert_eq!(app.store.tasks[selected].task.title, next_title);
    assert_eq!(app.store.tasks[selected].task.status, TaskStatus::Inbox);
}

#[tokio::test]
async fn done_alias_clamps_selection_when_last_row_is_done() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("First")).await;
    create_and_select_task(&mut app, test_task_draft("Second")).await;
    let selected = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.title == "Second")
        .unwrap();
    app.list.select_task(Some(selected));

    app.handle_normal_key(KeyCode::Char('d')).await.unwrap();

    assert_eq!(app.list.selected_task(), Some(0));
    let selected = app.list.selected_task().unwrap();
    assert_eq!(app.store.tasks[selected].task.title, "First");
}

#[tokio::test]
async fn done_and_cancel_aliases_update_selected_task() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Status alias")).await;

    app.handle_normal_key(KeyCode::Char('d')).await.unwrap();
    let selected = app.store.show_view(TaskView::Done).await.unwrap();
    app.list.select_task(selected);
    let selected = app.list.selected_task().unwrap();
    assert_eq!(app.store.tasks[selected].task.status, TaskStatus::Done);

    app.handle_normal_key(KeyCode::Char('x')).await.unwrap();
    let selected = app.store.show_view(TaskView::Done).await.unwrap();
    app.list.select_task(selected);
    let selected = app.list.selected_task().unwrap();
    assert_eq!(app.store.tasks[selected].task.status, TaskStatus::Canceled);
}

#[tokio::test]
async fn exact_priority_shortcut_updates_selected_task() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Priority shortcut")).await;

    app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('u')).await.unwrap();

    let selected = app.list.selected_task().unwrap();
    assert_eq!(
        app.store.tasks[selected].task.priority,
        TaskPriority::Urgent
    );
}

#[tokio::test]
async fn edit_shortcuts_require_selected_task() {
    let mut app = test_app().await;
    app.list.select_task(None);

    app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('e')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('t')).await.unwrap();

    assert!(app.overlay.is_none());
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("no selected task to edit")
    );
}

#[tokio::test]
async fn edit_description_conflict_preserves_overlay() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    let selected = create_and_select_task(
        &mut app,
        TaskDraft {
            description: "old".to_string(),
            ..test_task_draft("Conflict target")
        },
    )
    .await;
    let task_id = app.store.tasks[selected].task.id.clone();
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query(
        "INSERT INTO conflicts(task_id, field, base_version, local_value, remote_value,
         local_change_id, remote_change_id, variant_a, variant_b, created_at, resolved)
         VALUES (?, 'description', NULL, 'local', 'remote', NULL, ?, 'a', 'b', ?, 0)",
    )
    .bind(&task_id)
    .bind(crate::ids::new_id())
    .bind(crate::ids::now())
    .execute(&mut *conn)
    .await
    .unwrap();
    drop(conn);

    app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('e')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('d')).await.unwrap();
    type_chars(&mut app, " updated").await;
    app.handle_overlay_key(ctrl_s()).await.unwrap();

    assert!(toast_message(&app).is_some_and(|message| message.contains("conflicted-field")));
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::MultilineInput(state))
            if state.lines.join("\n") == "old updated"
    ));
}

#[tokio::test]
async fn detail_copy_hotkeys_copy_task_text_and_show_feedback() {
    let mut app = test_app().await;
    let selected = create_and_select_task(
        &mut app,
        TaskDraft {
            description: "First paragraph.\n\n- item".to_string(),
            ..test_task_draft("Copy target")
        },
    )
    .await;
    app.store.tasks[selected]
        .notes
        .push(crate::query::TaskNote {
            id: "note-id".to_string(),
            body: "Note body".to_string(),
            created_at: crate::ids::now(),
        });
    assert!(app.view().copy_description_available);
    assert!(app.view().copy_notes_available);

    for (key_code, expected_message) in [
        ('t', "copied task title"),
        ('d', "copied task description"),
        ('a', "copied task title and description"),
        ('n', "copied task notes"),
    ] {
        app.show_detail(3);
        app.dispatch_key(key(KeyCode::Char('y')), (80, 24).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char(key_code)), (80, 24).into())
            .await
            .unwrap();

        assert_eq!(toast_message(&app).as_deref(), Some(expected_message));
        assert_eq!(toast_severity(&app), Some(ToastSeverity::Success));
        assert!(app.overlay.is_none());
        assert!(app.detail.is_active());
    }
}

#[tokio::test]
async fn copying_empty_task_description_shows_info() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Title only")).await;
    app.show_detail(0);
    assert!(!app.view().copy_description_available);

    app.dispatch_key(key(KeyCode::Char('y')), (80, 24).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char('d')), (80, 24).into())
        .await
        .unwrap();

    assert_eq!(
        toast_message(&app).as_deref(),
        Some("task description is empty")
    );
    assert_eq!(toast_severity(&app), Some(ToastSeverity::Info));
}

#[tokio::test]
async fn copying_task_without_notes_shows_info() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("No notes")).await;
    assert!(!app.view().copy_notes_available);

    app.handle_normal_key(KeyCode::Char('y')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('n')).await.unwrap();

    assert_eq!(toast_message(&app).as_deref(), Some("task has no notes"));
    assert_eq!(toast_severity(&app), Some(ToastSeverity::Info));
}

#[tokio::test]
async fn table_copy_hotkeys_copy_task_text_and_show_feedback() {
    let mut app = test_app().await;
    let selected = create_and_select_task(
        &mut app,
        TaskDraft {
            description: "First paragraph.\n\n- item".to_string(),
            ..test_task_draft("Copy target")
        },
    )
    .await;
    app.store.tasks[selected]
        .notes
        .push(crate::query::TaskNote {
            id: "note-id".to_string(),
            body: "Note body".to_string(),
            created_at: crate::ids::now(),
        });
    assert!(app.view().copy_description_available);
    assert!(app.view().copy_notes_available);

    for (key_code, expected_message) in [
        ('t', "copied task title"),
        ('d', "copied task description"),
        ('a', "copied task title and description"),
        ('n', "copied task notes"),
    ] {
        app.handle_normal_key(KeyCode::Char('y')).await.unwrap();
        app.handle_normal_key(KeyCode::Char(key_code))
            .await
            .unwrap();

        assert_eq!(toast_message(&app).as_deref(), Some(expected_message));
        assert_eq!(toast_severity(&app), Some(ToastSeverity::Success));
        assert!(app.overlay.is_none());
    }
}

#[tokio::test]
async fn table_copy_menu_copies_display_ref_and_id() {
    let mut app = test_app().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Copy target")).await;
    let display_ref = app.store.tasks[selected].display_ref.clone();

    for key_code in ['r', 'i'] {
        app.handle_normal_key(KeyCode::Char('y')).await.unwrap();
        app.handle_normal_key(KeyCode::Char(key_code))
            .await
            .unwrap();

        assert_eq!(
            toast_message(&app).as_deref(),
            Some(format!("copied {display_ref}").as_str())
        );
        assert_eq!(toast_severity(&app), Some(ToastSeverity::Success));
    }
}

#[tokio::test]
async fn copy_requires_selected_task() {
    let mut app = test_app().await;
    app.list.select_task(None);

    app.copy_selected_ref(TaskRefKind::Short);

    assert_eq!(
        toast_message(&app).as_deref(),
        Some("no selected task to copy")
    );
}

#[tokio::test]
async fn undo_shortcut_reverts_last_mutation() {
    let mut app = test_app().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Before")).await;
    app.store
        .update_title(Some(selected), "After".to_string())
        .await
        .unwrap();
    assert_eq!(app.store.tasks[selected].task.title, "After");

    app.handle_normal_key(KeyCode::Char('u')).await.unwrap();
    assert_eq!(app.store.tasks[selected].task.title, "Before");
    assert!(toast_message(&app).is_some_and(|message| message.contains("undid")));
}

#[tokio::test]
async fn undo_shortcut_keeps_selected_row_position() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("First")).await;
    create_and_select_task(&mut app, test_task_draft("Second")).await;
    create_and_select_task(&mut app, test_task_draft("Third")).await;
    let selected = 1;
    app.list.select_task(Some(selected));

    app.handle_normal_key(KeyCode::Char('d')).await.unwrap();
    assert_eq!(app.list.selected_task(), Some(selected));

    app.handle_normal_key(KeyCode::Char('u')).await.unwrap();

    assert_eq!(app.list.selected_task(), Some(selected));
    assert_eq!(app.store.tasks[selected].task.status, TaskStatus::Inbox);
}

#[tokio::test]
async fn undo_command_reverts_last_mutation() {
    let mut app = test_app().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Before")).await;
    app.store
        .update_title(Some(selected), "After".to_string())
        .await
        .unwrap();

    app.begin_command().await;
    for ch in "undo".chars() {
        app.handle_overlay_key(key(KeyCode::Char(ch)))
            .await
            .unwrap();
    }
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert_eq!(app.store.tasks[selected].task.title, "Before");
}

#[tokio::test]
async fn undo_reports_nothing_to_undo() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('u')).await.unwrap();
    assert_eq!(toast_message(&app).as_deref(), Some("nothing to undo"));
    assert_eq!(toast_severity(&app), Some(ToastSeverity::Info));
}
