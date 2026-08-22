use super::*;
use ratatui::layout::Rect;

fn row_column_task_click_event(size: (u16, u16), viewport_row: u16) -> MouseEvent {
    let task_area = task_list_area(size);
    task_row_click(task_area.x + 1, task_area.y + 1 + viewport_row)
}

fn status_right_click_event(app: &App, size: (u16, u16), task_index: usize) -> MouseEvent {
    let task_area = task_list_area(size);
    let table = app.list.table_state();
    for row in task_area.y..task_area.y.saturating_add(task_area.height) {
        for column in task_area.x..task_area.x.saturating_add(task_area.width) {
            if crate::tui::ui::task_status_at_position(&app.store, table, task_area, column, row)
                .is_some_and(|hit| hit.task_index == task_index)
            {
                return right_click(column, row);
            }
        }
    }
    panic!("expected status hit target");
}

fn bulk_footer_click(
    action: crate::tui::event::Action,
    size: (u16, u16),
    count: usize,
) -> MouseEvent {
    let area = crate::tui::ui::footer_area(Rect::new(0, 0, size.0, size.1));
    let column = (area.x..area.x.saturating_add(area.width))
        .find(|column| {
            crate::tui::ui::bulk_footer_action_at(area, count, *column, area.y.saturating_add(1))
                == Some(action)
        })
        .expect("expected bulk footer action");
    task_row_click(column, area.y.saturating_add(1))
}

#[tokio::test]
async fn bulk_footer_mouse_opens_actions_and_clears_marks() {
    let mut app = test_app().await;
    let selected = create_and_select_task(&mut app, test_task_draft("marked")).await;
    let task_id = app.store.tasks[selected].task.id.clone();
    app.list.mark(task_id);
    let size = (100, 24);

    let actions = bulk_footer_click(crate::tui::event::Action::BeginCommand, size, 1);
    app.dispatch_mouse(actions, size.into()).await.unwrap();
    assert!(matches!(app.overlay, Some(OverlayState::Command { .. })));

    app.overlay = None;
    let clear = bulk_footer_click(crate::tui::event::Action::ClearMarks, size, 1);
    app.dispatch_mouse(clear, size.into()).await.unwrap();
    assert!(app.list.marked_task_ids().is_empty());
}

#[tokio::test]
async fn task_row_click_selects_task() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("task one")).await;

    let click = row_column_task_click_event((80, 24), 1);
    app.dispatch_mouse(click, (80, 24).into()).await.unwrap();

    assert_eq!(app.list.selected_task(), Some(0));
    assert_eq!(app.list.focus(), Focus::Tasks);
    assert!(app.overlay.is_none());
}

#[tokio::test]
async fn status_right_click_opens_status_menu_for_clicked_task() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("task one")).await;
    create_and_select_task(&mut app, test_task_draft("task two")).await;
    app.list.select_task(Some(1));

    let click = status_right_click_event(&app, (140, 24), 0);
    app.dispatch_mouse(click, (140, 24).into()).await.unwrap();

    assert_eq!(app.list.selected_task(), Some(0));
    assert_eq!(app.list.focus(), Focus::Tasks);
    assert_eq!(
        app.footer_choice.as_ref().map(|choice| choice.mode),
        Some(FooterChoiceMode::Status)
    );
    assert!(app.overlay.is_none());
}

#[tokio::test]
async fn recurrence_context_menu_matches_lifecycle() {
    let mut app = test_app().await;
    let (task_id, series_id) = add_recurring_series(&mut app, "Context series").await;
    app.list
        .select_task(app.store.show_view(TaskQuery::Recurring).await.unwrap());
    let size = (140, 24);

    let click = recurrence_right_click_event(&app, size, 0);
    app.dispatch_mouse(click, size.into()).await.unwrap();
    let Some(OverlayState::Picker(picker)) = app.overlay.as_ref() else {
        panic!("expected recurrence context menu");
    };
    assert!(matches!(
        picker.intent,
        PickerIntent::RecurrenceActions { .. }
    ));
    let values = picker
        .items
        .iter()
        .map(|item| item.value.as_str())
        .collect::<Vec<_>>();
    assert!(values.contains(&"pause"));
    assert!(values.contains(&"stop"));
    assert!(!values.contains(&"resume"));

    app.cancel_overlay();
    app.store
        .pause_recurrence(&series_id, Some(&task_id))
        .await
        .unwrap();
    let click = recurrence_right_click_event(&app, size, 0);
    app.dispatch_mouse(click, size.into()).await.unwrap();
    let Some(OverlayState::Picker(picker)) = app.overlay.as_ref() else {
        panic!("expected paused recurrence context menu");
    };
    let values = picker
        .items
        .iter()
        .map(|item| item.value.as_str())
        .collect::<Vec<_>>();
    assert!(values.contains(&"resume"));
    assert!(values.contains(&"stop"));
    assert!(!values.contains(&"pause"));

    app.cancel_overlay();
    let mut app = test_app().await;
    add_recurring_series(&mut app, "Stopped context").await;
    app.list
        .select_task(app.store.show_view(TaskQuery::Recurring).await.unwrap());
    app.execute_selected_recurrence_action(Action::StopRecurrence)
        .await
        .unwrap();
    app.handle_overlay_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .await
        .unwrap();
    let click = recurrence_right_click_event(&app, size, 0);
    app.dispatch_mouse(click, size.into()).await.unwrap();
    let Some(OverlayState::Picker(picker)) = app.overlay.as_ref() else {
        panic!("expected stopped recurrence context menu");
    };
    let values = picker
        .items
        .iter()
        .map(|item| item.value.as_str())
        .collect::<Vec<_>>();
    assert!(!values.contains(&"pause"));
    assert!(!values.contains(&"resume"));
    assert!(!values.contains(&"stop"));
    assert!(values.contains(&"history"));
    assert!(values.contains(&"edit-template"));
}

