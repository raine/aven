use super::*;

#[tokio::test]
async fn add_project_submit_uses_intent_independent_of_title() {
    let mut app = test_app().await;
    app.overlay = Some(OverlayState::TextInput(TextInputState::new(
        TextIntent::AddProject,
        "Renamed copy",
        "project name:",
        "Mobile App".to_string(),
    )));

    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert_eq!(
        toast_message(&app).as_deref(),
        Some("created project mobile-app · u undo")
    );
}

#[tokio::test]
async fn add_task_project_shortcut_uses_intent_independent_of_title_prefix() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
    let Some(OverlayState::AddTask(state)) = &mut app.overlay else {
        panic!("expected add task overlay");
    };
    state.project = "Create item".to_string();

    app.handle_overlay_key(ctrl_p()).await.unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::AddTask(state))
            if matches!(
                &state.mode,
                crate::tui::overlay::AddTaskMode::Picker { state: picker, .. }
                    if picker.intent == PickerIntent::AddTaskProject
            )
    ));
}

#[tokio::test]
async fn add_task_priority_shortcut_uses_intent_independent_of_title_prefix() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
    let Some(OverlayState::AddTask(state)) = &mut app.overlay else {
        panic!("expected add task overlay");
    };
    state.project = "Create item".to_string();

    app.handle_overlay_key(ctrl_r()).await.unwrap();

    assert_pending(&app, &["r"]);
    assert!(matches!(
        app.view().overlay,
        Some(OverlayView::AddTask(state)) if state.priority_prefix_active
    ));
}

#[tokio::test]
async fn delete_project_picker_and_name_confirm_use_distinct_intents() {
    let mut app = test_app().await;
    app.store
        .create_project("Mobile App".to_string())
        .await
        .unwrap();

    app.execute(Action::BeginDeleteProject).await.unwrap();
    assert!(matches!(
        app.overlay,
        Some(OverlayState::Picker(PickerState {
            intent: PickerIntent::DeleteProject,
            ref title,
            ..
        })) if title == DELETE_PROJECT_TITLE
    ));

    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(matches!(
        app.overlay,
        Some(OverlayState::TextInput(TextInputState {
            intent: TextIntent::ConfirmDeleteProject { .. },
            ref title,
            ..
        })) if title == DELETE_PROJECT_TITLE
    ));
}

#[tokio::test]
async fn delete_project_name_mismatch_keeps_confirmation_open() {
    let mut app = test_app().await;
    app.store
        .create_project("Mobile App".to_string())
        .await
        .unwrap();

    app.execute(Action::BeginDeleteProject).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    app.submit_delete_project_name("mobile-app".to_string(), "mobile".to_string())
        .await
        .unwrap();

    assert_eq!(
        toast_message(&app).as_deref(),
        Some("project name does not match")
    );
    assert!(matches!(
        app.overlay,
        Some(OverlayState::TextInput(TextInputState {
            intent: TextIntent::ConfirmDeleteProject { .. },
            ref title,
            ..
        })) if title == DELETE_PROJECT_TITLE
    ));
    assert!(
        app.store
            .projects
            .iter()
            .any(|project| project.key == "mobile-app")
    );
}

#[tokio::test]
async fn search_selected_blocker_adds_dependency() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Blocker needle")).await;
    let blocked_index = create_and_select_task(&mut app, test_task_draft("Blocked")).await;
    let blocked_id = app.store.tasks[blocked_index].task.id.clone();

    app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('B')).await.unwrap();
    type_chars(&mut app, "needle").await;
    settle_search_preview(&mut app).await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    let blocked_index = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == blocked_id)
        .unwrap();
    assert_eq!(app.store.tasks[blocked_index].depends_on.len(), 1);
    let toast = toast_message(&app);
    assert!(
        toast.is_some() && toast.as_deref().unwrap().contains("added dependency"),
        "expected success message, got: {:?}",
        toast,
    );
}

