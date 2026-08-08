use super::*;

#[tokio::test]
async fn status_shortcut_uses_footer_chooser_for_unmarked_task() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("unmarked")).await;

    app.handle_normal_key(KeyCode::Char('s')).await.unwrap();

    assert_eq!(
        app.footer_choice.as_ref().map(|choice| choice.mode),
        Some(FooterChoiceMode::Status)
    );
    assert!(app.overlay.is_none());

    app.dispatch_key(key(KeyCode::Char('t')), (80, 24).into())
        .await
        .unwrap();

    let selected = app.list.selected_task().unwrap();
    assert_eq!(app.store.tasks[selected].task.status, TaskStatus::Todo);
}

#[tokio::test]
async fn command_palette_captures_bulk_scope_and_single_target_limits() {
    let mut app = test_app().await;
    let first = create_and_select_task(&mut app, test_task_draft("first")).await;
    let first_id = app.store.tasks[first].task.id.clone();
    let second = create_and_select_task(&mut app, test_task_draft("second")).await;
    let second_id = app.store.tasks[second].task.id.clone();
    app.list.mark(first_id);
    app.list.mark(second_id);

    app.begin_command().await;

    let Some(OverlayState::Command { state }) = &app.overlay else {
        panic!("expected command palette");
    };
    assert_eq!(state.marked_task_count, 2);
    assert!(state.unavailable.iter().any(|override_| {
        override_.action == crate::tui::event::Action::BeginEditTitle
            && override_.reason == "one task only"
    }));

    type_chars(&mut app, "edit-title").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(app.overlay.is_none());
    assert_eq!(
        toast_message(&app).as_deref(),
        Some(":edit-title is disabled: one task only")
    );
}

#[tokio::test]
async fn status_shortcut_uses_footer_chooser_for_marked_tasks_with_undo() {
    let mut app = test_app().await;
    let first = create_and_select_task(&mut app, test_task_draft("first")).await;
    let first_id = app.store.tasks[first].task.id.clone();
    let second = create_and_select_task(&mut app, test_task_draft("second")).await;
    let second_id = app.store.tasks[second].task.id.clone();
    let third = create_and_select_task(&mut app, test_task_draft("third")).await;
    let third_id = app.store.tasks[third].task.id.clone();
    app.list.mark(first_id.clone());
    app.list.mark(second_id.clone());

    app.handle_normal_key(KeyCode::Char('s')).await.unwrap();
    assert_eq!(
        app.footer_choice.as_ref().map(|choice| choice.mode),
        Some(FooterChoiceMode::Status)
    );
    assert!(app.overlay.is_none());

    app.dispatch_key(key(KeyCode::Char('t')), (80, 24).into())
        .await
        .unwrap();

    let status_for = |app: &App, task_id: &str| {
        app.store
            .tasks
            .iter()
            .find(|item| item.task.id.as_str() == task_id)
            .unwrap()
            .task
            .status
    };
    assert_eq!(status_for(&app, &first_id), TaskStatus::Todo);
    assert_eq!(status_for(&app, &second_id), TaskStatus::Todo);
    assert_eq!(status_for(&app, &third_id), TaskStatus::Inbox);
    assert_eq!(app.list.marked_task_ids().len(), 2);

    app.handle_normal_key(KeyCode::Char('u')).await.unwrap();

    assert_eq!(status_for(&app, &first_id), TaskStatus::Inbox);
    assert_eq!(status_for(&app, &second_id), TaskStatus::Inbox);
    assert_eq!(status_for(&app, &third_id), TaskStatus::Inbox);
    assert_eq!(app.list.marked_task_ids().len(), 2);
}

