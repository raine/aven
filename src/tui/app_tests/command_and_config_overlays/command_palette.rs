use super::*;

type SidebarTargetPredicate = fn(&SidebarEntryTarget) -> bool;

fn command_candidate_names(app: &App) -> Vec<&str> {
    let Some(OverlayState::Command { state }) = &app.overlay else {
        panic!("command overlay");
    };
    state
        .candidates
        .iter()
        .filter_map(|candidate| state.catalog.command(candidate.index))
        .map(crate::tui::event::CatalogCommand::name)
        .collect()
}

#[tokio::test]
async fn colon_dispatch_opens_contextual_panel_for_selected_task() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Key dispatch target")).await;

    app.dispatch_key(shift_key(KeyCode::Char(':')), (100, 30).into())
        .await
        .unwrap();

    assert_eq!(command_candidate_names(&app)[0], "status-picker");
}

#[tokio::test]
async fn command_palette_opens_blocker_from_list_and_parent_detail() {
    let mut app = test_app().await;
    let (blocker_id, blocked_id) = create_blocked_pair(&mut app).await;
    app.store.refresh(Some(&blocked_id)).await.unwrap();
    let blocked_index = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == blocked_id)
        .unwrap();
    app.list.select_task(Some(blocked_index));

    app.begin_command().await;
    type_chars(&mut app, ":go-to-blocker").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert_eq!(app.store.tasks[0].task.id, blocker_id);
    assert!(app.detail.is_active());

    for code in [KeyCode::Char('g'), KeyCode::Char('[')] {
        app.dispatch_key(key(code), (80, 24).into()).await.unwrap();
    }
    assert_eq!(
        app.store
            .selected_task(app.list.selected_task())
            .map(|item| item.task.id.clone()),
        Some(blocked_id.clone())
    );

    app.begin_command().await;
    type_chars(&mut app, ":go-to-blocker").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert_eq!(app.store.tasks[0].task.id, blocker_id);
    assert!(app.detail.is_active());
}

#[tokio::test]
async fn colon_dispatch_opens_contextual_panel_for_marked_tasks() {
    let mut app = test_app().await;
    let first = create_and_select_task(&mut app, test_task_draft("First marked target")).await;
    let first_id = app.store.tasks[first].task.id.clone();
    let second = create_and_select_task(&mut app, test_task_draft("Second marked target")).await;
    let second_id = app.store.tasks[second].task.id.clone();
    app.list.mark(first_id);
    app.list.mark(second_id);

    app.dispatch_key(shift_key(KeyCode::Char(':')), (100, 30).into())
        .await
        .unwrap();

    assert_eq!(command_candidate_names(&app)[0], "status-picker");
}

#[tokio::test]
async fn colon_dispatch_opens_contextual_panel_for_parent_detail() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Detail key target")).await;
    app.show_detail(0);

    app.dispatch_key(shift_key(KeyCode::Char(':')), (100, 30).into())
        .await
        .unwrap();

    assert_eq!(command_candidate_names(&app)[0], "edit-title");
}

#[tokio::test]
async fn colon_dispatch_opens_contextual_panels_for_sidebar_targets() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Sidebar key target")).await;
    app.list.focus_sidebar();

    let cases: [(SidebarTargetPredicate, &str); 4] = [
        (
            |target| matches!(target, SidebarEntryTarget::View(TaskQuery::Queue)),
            "view-queue",
        ),
        (
            |target| {
                matches!(
                    target,
                    SidebarEntryTarget::Scope(TaskScopeTarget::Project(_))
                )
            },
            "scope-project",
        ),
        (
            |target| matches!(target, SidebarEntryTarget::Workspace(_)),
            "scope-all",
        ),
        (
            |target| matches!(target, SidebarEntryTarget::View(TaskQuery::Recurring)),
            "view-recurring",
        ),
    ];
    for (matches_target, expected_first) in cases {
        let index = app
            .store
            .sidebar_entries
            .iter()
            .position(|entry| entry.target.as_ref().is_some_and(matches_target))
            .expect("sidebar target");
        app.list.select_sidebar(Some(index));
        app.dispatch_key(shift_key(KeyCode::Char(':')), (100, 30).into())
            .await
            .unwrap();
        assert_eq!(command_candidate_names(&app)[0], expected_first);
        app.dispatch_key(key(KeyCode::Esc), (100, 30).into())
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn exact_status_alias_opens_status_picker() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Exact status target")).await;
    app.dispatch_key(shift_key(KeyCode::Char(':')), (100, 30).into())
        .await
        .unwrap();
    type_chars(&mut app, "status").await;
    app.dispatch_key(key(KeyCode::Enter), (100, 30).into())
        .await
        .unwrap();

    assert!(matches!(
        app.footer_choice,
        Some(crate::tui::app::FooterChoiceState {
            mode: crate::tui::app::FooterChoiceMode::Status,
            ..
        })
    ));
}