#[tokio::test]
async fn add_dependency_search_tab_keeps_picker_context() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Blocked")).await;

    app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('B')).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Search(state))
            if matches!(state.intent, SearchIntent::AddDependency { .. })
    ));
}

#[tokio::test]
async fn remove_shortcut_opens_current_dependency_picker() {
    let (_dir, _pool, mut app) = test_app_with_pool().await;
    let (blocker_id, blocked_id) = create_blocked_pair(&mut app).await;
    let blocked_index = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == blocked_id)
        .unwrap();
    app.list.select_task(Some(blocked_index));

    app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('U')).await.unwrap();

    let Some(OverlayState::Picker(state)) = &app.overlay else {
        panic!("expected dependency picker");
    };
    assert!(matches!(
        &state.intent,
        PickerIntent::RemoveDependency { selection }
            if selection.single_id() == Some(&blocked_id)
    ));
    assert_eq!(state.items.len(), 1);
    assert_eq!(state.items[0].value, blocker_id.to_string());
}

#[tokio::test]
async fn submitting_dependency_removal_removes_dependency() {
    let (_dir, _pool, mut app) = test_app_with_pool().await;
    let (_blocker_id, blocked_id) = create_blocked_pair(&mut app).await;
    let blocked_index = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == blocked_id)
        .unwrap();
    app.list.select_task(Some(blocked_index));

    app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('U')).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    let blocked_index = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == blocked_id)
        .unwrap();
    assert!(app.store.tasks[blocked_index].depends_on.is_empty());
    assert!(toast_message(&app).is_some_and(|message| { message.contains("removed dependency") }));
}

#[tokio::test]
async fn related_removal_picker_requires_confirmation_and_supports_undo() {
    let mut app = test_app().await;
    let related_index = create_and_select_task(&mut app, test_task_draft("Related task")).await;
    let related_id = app.store.tasks[related_index].task.id.clone();
    let subject_index = create_and_select_task(&mut app, test_task_draft("Subject task")).await;
    let subject_id = app.store.tasks[subject_index].task.id.clone();
    let added = app
        .store
        .add_related(Some(subject_index), &related_id)
        .await
        .unwrap()
        .unwrap();
    app.list.select_task(added.selected);

    for code in [KeyCode::Char('t'), KeyCode::Char('k'), KeyCode::Char('r')] {
        app.handle_normal_key(code).await.unwrap();
    }
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Confirm(ConfirmState {
            intent: ConfirmIntent::UnlinkRelated {
                selection,
                related_task_id,
            },
            title,
            prompt,
        })) if selection.single_id() == Some(&subject_id)
            && related_task_id == &related_id
            && title == "Unlink relationship"
            && prompt.contains("Related task")
    ));
    assert_eq!(
        app.store
            .load_task_item(&subject_id)
            .await
            .unwrap()
            .unwrap()
            .related
            .len(),
        1
    );

    app.handle_overlay_key(key(KeyCode::Char('n')))
        .await
        .unwrap();
    assert!(app.overlay.is_none());
    assert_eq!(
        app.store
            .load_task_item(&subject_id)
            .await
            .unwrap()
            .unwrap()
            .related
            .len(),
        1
    );

    for code in [KeyCode::Char('t'), KeyCode::Char('k'), KeyCode::Char('r')] {
        app.handle_normal_key(code).await.unwrap();
    }
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Char('y')))
        .await
        .unwrap();

    assert!(
        app.store
            .load_task_item(&subject_id)
            .await
            .unwrap()
            .unwrap()
            .related
            .is_empty()
    );
    assert!(toast_message(&app).is_some_and(|message| message.contains("removed related link")));

    app.handle_normal_key(KeyCode::Char('u')).await.unwrap();
    assert_eq!(
        app.store
            .load_task_item(&subject_id)
            .await
            .unwrap()
            .unwrap()
            .related
            .len(),
        1
    );
}

#[tokio::test]
async fn no_selected_task_shows_info() {
    let mut app = test_app().await;
    app.list.select_task(None);

    app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('B')).await.unwrap();

    assert!(app.overlay.is_none());
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("no selected task to edit")
    );
}