#[tokio::test]
async fn submit_edit_project_updates_only_marked_tasks() {
    let mut app = test_app().await;
    app.store
        .create_project("Mobile App".to_string())
        .await
        .unwrap();
    let first = create_and_select_task(&mut app, test_task_draft("first")).await;
    let first_id = app.store.tasks[first].task.id.clone();
    let second = create_and_select_task(&mut app, test_task_draft("second")).await;
    let second_id = app.store.tasks[second].task.id.clone();
    let third = create_and_select_task(&mut app, test_task_draft("third")).await;
    let third_id = app.store.tasks[third].task.id.clone();
    let original_project = app.store.tasks[third].task.project_key.clone();
    app.list.mark(first_id.clone());
    app.list.mark(second_id.clone());
    app.begin_edit_project();

    let (selection, mixed) = match &app.overlay {
        Some(OverlayState::Picker(PickerState {
            intent: PickerIntent::EditProject { selection, mixed },
            ..
        })) => (selection.clone(), *mixed),
        overlay => panic!("expected project edit intent, got {overlay:?}"),
    };
    app.submit_edit_project(selection, mixed, "mobile-app".to_string())
        .await
        .unwrap();

    let project_for = |app: &App, task_id: &str| {
        app.store
            .tasks
            .iter()
            .find(|item| item.task.id.as_str() == task_id)
            .unwrap()
            .task
            .project_key
            .clone()
    };
    assert_eq!(project_for(&app, &first_id), "mobile-app");
    assert_eq!(project_for(&app, &second_id), "mobile-app");
    assert_eq!(project_for(&app, &third_id), original_project);
}

#[tokio::test]
async fn project_edit_records_moved_task_for_detail_recall() {
    let mut app = test_app().await;
    app.store
        .create_project("Mobile App".to_string())
        .await
        .unwrap();
    let selected = create_and_select_task(&mut app, test_task_draft("moved task")).await;
    let changed_id = app.store.tasks[selected].task.id.clone();
    let original_project = app.store.tasks[selected].task.project_key.clone();
    app.show_scope(TaskScopeTarget::Project(original_project))
        .await
        .unwrap();
    app.list.select_task(Some(0));
    app.show_detail(0);
    app.begin_edit_project();
    let (selection, mixed) = match &app.overlay {
        Some(OverlayState::Picker(PickerState {
            intent: PickerIntent::EditProject { selection, mixed },
            ..
        })) => (selection.clone(), *mixed),
        overlay => panic!("expected project edit intent, got {overlay:?}"),
    };

    app.submit_edit_project(selection, mixed, "mobile-app".to_string())
        .await
        .unwrap();

    assert_eq!(app.list.last_changed_task_id(), Some(&changed_id));
    assert!(toast_message(&app).unwrap().contains("g . return"));
    app.execute(Action::ReturnToLastChange).await.unwrap();
    assert!(app.detail.is_active());
    assert_eq!(app.store.view_state.view, TaskView::Search);
    assert_eq!(app.store.tasks.len(), 1);
    assert_eq!(app.store.tasks[0].task.id, changed_id);
}

#[tokio::test]
async fn project_edit_keeps_captured_targets_and_anchor_across_refresh() {
    let mut app = test_app().await;
    app.store
        .create_project("Mobile App".to_string())
        .await
        .unwrap();
    let first = create_and_select_task(&mut app, test_task_draft("first")).await;
    let first_id = app.store.tasks[first].task.id.clone();
    let second = create_and_select_task(&mut app, test_task_draft("second")).await;
    let second_id = app.store.tasks[second].task.id.clone();
    let third = create_and_select_task(&mut app, test_task_draft("third")).await;
    let third_id = app.store.tasks[third].task.id.clone();
    app.list.mark(first_id.clone());
    app.list.mark(second_id.clone());
    app.begin_edit_project();

    app.list.clear_marks();
    app.list.mark(third_id.clone());
    app.list.select_task(Some(first));
    app.store.refresh(None).await.unwrap();
    let (selection, mixed) = match &app.overlay {
        Some(OverlayState::Picker(PickerState {
            intent: PickerIntent::EditProject { selection, mixed },
            ..
        })) => (selection.clone(), *mixed),
        overlay => panic!("expected project edit intent, got {overlay:?}"),
    };
    app.submit_edit_project(selection, mixed, "mobile-app".to_string())
        .await
        .unwrap();

    let project_for = |app: &App, task_id: &crate::ids::TaskId| {
        app.store
            .tasks
            .iter()
            .find(|item| &item.task.id == task_id)
            .unwrap()
            .task
            .project_key
            .clone()
    };
    assert_eq!(project_for(&app, &first_id), "mobile-app");
    assert_eq!(project_for(&app, &second_id), "mobile-app");
    assert_ne!(project_for(&app, &third_id), "mobile-app");
    assert_eq!(
        app.store
            .selected_task(app.list.selected_task())
            .map(|item| &item.task.id),
        Some(&third_id)
    );
}