#[tokio::test]
async fn unavailable_typed_commands_keep_contextual_reasons_through_key_dispatch() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Unavailable command target")).await;
    app.dispatch_key(shift_key(KeyCode::Char(':')), (100, 30).into())
        .await
        .unwrap();
    type_chars(&mut app, "attachment-open").await;
    app.dispatch_key(key(KeyCode::Enter), (100, 30).into())
        .await
        .unwrap();
    assert_eq!(
        toast_message(&app).as_deref(),
        Some(":attachment-open is disabled: requires a focused attachment")
    );

    app.dispatch_key(key(KeyCode::Esc), (100, 30).into())
        .await
        .unwrap();
    app.show_detail(0);
    app.dispatch_key(shift_key(KeyCode::Char(':')), (100, 30).into())
        .await
        .unwrap();
    type_chars(&mut app, "filter-label").await;
    app.dispatch_key(key(KeyCode::Enter), (100, 30).into())
        .await
        .unwrap();
    assert_eq!(
        toast_message(&app).as_deref(),
        Some(":filter-label is disabled: available only in the task list")
    );
}

#[tokio::test]
async fn detail_exit_command_runs_through_key_dispatch() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Detail exit target")).await;
    app.show_detail(0);
    app.dispatch_key(shift_key(KeyCode::Char(':')), (100, 30).into())
        .await
        .unwrap();
    type_chars(&mut app, "view-queue").await;
    app.dispatch_key(key(KeyCode::Enter), (100, 30).into())
        .await
        .unwrap();

    assert!(app.detail.is_inactive());
    assert_eq!(app.store.view_state.query, TaskQuery::Queue);
}

#[tokio::test]
async fn layout_command_changes_presentation_without_changing_the_query() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Layout command target")).await;
    app.show_view(TaskQuery::All).await.unwrap();
    app.show_detail(0);

    app.dispatch_key(shift_key(KeyCode::Char(':')), (100, 30).into())
        .await
        .unwrap();
    type_chars(&mut app, "layout-columns").await;
    app.dispatch_key(key(KeyCode::Enter), (100, 30).into())
        .await
        .unwrap();

    assert!(app.detail.is_inactive());
    assert_eq!(app.store.view_state.query, TaskQuery::All);
    assert_eq!(app.store.view_state.layout, TaskLayout::Columns);
}

#[tokio::test]
async fn captured_sidebar_project_is_selected_for_rename_picker() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Project picker target")).await;
    app.list.focus_sidebar();
    let index = app
        .store
        .sidebar_entries
        .iter()
        .position(|entry| {
            matches!(
                entry.target,
                Some(SidebarEntryTarget::Scope(TaskScopeTarget::Project(_)))
            )
        })
        .expect("project sidebar entry");
    let project = match app.store.sidebar_entries[index].target.as_ref().unwrap() {
        SidebarEntryTarget::Scope(TaskScopeTarget::Project(project)) => project.clone(),
        _ => unreachable!("project target"),
    };
    app.list.select_sidebar(Some(index));
    app.dispatch_key(shift_key(KeyCode::Char(':')), (100, 30).into())
        .await
        .unwrap();
    type_chars(&mut app, "rename-project").await;
    app.dispatch_key(key(KeyCode::Enter), (100, 30).into())
        .await
        .unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Picker(PickerState { items, selected, .. }))
            if items[*selected].value == project
    ));
}

