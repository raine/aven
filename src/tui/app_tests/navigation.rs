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
