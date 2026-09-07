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
    terminal.draw(|frame| app.render_frame(frame)).unwrap();

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

async fn click_sidebar_target(app: &mut App, target: SidebarEntryTarget, width: u16, height: u16) {
    app.list.focus_sidebar();
    app.list.select_sidebar_target(Some(&target));
    let _ = render_app_buffer(app, width, height);
    let index = app
        .list
        .sidebar_entries()
        .iter()
        .position(|entry| entry.target.as_ref() == Some(&target))
        .unwrap();
    let layout = crate::tui::ui::sidebar_layout(
        ratatui::layout::Rect::new(0, 0, width, height),
        Focus::Sidebar,
    )
    .unwrap();
    let row = layout.content.y + (index - app.list.sidebar_state().offset()) as u16;
    app.dispatch_mouse(click_at(layout.content.x, row), (width, height).into())
        .await
        .unwrap();
}

#[tokio::test]
async fn sidebar_sections_collapse_independently_and_headers_expand_with_mouse() {
    use crate::tui::store::SidebarSection;
    for width in [90, 140] {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();
        app.refresh().await.unwrap();
        for section in [
            SidebarSection::Views,
            SidebarSection::Scope,
            SidebarSection::Projects,
        ] {
            assert!(!app.list.section_collapsed(section));
            click_sidebar_target(&mut app, SidebarEntryTarget::Section(section), width, 24).await;
            assert!(app.list.section_collapsed(section));
            assert!(
                app.list
                    .sidebar_entries()
                    .iter()
                    .any(|entry| entry.target == Some(SidebarEntryTarget::Section(section)))
            );
        }
        assert!(app.list.sidebar_entries().iter().all(|entry| entry.section));
        app.select_edge(false).await.unwrap();
        for section in [
            SidebarSection::Views,
            SidebarSection::Scope,
            SidebarSection::Projects,
            SidebarSection::Views,
        ] {
            assert_eq!(
                app.list.sidebar_entries()[app.list.selected_sidebar().unwrap()].target,
                Some(SidebarEntryTarget::Section(section))
            );
            app.move_selection(1).await.unwrap();
        }
        assert_eq!(app.store.view_state.query, TaskQuery::Queue);
        assert_eq!(app.store.view_state.scope, TaskScope::Workspace);
        for section in [
            SidebarSection::Scope,
            SidebarSection::Views,
            SidebarSection::Projects,
        ] {
            click_sidebar_target(&mut app, SidebarEntryTarget::Section(section), width, 24).await;
            assert!(!app.list.section_collapsed(section));
            if section == SidebarSection::Scope {
                assert!(app.list.section_collapsed(SidebarSection::Views));
                assert!(app.list.section_collapsed(SidebarSection::Projects));
                assert!(app.list.sidebar_entries().iter().any(|entry| entry.target
                    == Some(SidebarEntryTarget::Scope(TaskScopeTarget::Workspace))));
            }
        }
        app.list
            .select_sidebar_target(Some(&SidebarEntryTarget::Section(SidebarSection::Views)));
        let buffer = render_app_buffer(&mut app, width, 40);
        assert!(
            buffer
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>()
                .contains(&format!(
                    " VIEWS{}▾ ",
                    " ".repeat(if width == 90 { 24 } else { 17 })
                ))
        );
    }
}

#[tokio::test]
async fn sidebar_header_indicators_render_at_right_with_whole_row_targets() {
    use crate::tui::store::SidebarSection;
    use crate::tui::ui::sidebar_click_at_for;
    use ratatui::layout::Rect;

    for width in [90, 140] {
        let mut app = test_app().await;
        app.list.focus_sidebar();
        for section in [
            SidebarSection::Views,
            SidebarSection::Scope,
            SidebarSection::Projects,
        ] {
            let target = SidebarEntryTarget::Section(section);
            for collapsed in [false, true] {
                app.list.select_sidebar_target(Some(&target));
                let buffer = render_app_buffer(&mut app, width, 40);
                let terminal = Rect::new(0, 0, width, 40);
                let layout = crate::tui::ui::sidebar_layout(terminal, Focus::Sidebar).unwrap();
                let index = app.list.selected_sidebar().unwrap();
                let entry = &app.list.sidebar_entries()[index];
                let row = layout.content.y + (index - app.list.sidebar_state().offset()) as u16;
                let text: String = (layout.content.x..layout.content.right())
                    .map(|column| buffer[(column, row)].symbol())
                    .collect();
                assert!(text.starts_with(&format!(" {} ", entry.label.to_uppercase())));
                assert_eq!(
                    buffer[(layout.content.right() - 2, row)].symbol(),
                    if collapsed { "▸" } else { "▾" }
                );
                for column in layout.content.x..layout.content.right() {
                    assert_eq!(
                        sidebar_click_at_for(
                            app.list.sidebar_entries(),
                            app.list.sidebar_state(),
                            Focus::Sidebar,
                            true,
                            terminal,
                            column,
                            row,
                        )
                        .unwrap()
                        .target,
                        target
                    );
                }
                app.dispatch_mouse(
                    click_at(layout.content.right() - 2, row),
                    (width, 40).into(),
                )
                .await
                .unwrap();
            }
        }
    }
}