#[tokio::test]
async fn every_ready_first_page_candidate_resolves_a_captured_target() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Resolvable command target")).await;
    let snapshot = app.capture_command_session(None);
    let candidates = app.command_catalog.query(crate::tui::event::CommandQuery {
        input: "",
        snapshot: &snapshot,
        unavailable: &[],
    });

    for candidate in candidates.iter().take(8) {
        if candidate.availability.reason().is_some() {
            continue;
        }
        let command = app.command_catalog.command(candidate.index).unwrap();
        let built_in = command.built_in().expect("default catalog command");
        assert!(
            app.resolve_builtin_command(&snapshot, built_in)
                .await
                .unwrap()
                .is_ok(),
            "{} did not resolve",
            command.name()
        );
    }
}

#[tokio::test]
async fn command_overlay_executes_unique_lookup_and_keeps_overlay_on_errors() {
    let mut app = test_app().await;
    let selected = create_and_select_task(&mut app, test_task_draft("Before refresh")).await;
    let task_id = app.store.tasks[selected].task.id.clone();
    app.store
        .database()
        .update_task(
            &app.store.active_workspace,
            &task_id,
            crate::operations::TaskUpdate {
                title: Some("After refresh".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    app.begin_command().await;
    for ch in "ref".chars() {
        app.handle_overlay_key(key(KeyCode::Char(ch)))
            .await
            .unwrap();
    }
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(app.overlay.is_none());
    assert_eq!(app.store.tasks[selected].task.title, "After refresh");

    app.begin_command().await;
    app.handle_overlay_key(key(KeyCode::Char('s')))
        .await
        .unwrap();
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(matches!(app.overlay, Some(OverlayState::Command { .. })));
    assert_eq!(toast_message(&app).as_deref(), Some("ambiguous command: s"));

    app.begin_command().await;
    for ch in "zzzz".chars() {
        app.handle_overlay_key(key(KeyCode::Char(ch)))
            .await
            .unwrap();
    }
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(matches!(app.overlay, Some(OverlayState::Command { .. })));
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("unknown command: zzzz")
    );
}

#[tokio::test]
async fn command_overlay_tab_cycles_without_a_filter() {
    let mut app = test_app().await;

    app.begin_command().await;
    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Command { state })
            if state.input.text == "add-task"
                && state.highlighted_name() == Some("add-task")
    ));

    app.handle_overlay_key(key(KeyCode::BackTab)).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Command { state })
            if !state.input.text.is_empty()
                && state.highlighted_name() == Some(state.input.text.as_str())
    ));
}

#[tokio::test]
async fn command_overlay_arrows_browse_and_enter_runs_selection() {
    let mut app = test_app().await;

    app.begin_command().await;
    type_chars(&mut app, "wel").await;
    app.handle_overlay_key(key(KeyCode::Down)).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Command { state })
            if state.input.text == "wel" && state.highlighted_name() == Some("welcome")
    ));

    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(matches!(app.overlay, Some(OverlayState::Onboarding { .. })));
}

#[tokio::test]
async fn command_overlay_arrow_selection_wraps() {
    let mut app = test_app().await;

    app.begin_command().await;
    app.handle_overlay_key(key(KeyCode::Up)).await.unwrap();
    let last = match &app.overlay {
        Some(OverlayState::Command { state }) => {
            assert!(!state.candidates.is_empty());
            assert_eq!(state.highlighted, Some(state.candidates.len() - 1));
            state.highlighted
        }
        overlay => panic!("expected command overlay, got {overlay:?}"),
    };
    app.handle_overlay_key(key(KeyCode::Down)).await.unwrap();

    assert!(last.is_some());
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Command { state }) if state.highlighted_name() == Some("add-task")
    ));
}

#[tokio::test]
async fn command_palette_selects_upcoming_view() {
    let mut app = test_app().await;

    app.begin_command().await;
    type_chars(&mut app, "upcoming").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(app.overlay.is_none());
    assert_eq!(app.store.view_state.query, TaskQuery::Upcoming);
}