#[tokio::test]
async fn column_view_installs_custom_configuration() {
    let mut app = test_app().await;
    let mut config = crate::config::AppConfig::default();
    config.tui.columns.reverse();

    app.set_config(config);

    assert_eq!(app.store.task_columns[0].name, "Done");
}

#[tokio::test]
async fn layout_shortcut_toggles_columns_and_list() {
    let mut app = test_app().await;

    app.handle_normal_key(KeyCode::Char('v')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('w')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('v')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('l')).await.unwrap();

    assert_eq!(app.store.view_state.query, TaskQuery::All);
    assert_eq!(app.store.view_state.layout, TaskLayout::Columns);

    app.handle_normal_key(KeyCode::Char('v')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('l')).await.unwrap();

    assert_eq!(app.store.view_state.layout, TaskLayout::List);
}

#[tokio::test]
async fn layout_switch_preserves_query_rows_selection_marks_and_history() {
    let mut app = test_app().await;
    let index = create_and_select_task(&mut app, test_task_draft("layout task")).await;
    let task_id = app.store.tasks[index].task.id.clone();
    app.list.select_task(Some(index));
    app.list.mark(task_id.clone());
    let task_ids = app
        .store
        .tasks
        .iter()
        .map(|item| item.task.id.clone())
        .collect::<Vec<_>>();

    app.set_layout(TaskLayout::Columns);

    assert_eq!(app.store.view_state.query, TaskQuery::Queue);
    assert_eq!(app.store.view_state.layout, TaskLayout::List);
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("active query does not support columns layout")
    );

    app.store.show_view(TaskQuery::All).await.unwrap();
    app.list.select_task(
        app.store
            .tasks
            .iter()
            .position(|item| item.task.id == task_id),
    );
    let all_ids = app
        .store
        .tasks
        .iter()
        .map(|item| item.task.id.clone())
        .collect::<Vec<_>>();
    app.set_layout(TaskLayout::Columns);
    app.set_layout(TaskLayout::List);

    assert_eq!(app.store.view_state.query, TaskQuery::All);
    assert_eq!(app.store.view_state.layout, TaskLayout::List);
    assert_eq!(
        all_ids,
        app.store
            .tasks
            .iter()
            .map(|item| item.task.id.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(task_ids, all_ids);
    assert_eq!(
        app.store.tasks[app.list.selected_task().unwrap()].task.id,
        task_id
    );
    assert!(app.list.marked_task_ids().contains(&task_id));
    assert!(app.list.navigation_is_empty());
}

#[tokio::test]
async fn incompatible_query_coerces_layout_and_back_restores_both() {
    let mut app = test_app().await;
    app.store.show_view(TaskQuery::All).await.unwrap();
    app.set_layout(TaskLayout::Columns);

    app.show_view(TaskQuery::Queue).await.unwrap();
    assert_eq!(app.store.view_state.query, TaskQuery::Queue);
    assert_eq!(app.store.view_state.layout, TaskLayout::List);

    app.go_back().await.unwrap();
    assert_eq!(app.store.view_state.query, TaskQuery::All);
    assert_eq!(app.store.view_state.layout, TaskLayout::Columns);
}

#[tokio::test]
async fn status_change_removes_card_outside_scoped_columns_query() {
    let mut app = test_app().await;
    let first = create_and_select_task(&mut app, test_task_draft("First todo")).await;
    let first_id = app.store.tasks[first].task.id.clone();
    app.update_status(TaskStatus::Todo).await.unwrap();
    let second = create_and_select_task(&mut app, test_task_draft("Second todo")).await;
    let second_id = app.store.tasks[second].task.id.clone();
    app.update_status(TaskStatus::Todo).await.unwrap();
    app.store.show_view(TaskQuery::Todo).await.unwrap();
    app.store
        .view_state
        .set_layout(TaskLayout::Columns)
        .unwrap();
    app.list.select_task(
        app.store
            .tasks
            .iter()
            .position(|item| item.task.id == first_id),
    );

    app.update_status(TaskStatus::Active).await.unwrap();

    assert!(app.store.tasks.iter().all(|item| item.task.id != first_id));
    assert_eq!(app.store.tasks.len(), 1);
    assert_eq!(
        app.store.tasks[app.list.selected_task().unwrap()].task.id,
        second_id
    );
}

#[tokio::test]
async fn column_view_preview_shortcut_toggles_session_visibility() {
    let mut app = test_app().await;
    app.store.show_view(TaskQuery::All).await.unwrap();
    app.store
        .view_state
        .set_layout(TaskLayout::Columns)
        .unwrap();
    assert!(app.store.columns_preview_visible);

    app.handle_normal_key(KeyCode::Char('g')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('d')).await.unwrap();
    assert!(!app.store.columns_preview_visible);

    app.handle_normal_key(KeyCode::Char('g')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('d')).await.unwrap();
    assert!(app.store.columns_preview_visible);
}

#[tokio::test]
async fn column_move_shortcut_updates_status_and_undo_restores_it() {
    let mut app = test_app().await;
    let index = create_and_select_task(&mut app, test_task_draft("move me")).await;
    let task_id = app.store.tasks[index].task.id.clone();
    app.store.show_view(TaskQuery::All).await.unwrap();
    app.store
        .view_state
        .set_layout(TaskLayout::Columns)
        .unwrap();
    app.list.select_task(
        app.store
            .tasks
            .iter()
            .position(|item| item.task.id == task_id),
    );

    app.handle_normal_key(KeyCode::Char('>')).await.unwrap();

    let moved = app
        .store
        .tasks
        .iter()
        .find(|item| item.task.id == task_id)
        .unwrap();
    assert_eq!(moved.task.status, TaskStatus::Backlog);
    assert_eq!(
        app.store
            .selected_task(app.list.selected_task())
            .unwrap()
            .task
            .id,
        task_id
    );

    app.handle_normal_key(KeyCode::Char('u')).await.unwrap();
    assert_eq!(
        app.store
            .tasks
            .iter()
            .find(|item| item.task.id == task_id)
            .unwrap()
            .task
            .status,
        TaskStatus::Inbox
    );
}

#[tokio::test]
async fn column_move_shortcut_advances_marked_tasks_from_their_own_lanes() {
    let mut app = test_app().await;
    let inbox = create_and_select_task(&mut app, test_task_draft("inbox task")).await;
    let inbox_id = app.store.tasks[inbox].task.id.clone();
    let mut todo_draft = test_task_draft("ready task");
    todo_draft.status = "todo".to_string();
    let todo = create_and_select_task(&mut app, todo_draft).await;
    let todo_id = app.store.tasks[todo].task.id.clone();
    app.list.mark(inbox_id.clone());
    app.list.mark(todo_id.clone());
    app.store.show_view(TaskQuery::All).await.unwrap();
    app.store
        .view_state
        .set_layout(TaskLayout::Columns)
        .unwrap();

    app.handle_normal_key(KeyCode::Char('>')).await.unwrap();

    let status_for = |task_id: &str| {
        app.store
            .tasks
            .iter()
            .find(|item| item.task.id.as_str() == task_id)
            .unwrap()
            .task
            .status
    };
    assert_eq!(status_for(&inbox_id), TaskStatus::Backlog);
    assert_eq!(status_for(&todo_id), TaskStatus::Active);

    app.handle_normal_key(KeyCode::Char('u')).await.unwrap();
    let status_for = |task_id: &str| {
        app.store
            .tasks
            .iter()
            .find(|item| item.task.id.as_str() == task_id)
            .unwrap()
            .task
            .status
    };
    assert_eq!(status_for(&inbox_id), TaskStatus::Inbox);
    assert_eq!(status_for(&todo_id), TaskStatus::Todo);
}

#[tokio::test]
async fn column_move_picker_uses_lane_names_and_first_statuses() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("move with picker")).await;
    app.store.show_view(TaskQuery::All).await.unwrap();
    app.store
        .view_state
        .set_layout(TaskLayout::Columns)
        .unwrap();

    app.handle_normal_key(KeyCode::Char('m')).await.unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Picker(PickerState { intent: PickerIntent::MoveToColumn { .. }, title, items, .. }))
            if title == "Move to column"
                && items.iter().any(|item| item.label == "Done" && item.value == "done")
                && items.iter().all(|item| !item.label.contains('→'))
    ));
}