#[tokio::test]
async fn empty_project_retry_keeps_captured_targets() {
    let mut app = test_app().await;
    let first = create_and_select_task(&mut app, test_task_draft("first")).await;
    let first_id = app.store.tasks[first].task.id.clone();
    let second = create_and_select_task(&mut app, test_task_draft("second")).await;
    let second_id = app.store.tasks[second].task.id.clone();
    let third = create_and_select_task(&mut app, test_task_draft("third")).await;
    let third_id = app.store.tasks[third].task.id.clone();
    app.list.mark(first_id.clone());
    app.list.mark(second_id.clone());
    app.begin_edit_project();
    let (selection, mixed) = match &app.overlay {
        Some(OverlayState::Picker(PickerState {
            intent: PickerIntent::EditProject { selection, mixed },
            ..
        })) => (selection.clone(), *mixed),
        overlay => panic!("expected project edit intent, got {overlay:?}"),
    };

    let expected_ids = selection.ids().cloned().collect::<Vec<_>>();
    app.list.clear_marks();
    app.list.mark(third_id);
    app.handle_overlay_submit(crate::tui::overlay::OverlaySubmit::Picker {
        intent: PickerIntent::EditProject { selection, mixed },
        values: Vec::new(),
        partial_values: Vec::new(),
    })
    .await
    .unwrap();

    let retried_ids = match &app.overlay {
        Some(OverlayState::Picker(PickerState {
            intent: PickerIntent::EditProject { selection, .. },
            ..
        })) => selection.ids().cloned().collect::<Vec<_>>(),
        overlay => panic!("expected retried project edit intent, got {overlay:?}"),
    };
    assert_eq!(retried_ids, expected_ids);
}

#[tokio::test]
async fn submit_edit_priority_updates_only_marked_tasks() {
    let mut app = test_app().await;
    let first = create_and_select_task(&mut app, test_task_draft("first")).await;
    let first_id = app.store.tasks[first].task.id.clone();
    let second = create_and_select_task(&mut app, test_task_draft("second")).await;
    let second_id = app.store.tasks[second].task.id.clone();
    let third = create_and_select_task(&mut app, test_task_draft("third")).await;
    let third_id = app.store.tasks[third].task.id.clone();
    app.list.mark(first_id.clone());
    app.list.mark(second_id.clone());
    app.begin_edit_priority();

    let (selection, mixed) = match &app.overlay {
        Some(OverlayState::Picker(PickerState {
            intent: PickerIntent::EditPriority { selection, mixed },
            ..
        })) => (selection.clone(), *mixed),
        overlay => panic!("expected priority edit intent, got {overlay:?}"),
    };
    app.submit_edit_priority(selection, mixed, "high".to_string())
        .await
        .unwrap();

    let priority_for = |app: &App, task_id: &str| {
        app.store
            .tasks
            .iter()
            .find(|item| item.task.id.as_str() == task_id)
            .unwrap()
            .task
            .priority
    };
    assert_eq!(priority_for(&app, &first_id), TaskPriority::High);
    assert_eq!(priority_for(&app, &second_id), TaskPriority::High);
    assert_eq!(priority_for(&app, &third_id), TaskPriority::None);
}