#[tokio::test]
async fn detail_command_overlay_can_complete_and_run_navigation_commands() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Detail command target")).await;
    app.show_detail(0);

    app.begin_command().await;
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Command { state })
            if state.session.routing_domain().command_context() == crate::tui::event::CommandContext::Detail
    ));
    type_chars(&mut app, "view-tod").await;
    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Command { state }) if state.input.text == "view-todo"
    ));
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(app.overlay.is_none());
    assert_eq!(app.store.view_state.query, TaskQuery::Todo);
    assert!(app.detail.is_inactive());
}

#[tokio::test]
async fn detail_command_overlay_routes_supported_focused_task_actions() {
    let mut app = test_app().await;
    let selected =
        create_and_select_task(&mut app, test_task_draft("Focused command target")).await;
    let selected_id = app.store.tasks[selected].task.id.clone();
    let linked = create_and_select_task(&mut app, test_task_draft("Linked task")).await;
    let linked = app.store.tasks[linked].clone();
    let selected = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.id == selected_id)
        .unwrap();
    app.store.tasks[selected].depends_on = vec![crate::query::TaskDependencyLink {
        project_key: "app".to_string(),
        task_id: linked.task.id.clone(),
        display_ref: linked.display_ref.clone(),
        title: linked.task.title.clone(),
        status: linked.task.status.as_str().to_string(),
        priority: linked.task.priority.as_str().to_string(),
        unresolved: true,
    }];
    app.list.select_task(Some(selected));
    app.show_detail(0);
    app.detail.state_mut().unwrap().focused_target = Some(DetailTargetId::Task {
        section: DetailSection::DependsOn,
        task_id: linked.task.id.clone(),
    });

    app.begin_command().await;
    type_chars(&mut app, "edit-title").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::TextInput(state))
            if matches!(
                &state.intent,
                TextIntent::EditTitle { selection }
                    if selection.single_id() == Some(&linked.task.id)
            )
    ));
    assert!(app.detail.is_active());

    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
    app.begin_command().await;
    type_chars(&mut app, "search").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(matches!(app.overlay, Some(OverlayState::Search(_))));
    assert!(app.detail.is_active());
}

#[tokio::test]
async fn command_panel_keeps_captured_task_across_refresh_and_cursor_change() {
    let mut app = test_app().await;
    let original =
        create_and_select_task(&mut app, test_task_draft("Captured command target")).await;
    let original_id = app.store.tasks[original].task.id.clone();

    app.begin_command().await;
    let command_overlay = app.overlay.take().expect("command overlay");
    create_and_select_task(&mut app, test_task_draft("Later cursor target")).await;
    app.overlay = Some(command_overlay);
    type_chars(&mut app, "edit-title").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::TextInput(state))
            if matches!(
                &state.intent,
                TextIntent::EditTitle { selection }
                    if selection.single_id() == Some(&original_id)
            )
    ));
}

#[tokio::test]
async fn captured_delete_prompt_and_intent_keep_the_same_task_after_cursor_change() {
    let mut app = test_app().await;
    let original =
        create_and_select_task(&mut app, test_task_draft("Captured delete target")).await;
    let original_id = app.store.tasks[original].task.id.clone();
    let original_ref = app.store.tasks[original].display_ref.clone();

    app.begin_command().await;
    let command_overlay = app.overlay.take().expect("command overlay");
    create_and_select_task(&mut app, test_task_draft("Live delete target")).await;
    app.overlay = Some(command_overlay);
    type_chars(&mut app, "delete").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Confirm(ConfirmState {
            intent: ConfirmIntent::DeleteTasks { selection },
            prompt,
            ..
        })) if selection.single_id() == Some(&original_id)
            && prompt.contains(&original_ref)
            && prompt.contains("Captured delete target")
            && !prompt.contains("Live delete target")
    ));
}