#[tokio::test]
async fn collapsed_sidebar_survives_refresh_view_and_project_scope_changes() {
    use crate::tui::store::SidebarSection;
    let mut app = test_app().await;
    app.store
        .create_project("Mobile App".to_string())
        .await
        .unwrap();
    app.refresh().await.unwrap();
    click_sidebar_target(
        &mut app,
        SidebarEntryTarget::Section(SidebarSection::Views),
        140,
        30,
    )
    .await;
    click_sidebar_target(
        &mut app,
        SidebarEntryTarget::Scope(TaskScopeTarget::Project("mobile-app".to_string())),
        140,
        30,
    )
    .await;
    app.show_view(TaskQuery::Ready).await.unwrap();
    app.refresh().await.unwrap();
    assert!(app.list.section_collapsed(SidebarSection::Views));
    assert!(
        !app.list
            .sidebar_entries()
            .iter()
            .any(|entry| matches!(entry.target, Some(SidebarEntryTarget::View(_))))
    );
    assert_eq!(
        app.store.view_state.scope,
        TaskScope::Project("mobile-app".to_string())
    );
    assert_eq!(app.store.view_state.query, TaskQuery::Ready);
    assert_eq!(
        app.list.sidebar_entries()[app.list.selected_sidebar().unwrap()].target,
        Some(SidebarEntryTarget::Scope(TaskScopeTarget::Project(
            "mobile-app".to_string()
        )))
    );
    click_sidebar_target(
        &mut app,
        SidebarEntryTarget::Section(SidebarSection::Projects),
        140,
        30,
    )
    .await;
    assert_eq!(
        app.store.view_state.scope,
        TaskScope::Project("mobile-app".to_string())
    );
    app.refresh().await.unwrap();
    assert!(app.list.section_collapsed(SidebarSection::Projects));
    assert!(
        render_app_buffer(&mut app, 140, 30)
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
            .contains(&format!(" PROJECTS{}▸ ", " ".repeat(14)))
    );
}

#[tokio::test]
async fn sidebar_configuration_orders_rows_and_keeps_selection_on_target() {
    use crate::config::SidebarView;
    let mut app = test_app().await;
    app.list
        .select_sidebar_target(Some(&SidebarEntryTarget::View(TaskQuery::Ready)));
    let mut config = crate::config::AppConfig::default();
    config.tui.sidebar.views = vec![SidebarView::Search, SidebarView::Ready];
    app.store.set_config(config.clone());
    app.preserve_or_restore_sidebar_selection();
    let views: Vec<_> = app
        .list
        .sidebar_entries()
        .iter()
        .filter_map(|entry| match entry.target {
            Some(SidebarEntryTarget::View(view)) => Some(view),
            _ => None,
        })
        .collect();
    assert_eq!(views, vec![TaskQuery::Search, TaskQuery::Ready]);
    assert_eq!(
        app.list.sidebar_entries()[app.list.selected_sidebar().unwrap()].target,
        Some(SidebarEntryTarget::View(TaskQuery::Ready))
    );
    config.tui.sidebar.views.clear();
    app.store.set_config(config);
    app.preserve_or_restore_sidebar_selection();
    assert!(app.list.sidebar_entries()[app.list.selected_sidebar().unwrap()].section);
    app.list.focus_sidebar();
    for _ in 0..10 {
        app.move_selection(1).await.unwrap();
        assert!(
            app.list.sidebar_entries()[app.list.selected_sidebar().unwrap()]
                .target
                .is_some()
        );
    }
    app.select_edge(false).await.unwrap();
    app.dispatch_key(key(KeyCode::Enter), (140, 30).into())
        .await
        .unwrap();
    assert!(
        app.list
            .section_collapsed(crate::tui::store::SidebarSection::Views)
    );
    assert_eq!(app.store.view_state.query, TaskQuery::Queue);
}

#[tokio::test]
async fn hidden_views_remain_accessible_through_header_shortcut_and_palette() {
    let mut app = test_app().await;
    let mut config = crate::config::AppConfig::default();
    config.tui.sidebar.views.clear();
    app.store.set_config(config);
    app.preserve_or_restore_sidebar_selection();
    app.show_view_menu(5, 0);
    let Some(OverlayState::HeaderMenu(state)) = &app.overlay else {
        panic!("view menu");
    };
    assert_eq!(state.items.len(), 17);
    app.handle_overlay_key(key(KeyCode::Char('i')))
        .await
        .unwrap();
    assert_eq!(app.store.view_state.query, TaskQuery::Inbox);
    app.dispatch_key(key(KeyCode::Char('v')), (140, 30).into())
        .await
        .unwrap();
    app.dispatch_key(key(KeyCode::Char('y')), (140, 30).into())
        .await
        .unwrap();
    assert_eq!(app.store.view_state.query, TaskQuery::Ready);
    app.dispatch_key(shift_key(KeyCode::Char(':')), (140, 30).into())
        .await
        .unwrap();
    type_chars(&mut app, "view-done").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert_eq!(app.store.view_state.query, TaskQuery::Done);
    assert!(
        !app.list
            .sidebar_entries()
            .iter()
            .any(|entry| matches!(entry.target, Some(SidebarEntryTarget::View(_))))
    );
    assert_eq!(
        app.list.sidebar_entries()[app.list.selected_sidebar().unwrap()].target,
        Some(SidebarEntryTarget::Section(
            crate::tui::store::SidebarSection::Views
        ))
    );
}