#[tokio::test]
async fn column_relative_move_keeps_marked_batch_unchanged_at_edge() {
    let mut app = test_app().await;
    let inbox = create_and_select_task(&mut app, test_task_draft("inbox edge")).await;
    let inbox_id = app.store.tasks[inbox].task.id.clone();
    let mut backlog_draft = test_task_draft("backlog beside edge");
    backlog_draft.status = "backlog".to_string();
    let backlog = create_and_select_task(&mut app, backlog_draft).await;
    let backlog_id = app.store.tasks[backlog].task.id.clone();
    app.list.mark(inbox_id.clone());
    app.list.mark(backlog_id.clone());
    app.store.show_view(TaskQuery::All).await.unwrap();
    app.store
        .view_state
        .set_layout(TaskLayout::Columns)
        .unwrap();

    app.handle_normal_key(KeyCode::Char('<')).await.unwrap();

    let status_for = |task_id: &str| {
        app.store
            .tasks
            .iter()
            .find(|item| item.task.id.as_str() == task_id)
            .unwrap()
            .task
            .status
    };
    assert_eq!(status_for(&inbox_id), TaskStatus::Inbox);
    assert_eq!(status_for(&backlog_id), TaskStatus::Backlog);
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("already at first column")
    );
}

#[tokio::test]
async fn choosing_current_column_preserves_grouped_status() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("stay canceled")).await;
    app.store.show_view(TaskQuery::All).await.unwrap();
    app.store
        .view_state
        .set_layout(TaskLayout::Columns)
        .unwrap();
    app.update_status(TaskStatus::Canceled).await.unwrap();

    let selection = app.resolve_task_selection().unwrap();
    app.move_tasks_to_column(selection, "done".to_string())
        .await
        .unwrap();

    assert_eq!(
        app.store
            .selected_task(app.list.selected_task())
            .unwrap()
            .task
            .status,
        TaskStatus::Canceled
    );
}