#[tokio::test]
async fn command_project_picker_keeps_captured_marks_after_cursor_change() {
    let mut app = test_app().await;
    let original =
        create_and_select_task(&mut app, test_task_draft("Captured picker target")).await;
    let original_id = app.store.tasks[original].task.id.clone();
    app.list.mark(original_id.clone());

    app.begin_command().await;
    let command_overlay = app.overlay.take().expect("command overlay");
    app.list.clear_marks();
    create_and_select_task(&mut app, test_task_draft("Live picker target")).await;
    app.overlay = Some(command_overlay);
    type_chars(&mut app, "edit-project").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Picker(PickerState {
            intent: PickerIntent::EditProject { selection, .. },
            ..
        })) if selection.single_id() == Some(&original_id) && selection.uses_marks()
    ));
}

#[tokio::test]
async fn command_panel_rejects_stale_captured_task_without_cursor_fallback() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Live cursor target")).await;
    app.begin_command().await;
    let stale_id = crate::test_support::task_id("stale-command-target");
    let Some(OverlayState::Command { state }) = app.overlay.as_mut() else {
        panic!("command overlay");
    };
    let crate::tui::event::CommandSessionSnapshot {
        surface:
            crate::tui::event::CommandSurfaceSnapshot::List {
                primary_task_id, ..
            },
        ..
    } = &mut state.session
    else {
        panic!("list command session");
    };
    *primary_task_id = Some(stale_id);

    type_chars(&mut app, "edit-title").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(!matches!(app.overlay, Some(OverlayState::TextInput(_))));
    assert_eq!(
        toast_message(&app).as_deref(),
        Some(":edit-title is disabled: a captured task is stale")
    );
}

#[tokio::test]
async fn command_overlay_tab_selects_first_partial_command_match() {
    let mut app = test_app().await;

    app.begin_command().await;
    type_chars(&mut app, ":delet").await;
    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Command { state })
            if state.input.text == "delete-label"
                && state.input.cursor == "delete-label".len()
                && state.highlighted_name() == Some("delete-label")
    ));
}

#[tokio::test]
async fn command_overlay_tab_cycles_from_exact_command_match() {
    let mut app = test_app().await;

    app.begin_command().await;
    type_chars(&mut app, ":delete").await;
    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Command { state })
            if state.input.text == "delete"
                && state.input.cursor == "delete".len()
                && state.highlighted_name() == Some("delete")
    ));

    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Command { state })
            if state.input.text == "delete-label"
                && state.highlighted_name() == Some("delete-label")
    ));

    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Command { state })
            if state.input.text == "delete-project"
                && state.highlighted_name() == Some("delete-project")
    ));

    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Command { state })
            if state.input.text == "attachment-delete"
                && state.highlighted_name() == Some("attachment-delete")
    ));

    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Command { state })
            if state.input.text == "filter-deleted"
                && state.highlighted_name() == Some("filter-deleted")
    ));
}

#[tokio::test]
async fn command_overlay_tab_cycles_ambiguous_matches() {
    let mut app = test_app().await;

    app.begin_command().await;
    type_chars(&mut app, "stat").await;
    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Command { state })
            if state.input.text == "status-picker"
                && state.input.cursor == "status-picker".len()
                && state.cycle_input.as_deref() == Some("stat")
                && state.highlighted_name() == Some("status-picker")
    ));

    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Command { state })
            if state.input.text == "status-inbox"
                && state.input.cursor == "status-inbox".len()
                && state.cycle_input.as_deref() == Some("stat")
                && state.highlighted_name() == Some("status-inbox")
    ));
}

#[tokio::test]
async fn command_overlay_tab_cycles_from_exact_alias_match() {
    let mut app = test_app().await;

    app.begin_command().await;
    type_chars(&mut app, ":todo").await;
    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Command { state })
            if state.input.text == "status-todo"
                && state.input.cursor == "status-todo".len()
                && state.highlighted_name() == Some("status-todo")
    ));

    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Command { state })
            if state.input.text == "view-todo"
                && state.highlighted_name() == Some("view-todo")
    ));
}