#[tokio::test]
async fn mixed_marked_due_dates_keep_then_set_and_undo_as_one_batch() {
    let mut app = test_app().await;
    let first = create_and_select_task(
        &mut app,
        TaskDraft {
            metadata: Vec::new(),
            due_on: Some("2099-01-01".to_string()),
            ..test_task_draft("first")
        },
    )
    .await;
    let first_id = app.store.tasks[first].task.id.clone();
    let second = create_and_select_task(&mut app, test_task_draft("second")).await;
    let second_id = app.store.tasks[second].task.id.clone();
    let third = create_and_select_task(&mut app, test_task_draft("third")).await;
    let third_id = app.store.tasks[third].task.id.clone();
    app.list.mark(first_id.clone());
    app.list.mark(second_id.clone());

    app.begin_edit_due();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::TextInput(state))
            if state.title == "Edit due date · 2 marked tasks"
                && state.prompt.contains("Current: varies")
                && state.input.as_str().is_empty()
    ));
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert_eq!(
        app.store
            .tasks
            .iter()
            .find(|item| item.task.id == first_id)
            .unwrap()
            .task
            .due_on
            .as_deref(),
        Some("2099-01-01")
    );

    app.begin_edit_due();
    type_chars(&mut app, "2099-02-02").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    for task_id in [&first_id, &second_id] {
        assert_eq!(
            app.store
                .tasks
                .iter()
                .find(|item| &item.task.id == task_id)
                .unwrap()
                .task
                .due_on
                .as_deref(),
            Some("2099-02-02")
        );
    }
    assert!(
        app.store
            .tasks
            .iter()
            .find(|item| item.task.id == third_id)
            .unwrap()
            .task
            .due_on
            .is_none()
    );
    assert_eq!(app.list.marked_task_ids().len(), 2);

    app.undo_last().await.unwrap();
    assert_eq!(
        app.store
            .tasks
            .iter()
            .find(|item| item.task.id == first_id)
            .unwrap()
            .task
            .due_on
            .as_deref(),
        Some("2099-01-01")
    );
    assert!(
        app.store
            .tasks
            .iter()
            .find(|item| item.task.id == second_id)
            .unwrap()
            .task
            .due_on
            .is_none()
    );
}

#[tokio::test]
async fn clearing_marked_due_dates_requires_confirmation() {
    let mut app = test_app().await;
    let first = create_and_select_task(
        &mut app,
        TaskDraft {
            metadata: Vec::new(),
            due_on: Some("2099-01-01".to_string()),
            ..test_task_draft("first clear")
        },
    )
    .await;
    let first_id = app.store.tasks[first].task.id.clone();
    let second = create_and_select_task(
        &mut app,
        TaskDraft {
            metadata: Vec::new(),
            due_on: Some("2099-02-02".to_string()),
            ..test_task_draft("second clear")
        },
    )
    .await;
    let second_id = app.store.tasks[second].task.id.clone();
    app.list.mark(first_id.clone());
    app.list.mark(second_id.clone());

    app.begin_edit_due();
    app.handle_overlay_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL))
        .await
        .unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Confirm(state))
            if matches!(state.intent, ConfirmIntent::ClearDue { .. })
                && state.prompt == "Clear due date on 2 marked tasks?"
    ));
    app.handle_overlay_key(key(KeyCode::Char('y')))
        .await
        .unwrap();

    for task_id in [first_id, second_id] {
        assert!(
            app.store
                .tasks
                .iter()
                .find(|item| item.task.id == task_id)
                .unwrap()
                .task
                .due_on
                .is_none()
        );
    }
}