#[tokio::test]
async fn sidebar_heading_command_keeps_captured_section_identity() {
    use crate::tui::store::SidebarSection;
    let mut app = test_app().await;
    app.list.focus_sidebar();
    app.list
        .select_sidebar_target(Some(&SidebarEntryTarget::Section(SidebarSection::Views)));
    app.dispatch_key(shift_key(KeyCode::Char(':')), (140, 30).into())
        .await
        .unwrap();
    type_chars(&mut app, "detail").await;
    app.list
        .select_sidebar_target(Some(&SidebarEntryTarget::Section(SidebarSection::Projects)));
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(app.list.section_collapsed(SidebarSection::Views));
    assert!(!app.list.section_collapsed(SidebarSection::Projects));
}

#[tokio::test]
async fn sidebar_sections_restore_on_reopen_and_expansion_persists() {
    use crate::tui::store::SidebarSection::{Projects, Scope, Views};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sidebar.db");
    let open = async || {
        App::new_for_tests(aven_core::db::Database::open(&path).await.unwrap())
            .await
            .unwrap()
    };
    let mut app = open().await;
    for section in [Views, Scope, Projects] {
        assert!(!app.list.section_collapsed(section));
    }
    for section in [Views, Projects] {
        app.apply_sidebar_target(Some(SidebarEntryTarget::Section(section)))
            .await
            .unwrap();
    }
    drop(app);
    let mut app = open().await;
    assert!(app.list.section_collapsed(Views));
    assert!(!app.list.section_collapsed(Scope));
    assert!(app.list.section_collapsed(Projects));
    assert!(
        !app.list
            .sidebar_entries()
            .iter()
            .any(|entry| matches!(entry.target, Some(SidebarEntryTarget::View(_))))
    );
    app.show_view(TaskQuery::Ready).await.unwrap();
    app.refresh().await.unwrap();
    assert!(app.list.section_collapsed(Views));
    for section in [Views, Scope] {
        app.apply_sidebar_target(Some(SidebarEntryTarget::Section(section)))
            .await
            .unwrap();
    }
    drop(app);
    let app = open().await;
    assert!(!app.list.section_collapsed(Views));
    assert!(app.list.section_collapsed(Scope));
    assert!(app.list.section_collapsed(Projects));
    assert!(app.list.sidebar_entries().iter().any(|entry| matches!(
        entry.target,
        Some(SidebarEntryTarget::View(TaskQuery::Ready))
    )));
}

#[tokio::test]
async fn sidebar_write_failure_warns_and_keeps_runtime_choice() {
    use crate::tui::store::SidebarSection;
    let mut app = test_app().await;
    let pool = crate::test_support::open_db(
        &app._test_database_dir
            .as_ref()
            .unwrap()
            .path()
            .join("test.db"),
    )
    .await
    .unwrap();
    sqlx::query("CREATE TRIGGER reject_sidebar BEFORE INSERT ON meta WHEN NEW.key LIKE 'tui_sidebar_%' BEGIN SELECT RAISE(FAIL, 'sidebar write blocked'); END")
        .execute(&pool).await.unwrap();
    app.apply_sidebar_target(Some(SidebarEntryTarget::Section(SidebarSection::Views)))
        .await
        .unwrap();
    assert!(app.list.section_collapsed(SidebarSection::Views));
    assert!(
        toast_message(&app)
            .unwrap()
            .contains("could not save sidebar state")
    );
}

#[tokio::test]
async fn sidebar_load_failure_warns_and_defaults_to_expanded() {
    use crate::tui::store::SidebarSection;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sidebar.db");
    let database = aven_core::db::Database::open(&path).await.unwrap();
    let store = TuiStore::new(database, crate::workspaces::Workspace::default())
        .await
        .unwrap();
    let pool = crate::test_support::open_db(&path).await.unwrap();
    sqlx::query("DROP TABLE meta").execute(&pool).await.unwrap();
    let app = App::new_with_store(store).await.unwrap();
    assert!(
        toast_message(&app)
            .unwrap()
            .contains("could not load sidebar state")
    );
    for section in [
        SidebarSection::Views,
        SidebarSection::Scope,
        SidebarSection::Projects,
    ] {
        assert!(!app.list.section_collapsed(section));
    }
}