#[tokio::test]
async fn status_right_click_ignores_non_status_columns() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("task one")).await;
    create_and_select_task(&mut app, test_task_draft("task two")).await;
    app.list.select_task(Some(1));

    let click = row_column_task_click_event((140, 24), 1);
    app.dispatch_mouse(right_click(click.column, click.row), (140, 24).into())
        .await
        .unwrap();

    assert_eq!(app.list.selected_task(), Some(1));
    assert!(app.overlay.is_none());
}

#[tokio::test]
async fn status_right_click_reuses_status_update_and_undo() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("task one")).await;

    let size = (140, 24).into();
    let click = status_right_click_event(&app, (140, 24), 0);
    app.dispatch_mouse(click, size).await.unwrap();
    assert_eq!(
        app.footer_choice.as_ref().map(|choice| choice.mode),
        Some(FooterChoiceMode::Status)
    );
    app.dispatch_key(key(KeyCode::Char('a')), size)
        .await
        .unwrap();

    assert_eq!(app.store.tasks[0].task.status, TaskStatus::Active);
    assert!(toast_message(&app).is_some_and(|message| message.ends_with("status=active · u undo")));

    app.handle_normal_key(KeyCode::Char('u')).await.unwrap();
    assert_eq!(app.store.tasks[0].task.status, TaskStatus::Inbox);
    assert!(toast_message(&app).is_some_and(|message| message.contains("undid")));
}

#[tokio::test]
async fn task_row_click_opens_detail_on_double_click() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("task one")).await;

    let click = row_column_task_click_event((80, 24), 1);
    app.dispatch_mouse(click, (80, 24).into()).await.unwrap();
    assert_eq!(app.list.selected_task(), Some(0));
    assert!(app.overlay.is_none());
    assert!(app.list.has_task_click());

    app.dispatch_mouse(click, (80, 24).into()).await.unwrap();
    assert!(!app.list.has_task_click());
    assert_eq!(app.list.selected_task(), Some(0));
    assert!(app.overlay.is_none());
    assert!(app.detail.is_active());
}

#[tokio::test]
async fn task_row_click_wide_layout_respects_sidebar_offset() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("task one")).await;
    create_and_select_task(&mut app, test_task_draft("task two")).await;
    app.list.select_task(Some(1));
    app.list.focus_tasks();

    let sidebar = crate::tui::ui::sidebar_layout(Rect::new(0, 0, 140, 24), Focus::Tasks)
        .unwrap()
        .sidebar;
    let sidebar_click = task_row_click(
        sidebar.x.saturating_add(sidebar.width).saturating_sub(1),
        sidebar.y + 2,
    );
    app.dispatch_mouse(sidebar_click, (140, 24).into())
        .await
        .unwrap();
    assert_eq!(app.list.selected_task(), Some(1));
    assert_eq!(app.list.focus(), Focus::Tasks);

    let click = row_column_task_click_event((140, 24), 1);
    app.dispatch_mouse(click, (140, 24).into()).await.unwrap();

    assert_eq!(app.list.selected_task(), Some(0));
    assert_eq!(app.list.focus(), Focus::Tasks);
}

#[tokio::test]
async fn task_row_click_preview_area_miss_is_ignored() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("task one")).await;
    create_and_select_task(&mut app, test_task_draft("task two")).await;
    app.list.select_task(Some(1));

    let task_area = task_list_area((140, 40));
    let preview_row = task_area.y + task_area.height.saturating_sub(3);
    let click = task_row_click(task_area.x + 1, preview_row);
    app.dispatch_mouse(click, (140, 40).into()).await.unwrap();

    assert_eq!(app.list.selected_task(), Some(1));
    assert!(app.overlay.is_none());
}

#[tokio::test]
async fn task_row_click_stale_state_is_reset_after_non_task_hit() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("task one")).await;

    let row_click = row_column_task_click_event((80, 24), 1);
    app.dispatch_mouse(row_click, (80, 24).into())
        .await
        .unwrap();
    app.dispatch_mouse(task_row_click(10, 23), (80, 24).into())
        .await
        .unwrap();
    app.dispatch_mouse(row_click, (80, 24).into())
        .await
        .unwrap();

    assert_eq!(app.list.selected_task(), Some(0));
    assert!(app.overlay.is_none());
}

#[tokio::test]
async fn task_row_click_ignores_narrow_sidebar_overlay() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("task one")).await;
    app.list.focus_sidebar();

    let overlay = crate::tui::ui::sidebar_layout(Rect::new(0, 0, 80, 40), Focus::Sidebar)
        .expect("sidebar overlay should exist in narrow layout")
        .sidebar;
    let click = task_row_click(overlay.x + 1, overlay.y + 1);
    app.dispatch_mouse(click, (80, 40).into()).await.unwrap();

    assert_eq!(app.list.selected_task(), Some(0));
    assert!(app.overlay.is_none());
    assert_eq!(app.list.focus(), Focus::Sidebar);
}