#[tokio::test]
async fn column_lane_header_click_moves_selected_task() {
    let mut app = test_app().await;
    let index = create_and_select_task(&mut app, test_task_draft("mouse move")).await;
    let task_id = app.store.tasks[index].task.id.clone();
    app.store.show_view(TaskQuery::All).await.unwrap();
    app.store
        .view_state
        .set_layout(TaskLayout::Columns)
        .unwrap();
    app.list.select_task(
        app.store
            .tasks
            .iter()
            .position(|item| item.task.id == task_id),
    );

    app.dispatch_mouse(left_click(16, 2), (80, 24).into())
        .await
        .unwrap();

    assert_eq!(
        app.store
            .tasks
            .iter()
            .find(|item| item.task.id == task_id)
            .unwrap()
            .task
            .status,
        TaskStatus::Backlog
    );
}

#[tokio::test]
async fn column_card_drag_moves_task_and_undo_restores_status() {
    let mut app = test_app().await;
    let index = create_and_select_task(&mut app, test_task_draft("drag me")).await;
    let task_id = app.store.tasks[index].task.id.clone();
    app.store.show_view(TaskQuery::All).await.unwrap();
    app.store
        .view_state
        .set_layout(TaskLayout::Columns)
        .unwrap();

    app.dispatch_mouse(left_click(1, 4), (80, 24).into())
        .await
        .unwrap();
    app.dispatch_mouse(left_drag(16, 10), (80, 24).into())
        .await
        .unwrap();
    assert_eq!(app.list.column_drag().unwrap().hovered_lane, Some(1));
    app.dispatch_mouse(left_release(16, 10), (80, 24).into())
        .await
        .unwrap();

    assert_eq!(
        app.store
            .tasks
            .iter()
            .find(|item| item.task.id == task_id)
            .unwrap()
            .task
            .status,
        TaskStatus::Backlog
    );
    assert!(app.list.column_drag().is_none());

    app.handle_normal_key(KeyCode::Char('u')).await.unwrap();
    assert_eq!(
        app.store
            .tasks
            .iter()
            .find(|item| item.task.id == task_id)
            .unwrap()
            .task
            .status,
        TaskStatus::Inbox
    );
}