#[tokio::test]
async fn begin_delete_task_confirms_marked_tasks_when_tasks_are_marked() {
    let mut app = test_app().await;
    let first = create_and_select_task(&mut app, test_task_draft("first")).await;
    let first_id = app.store.tasks[first].task.id.clone();
    let second = create_and_select_task(&mut app, test_task_draft("second")).await;
    let second_id = app.store.tasks[second].task.id.clone();
    app.list.mark(first_id);

    app.begin_delete_task();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Confirm(ConfirmState { prompt, .. }))
            if prompt == "Delete 1 marked task?"
    ));

    app.overlay = None;
    app.list.mark(second_id);

    app.begin_delete_task();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Confirm(ConfirmState { prompt, .. }))
            if prompt == "Delete 2 marked tasks?"
    ));
}

#[tokio::test]
async fn update_deleted_updates_only_marked_tasks() {
    let mut app = test_app().await;
    let first = create_and_select_task(&mut app, test_task_draft("first")).await;
    let first_id = app.store.tasks[first].task.id.clone();
    let second = create_and_select_task(&mut app, test_task_draft("second")).await;
    let second_id = app.store.tasks[second].task.id.clone();
    let third = create_and_select_task(&mut app, test_task_draft("third")).await;
    let third_id = app.store.tasks[third].task.id.clone();
    app.list.mark(first_id.clone());
    app.list.mark(second_id.clone());

    app.update_deleted(true).await.unwrap();

    assert!(!app.store.tasks.iter().any(|item| item.task.id == first_id));
    assert!(!app.store.tasks.iter().any(|item| item.task.id == second_id));
    assert!(app.store.tasks.iter().any(|item| item.task.id == third_id));
    assert!(app.list.marked_task_ids().is_empty());
}

#[tokio::test]
async fn edit_labels_shortcut_uses_one_mark_as_single_task_scope() {
    let mut app = test_app().await;
    let index = create_and_select_task(&mut app, test_task_draft("marked")).await;
    let id = app.store.tasks[index].task.id.clone();
    app.list.mark(id);

    app.handle_normal_key(KeyCode::Char('e')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('l')).await.unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::TagCombobox(state))
            if matches!(state.intent, TagComboboxIntent::EditLabels { .. })
                && state.title == EDIT_LABELS_TITLE
    ));
}

#[tokio::test]
async fn edit_labels_shortcut_uses_selected_task_without_marks() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("selected")).await;

    app.handle_normal_key(KeyCode::Char('e')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('l')).await.unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::TagCombobox(state))
            if matches!(state.intent, TagComboboxIntent::EditLabels { .. })
    ));
}

#[tokio::test]
async fn submit_edit_labels_multi_updates_only_marked_tasks() {
    let mut app = test_app().await;
    app.store.create_label("batch".to_string()).await.unwrap();
    let first = create_and_select_task(&mut app, test_task_draft("first")).await;
    let first_id = app.store.tasks[first].task.id.clone();
    let second = create_and_select_task(&mut app, test_task_draft("second")).await;
    let second_id = app.store.tasks[second].task.id.clone();
    let third = create_and_select_task(&mut app, test_task_draft("third")).await;
    let third_id = app.store.tasks[third].task.id.clone();
    app.list.mark(first_id.clone());
    app.list.mark(second_id.clone());
    app.begin_edit_labels();

    let selection = match &app.overlay {
        Some(OverlayState::TagCombobox(state)) => match &state.intent {
            TagComboboxIntent::EditLabelsMulti { selection } => selection.clone(),
            intent => panic!("expected multi-label edit intent, got {intent:?}"),
        },
        overlay => panic!("expected label editor, got {overlay:?}"),
    };
    app.submit_edit_labels_multi(selection, vec!["batch".to_string()], Vec::new())
        .await
        .unwrap();

    let labels_for = |app: &App, task_id: &str| {
        app.store
            .tasks
            .iter()
            .find(|item| item.task.id.as_str() == task_id)
            .unwrap()
            .labels
            .clone()
    };
    assert_eq!(labels_for(&app, &first_id), vec!["batch".to_string()]);
    assert_eq!(labels_for(&app, &second_id), vec!["batch".to_string()]);
    assert!(labels_for(&app, &third_id).is_empty());
}