#[tokio::test]
async fn command_overlay_edit_resets_completion_cycle() {
    let mut app = test_app().await;

    app.begin_command().await;
    type_chars(&mut app, "stat").await;
    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Backspace))
        .await
        .unwrap();

    assert!(matches!(
        app.overlay,
        Some(OverlayState::Command { state })
            if state.cycle_input.is_none() && state.highlighted.is_none()
    ));
}

#[tokio::test]
async fn command_panel_clears_stale_captured_marks_without_hydration() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Stale mark target")).await;
    let captured_id = app.store.tasks[0].task.id.clone();
    app.list.mark(captured_id);
    app.begin_command().await;
    let stale_id = crate::test_support::task_id("stale-mark-command-target");
    let Some(OverlayState::Command { state }) = app.overlay.as_mut() else {
        panic!("command overlay");
    };
    let crate::tui::event::CommandSurfaceSnapshot::List {
        marked_task_ids, ..
    } = &mut state.session.surface
    else {
        panic!("list command session");
    };
    marked_task_ids.clear();
    marked_task_ids.push(stale_id.clone());
    app.list.mark(stale_id.clone());

    type_chars(&mut app, "clear-marks").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(!app.list.marked_task_ids().contains(&stale_id));
}

#[tokio::test]
async fn command_panel_rejects_missing_required_task_target() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Missing command target")).await;
    app.begin_command().await;
    let Some(OverlayState::Command { state }) = app.overlay.as_mut() else {
        panic!("command overlay");
    };
    let crate::tui::event::CommandSurfaceSnapshot::List {
        primary_task_id, ..
    } = &mut state.session.surface
    else {
        panic!("list command session");
    };
    *primary_task_id = None;

    type_chars(&mut app, "edit-title").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(!matches!(app.overlay, Some(OverlayState::TextInput(_))));
    assert_eq!(
        toast_message(&app).as_deref(),
        Some(":edit-title is disabled: requires a selected task")
    );
}

#[tokio::test]
async fn detail_command_panel_disables_list_only_filter_and_mark_commands() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Detail list-only target")).await;
    app.show_detail(0);

    for command_name in ["filter-label", "filter-priority", "toggle-mark-all"] {
        app.begin_command().await;
        type_chars(&mut app, command_name).await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(app.detail.is_active());
        assert!(!matches!(app.overlay, Some(OverlayState::Picker(_))));
        let expected = format!(":{command_name} is disabled: available only in the task list");
        assert_eq!(toast_message(&app).as_deref(), Some(expected.as_str()));
    }
}

#[tokio::test]
async fn detail_scope_picker_exits_detail_before_retargeting() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Scope picker target")).await;
    app.show_detail(0);

    app.begin_command().await;
    type_chars(&mut app, "scope-project").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(app.detail.is_inactive());
    assert!(matches!(
        app.overlay,
        Some(OverlayState::Picker(PickerState {
            intent: PickerIntent::ScopeProject,
            ..
        }))
    ));
}

#[tokio::test]
async fn detail_workspace_picker_exits_detail_before_retargeting() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Workspace picker target")).await;
    app.show_detail(0);

    app.begin_command().await;
    type_chars(&mut app, "workspace-switch").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    assert!(app.detail.is_inactive());
    assert!(matches!(
        app.overlay,
        Some(OverlayState::Picker(PickerState {
            intent: PickerIntent::SwitchWorkspace,
            ..
        }))
    ));
}

#[tokio::test]
async fn command_overlay_tab_completes_highlighted_row() {
    let mut app = test_app().await;

    app.begin_command().await;
    type_chars(&mut app, "stat").await;
    app.handle_overlay_key(key(KeyCode::Down)).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Down)).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Command { state })
            if state.input.text == "status-inbox"
                && state.highlighted_name() == Some("status-inbox")
    ));
}

fn command_input_and_choices(app: &App) -> (String, Vec<String>) {
    let Some(OverlayState::Command { state }) = &app.overlay else {
        panic!("command overlay");
    };
    (
        state.input.text.clone(),
        state
            .candidates
            .iter()
            .map(|candidate| candidate.input(&state.catalog))
            .collect(),
    )
}