#[tokio::test]
async fn column_card_drag_to_its_current_lane_is_a_no_op() {
    let mut app = test_app().await;
    let index = create_and_select_task(&mut app, test_task_draft("stay here")).await;
    let task_id = app.store.tasks[index].task.id.clone();
    app.store.show_view(TaskQuery::All).await.unwrap();
    app.store
        .view_state
        .set_layout(TaskLayout::Columns)
        .unwrap();

    app.dispatch_mouse(left_click(1, 4), (80, 24).into())
        .await
        .unwrap();
    app.dispatch_mouse(left_drag(2, 10), (80, 24).into())
        .await
        .unwrap();
    app.dispatch_mouse(left_release(2, 10), (80, 24).into())
        .await
        .unwrap();

    assert_eq!(
        app.store
            .tasks
            .iter()
            .find(|item| item.task.id == task_id)
            .unwrap()
            .task
            .status,
        TaskStatus::Inbox
    );
    assert!(app.list.column_drag().is_none());
}

#[tokio::test]
async fn column_card_right_click_opens_status_choices() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("mouse status")).await;
    app.store.show_view(TaskQuery::All).await.unwrap();
    app.store
        .view_state
        .set_layout(TaskLayout::Columns)
        .unwrap();

    app.dispatch_mouse(right_click(1, 4), (80, 24).into())
        .await
        .unwrap();

    assert_eq!(
        app.footer_choice.as_ref().map(|choice| choice.mode),
        Some(FooterChoiceMode::Status)
    );
}

#[tokio::test]
async fn column_view_navigates_within_and_between_lanes() {
    let mut app = test_app().await;
    for (title, status) in [
        ("active one", "active"),
        ("active two", "active"),
        ("todo", "todo"),
    ] {
        let mut draft = test_task_draft(title);
        draft.status = status.to_string();
        app.store.create_task(draft, None).await.unwrap();
    }
    app.store.show_view(TaskQuery::All).await.unwrap();
    app.store
        .view_state
        .set_layout(TaskLayout::Columns)
        .unwrap();
    let active = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.title == "active one")
        .unwrap();
    let todo = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.title == "todo")
        .unwrap();
    app.list.select_task(Some(active));

    app.move_selection(1).await.unwrap();
    assert_eq!(
        app.store
            .selected_task(app.list.selected_task())
            .unwrap()
            .task
            .status,
        TaskStatus::Active
    );
    app.move_left();
    assert_eq!(app.list.selected_task(), Some(todo));
    app.move_right().await.unwrap();
    assert_eq!(
        app.store
            .selected_task(app.list.selected_task())
            .unwrap()
            .task
            .status,
        TaskStatus::Active
    );
}

#[tokio::test]
async fn column_view_keeps_task_selected_when_status_becomes_done() {
    let mut app = test_app().await;
    let mut draft = test_task_draft("finish me");
    draft.status = "active".to_string();
    create_and_select_task(&mut app, draft).await;
    app.show_view(TaskQuery::All).await.unwrap();
    app.set_layout(TaskLayout::Columns);
    let selected_id = app
        .store
        .selected_task(app.list.selected_task())
        .unwrap()
        .task
        .id
        .clone();

    app.update_status(TaskStatus::Done).await.unwrap();

    let selected = app.store.selected_task(app.list.selected_task()).unwrap();
    assert_eq!(selected.task.id, selected_id);
    assert_eq!(selected.task.status, TaskStatus::Done);
}
