use super::*;

#[tokio::test]
async fn sidebar_click_selects_project_scope_in_wide_layout() {
    let mut app = test_app().await;
    app.store
        .create_project("Mobile App".to_string())
        .await
        .unwrap();
    app.refresh().await.unwrap();

    let project_index = app
        .store
        .sidebar_entries
        .iter()
        .position(|entry| {
            matches!(
                &entry.target,
                Some(SidebarEntryTarget::Scope(TaskScopeTarget::Project(project)))
                    if project == "mobile-app"
            )
        })
        .expect("mobile-app sidebar entry");
    let terminal_size: ratatui::layout::Size = (140, 24).into();
    let layout = crate::tui::ui::sidebar_layout(
        ratatui::layout::Rect::new(0, 0, terminal_size.width, terminal_size.height),
        Focus::Tasks,
    )
    .expect("wide sidebar layout");
    assert!(project_index >= usize::from(layout.content.height));

    app.list.select_sidebar(Some(project_index));
    let _ = render_app_buffer(&mut app, terminal_size.width, terminal_size.height);
    let offset = app.list.sidebar_state().offset();
    assert!(offset > 0, "render must scroll the project into view");
    let visible_index = project_index
        .checked_sub(offset)
        .expect("sidebar offset must not exceed the selected project index");
    let visible_row = u16::try_from(visible_index).expect("visible row must fit in u16");
    assert!(visible_row < layout.content.height);
    assert_eq!(app.store.view_state.scope, TaskScope::Workspace);

    app.dispatch_mouse(
        click_at(layout.content.x, layout.content.y + visible_row),
        terminal_size,
    )
    .await
    .unwrap();

    assert_eq!(
        app.store.view_state.scope,
        TaskScope::Project("mobile-app".to_string())
    );
    assert_eq!(app.list.focus(), Focus::Tasks);
    assert_eq!(app.list.selected_sidebar(), Some(project_index));
    assert!(app.overlay.is_none());
}

#[tokio::test]
async fn sidebar_click_selects_saved_view_in_narrow_overlay() {
    let mut app = test_app().await;
    app.list.focus_sidebar();

    let view_row = app
        .store
        .sidebar_entries
        .iter()
        .position(|entry| {
            matches!(
                &entry.target,
                Some(SidebarEntryTarget::View(TaskQuery::Open))
            )
        })
        .unwrap() as u16;
    let terminal_size: ratatui::layout::Size = (90, 24).into();
    let layout = crate::tui::ui::sidebar_layout(
        ratatui::layout::Rect::new(0, 0, terminal_size.width, terminal_size.height),
        Focus::Sidebar,
    )
    .unwrap();
    let row = layout.content.y + view_row;

    app.dispatch_mouse(click_at(layout.content.x, row), terminal_size)
        .await
        .unwrap();

    assert_eq!(app.store.view_state.query, TaskQuery::Open);
    assert_eq!(app.list.focus(), Focus::Tasks);
    assert_eq!(app.list.selected_sidebar(), Some(view_row as usize));
    assert!(app.overlay.is_none());
}

#[tokio::test]
async fn sidebar_click_uses_scroll_offset_in_wide_layout() {
    let mut app = test_app().await;
    for index in 0..25 {
        app.store
            .create_project(format!("Project {index}"))
            .await
            .unwrap();
    }
    app.refresh().await.unwrap();

    let project_index = app
        .store
        .sidebar_entries
        .iter()
        .position(|entry| {
            matches!(
                &entry.target,
                Some(SidebarEntryTarget::Scope(TaskScopeTarget::Project(project)))
                    if project == "project-24"
            )
        })
        .unwrap();
    app.list.focus_sidebar();
    app.list.select_sidebar(Some(project_index));

    let terminal_size: ratatui::layout::Size = (120, 24).into();
    let backend = ratatui::backend::TestBackend::new(terminal_size.width, terminal_size.height);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    let view = app.view();
    terminal
        .draw(|frame| {
            crate::tui::ui::render(frame, &app.store, &mut app.widgets, &mut app.list, &view)
        })
        .unwrap();

    let offset = app.list.sidebar_state().offset();
    assert!(offset > 0);
    let layout = crate::tui::ui::sidebar_layout(
        ratatui::layout::Rect::new(0, 0, terminal_size.width, terminal_size.height),
        Focus::Sidebar,
    )
    .unwrap();
    let visible_row = u16::try_from(project_index - offset).unwrap();

    app.dispatch_mouse(
        click_at(layout.content.x, layout.content.y + visible_row),
        terminal_size,
    )
    .await
    .unwrap();

    assert_eq!(
        app.store.view_state.scope,
        TaskScope::Project("project-24".to_string())
    );
    assert_eq!(app.list.selected_sidebar(), Some(project_index));
}