#[tokio::test]
async fn theme_missing_argument_prepares_choices_and_highlighted_enter_preserves_value() {
    let mut app = test_app().await;
    app.begin_command().await;
    type_chars(&mut app, "the").await;
    app.handle_overlay_key(key(KeyCode::Down)).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert_eq!(
        command_input_and_choices(&app),
        (
            "theme ".to_string(),
            vec![
                "theme dark".to_string(),
                "theme light".to_string(),
                "theme terminal".to_string()
            ],
        )
    );
    assert_eq!(app.appearance, crate::tui::theme::Appearance::Dark);
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("choose a value: :theme <dark|light|terminal>")
    );
    let rendered = render_app_text(&mut app, 100, 30);
    for label in [":theme dark", ":theme light", ":theme terminal"] {
        assert!(rendered.contains(label));
    }
    app.handle_overlay_key(key(KeyCode::Down)).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Down)).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(app.overlay.is_none());
    assert_eq!(app.appearance, crate::tui::theme::Appearance::Light);
}

#[tokio::test]
async fn theme_errors_keep_input_and_theme_while_retired_command_is_unknown() {
    let mut app = test_app().await;
    app.set_appearance(crate::tui::theme::Appearance::Terminal);
    for input in [
        "theme",
        "theme ",
        "theme sepia",
        "theme auto",
        "theme dark light",
        "theme DARK",
    ] {
        app.begin_command().await;
        type_chars(&mut app, input).await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        assert_eq!(app.appearance, crate::tui::theme::Appearance::Terminal);
        let (text, choices) = command_input_and_choices(&app);
        if input.trim() == "theme" {
            assert_eq!(text, "theme ");
            assert_eq!(choices.len(), 3);
        } else {
            assert_eq!(text, input);
            assert!(choices.is_empty());
        }
        assert!(
            toast_message(&app)
                .unwrap()
                .contains(":theme <dark|light|terminal>")
        );
        app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
    }
    app.begin_command().await;
    type_chars(&mut app, "toggle-background").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert_eq!(
        toast_message(&app).as_deref(),
        Some("unknown command: toggle-background")
    );
    assert_eq!(app.appearance, crate::tui::theme::Appearance::Terminal);
}

#[tokio::test]
async fn tab_completes_command_then_cycles_arguments_and_resets_on_edit() {
    let mut app = test_app().await;
    app.begin_command().await;
    type_chars(&mut app, "the").await;
    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
    assert_eq!(command_input_and_choices(&app).0, "theme ");
    for (code, expected) in [
        (KeyCode::Tab, "theme dark"),
        (KeyCode::Tab, "theme light"),
        (KeyCode::Tab, "theme terminal"),
        (KeyCode::Tab, "theme dark"),
        (KeyCode::BackTab, "theme terminal"),
    ] {
        app.handle_overlay_key(key(code)).await.unwrap();
        let (input, choices) = command_input_and_choices(&app);
        assert_eq!(input, expected);
        assert_eq!(choices, ["theme dark", "theme light", "theme terminal"]);
        let Some(OverlayState::Command { state }) = &app.overlay else {
            panic!("command overlay");
        };
        assert_eq!(
            state.candidates[state.highlighted.unwrap()].input(&state.catalog),
            expected
        );
        let rendered = render_app_text(&mut app, 100, 30);
        for choice in choices {
            assert!(rendered.contains(&format!(":{choice}")));
        }
    }
    for _ in 0..8 {
        app.handle_overlay_key(key(KeyCode::Backspace))
            .await
            .unwrap();
    }
    type_chars(&mut app, "l").await;
    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
    assert_eq!(
        command_input_and_choices(&app),
        ("theme light".to_string(), vec!["theme light".to_string()])
    );
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert_eq!(app.appearance, crate::tui::theme::Appearance::Light);
    assert!(app.overlay.is_none());
}

#[tokio::test]
async fn command_name_cycle_keeps_matches_visible_until_editing() {
    let mut app = test_app().await;
    app.begin_command().await;
    type_chars(&mut app, ":edit-").await;
    let (_, matches) = command_input_and_choices(&app);
    assert!(matches.len() > 1);

    for expected in matches.iter().chain(matches.iter().take(1)) {
        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
        let (input, choices) = command_input_and_choices(&app);
        assert_eq!(&input, expected);
        assert_eq!(choices, matches);
        let Some(OverlayState::Command { state }) = &app.overlay else {
            panic!("command overlay");
        };
        assert_eq!(state.highlighted_name(), Some(expected.as_str()));
    }
    app.handle_overlay_key(key(KeyCode::BackTab)).await.unwrap();
    assert_eq!(command_input_and_choices(&app).0, *matches.last().unwrap());
    assert_eq!(command_input_and_choices(&app).1, matches);

    type_chars(&mut app, "zzzz").await;
    assert!(command_input_and_choices(&app).1.is_empty());
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Command { state }) if state.cycle_input.is_none()
    ));
}

#[tokio::test]
async fn argument_cycle_keeps_rows_browsable_and_enter_applies_highlight() {
    let mut app = test_app().await;
    app.begin_command().await;
    type_chars(&mut app, "theme").await;
    for _ in 0..2 {
        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
    }
    assert_eq!(command_input_and_choices(&app).0, "theme dark");
    app.handle_overlay_key(key(KeyCode::Down)).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
    assert_eq!(command_input_and_choices(&app).0, "theme light");
    app.handle_overlay_key(key(KeyCode::Down)).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(app.overlay.is_none());
    assert_eq!(app.appearance, crate::tui::theme::Appearance::Terminal);
}

#[tokio::test]
async fn tab_name_cycle_continues_past_theme_when_multiple_commands_match() {
    let mut app = test_app().await;
    app.begin_command().await;
    let Some(OverlayState::Command { state }) = &app.overlay else {
        panic!("command overlay")
    };
    let names = state
        .candidates
        .iter()
        .map(|candidate| candidate.input(&state.catalog))
        .collect::<Vec<_>>();
    assert!(names.iter().any(|name| name == "theme"));
    for expected in &names {
        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
        assert_eq!(&command_input_and_choices(&app).0, expected);
    }
    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
    assert_eq!(command_input_and_choices(&app).0, names[0]);
}

#[tokio::test]
async fn pasted_theme_arguments_refresh_choices_and_accept_whitespace() {
    let mut app = test_app().await;
    app.begin_command().await;
    app.dispatch_paste("  :theme   light  ").await.unwrap();
    assert_eq!(command_input_and_choices(&app).1, vec!["theme light"]);
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert_eq!(app.appearance, crate::tui::theme::Appearance::Light);
    assert!(app.overlay.is_none());
}

#[tokio::test]
async fn theme_is_global_in_detail_and_preserves_task_and_config() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Theme target")).await;
    app.show_detail(0);
    let task_id = app.store.tasks[0].task.id.clone();
    let config_appearance = app.store.config().tui.appearance;
    app.begin_command().await;
    type_chars(&mut app, "theme terminal").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(app.detail.is_active());
    assert!(app.overlay.is_none());
    assert_eq!(app.store.tasks[0].task.id, task_id);
    assert_eq!(app.appearance, crate::tui::theme::Appearance::Terminal);
    assert_eq!(app.store.config().tui.appearance, config_appearance);
}

#[test]
fn choice_query_preserves_whitespace_boundary_and_description_search() {
    for input in ["theme ", ":theme   ", "  :theme  "] {
        let state = CommandState::test_with_input(input);
        assert_eq!(state.candidates.len(), 3);
        assert!(
            state
                .candidates
                .iter()
                .all(|candidate| candidate.choice.is_some())
        );
    }
    let state = CommandState::test_with_input("theme");
    assert!(
        state
            .candidates
            .iter()
            .all(|candidate| candidate.choice.is_none())
    );
    let state = CommandState::test_with_input("set status");
    assert!(!state.candidates.is_empty());
    assert!(state.candidates.iter().all(|candidate| {
        state
            .catalog
            .command(candidate.index)
            .unwrap()
            .name()
            .starts_with("status-")
            && candidate.choice.is_none()
    }));
}