#[tokio::test]
async fn compatible_query_transitions_follow_selected_task_identity() {
    let mut app = test_app().await;
    for title in ["Zulu task", "Alpha task", "Middle task"] {
        app.store
            .create_task(test_task_draft(title), None)
            .await
            .unwrap();
    }
    app.list.select_task(Some(1));
    let selected_id = app.store.tasks[1].task.id.clone();

    app.set_sort(TaskOrder::Title).await.unwrap();

    let selected = app.list.selected_task().unwrap();
    assert_eq!(app.store.tasks[selected].task.id, selected_id);
    assert_eq!(app.store.view_state.order, TaskOrder::Title);
}

#[tokio::test]
async fn sidebar_transition_keeps_refresh_selected_task() {
    let mut app = test_app().await;
    for title in ["Zulu task", "Alpha task", "Middle task"] {
        app.store
            .create_task(test_task_draft(title), None)
            .await
            .unwrap();
    }
    app.list.select_task(Some(1));
    let selected_id = app.store.tasks[1].task.id.clone();
    let open = app
        .store
        .sidebar_entries
        .iter()
        .position(|entry| entry.target == Some(SidebarEntryTarget::View(TaskQuery::Open)))
        .unwrap();
    app.list.select_sidebar(Some(open));

    app.apply_sidebar_selection().await.unwrap();

    let selected = app.list.selected_task().unwrap();
    assert_eq!(app.store.tasks[selected].task.id, selected_id);
}

#[tokio::test]
async fn clearing_filters_restores_applicable_historical_identity() {
    let mut app = test_app().await;
    for title in ["First urgent", "Second urgent"] {
        app.store
            .create_task(
                TaskDraft {
                    priority: "urgent".to_string(),
                    ..test_task_draft(title)
                },
                None,
            )
            .await
            .unwrap();
    }
    app.store
        .create_task(test_task_draft("Hidden task"), None)
        .await
        .unwrap();
    let hidden = app
        .store
        .tasks
        .iter()
        .position(|item| item.task.title == "Hidden task")
        .unwrap();
    let hidden_id = app.store.tasks[hidden].task.id.clone();
    app.list.select_task(Some(hidden));

    app.submit_filter_priority(vec!["urgent".to_string()])
        .await
        .unwrap();
    assert!(
        app.store
            .selected_task(app.list.selected_task())
            .is_some_and(|item| item.task.id != hidden_id)
    );

    app.clear_filters().await.unwrap();

    assert_eq!(
        app.store
            .selected_task(app.list.selected_task())
            .unwrap()
            .task
            .id,
        hidden_id
    );
}

#[tokio::test]
async fn failed_query_transition_preserves_selection_and_navigation() {
    let mut app = test_app().await;
    for title in ["First", "Second"] {
        app.store
            .create_task(test_task_draft(title), None)
            .await
            .unwrap();
    }
    app.list.select_task(Some(1));
    let selected_id = app.store.tasks[1].task.id.clone();
    app.store.fail_next_refresh();

    assert!(app.set_sort(TaskOrder::Title).await.is_err());

    assert_eq!(app.store.view_state.query, TaskQuery::Queue);
    assert_eq!(
        app.store
            .selected_task(app.list.selected_task())
            .unwrap()
            .task
            .id,
        selected_id
    );
    assert!(app.list.navigation_is_empty());
}
