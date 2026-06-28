use super::*;
use crate::operations::TaskDraft;
use crate::tui::app_conflicts::CONFLICT_CONFIRM_LOCAL_TITLE;
use crate::tui::app_edit::{
    EDIT_DESCRIPTION_TITLE, EDIT_LABELS_TITLE, EDIT_PRIORITY_TITLE, EDIT_PROJECT_TITLE,
    EDIT_STATUS_TITLE, EDIT_TITLE_TITLE,
};
use crate::tui::app_filters::{SCOPE_PROJECT_TITLE, SWITCH_WORKSPACE_TITLE};
use crate::tui::app_projects::{DELETE_PROJECT_TITLE, DELETE_TASK_TITLE};
use crate::tui::authoring::{ADD_NOTE_TITLE, AddTaskStep};
use crate::tui::config_overlay::{CONFIG_INFO_TITLE, CONFIG_INIT_TITLE, CONFIG_PATHS_TITLE};
use crate::tui::event::Action;
use crate::tui::overlay::{
    CommandState, ConfirmState, LineEdit, MultilineInputState, OverlayRoute, OverlayState,
    OverlayView, PickerItem, PickerMode, PickerState, SearchState, TextInputState, TextPanelState,
};
use crate::tui::store::{SidebarEntryTarget, TaskOrder, TaskScope, TaskScopeTarget, TaskView};
use crate::tui::toast::ToastSeverity;
use crate::tui::ui::ViewSurface;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use sqlx::SqlitePool;

fn toast_message(app: &App) -> Option<String> {
    app.notification
        .as_ref()
        .map(|notification| notification.toast_view().message)
}

fn toast_severity(app: &App) -> Option<ToastSeverity> {
    app.notification
        .as_ref()
        .map(|notification| notification.toast_view().severity)
}

async fn test_app() -> App {
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::db::open_db(&dir.path().join("test.db"))
        .await
        .unwrap();
    reset_default_workspace(&pool).await;
    App::new_for_tests(pool).await.unwrap()
}

fn test_task_draft(title: &str) -> TaskDraft {
    TaskDraft {
        title: title.to_string(),
        description: String::new(),
        project: None,
        status: "inbox".to_string(),
        priority: "none".to_string(),
        labels: Vec::new(),
    }
}

async fn create_and_select_task(app: &mut App, draft: TaskDraft) -> usize {
    let (_, selected) = app.store.create_task(draft, None).await.unwrap();
    let selected = selected.unwrap();
    app.widgets.table.select(Some(selected));
    selected
}

async fn test_app_with_pool() -> (tempfile::TempDir, SqlitePool, App) {
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::db::open_db(&dir.path().join("test.db"))
        .await
        .unwrap();
    reset_default_workspace(&pool).await;
    let app = App::new_for_tests(pool.clone()).await.unwrap();
    (dir, pool, app)
}

async fn reset_default_workspace(pool: &SqlitePool) {
    let mut conn = pool.acquire().await.unwrap();
    let default = crate::workspaces::ensure_default_workspace(&mut conn)
        .await
        .unwrap();
    crate::workspaces::set_active_workspace(default);
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn shift_key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::SHIFT)
}

fn ctrl_s() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)
}

fn ctrl_e() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL)
}

fn ctrl_x() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL)
}

fn ctrl_c() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
}

fn ctrl_p() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL)
}

fn ctrl_r() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)
}

fn ctrl_t() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL)
}

fn ctrl_n() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL)
}

fn ctrl_d() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL)
}

fn ctrl_u() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL)
}

fn header_click(column: u16) -> MouseEvent {
    click_at(column, 0)
}

fn click_at(column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

fn mouse_wheel(kind: MouseEventKind) -> MouseEvent {
    MouseEvent {
        kind,
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    }
}

fn task_row_click(column: u16, row: u16) -> MouseEvent {
    click_at(column, row)
}

fn left_click(column: u16, row: u16) -> MouseEvent {
    click_at(column, row)
}

fn right_click(column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Right),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

fn wheel_down(column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

#[track_caller]
fn picker_row_click(app: &App, visible_row: u16, size: ratatui::layout::Size) -> MouseEvent {
    let Some(overlay) = app.overlay.as_ref() else {
        panic!("expected overlay");
    };
    match OverlayView::from(overlay) {
        OverlayView::Picker(view) => {
            let layout = crate::tui::overlay::picker_layout(&view, size);
            left_click(
                layout.inner.x.saturating_add(2),
                layout
                    .inner
                    .y
                    .saturating_add(layout.list_start)
                    .saturating_add(visible_row),
            )
        }
        OverlayView::TagCombobox(view) => {
            let layout = crate::tui::overlay::tag_combobox_layout(&view, size);
            left_click(
                layout.inner.x.saturating_add(2),
                layout
                    .inner
                    .y
                    .saturating_add(layout.list_start)
                    .saturating_add(visible_row),
            )
        }
        _ => panic!("expected picker overlay"),
    }
}

#[track_caller]
fn confirm_hint_click(app: &App, column: u16, size: ratatui::layout::Size) -> MouseEvent {
    let Some(OverlayState::Confirm(state)) = app.overlay.as_ref() else {
        panic!("expected confirm overlay");
    };
    let layout = crate::tui::overlay::confirm_layout(size, &state.prompt);
    left_click(
        layout.inner.x.saturating_add(column),
        layout.inner.y.saturating_add(layout.hint_row),
    )
}

fn detail_metadata_click(target: crate::tui::ui::DetailMetadataTarget) -> MouseEvent {
    let row = match target {
        crate::tui::ui::DetailMetadataTarget::Status => 8,
        crate::tui::ui::DetailMetadataTarget::Priority => 11,
    };
    left_click(88, row)
}

fn render_app_buffer(app: &mut App, width: u16, height: u16) -> ratatui::buffer::Buffer {
    let backend = ratatui::backend::TestBackend::new(width, height);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    let view = app.view();
    terminal
        .draw(|frame| crate::tui::ui::render(frame, &app.store, &mut app.widgets, &view))
        .unwrap();
    terminal.backend().buffer().clone()
}

fn render_app_text(app: &mut App, width: u16, height: u16) -> String {
    render_app_buffer(app, width, height)
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

#[tokio::test]
async fn sidebar_click_selects_project_scope_in_wide_layout() {
    let mut app = test_app().await;
    app.store
        .create_project("Mobile App".to_string())
        .await
        .unwrap();
    app.refresh().await.unwrap();

    let project_row = app
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
        .unwrap() as u16;
    let terminal_size: ratatui::layout::Size = (140, 24).into();
    let layout = crate::tui::ui::sidebar_layout(
        ratatui::layout::Rect::new(0, 0, terminal_size.width, terminal_size.height),
        Focus::Tasks,
    )
    .unwrap();
    let row = layout.content.y + project_row;

    app.dispatch_mouse(click_at(layout.content.x, row), terminal_size)
        .await
        .unwrap();

    assert_eq!(
        app.store.view_state.scope,
        TaskScope::Project("mobile-app".to_string())
    );
    assert_eq!(app.focus, Focus::Tasks);
    assert_eq!(app.widgets.sidebar.selected(), Some(project_row as usize));
    assert!(app.overlay.is_none());
}

#[tokio::test]
async fn sidebar_click_selects_saved_view_in_narrow_overlay() {
    let mut app = test_app().await;
    app.focus = Focus::Sidebar;

    let view_row = app
        .store
        .sidebar_entries
        .iter()
        .position(|entry| {
            matches!(
                &entry.target,
                Some(SidebarEntryTarget::View(TaskView::Open))
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

    assert_eq!(app.store.view_state.view, TaskView::Open);
    assert_eq!(app.focus, Focus::Tasks);
    assert_eq!(app.widgets.sidebar.selected(), Some(view_row as usize));
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
    app.focus = Focus::Sidebar;
    app.widgets.sidebar.select(Some(project_index));

    let terminal_size: ratatui::layout::Size = (120, 24).into();
    let backend = ratatui::backend::TestBackend::new(terminal_size.width, terminal_size.height);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    let view = app.view();
    terminal
        .draw(|frame| crate::tui::ui::render(frame, &app.store, &mut app.widgets, &view))
        .unwrap();

    let offset = app.widgets.sidebar.offset();
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
    assert_eq!(app.widgets.sidebar.selected(), Some(project_index));
}

async fn type_chars(app: &mut App, input: &str) {
    for ch in input.chars() {
        app.handle_overlay_key(key(KeyCode::Char(ch)))
            .await
            .unwrap();
    }
}

fn assert_pending(app: &App, expected: &[&str]) {
    let expected = expected
        .iter()
        .map(|label| label.to_string())
        .collect::<Vec<_>>();
    assert_eq!(app.view().pending_shortcut, expected);
}

fn assert_pending_empty(app: &App) {
    assert!(app.view().pending_shortcut.is_empty());
}

async fn insert_title_conflict(
    pool: &SqlitePool,
    app: &mut App,
    selected: usize,
    local: &str,
    remote: &str,
) {
    let task_id = app.store.tasks[selected].task.id.clone();
    insert_title_conflict_for_task_id(pool, app, &task_id, local, remote).await;
}

async fn insert_title_conflict_for_task_id(
    pool: &SqlitePool,
    app: &mut App,
    task_id: &str,
    local: &str,
    remote: &str,
) {
    insert_conflict_for_task_id(pool, app, task_id, "title", local, remote).await;
}

async fn insert_conflict_for_task_id(
    pool: &SqlitePool,
    app: &mut App,
    task_id: &str,
    field: &str,
    local: &str,
    remote: &str,
) {
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query(
        "INSERT INTO conflicts(task_id, field, base_version, local_value, remote_value,
         local_change_id, remote_change_id, variant_a, variant_b, created_at, resolved)
         VALUES (?, ?, NULL, ?, ?, NULL, ?, 'a', 'b', ?, 0)",
    )
    .bind(task_id)
    .bind(field)
    .bind(local)
    .bind(remote)
    .bind(crate::ids::new_id())
    .bind(crate::ids::now())
    .execute(&mut *conn)
    .await
    .unwrap();
    drop(conn);
    app.refresh().await.unwrap();
}

mod theme_background {
    use super::*;
    use ratatui::style::Color;

    #[tokio::test]
    async fn tui_background_uses_terminal_background_for_main_surface() {
        let mut app = test_app().await;

        let buf = render_app_buffer(&mut app, 120, 30);

        assert_eq!(buf[(119, 10)].bg, Color::Reset);
    }
}

mod keyboard_dispatch {
    use super::*;

    #[tokio::test]
    async fn ctrl_c_quits_from_normal_mode() {
        let mut app = test_app().await;
        app.dispatch_key(ctrl_c(), (80, 24).into()).await.unwrap();
        assert!(app.should_quit);
    }

    #[tokio::test]
    async fn ctrl_c_quits_while_overlay_captures_input() {
        let mut app = test_app().await;
        app.begin_search();
        app.dispatch_key(ctrl_c(), (80, 24).into()).await.unwrap();
        assert!(app.should_quit);
    }

    #[tokio::test]
    async fn prefix_key_enters_prefix_mode() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        assert_pending(&app, &["t"]);
    }

    #[tokio::test]
    async fn add_task_alias_executes_immediately() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        assert_pending_empty(&app);
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state)) if state.focus == AddTaskStep::Title
        ));
    }

    #[tokio::test]
    async fn normal_dispatch_ignores_modified_shortcuts() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Priority target")).await;

        app.dispatch_key(ctrl_p(), (80, 24).into()).await.unwrap();

        assert!(app.overlay.is_none());
        assert_pending_empty(&app);

        app.dispatch_key(key(KeyCode::Char('e')), (80, 24).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('p')), (80, 24).into())
            .await
            .unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Picker(state)) if state.title == EDIT_PRIORITY_TITLE
        ));
    }

    #[tokio::test]
    async fn prefix_is_inactive_while_overlay_captures_input() {
        let mut app = test_app().await;
        app.begin_search();
        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();

        assert_pending_empty(&app);
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Search(state)) if state.input.as_str() == "t"
        ));
    }

    #[tokio::test]
    async fn esc_cancels_prefix_before_overlay() {
        let mut app = test_app().await;
        app.overlay = Some(OverlayState::Detail { scroll: 0 });
        app.dispatch_key(key(KeyCode::Char('t')), (80, 24).into())
            .await
            .unwrap();
        assert_pending(&app, &["t"]);
        app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
            .await
            .unwrap();
        assert_pending_empty(&app);
        assert!(matches!(
            app.overlay,
            Some(OverlayState::Detail { scroll: 0 })
        ));

        app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
            .await
            .unwrap();
        assert!(app.overlay.is_none());
    }

    #[tokio::test]
    async fn invalid_continuation_shows_message() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('z')).await.unwrap();
        assert_pending_empty(&app);
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("invalid shortcut: t z")
        );
    }

    #[tokio::test]
    async fn valid_continuation_executes_and_clears() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        assert_pending_empty(&app);
    }

    #[tokio::test]
    async fn order_shortcut_sets_sort() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('o')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('p')).await.unwrap();
        assert_eq!(app.store.view_state.order, TaskOrder::Priority);
        assert_eq!(toast_message(&app).as_deref(), Some("order priority asc"));
    }

    #[tokio::test]
    async fn created_order_shortcut_defaults_to_descending() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('o')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
        assert_eq!(app.store.view_state.order, TaskOrder::Created);
        assert_eq!(app.store.sort_direction_label(), "desc");
        assert_eq!(toast_message(&app).as_deref(), Some("order created desc"));
    }

    #[tokio::test]
    async fn order_reverse_shortcut_toggles_direction() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('o')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('r')).await.unwrap();
        assert_eq!(app.store.sort_direction_label(), "desc");
        assert_eq!(toast_message(&app).as_deref(), Some("order created desc"));
    }

    #[tokio::test]
    async fn due_order_shortcut_reports_unsupported() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('o')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('d')).await.unwrap();
        assert_eq!(
            toast_message(&app).as_deref(),
            Some(":order-due is disabled: tasks do not have due dates")
        );
    }

    #[tokio::test]
    async fn h_and_l_move_between_sidebar_and_tasks() {
        let mut app = test_app().await;
        app.focus = Focus::Tasks;
        app.handle_normal_key(KeyCode::Char('h')).await.unwrap();
        assert_eq!(app.focus, Focus::Sidebar);

        app.handle_normal_key(KeyCode::Char('l')).await.unwrap();
        assert_eq!(app.focus, Focus::Tasks);
    }

    #[tokio::test]
    async fn planned_and_disabled_shortcut_and_command_report_non_executing() {
        let mut app = test_app().await;

        app.handle_normal_key(KeyCode::Char('o')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('d')).await.unwrap();
        assert_eq!(
            toast_message(&app).as_deref(),
            Some(":order-due is disabled: tasks do not have due dates")
        );

        app.begin_command();
        type_chars(&mut app, "order-due").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        assert_eq!(
            toast_message(&app).as_deref(),
            Some(":order-due is disabled: tasks do not have due dates")
        );
        assert!(app.overlay.is_none());
    }

    #[tokio::test]
    async fn esc_closes_every_overlay_variant() {
        let overlays = vec![
            OverlayState::Help { scroll: 0 },
            OverlayState::Detail { scroll: 0 },
            OverlayState::DetailHelp { scroll: 0 },
            OverlayState::Search(SearchState {
                input: LineEdit::new("q".to_string()),
                results: Vec::new(),
                selected: 0,
                total_matches: 0,
            }),
            OverlayState::Command {
                state: CommandState::new(LineEdit::new("ref".to_string())),
            },
            OverlayState::TextInput(TextInputState::new(
                OverlayRoute::MessageOnly,
                "T",
                "P",
                "x".to_string(),
            )),
            OverlayState::MultilineInput(MultilineInputState {
                route: OverlayRoute::MessageOnly,
                title: "M".to_string(),
                prompt: "P".to_string(),
                lines: vec!["x".to_string()],
                row: 0,
                column: 1,
            }),
            OverlayState::Picker(PickerState {
                route: OverlayRoute::MessageOnly,
                title: "Pick".to_string(),
                filter: LineEdit::blank(),
                items: vec![PickerItem {
                    label: "One".to_string(),
                    value: "one".to_string(),
                    selected: false,
                }],
                selected: 0,
                scroll: 0,
                multi: false,
                mode: PickerMode::Navigate,
            }),
            OverlayState::Confirm(ConfirmState {
                route: OverlayRoute::MessageOnly,
                title: "C".to_string(),
                prompt: "?".to_string(),
            }),
            OverlayState::TextPanel(TextPanelState {
                title: "Panel".to_string(),
                lines: vec!["line".to_string()],
                scroll: 0,
            }),
            OverlayState::SyncStatus(Box::default()),
        ];

        for overlay in overlays {
            let detail_help = matches!(overlay, OverlayState::DetailHelp { .. });
            let mut app = test_app().await;
            app.overlay = Some(overlay);
            app.dispatch_key(key(KeyCode::Esc), (80, 24).into())
                .await
                .unwrap();
            if detail_help {
                assert!(matches!(
                    app.overlay,
                    Some(OverlayState::Detail { scroll: 0 })
                ));
            } else {
                assert!(app.overlay.is_none());
            }
            assert_pending_empty(&app);
        }
    }
}

mod command_and_config_overlays {
    use super::*;

    #[tokio::test]
    async fn command_overlay_executes_unique_lookup_and_keeps_overlay_on_errors() {
        let mut app = test_app().await;

        app.begin_command();
        for ch in "ref".chars() {
            app.handle_overlay_key(key(KeyCode::Char(ch)))
                .await
                .unwrap();
        }
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        assert!(app.overlay.is_none());

        app.begin_command();
        app.handle_overlay_key(key(KeyCode::Char('s')))
            .await
            .unwrap();
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        assert!(matches!(app.overlay, Some(OverlayState::Command { .. })));
        assert_eq!(toast_message(&app).as_deref(), Some("ambiguous command: s"));

        app.begin_command();
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
    async fn command_overlay_tab_completes_unique_suffix_alias() {
        let mut app = test_app().await;

        app.begin_command();
        type_chars(&mut app, ":todo").await;
        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::Command { state })
                if state.input.text == "status-todo" && state.input.cursor == "status-todo".len()
        ));
    }

    #[tokio::test]
    async fn command_overlay_tab_cycles_ambiguous_matches() {
        let mut app = test_app().await;

        app.begin_command();
        type_chars(&mut app, "stat").await;
        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Command { state })
                if state.input.text == "status-picker"
                    && state.input.cursor == "status-picker".len()
                    && state.cycle_input.as_deref() == Some("stat")
                    && state.highlighted.as_deref() == Some("status-picker")
        ));

        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Command { state })
                if state.input.text == "status-inbox"
                    && state.input.cursor == "status-inbox".len()
                    && state.cycle_input.as_deref() == Some("stat")
                    && state.highlighted.as_deref() == Some("status-inbox")
        ));
    }

    #[tokio::test]
    async fn command_overlay_single_completion_keeps_highlight_on_next_tab() {
        let mut app = test_app().await;

        app.begin_command();
        type_chars(&mut app, ":todo").await;
        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Command { state })
                if state.input.text == "status-todo"
                    && state.highlighted.as_deref() == Some("status-todo")
        ));
    }

    #[tokio::test]
    async fn command_overlay_edit_resets_completion_cycle() {
        let mut app = test_app().await;

        app.begin_command();
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
    async fn search_overlay_shows_live_results_and_navigation() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("needle first")).await;
        create_and_select_task(&mut app, test_task_draft("needle second")).await;

        app.begin_search();
        type_chars(&mut app, "needle").await;

        let Some(OverlayState::Search(state)) = &app.overlay else {
            panic!("expected search overlay");
        };
        assert_eq!(state.results.len(), 2);
        assert_eq!(state.selected, 0);
        assert!(
            state
                .results
                .iter()
                .any(|result| result.title == "needle first")
        );
        assert!(
            state
                .results
                .iter()
                .any(|result| result.title == "needle second")
        );

        app.handle_overlay_key(key(KeyCode::Down)).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Search(state)) if state.selected == 1
        ));
    }

    #[tokio::test]
    async fn search_overlay_allows_j_and_k_text_input() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("keyboard needle")).await;

        app.begin_search();
        type_chars(&mut app, "jk").await;

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Search(state)) if state.input.as_str() == "jk"
        ));
    }

    #[tokio::test]
    async fn search_overlay_ctrl_n_and_ctrl_p_select_results() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("needle first")).await;
        create_and_select_task(&mut app, test_task_draft("needle second")).await;

        app.begin_search();
        type_chars(&mut app, "needle").await;
        app.handle_overlay_key(ctrl_n()).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Search(state)) if state.selected == 1
        ));

        app.handle_overlay_key(ctrl_p()).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Search(state)) if state.selected == 0
        ));
    }

    #[tokio::test]
    async fn search_overlay_refreshes_results_after_paste() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("pasted needle")).await;

        app.begin_search();
        app.dispatch_paste("needle").await.unwrap();

        let Some(OverlayState::Search(state)) = &app.overlay else {
            panic!("expected search overlay");
        };
        assert_eq!(state.input.as_str(), "needle");
        assert!(
            state
                .results
                .iter()
                .any(|result| result.title == "pasted needle")
        );
    }

    #[tokio::test]
    async fn search_overlay_enter_opens_selected_task_detail() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("detail needle")).await;

        app.begin_search();
        type_chars(&mut app, "needle").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::Detail { scroll: 0 })
        ));
        let selected = app.widgets.table.selected().unwrap();
        assert_eq!(app.store.tasks[selected].task.title, "detail needle");
    }

    #[tokio::test]
    async fn search_overlay_tab_opens_results_list() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("list needle")).await;

        app.begin_search();
        type_chars(&mut app, "needle").await;
        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();

        assert!(app.overlay.is_none());
        assert_eq!(app.store.tasks.len(), 1);
        assert_eq!(app.store.tasks[0].task.title, "list needle");
    }

    #[tokio::test]
    async fn search_replaces_existing_overlay() {
        let mut app = test_app().await;
        app.overlay = Some(OverlayState::Help { scroll: 0 });
        app.begin_search();
        assert!(matches!(app.overlay, Some(OverlayState::Search(_))));
    }

    #[tokio::test]
    async fn toggle_help_closes_active_help_overlay() {
        let mut app = test_app().await;
        app.overlay = Some(OverlayState::Help { scroll: 0 });
        app.toggle_help_at_height(24);
        assert!(app.overlay.is_none());
    }

    #[tokio::test]
    async fn help_key_opens_help_overlay() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('?')).await.unwrap();
        assert!(matches!(app.overlay, Some(OverlayState::Help { .. })));
    }

    #[tokio::test]
    async fn config_info_opens_text_panel() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('C')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('c')).await.unwrap();

        let Some(OverlayState::TextPanel(panel)) = app.overlay else {
            panic!("expected text panel");
        };
        assert_eq!(panel.title, CONFIG_INFO_TITLE);
        assert!(panel.lines.iter().any(|line| line.contains("config path:")));
    }

    #[tokio::test]
    async fn config_status_opens_sync_status() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('C')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('s')).await.unwrap();

        let Some(OverlayState::SyncStatus(status)) = app.overlay else {
            panic!("expected sync status");
        };
        assert_eq!(*status, app.store.sync_status);
    }

    #[tokio::test]
    async fn config_paths_opens_text_panel() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('C')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('d')).await.unwrap();

        let Some(OverlayState::TextPanel(panel)) = app.overlay else {
            panic!("expected text panel");
        };
        assert_eq!(panel.title, CONFIG_PATHS_TITLE);
        assert!(
            panel
                .lines
                .iter()
                .any(|line| line.contains("effective database:"))
        );
    }

    #[tokio::test]
    async fn database_stats_opens_text_panel() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Stats target")).await;
        let selected = app.widgets.table.selected();
        app.store.update_status(selected, "done").await.unwrap();
        create_and_select_task(
            &mut app,
            TaskDraft {
                title: "Urgent task".to_string(),
                description: String::new(),
                project: None,
                status: "inbox".to_string(),
                priority: "urgent".to_string(),
                labels: Vec::new(),
            },
        )
        .await;
        app.store
            .update_deleted(app.widgets.table.selected(), true)
            .await
            .unwrap();

        app.handle_normal_key(KeyCode::Char('C')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('D')).await.unwrap();

        let Some(OverlayState::DatabaseStats { stats, scroll }) = app.overlay else {
            panic!("expected database stats");
        };
        assert_eq!(scroll, 0);
        assert_eq!(stats.total_tasks, 2);
        assert_eq!(stats.open_tasks, 0);
        assert_eq!(stats.deleted_tasks, 1);
        assert_eq!(stats.statuses.done, 1);
        assert_eq!(stats.priorities.urgent, 0);
        assert_eq!(stats.notes, 0);
        assert!(stats.sqlite_page_size > 0);
        assert!(stats.sqlite_page_count > 0);
    }

    #[tokio::test]
    async fn command_panel_runs_database_stats() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Stats target")).await;

        app.begin_command();
        type_chars(&mut app, "database-stats").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::DatabaseStats { .. })
        ));
    }

    #[tokio::test]
    async fn config_init_requires_confirmation() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('C')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('i')).await.unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::Confirm(ConfirmState { ref title, .. })) if title == CONFIG_INIT_TITLE
        ));
    }

    #[tokio::test]
    async fn config_init_cancel_does_not_set_success_message() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('C')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('i')).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Char('n')))
            .await
            .unwrap();
        assert!(app.overlay.is_none());
        assert!(app.notification.is_none());
    }

    #[tokio::test]
    async fn command_panel_runs_config_show() {
        let mut app = test_app().await;
        app.begin_command();
        type_chars(&mut app, "config-show").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::TextPanel(TextPanelState { ref title, .. })) if title == CONFIG_INFO_TITLE
        ));
    }

    #[tokio::test]
    async fn command_panel_runs_workspace_switch() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        let mut conn = pool.acquire().await.unwrap();
        crate::workspaces::create_workspace(&mut conn, "Client Work")
            .await
            .unwrap();
        drop(conn);

        app.begin_command();
        type_chars(&mut app, "workspace-switch").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Picker(PickerState { title, items, .. }))
                if title == SWITCH_WORKSPACE_TITLE
                    && items.iter().any(|item| item.value == "client-work")
        ));

        reset_default_workspace(&pool).await;
    }
}

mod filters_and_workspaces {
    use super::*;

    #[tokio::test]
    async fn scope_project_shortcut_opens_project_picker() {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();

        app.handle_normal_key(KeyCode::Char('g')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('p')).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Picker(PickerState { title, .. })) if title == SCOPE_PROJECT_TITLE
        ));
    }

    #[tokio::test]
    async fn done_view_shortcut_keeps_project_scope() {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();
        app.store.create_project("Ops".to_string()).await.unwrap();
        for (title, project) in [("Mobile done", "mobile-app"), ("Ops done", "ops")] {
            let (_, selected) = app
                .store
                .create_task(
                    TaskDraft {
                        title: title.to_string(),
                        description: String::new(),
                        project: Some(project.to_string()),
                        status: "inbox".to_string(),
                        priority: "none".to_string(),
                        labels: Vec::new(),
                    },
                    None,
                )
                .await
                .unwrap();
            app.store.update_status(selected, "done").await.unwrap();
        }
        let selected = app
            .store
            .show_scope(TaskScopeTarget::Project("mobile-app".to_string()))
            .await
            .unwrap();
        app.apply_filter_selection(selected);

        app.handle_normal_key(KeyCode::Char('v')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('d')).await.unwrap();

        assert_eq!(
            app.store.view_state.scope,
            TaskScope::Project("mobile-app".to_string())
        );
        assert_eq!(app.store.view_state.view, TaskView::Done);
        assert_eq!(app.store.tasks.len(), 1);
        assert_eq!(app.store.tasks[0].task.title, "Mobile done");
        assert_eq!(toast_message(&app).as_deref(), Some("view updated"));
    }

    #[tokio::test]
    async fn filter_shortcuts_apply_label_status_priority_and_deleted() {
        let mut app = test_app().await;
        app.store.create_label("backend".to_string()).await.unwrap();
        create_and_select_task(
            &mut app,
            TaskDraft {
                title: "Filtered task".to_string(),
                description: String::new(),
                project: None,
                status: "inbox".to_string(),
                priority: "urgent".to_string(),
                labels: vec!["backend".to_string()],
            },
        )
        .await;

        app.handle_normal_key(KeyCode::Char('f')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('l')).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Char('/')))
            .await
            .unwrap();
        type_chars(&mut app, "backend").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        assert_eq!(
            app.store.view_state.filter_modifiers.label.as_deref(),
            Some("backend")
        );
        assert_eq!(toast_message(&app).as_deref(), Some("label filter applied"));
        app.handle_normal_key(KeyCode::Char('f')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('p')).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Char('/')))
            .await
            .unwrap();
        type_chars(&mut app, "urgent").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        assert_eq!(
            app.store.view_state.filter_modifiers.priority.as_deref(),
            Some("urgent")
        );

        app.handle_normal_key(KeyCode::Char('f')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('x')).await.unwrap();
        assert!(app.store.view_state.filter_modifiers.include_deleted);
        assert!(!app.store.view_state.filter_modifiers.deleted_only);
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("showing deleted tasks")
        );

        app.handle_normal_key(KeyCode::Char('f')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('x')).await.unwrap();
        assert!(app.store.view_state.filter_modifiers.include_deleted);
        assert!(app.store.view_state.filter_modifiers.deleted_only);
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("showing deleted tasks only")
        );
    }

    #[tokio::test]
    async fn switch_workspace_shortcut_opens_picker() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        let mut conn = pool.acquire().await.unwrap();
        crate::workspaces::create_workspace(&mut conn, "Client Work")
            .await
            .unwrap();
        drop(conn);

        app.handle_normal_key(KeyCode::Char('g')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('w')).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Picker(PickerState { title, .. })) if title == SWITCH_WORKSPACE_TITLE
        ));

        reset_default_workspace(&pool).await;
    }

    #[tokio::test]
    async fn refresh_reports_invalid_project_scope_fallback() {
        let mut app = test_app().await;
        app.store.view_state.scope = TaskScope::Project("missing".to_string());
        app.store.view_state.view = TaskView::Todo;

        app.refresh().await.unwrap();

        assert_eq!(app.store.view_state.scope, TaskScope::Workspace);
        assert_eq!(app.store.view_state.view, TaskView::Todo);
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("project scope missing is no longer available")
        );
        assert_eq!(toast_severity(&app), Some(ToastSeverity::Warning));
    }

    #[tokio::test]
    async fn switch_workspace_changes_active_workspace() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        create_and_select_task(&mut app, test_task_draft("Default only")).await;

        let mut conn = pool.acquire().await.unwrap();
        crate::workspaces::create_workspace(&mut conn, "Client Work")
            .await
            .unwrap();
        drop(conn);
        app.refresh().await.unwrap();

        app.store.view_state.view = TaskView::Todo;

        let (message, selected) = app
            .store
            .switch_workspace("client-work".to_string())
            .await
            .unwrap();
        app.apply_filter_selection(selected);
        app.set_success(message);

        assert_eq!(app.store.active_workspace.key, "client-work");
        assert_eq!(app.store.view_state.view, TaskView::Todo);
        assert!(app.store.tasks.is_empty());
        assert!(app.overlay.is_none());
        assert!(
            toast_message(&app)
                .is_some_and(|message| message.contains("switched workspace to client-work"))
        );

        reset_default_workspace(&pool).await;
    }

    #[tokio::test]
    async fn clear_filters_shortcut_resets_default_view() {
        let mut app = test_app().await;
        app.store.view_state.view = TaskView::Todo;

        app.handle_normal_key(KeyCode::Char('f')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('c')).await.unwrap();

        assert_eq!(app.store.view_state.view, TaskView::Todo);
        assert_eq!(toast_message(&app).as_deref(), Some("filters cleared"));
        assert_eq!(toast_severity(&app), Some(ToastSeverity::Success));
    }

    #[tokio::test]
    async fn go_conflicts_shortcut_sets_conflicts_view() {
        let mut app = test_app().await;

        app.handle_normal_key(KeyCode::Char('v')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('c')).await.unwrap();

        assert_eq!(app.store.view_state.view, TaskView::Conflicts);
        assert_eq!(app.store.view_state.view, TaskView::Conflicts);
    }

    #[tokio::test]
    async fn header_click_opens_scope_menu_and_selects_scope() {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();
        let selected = app
            .store
            .show_scope(TaskScopeTarget::Project("mobile-app".to_string()))
            .await
            .unwrap();
        app.apply_filter_selection(selected);

        app.dispatch_mouse(header_click(36), (140, 24).into())
            .await
            .unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::HeaderMenu(state))
                if state.column == 28
                    && state.row == 0
                    && state.items.iter().any(|item| item.label == "workspace")
                    && state.items.iter().any(|item| item.label.contains("Mobile App"))
        ));

        app.dispatch_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 30,
                row: 2,
                modifiers: KeyModifiers::NONE,
            },
            (140, 24).into(),
        )
        .await
        .unwrap();
        assert_eq!(app.store.view_state.scope, TaskScope::Workspace);
        assert_eq!(toast_message(&app).as_deref(), Some("scope updated"));
    }

    #[tokio::test]
    async fn header_click_opens_view_menu_and_selects_view() {
        let mut app = test_app().await;

        app.dispatch_mouse(header_click(58), (140, 24).into())
            .await
            .unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::HeaderMenu(state))
                if state.column == 48
                    && state.row == 0
                    && state.items.iter().any(|item| item.label == "inbox")
        ));

        app.dispatch_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 50,
                row: 5,
                modifiers: KeyModifiers::NONE,
            },
            (140, 24).into(),
        )
        .await
        .unwrap();
        assert_eq!(app.store.view_state.view, TaskView::Inbox);
        assert!(app.overlay.is_none());
        assert_eq!(toast_message(&app).as_deref(), Some("view updated"));
    }

    #[tokio::test]
    async fn header_click_opens_workspace_menu_and_switches_workspace() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        let mut conn = pool.acquire().await.unwrap();
        crate::workspaces::create_workspace(&mut conn, "Client Work")
            .await
            .unwrap();
        drop(conn);

        app.dispatch_mouse(header_click(10), (140, 24).into())
            .await
            .unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::HeaderMenu(state))
                if state.column == 8
                    && state.row == 0
                    && state.items.iter().any(|item| item.label.contains("Client Work"))
        ));

        app.dispatch_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 10,
                row: 3,
                modifiers: KeyModifiers::NONE,
            },
            (140, 24).into(),
        )
        .await
        .unwrap();
        assert_eq!(app.store.active_workspace.key, "client-work");
        assert!(app.overlay.is_none());
        assert!(
            toast_message(&app)
                .is_some_and(|message| message.contains("switched workspace to client-work"))
        );

        reset_default_workspace(&pool).await;
    }

    #[tokio::test]
    async fn header_metric_click_still_selects_view_directly() {
        let mut app = test_app().await;

        app.dispatch_mouse(header_click(65), (140, 24).into())
            .await
            .unwrap();
        assert_eq!(app.store.view_state.view, TaskView::Queue);
        assert_eq!(toast_message(&app).as_deref(), Some("view updated"));

        let mut app = test_app().await;
        let inbox_column = (0..140)
            .find(|column| {
                matches!(
                    crate::tui::ui::header_target_at(
                        &app.store,
                        ratatui::layout::Rect::new(0, 0, 140, 2),
                        *column,
                        0,
                    ),
                    Some(crate::tui::ui::HeaderTarget::MetricView(TaskView::Inbox))
                )
            })
            .unwrap();
        app.dispatch_mouse(header_click(inbox_column), (140, 24).into())
            .await
            .unwrap();
        assert_eq!(app.store.view_state.view, TaskView::Inbox);
    }

    #[tokio::test]
    async fn header_click_opens_sync_status() {
        let mut app = test_app().await;

        app.dispatch_mouse(header_click(135), (140, 24).into())
            .await
            .unwrap();

        let Some(OverlayState::SyncStatus(status)) = app.overlay else {
            panic!("expected sync status");
        };
        assert_eq!(*status, app.store.sync_status);
    }

    #[tokio::test]
    async fn header_click_opens_order_menu_and_selects_order() {
        let mut app = test_app().await;

        app.dispatch_mouse(header_click(127), (140, 24).into())
            .await
            .unwrap();
        assert!(matches!(
            app.overlay,
            Some(OverlayState::OrderMenu(state))
                if state.column == 114 && state.row == 0 && state.selected == TaskOrder::Created
        ));

        app.dispatch_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 130,
                row: 5,
                modifiers: KeyModifiers::NONE,
            },
            (140, 24).into(),
        )
        .await
        .unwrap();

        assert_eq!(app.store.view_state.view, TaskView::Open);
        assert_eq!(app.store.view_state.order, TaskOrder::Project);
        assert!(app.overlay.is_none());
        assert_eq!(toast_message(&app).as_deref(), Some("order project asc"));
    }

    #[tokio::test]
    async fn header_click_ignores_capturing_overlay() {
        let mut app = test_app().await;
        app.begin_search();

        app.dispatch_mouse(header_click(45), (140, 24).into())
            .await
            .unwrap();

        assert_eq!(app.store.view_state.view, TaskView::Queue);
        assert!(matches!(app.overlay, Some(OverlayState::Search(_))));
    }

    #[tokio::test]
    async fn header_home_click_closes_detail_overlay() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Detail target")).await;
        app.overlay = Some(OverlayState::Detail { scroll: 3 });

        app.dispatch_mouse(header_click(2), (140, 24).into())
            .await
            .unwrap();

        assert!(app.overlay.is_none());
        assert!(!app.detail_context);
        assert_eq!(app.widgets.table.selected(), Some(0));
    }

    #[tokio::test]
    async fn header_home_click_closes_detail_underlay() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Detail target")).await;
        app.detail_context = true;
        app.overlay = Some(OverlayState::Picker(PickerState {
            route: OverlayRoute::MessageOnly,
            title: "Pick".to_string(),
            filter: LineEdit::blank(),
            items: vec![PickerItem {
                label: "One".to_string(),
                value: "one".to_string(),
                selected: false,
            }],
            selected: 0,
            scroll: 0,
            multi: false,
            mode: PickerMode::Navigate,
        }));

        app.dispatch_mouse(header_click(2), (140, 24).into())
            .await
            .unwrap();

        assert!(app.overlay.is_none());
        assert!(!app.detail_context);
        assert_eq!(app.widgets.table.selected(), Some(0));
    }

    #[tokio::test]
    async fn mouse_wheel_moves_task_selection_down_and_up() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("first")).await;
        create_and_select_task(&mut app, test_task_draft("second")).await;
        app.widgets.table.select(Some(0));

        app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), (80, 24).into())
            .await
            .unwrap();
        assert_eq!(app.widgets.table.selected(), Some(1));

        app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollUp), (80, 24).into())
            .await
            .unwrap();
        assert_eq!(app.widgets.table.selected(), Some(0));
    }

    #[tokio::test]
    async fn mouse_wheel_stops_at_task_list_edges() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("first")).await;
        create_and_select_task(&mut app, test_task_draft("second")).await;
        app.widgets.table.select(Some(0));

        app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollUp), (80, 24).into())
            .await
            .unwrap();
        assert_eq!(app.widgets.table.selected(), Some(0));

        app.widgets.table.select(Some(1));
        app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), (80, 24).into())
            .await
            .unwrap();
        assert_eq!(app.widgets.table.selected(), Some(1));
    }

    #[tokio::test]
    async fn mouse_wheel_ignored_with_overlay() {
        let mut app = test_app().await;
        let _ = create_and_select_task(&mut app, test_task_draft("task")).await;
        app.begin_search();
        let selected = app.widgets.table.selected();

        app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), (80, 24).into())
            .await
            .unwrap();

        assert_eq!(app.widgets.table.selected(), selected);
        assert!(matches!(app.overlay, Some(OverlayState::Search(_))));
    }

    #[tokio::test]
    async fn mouse_wheel_ignored_in_sidebar_focus() {
        let mut app = test_app().await;
        let _ = create_and_select_task(&mut app, test_task_draft("task")).await;
        app.focus = Focus::Sidebar;

        app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), (80, 24).into())
            .await
            .unwrap();

        assert_eq!(app.widgets.table.selected(), Some(0));
        assert_eq!(app.focus, Focus::Sidebar);
    }

    #[tokio::test]
    async fn mouse_wheel_ignored_with_detail_underlay() {
        let mut app = test_app().await;
        let _ = create_and_select_task(&mut app, test_task_draft("task")).await;
        app.detail_context = true;

        app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), (80, 24).into())
            .await
            .unwrap();

        assert_eq!(app.widgets.table.selected(), Some(0));
    }

    #[tokio::test]
    async fn mouse_wheel_ignored_for_small_terminal() {
        let mut app = test_app().await;
        let _ = create_and_select_task(&mut app, test_task_draft("task")).await;

        app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), (69, 24).into())
            .await
            .unwrap();
        assert_eq!(app.widgets.table.selected(), Some(0));

        app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), (80, 17).into())
            .await
            .unwrap();
        assert_eq!(app.widgets.table.selected(), Some(0));
    }

    #[tokio::test]
    async fn picker_row_click_submits_clicked_row() {
        let mut app = test_app().await;
        let size = (100, 24).into();
        app.begin_filter_priority();

        app.dispatch_mouse(picker_row_click(&app, 2, size), size)
            .await
            .unwrap();

        assert_eq!(
            app.store.view_state.filter_modifiers.priority.as_deref(),
            Some("medium")
        );
        assert!(app.overlay.is_none());
    }

    #[tokio::test]
    async fn picker_row_click_toggles_multi_select_row() {
        let mut app = test_app().await;
        let size = (100, 24).into();
        app.store.create_label("bug".to_string()).await.unwrap();
        create_and_select_task(
            &mut app,
            TaskDraft {
                labels: vec!["bug".to_string()],
                ..test_task_draft("Labeled target")
            },
        )
        .await;
        app.begin_edit_labels();

        app.dispatch_mouse(picker_row_click(&app, 0, size), size)
            .await
            .unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::TagCombobox(state))
                if state.options.iter().any(|item| item == "bug")
                    && state.selected.iter().any(|item| item == "bug")
        ));
    }

    #[tokio::test]
    async fn confirm_hint_click_confirms_and_cancels() {
        let mut app = test_app().await;
        let size = (100, 24).into();
        app.overlay = Some(OverlayState::Confirm(ConfirmState {
            route: OverlayRoute::MessageOnly,
            title: "Confirm".to_string(),
            prompt: "Continue?".to_string(),
        }));

        app.dispatch_mouse(confirm_hint_click(&app, 0, size), size)
            .await
            .unwrap();

        assert!(app.overlay.is_none());
        assert_eq!(toast_message(&app).as_deref(), Some("confirmed overlay"));

        let mut app = test_app().await;
        app.overlay = Some(OverlayState::Confirm(ConfirmState {
            route: OverlayRoute::MessageOnly,
            title: "Confirm".to_string(),
            prompt: "Continue?".to_string(),
        }));

        app.dispatch_mouse(confirm_hint_click(&app, 7, size), size)
            .await
            .unwrap();

        assert!(app.overlay.is_none());
        assert!(app.notification.is_none());
    }

    #[tokio::test]
    async fn text_panel_mouse_scrolls_and_closes_outside() {
        let mut app = test_app().await;
        let size = (100, 24).into();
        app.overlay = Some(OverlayState::TextPanel(TextPanelState::new(
            "Panel",
            (0..20).map(|index| format!("line {index}")).collect(),
        )));

        app.dispatch_mouse(wheel_down(50, 12), size).await.unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::TextPanel(TextPanelState { scroll: 1, .. }))
        ));

        app.dispatch_mouse(left_click(0, 0), size).await.unwrap();

        assert!(app.overlay.is_none());
    }
}

mod task_row_mouse {
    use super::*;
    use ratatui::layout::Rect;

    fn task_list_area(size: (u16, u16)) -> Rect {
        let (width, height) = size;
        let body = Rect::new(0, 2, width, height.saturating_sub(4));
        if body.width < 100 {
            body
        } else {
            let sidebar_width = body.width.min(26);
            Rect::new(
                sidebar_width,
                body.y,
                body.width.saturating_sub(sidebar_width),
                body.height,
            )
        }
    }

    fn row_column_task_click_event(size: (u16, u16), viewport_row: u16) -> MouseEvent {
        let task_area = task_list_area(size);
        task_row_click(task_area.x + 1, task_area.y + 1 + viewport_row)
    }

    fn status_right_click_event(app: &App, size: (u16, u16), task_index: usize) -> MouseEvent {
        let task_area = task_list_area(size);
        let table = &app.widgets.table;
        for row in task_area.y..task_area.y.saturating_add(task_area.height) {
            for column in task_area.x..task_area.x.saturating_add(task_area.width) {
                if crate::tui::ui::task_status_at_position(
                    &app.store, table, task_area, column, row,
                )
                .is_some_and(|hit| hit.task_index == task_index)
                {
                    return right_click(column, row);
                }
            }
        }
        panic!("expected status hit target");
    }

    #[tokio::test]
    async fn task_row_click_selects_task() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("task one")).await;

        let click = row_column_task_click_event((80, 24), 1);
        app.dispatch_mouse(click, (80, 24).into()).await.unwrap();

        assert_eq!(app.widgets.table.selected(), Some(0));
        assert_eq!(app.focus, Focus::Tasks);
        assert!(app.overlay.is_none());
    }

    #[tokio::test]
    async fn status_right_click_opens_status_menu_for_clicked_task() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("task one")).await;
        create_and_select_task(&mut app, test_task_draft("task two")).await;
        app.widgets.table.select(Some(1));

        let click = status_right_click_event(&app, (140, 24), 0);
        app.dispatch_mouse(click, (140, 24).into()).await.unwrap();

        assert_eq!(app.widgets.table.selected(), Some(0));
        assert_eq!(app.focus, Focus::Tasks);
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::HeaderMenu(state))
                if state.kind == crate::tui::overlay::HeaderMenuKind::Status
                    && state.column == click.column
                    && state.row == click.row
                    && state.items.iter().any(|item| item.label == "inbox" && item.selected)
        ));
    }

    #[tokio::test]
    async fn status_right_click_ignores_non_status_columns() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("task one")).await;
        create_and_select_task(&mut app, test_task_draft("task two")).await;
        app.widgets.table.select(Some(1));

        let click = row_column_task_click_event((140, 24), 1);
        app.dispatch_mouse(right_click(click.column, click.row), (140, 24).into())
            .await
            .unwrap();

        assert_eq!(app.widgets.table.selected(), Some(1));
        assert!(app.overlay.is_none());
    }

    #[tokio::test]
    async fn status_right_click_reuses_status_update_and_undo() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("task one")).await;

        let size = (140, 24).into();
        let click = status_right_click_event(&app, (140, 24), 0);
        app.dispatch_mouse(click, size).await.unwrap();
        let Some(OverlayState::HeaderMenu(state)) = app.overlay.as_ref() else {
            panic!("expected status menu");
        };
        let area = state.area(size.width, size.height);
        app.dispatch_mouse(left_click(area.x + 1, area.y + 4), size)
            .await
            .unwrap();

        assert_eq!(app.store.tasks[0].task.status, "active");
        assert!(toast_message(&app).is_some_and(|message| message.ends_with("status=active")));

        app.handle_normal_key(KeyCode::Char('u')).await.unwrap();
        assert_eq!(app.store.tasks[0].task.status, "inbox");
        assert!(toast_message(&app).is_some_and(|message| message.contains("undid")));
    }

    #[tokio::test]
    async fn task_row_click_opens_detail_on_double_click() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("task one")).await;

        let click = row_column_task_click_event((80, 24), 1);
        app.dispatch_mouse(click, (80, 24).into()).await.unwrap();
        assert_eq!(app.widgets.table.selected(), Some(0));
        assert!(app.overlay.is_none());
        assert!(app.last_task_click.is_some());

        app.dispatch_mouse(click, (80, 24).into()).await.unwrap();
        assert!(app.last_task_click.is_none());
        assert_eq!(app.widgets.table.selected(), Some(0));
        assert!(matches!(
            app.overlay,
            Some(OverlayState::Detail { scroll: 0 })
        ));
    }

    #[tokio::test]
    async fn task_row_click_wide_layout_respects_sidebar_offset() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("task one")).await;
        create_and_select_task(&mut app, test_task_draft("task two")).await;
        app.widgets.table.select(Some(1));
        app.focus = Focus::Tasks;

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
        assert_eq!(app.widgets.table.selected(), Some(1));
        assert_eq!(app.focus, Focus::Tasks);

        let click = row_column_task_click_event((140, 24), 1);
        app.dispatch_mouse(click, (140, 24).into()).await.unwrap();

        assert_eq!(app.widgets.table.selected(), Some(0));
        assert_eq!(app.focus, Focus::Tasks);
    }

    #[tokio::test]
    async fn task_row_click_preview_area_miss_is_ignored() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("task one")).await;
        create_and_select_task(&mut app, test_task_draft("task two")).await;
        app.widgets.table.select(Some(1));

        let task_area = task_list_area((140, 40));
        let preview_row = task_area.y + task_area.height.saturating_sub(3);
        let click = task_row_click(task_area.x + 1, preview_row);
        app.dispatch_mouse(click, (140, 40).into()).await.unwrap();

        assert_eq!(app.widgets.table.selected(), Some(1));
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

        assert_eq!(app.widgets.table.selected(), Some(0));
        assert!(app.overlay.is_none());
    }

    #[tokio::test]
    async fn task_row_click_ignores_narrow_sidebar_overlay() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("task one")).await;
        app.focus = Focus::Sidebar;

        let overlay = crate::tui::ui::sidebar_layout(Rect::new(0, 0, 80, 40), Focus::Sidebar)
            .expect("sidebar overlay should exist in narrow layout")
            .sidebar;
        let click = task_row_click(overlay.x + 1, overlay.y + 1);
        app.dispatch_mouse(click, (80, 40).into()).await.unwrap();

        assert_eq!(app.widgets.table.selected(), Some(0));
        assert!(app.overlay.is_none());
        assert_eq!(app.focus, Focus::Sidebar);
    }
}

mod authoring {
    use super::*;

    #[tokio::test]
    async fn add_task_shortcut_opens_title_prompt() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('n')).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state))
                if state.focus == AddTaskStep::Title && state.title.as_str().is_empty()
        ));
    }

    #[tokio::test]
    async fn add_task_alias_creates_task_after_title() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        type_chars(&mut app, "Write docs").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(app.overlay.is_none());
        let selected = app.widgets.table.selected().unwrap();
        let task = &app.store.tasks[selected];
        assert_eq!(task.task.title, "Write docs");
        assert_eq!(task.task.status, "inbox");
        assert_eq!(task.task.priority, "none");
        assert_eq!(task.task.description, "");
        assert!(task.labels.is_empty());
    }

    #[tokio::test]
    async fn add_task_status_hotkey_selects_direct_status() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        type_chars(&mut app, "Write docs").await;

        app.handle_overlay_key(ctrl_t()).await.unwrap();

        assert_pending(&app, &["t"]);
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state)) if state.status == "inbox"
        ));
        assert!(matches!(
            app.view().overlay,
            Some(OverlayView::AddTask(state)) if state.status_prefix_active
        ));
        assert_eq!(toast_message(&app), None);

        app.handle_overlay_key(key(KeyCode::Char('a')))
            .await
            .unwrap();

        assert_pending_empty(&app);
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state)) if state.status == "active"
        ));
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("add task status=active")
        );

        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(app.overlay.is_none());
        let selected = app.widgets.table.selected().unwrap();
        assert_eq!(app.store.tasks[selected].task.title, "Write docs");
        assert_eq!(app.store.tasks[selected].task.status, "active");
    }

    #[tokio::test]
    async fn add_task_priority_hotkey_selects_direct_priority() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        type_chars(&mut app, "Fix release").await;

        app.handle_overlay_key(ctrl_r()).await.unwrap();

        assert_pending(&app, &["r"]);
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state)) if state.priority == "none"
        ));
        assert!(matches!(
            app.view().overlay,
            Some(OverlayView::AddTask(state)) if state.priority_prefix_active
        ));
        assert_eq!(toast_message(&app), None);

        app.handle_overlay_key(key(KeyCode::Char('h')))
            .await
            .unwrap();

        assert_pending_empty(&app);
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state)) if state.priority == "high"
        ));
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("add task priority=high")
        );

        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(app.overlay.is_none());
        let selected = app.widgets.table.selected().unwrap();
        assert_eq!(app.store.tasks[selected].task.title, "Fix release");
        assert_eq!(app.store.tasks[selected].task.priority, "high");
    }

    #[tokio::test]
    async fn add_task_start_view_keeps_main_surface() {
        let mut app = test_app().await;
        app.open_add_task_on_start(false).await.unwrap();

        let view = app.view();

        assert_eq!(view.surface, ViewSurface::Main);
        assert!(matches!(app.overlay, Some(OverlayState::AddTask(_))));
    }

    #[tokio::test]
    async fn add_task_only_view_uses_popup_surface() {
        let mut app = test_app().await;
        app.add_task_only = true;
        app.begin_add_task().await.unwrap();

        let view = app.view();

        assert_eq!(view.surface, ViewSurface::AddTask);
    }

    #[tokio::test]
    async fn add_task_only_render_skips_normal_tui() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Existing queue task")).await;
        app.add_task_only = true;
        app.begin_add_task().await.unwrap();

        let rendered = render_app_text(&mut app, 50, 12);

        assert!(rendered.contains("Add task"));
        assert!(rendered.contains("Enter title here"));
        assert!(!rendered.contains("terminal too small for aven tui"));
        assert!(!rendered.contains("Existing queue task"));
    }

    #[tokio::test]
    async fn add_task_only_natural_render_uses_popup_surface() {
        let mut app = test_app().await;
        app.add_task_only = true;
        app.begin_add_task().await.unwrap();
        app.begin_add_task_natural();

        let rendered = render_app_text(&mut app, 50, 12);

        assert!(rendered.contains("Add task: natural language"));
        assert!(rendered.contains("Describe the task in natural language"));
        assert!(rendered.contains("^S parse"));
        assert!(!rendered.contains("terminal too small for aven tui"));
    }

    #[tokio::test]
    async fn add_task_uses_active_project_view() {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();
        let selected = app
            .store
            .show_scope(TaskScopeTarget::Project("mobile-app".to_string()))
            .await
            .unwrap();
        app.apply_filter_selection(selected);

        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        type_chars(&mut app, "Write docs").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        let selected = app.widgets.table.selected().unwrap();
        let task = &app.store.tasks[selected];
        assert_eq!(task.task.title, "Write docs");
        assert_eq!(task.task.project_key, "mobile-app");
        assert_eq!(
            app.store.view_state.scope,
            TaskScope::Project("mobile-app".to_string())
        );
    }

    #[tokio::test]
    async fn add_task_flow_configures_project_and_priority_from_title() {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();

        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('n')).await.unwrap();
        type_chars(&mut app, "Write docs").await;
        app.handle_overlay_key(ctrl_p()).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Picker(state)) if state.route == OverlayRoute::AddTaskTitleProject
        ));
        type_chars(&mut app, "mobile").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        app.handle_overlay_key(ctrl_r()).await.unwrap();

        assert_pending(&app, &["r"]);
        app.handle_overlay_key(key(KeyCode::Char('h')))
            .await
            .unwrap();
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(app.overlay.is_none());
        let selected = app.widgets.table.selected().unwrap();
        let task = &app.store.tasks[selected];
        assert_eq!(task.task.title, "Write docs");
        assert_eq!(task.task.project_key, "mobile-app");
        assert_eq!(task.task.priority, "high");
        assert_eq!(task.task.description, "");
        assert!(task.labels.is_empty());
    }

    #[tokio::test]
    async fn add_task_tab_opens_description_step() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        type_chars(&mut app, "Write docs").await;
        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state))
                if state.focus == AddTaskStep::Description
                    && state.title.as_str() == "Write docs"
        ));
    }

    #[tokio::test]
    async fn add_task_picker_escape_returns_to_add_task_only_dialog() {
        let mut app = test_app().await;
        app.add_task_only = true;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        type_chars(&mut app, "Write docs").await;
        app.handle_overlay_key(ctrl_p()).await.unwrap();

        app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();

        assert!(!app.should_quit);
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state)) if state.title.as_str() == "Write docs"
        ));
    }

    #[tokio::test]
    async fn add_task_description_flow_creates_task_with_description() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        type_chars(&mut app, "Write docs").await;
        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
        type_chars(&mut app, "Include setup details").await;
        app.handle_overlay_key(ctrl_s()).await.unwrap();

        assert!(app.overlay.is_none());
        let selected = app.widgets.table.selected().unwrap();
        let task = &app.store.tasks[selected];
        assert_eq!(task.task.title, "Write docs");
        assert_eq!(task.task.description, "Include setup details");
    }

    #[tokio::test]
    async fn add_task_description_ctrl_x_ctrl_e_opens_external_editor_and_returns_to_composer() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        type_chars(&mut app, "Write docs").await;
        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
        type_chars(&mut app, "Details").await;
        app.handle_overlay_key(ctrl_x()).await.unwrap();
        app.handle_overlay_key(ctrl_e()).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state))
                if state.focus == AddTaskStep::Description
                    && state.title.as_str() == "Write docs"
                    && state.description.lines == vec!["Details from editor".to_string()]
        ));
    }

    #[tokio::test]
    async fn add_task_description_ctrl_x_non_editor_key_clears_prefix_and_edits_text() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
        app.handle_overlay_key(ctrl_x()).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Char('z')))
            .await
            .unwrap();

        assert_pending_empty(&app);
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state))
                if state.focus == AddTaskStep::Description
                    && state.description.lines == vec!["z".to_string()]
        ));
    }

    #[tokio::test]
    async fn add_task_description_ctrl_e_moves_to_line_end() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        type_chars(&mut app, "Write docs").await;
        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
        type_chars(&mut app, "Details").await;
        app.handle_overlay_key(ctrl_e()).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state))
                if state.focus == AddTaskStep::Description
                    && state.description.column == "Details".len()
                    && state.description.lines == vec!["Details".to_string()]
        ));
    }

    #[tokio::test]
    async fn add_task_project_and_priority_return_to_description_step() {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();

        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        type_chars(&mut app, "Write docs").await;
        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
        type_chars(&mut app, "Details").await;
        app.handle_overlay_key(ctrl_p()).await.unwrap();
        type_chars(&mut app, "mobile").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state))
                if state.focus == AddTaskStep::Description
                    && state.description.lines == vec!["Details".to_string()]
        ));

        app.handle_overlay_key(ctrl_r()).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Char('h')))
            .await
            .unwrap();
        app.handle_overlay_key(ctrl_s()).await.unwrap();

        let selected = app.widgets.table.selected().unwrap();
        let task = &app.store.tasks[selected];
        assert_eq!(task.task.project_key, "mobile-app");
        assert_eq!(task.task.priority, "high");
        assert_eq!(task.task.description, "Details");
    }

    #[tokio::test]
    async fn idle_poll_timeout_uses_refresh_deadline() {
        let app = test_app().await;

        let timeout = app.next_poll_timeout();

        assert!(timeout <= std::time::Duration::from_secs(5));
        assert!(timeout > std::time::Duration::from_secs(4));
        assert!(!app.has_time_based_redraw());
    }

    #[tokio::test]
    async fn refresh_attempt_schedules_next_deadline() {
        let mut app = test_app().await;
        app.next_refresh_at = std::time::Instant::now() - std::time::Duration::from_secs(1);

        assert!(app.refresh_is_due());
        app.schedule_next_refresh();

        assert!(!app.refresh_is_due());
        assert!(app.refresh_timeout() <= std::time::Duration::from_secs(5));
        assert!(app.refresh_timeout() > std::time::Duration::from_secs(4));
    }

    #[tokio::test]
    async fn loading_poll_timeout_uses_spinner_cadence() {
        let mut app = test_app().await;
        app.notification = Some(Notification::loading("parsing task with LLM"));

        assert_eq!(
            app.next_poll_timeout(),
            std::time::Duration::from_millis(120)
        );
        assert!(app.has_time_based_redraw());
        assert_eq!(toast_severity(&app), Some(ToastSeverity::Info));
        assert!(toast_message(&app).is_some_and(|message| {
            message.contains("parsing task with LLM") && !message.contains("•")
        }));
    }

    #[tokio::test]
    async fn toast_expiry_clears_message_once() {
        let mut app = test_app().await;
        app.set_success("created task APP-TEST");
        let first_timeout = app.next_poll_timeout();
        app.notification = Some(Notification::Toast {
            toast: crate::tui::toast::Toast::new("created task APP-TEST", ToastSeverity::Success),
            created_at: std::time::Instant::now() - std::time::Duration::from_secs(5),
        });

        assert!(first_timeout <= std::time::Duration::from_secs(4));
        assert!(app.clear_expired_notification());
        assert!(app.notification.is_none());
        assert!(!app.clear_expired_notification());
    }

    #[tokio::test]
    async fn unfinished_task_intake_poll_does_not_request_redraw() {
        let mut app = test_app().await;
        let handle = tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            Ok(test_task_draft("pending task"))
        });
        app.notification = Some(Notification::loading("adding task with LLM"));
        app.pending_task_intake = Some(PendingTaskIntake {
            handle,
            retry: NaturalRetry::AddTask,
            value: "pending task".to_string(),
            create_on_success: true,
        });

        assert!(!app.poll_pending_task_intake().await.unwrap());
        let pending = app.pending_task_intake.take().unwrap();
        pending.handle.abort();
    }

    #[tokio::test]
    async fn finished_task_intake_poll_requests_redraw() {
        let mut app = test_app().await;
        let handle = tokio::spawn(async { Ok(test_task_draft("ready task")) });
        app.notification = Some(Notification::loading("adding task with LLM"));
        app.pending_task_intake = Some(PendingTaskIntake {
            handle,
            retry: NaturalRetry::AddTask,
            value: "ready task".to_string(),
            create_on_success: true,
        });

        for _ in 0..100 {
            if app
                .pending_task_intake
                .as_ref()
                .unwrap()
                .handle
                .is_finished()
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }

        assert!(app.poll_pending_task_intake().await.unwrap());
        assert!(app.pending_task_intake.is_none());
        assert!(app.ready_task_intake.is_some());
        assert!(matches!(
            app.notification.as_ref(),
            Some(Notification::Loading { message, .. }) if message == "adding task with LLM"
        ));

        assert!(app.poll_pending_task_intake().await.unwrap());
        assert!(app.ready_task_intake.is_none());
        assert!(toast_message(&app).is_some_and(|message| message.starts_with("created task ")));
    }

    #[tokio::test]
    async fn add_task_ctrl_n_creates_task_in_background_in_full_tui() {
        let mut app = test_app().await;
        configure_task_intake(
            &mut app,
            "parse-title.sh",
            r#"{"title":"fix parsed dispatch","description":"from parsed title","project":null,"priority":"medium","labels":[]}"#,
        );

        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        type_chars(&mut app, "in slack-agent fix dispatch").await;
        app.handle_overlay_key(ctrl_n()).await.unwrap();

        assert!(matches!(
            app.notification.as_ref(),
            Some(Notification::Loading { message, .. }) if message == "adding task with LLM"
        ));
        assert!(app.overlay.is_none());
        assert!(
            toast_message(&app).is_some_and(|message| message.contains("adding task with LLM"))
        );
        for _ in 0..500 {
            app.poll_pending_task_intake().await.unwrap();
            if toast_message(&app).is_some_and(|message| message.starts_with("created task ")) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        let selected = app.widgets.table.selected().unwrap();
        let task = &app.store.tasks[selected];
        assert_eq!(task.task.title, "fix parsed dispatch");
        assert_eq!(task.task.description, "from parsed title");
        assert_eq!(task.task.priority, "medium");
        assert!(toast_message(&app).is_some_and(|message| message.starts_with("created task ")));
    }

    #[tokio::test]
    async fn add_task_ctrl_n_from_description_sends_title_and_description() {
        let mut app = test_app().await;
        let capture = configure_task_intake_capture(
            &mut app,
            "parse-description.sh",
            r#"{"title":"parsed docs task","description":"parsed handoff","project":null,"priority":"none","labels":[]}"#,
        );

        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        type_chars(&mut app, "Write docs").await;
        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();
        type_chars(&mut app, "Include setup details").await;
        app.handle_overlay_key(ctrl_n()).await.unwrap();

        for _ in 0..500 {
            app.poll_pending_task_intake().await.unwrap();
            if capture.exists()
                && toast_message(&app).is_some_and(|message| message.starts_with("created task "))
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        let prompt = std::fs::read_to_string(capture).unwrap();
        assert!(prompt.contains(
            "Raw intake text:\nTitle:\nWrite docs\n\nDescription:\nInclude setup details"
        ));
        let selected = app.widgets.table.selected().unwrap();
        let task = &app.store.tasks[selected];
        assert_eq!(task.task.title, "parsed docs task");
        assert_eq!(task.task.description, "parsed handoff");
    }

    #[tokio::test]
    async fn add_task_only_ctrl_n_exits_immediately() {
        let mut app = test_app().await;
        app.add_task_only = true;

        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        type_chars(&mut app, "add from popup").await;
        app.handle_overlay_key(ctrl_n()).await.unwrap();

        assert!(app.should_quit);
        assert!(app.pending_task_intake.is_none());
        assert!(app.overlay.is_none());
        assert_eq!(
            app.add_task_only_message.as_deref(),
            Some("adding task in background")
        );
    }

    #[tokio::test]
    async fn add_task_only_natural_dialog_submit_exits_immediately() {
        let mut app = test_app().await;
        app.add_task_only = true;

        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        app.begin_add_task_natural();
        type_chars(&mut app, "dialog add from popup").await;
        app.handle_overlay_key(ctrl_s()).await.unwrap();

        assert!(app.should_quit);
        assert!(app.pending_task_intake.is_none());
        assert!(app.overlay.is_none());
        assert_eq!(
            app.add_task_only_message.as_deref(),
            Some("adding task in background")
        );
    }

    #[tokio::test]
    async fn add_task_ctrl_n_error_reopens_add_task_dialog() {
        let mut app = test_app().await;
        configure_task_intake_failure(&mut app, "parse-title-fail.sh");

        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        type_chars(&mut app, "raw natural title").await;
        app.handle_overlay_key(ctrl_n()).await.unwrap();
        for _ in 0..100 {
            app.poll_pending_task_intake().await.unwrap();
            if matches!(&app.overlay, Some(OverlayState::AddTask(state)) if state.title.as_str() == "raw natural title")
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state))
                if state.title.as_str() == "raw natural title"
        ));
        assert!(toast_message(&app).is_some_and(|message| {
            message.contains("task intake failed") && message.contains("logged to")
        }));
    }

    fn configure_task_intake(app: &mut App, script_name: &str, output: &str) {
        let dir = tempfile::tempdir().unwrap().keep();
        let command = dir.join(script_name);
        std::fs::write(
            &command,
            format!("#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{}'\n", output),
        )
        .unwrap();
        set_executable(&command);
        app.add_task_config.agent.task_intake.command = Some(command.display().to_string());
        app.add_task_config.agent.task_intake.args = Vec::new();
        app.add_task_config.agent.task_intake.timeout_seconds = Some(5);
    }

    fn configure_task_intake_capture(
        app: &mut App,
        script_name: &str,
        output: &str,
    ) -> std::path::PathBuf {
        let dir = tempfile::tempdir().unwrap().keep();
        let command = dir.join(script_name);
        let capture = dir.join("prompt.txt");
        std::fs::write(
            &command,
            format!(
                "#!/bin/sh\ncat > '{}'\nprintf '%s\\n' '{}'\n",
                capture.display(),
                output
            ),
        )
        .unwrap();
        set_executable(&command);
        app.add_task_config.agent.task_intake.command = Some(command.display().to_string());
        app.add_task_config.agent.task_intake.args = Vec::new();
        app.add_task_config.agent.task_intake.timeout_seconds = Some(5);
        capture
    }

    fn configure_task_intake_failure(app: &mut App, script_name: &str) {
        let dir = tempfile::tempdir().unwrap().keep();
        let command = dir.join(script_name);
        std::fs::write(&command, "#!/bin/sh\ncat >/dev/null\nexit 1\n").unwrap();
        set_executable(&command);
        app.add_task_config.agent.task_intake.command = Some(command.display().to_string());
        app.add_task_config.agent.task_intake.args = Vec::new();
        app.add_task_config.agent.task_intake.timeout_seconds = Some(5);
    }

    #[cfg(unix)]
    fn set_executable(path: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).unwrap();
    }

    #[cfg(not(unix))]
    fn set_executable(_path: &std::path::Path) {}

    #[tokio::test]
    async fn add_task_flow_cancels_at_title_step() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('n')).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
        assert!(app.overlay.is_none());
    }

    #[tokio::test]
    async fn add_task_blank_title_is_rejected() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('n')).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("task title is required")
        );
        assert_eq!(toast_severity(&app), Some(ToastSeverity::Warning));
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::AddTask(state)) if state.focus == AddTaskStep::Title
        ));
    }

    #[tokio::test]
    async fn add_note_requires_selected_task() {
        let mut app = test_app().await;
        app.widgets.table.select(None);
        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('N')).await.unwrap();

        assert!(app.overlay.is_none());
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("no selected task for note")
        );
        assert_eq!(toast_severity(&app), Some(ToastSeverity::Info));
    }

    #[tokio::test]
    async fn add_note_alias_requires_selected_task() {
        let mut app = test_app().await;
        app.widgets.table.select(None);
        app.handle_normal_key(KeyCode::Char('n')).await.unwrap();

        assert!(app.overlay.is_none());
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("no selected task for note")
        );
    }

    #[tokio::test]
    async fn add_note_flow_creates_note_for_selected_task() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Note target")).await;

        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('N')).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::MultilineInput(state)) if state.title == ADD_NOTE_TITLE
        ));

        type_chars(&mut app, "Important detail").await;
        app.handle_overlay_key(ctrl_s()).await.unwrap();

        assert!(app.overlay.is_none());
        assert!(toast_message(&app).is_some_and(|message| message.starts_with("added note ")));
    }
}

mod detail_mode {
    use super::*;

    #[tokio::test]
    async fn q_closes_detail_overlay() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Detail target")).await;
        app.overlay = Some(OverlayState::Detail { scroll: 0 });

        app.dispatch_key(key(KeyCode::Char('q')), (80, 24).into())
            .await
            .unwrap();

        assert!(app.overlay.is_none());
        assert!(!app.should_quit);
    }

    #[tokio::test]
    async fn help_key_opens_detail_help_from_detail_overlay() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Detail help target")).await;
        app.overlay = Some(OverlayState::Detail { scroll: 0 });

        app.dispatch_key(key(KeyCode::Char('?')), (80, 24).into())
            .await
            .unwrap();

        assert!(matches!(app.overlay, Some(OverlayState::DetailHelp { .. })));
        assert_eq!(app.focus, Focus::Tasks);
        assert!(app.widgets.table.selected().is_some());
    }

    #[tokio::test]
    async fn closing_detail_help_returns_to_detail_overlay() {
        let mut app = test_app().await;
        app.overlay = Some(OverlayState::DetailHelp { scroll: 0 });

        app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::Detail { scroll: 0 })
        ));
    }

    #[tokio::test]
    async fn second_help_key_returns_from_detail_help_to_detail_overlay() {
        let mut app = test_app().await;
        app.overlay = Some(OverlayState::DetailHelp { scroll: 0 });

        app.dispatch_key(key(KeyCode::Char('?')), (80, 24).into())
            .await
            .unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::Detail { scroll: 0 })
        ));
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
        app.overlay = Some(OverlayState::Detail { scroll: 0 });

        app.dispatch_key(ctrl_d(), (80, 24).into()).await.unwrap();
        assert!(matches!(
            app.overlay,
            Some(OverlayState::Detail { scroll: 18 })
        ));

        app.dispatch_key(key(KeyCode::PageDown), (80, 24).into())
            .await
            .unwrap();
        assert!(matches!(
            app.overlay,
            Some(OverlayState::Detail { scroll: 36 })
        ));

        app.dispatch_key(ctrl_u(), (80, 24).into()).await.unwrap();
        assert!(matches!(
            app.overlay,
            Some(OverlayState::Detail { scroll: 18 })
        ));

        app.dispatch_key(key(KeyCode::Char('k')), (80, 24).into())
            .await
            .unwrap();
        assert!(matches!(
            app.overlay,
            Some(OverlayState::Detail { scroll: 17 })
        ));

        app.dispatch_key(key(KeyCode::PageUp), (80, 24).into())
            .await
            .unwrap();
        assert!(matches!(
            app.overlay,
            Some(OverlayState::Detail { scroll: 0 })
        ));
    }

    #[tokio::test]
    async fn detail_scroll_resists_down_input_at_bottom() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Short detail")).await;
        app.overlay = Some(OverlayState::Detail { scroll: 0 });

        for _ in 0..10 {
            app.dispatch_key(key(KeyCode::Char('j')), (80, 24).into())
                .await
                .unwrap();
        }
        assert!(matches!(
            app.overlay,
            Some(OverlayState::Detail { scroll: 0 })
        ));

        app.dispatch_key(key(KeyCode::Char('k')), (80, 24).into())
            .await
            .unwrap();
        assert!(matches!(
            app.overlay,
            Some(OverlayState::Detail { scroll: 0 })
        ));
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
        app.overlay = Some(OverlayState::Detail { scroll: 0 });

        app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), (80, 24).into())
            .await
            .unwrap();
        assert!(matches!(
            app.overlay,
            Some(OverlayState::Detail { scroll: 1 })
        ));

        app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollUp), (80, 24).into())
            .await
            .unwrap();
        assert!(matches!(
            app.overlay,
            Some(OverlayState::Detail { scroll: 0 })
        ));
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
        app.overlay = Some(OverlayState::Detail { scroll: 0 });

        for _ in 0..200 {
            app.dispatch_mouse(mouse_wheel(MouseEventKind::ScrollDown), (80, 24).into())
                .await
                .unwrap();
        }

        assert!(matches!(
            app.overlay,
            Some(OverlayState::Detail { scroll }) if scroll == expected
        ));
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
            Some(OverlayState::TextPanel(panel)) => {
                crate::tui::ui::text_panel_scroll_cap(&panel.lines)
            }
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

    #[tokio::test]
    async fn detail_next_and_previous_task_stay_in_detail() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("First")).await;
        create_and_select_task(&mut app, test_task_draft("Second")).await;
        let first = app
            .store
            .tasks
            .iter()
            .position(|item| item.task.title == "First")
            .unwrap();
        let second = app
            .store
            .tasks
            .iter()
            .position(|item| item.task.title == "Second")
            .unwrap();
        app.widgets.table.select(Some(first));
        app.overlay = Some(OverlayState::Detail { scroll: 7 });

        app.dispatch_key(key(KeyCode::Char(']')), (80, 24).into())
            .await
            .unwrap();
        assert_eq!(app.widgets.table.selected(), Some(second));
        assert!(matches!(
            app.overlay,
            Some(OverlayState::Detail { scroll: 0 })
        ));
        assert_eq!(toast_message(&app).as_deref(), Some("selected next task"));

        app.dispatch_key(key(KeyCode::Char('[')), (80, 24).into())
            .await
            .unwrap();
        assert_eq!(app.widgets.table.selected(), Some(first));
        assert!(matches!(
            app.overlay,
            Some(OverlayState::Detail { scroll: 0 })
        ));
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("selected previous task")
        );
    }

    #[tokio::test]
    async fn add_note_from_detail_returns_to_detail() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Note target")).await;
        app.overlay = Some(OverlayState::Detail { scroll: 0 });

        app.dispatch_key(key(KeyCode::Char('t')), (80, 24).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('N')), (80, 24).into())
            .await
            .unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::MultilineInput(state)) if state.title == ADD_NOTE_TITLE
        ));
        assert!(app.view().detail_underlay);

        type_chars(&mut app, "Important detail").await;
        app.handle_overlay_key(ctrl_s()).await.unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::Detail { scroll: 0 })
        ));
        let selected = app.widgets.table.selected().unwrap();
        assert_eq!(app.store.tasks[selected].notes.len(), 1);
    }

    #[tokio::test]
    async fn detail_shortcuts_do_not_leave_detail_before_opening_overlay() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Detail target")).await;
        app.overlay = Some(OverlayState::Detail { scroll: 0 });

        app.dispatch_key(key(KeyCode::Char('e')), (80, 24).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('p')), (80, 24).into())
            .await
            .unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Picker(PickerState { title, .. })) if title == EDIT_PRIORITY_TITLE
        ));
        assert!(app.view().detail_underlay);
    }

    #[tokio::test]
    async fn edit_title_from_detail_renders_inline_cursor() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Detail title target")).await;
        app.overlay = Some(OverlayState::Detail { scroll: 5 });

        app.dispatch_key(key(KeyCode::Char('t')), (100, 30).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('e')), (100, 30).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('t')), (100, 30).into())
            .await
            .unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::TextInput(state)) if state.route == OverlayRoute::EditTitle
        ));
        assert!(app.view().detail_underlay);
        assert_eq!(app.view().detail_underlay_scroll, 0);

        let rendered = render_app_text(&mut app, 100, 30);

        assert!(rendered.contains("Detail title target"));
        assert!(!rendered.contains("Edit title"));
        assert!(!rendered.contains("Enter submit"));
    }

    #[tokio::test]
    async fn cancel_edit_title_from_detail_returns_to_detail() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Detail title target")).await;
        app.overlay = Some(OverlayState::Detail { scroll: 0 });

        app.dispatch_key(key(KeyCode::Char('t')), (100, 30).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('e')), (100, 30).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('t')), (100, 30).into())
            .await
            .unwrap();
        app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::Detail { scroll: 0 })
        ));
        assert!(app.authoring.is_idle());
    }

    #[tokio::test]
    async fn submit_edit_title_from_detail_returns_to_detail() {
        let mut app = test_app().await;
        let selected =
            create_and_select_task(&mut app, test_task_draft("Detail title target")).await;
        app.overlay = Some(OverlayState::Detail { scroll: 0 });

        app.dispatch_key(key(KeyCode::Char('t')), (100, 30).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('e')), (100, 30).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('t')), (100, 30).into())
            .await
            .unwrap();
        app.handle_overlay_key(key(KeyCode::End)).await.unwrap();
        type_chars(&mut app, " updated").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::Detail { scroll: 0 })
        ));
        assert_eq!(
            app.store.tasks[selected].task.title,
            "Detail title target updated"
        );
    }

    #[tokio::test]
    async fn detail_edit_chords_open_advertised_editors() {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();
        app.store.create_label("Bug".to_string()).await.unwrap();
        create_and_select_task(
            &mut app,
            TaskDraft {
                title: "Detail target".to_string(),
                description: "existing description".to_string(),
                project: None,
                status: "inbox".to_string(),
                priority: "none".to_string(),
                labels: vec!["bug".to_string()],
            },
        )
        .await;

        for (events, expected_route) in [
            (vec![key(KeyCode::Char('s'))], OverlayRoute::EditStatus),
            (
                vec![key(KeyCode::Char('e')), key(KeyCode::Char('p'))],
                OverlayRoute::EditPriority,
            ),
            (
                vec![key(KeyCode::Char('e')), key(KeyCode::Char('l'))],
                OverlayRoute::EditLabels,
            ),
            (vec![shift_key(KeyCode::Char('N'))], OverlayRoute::AddNote),
            (
                vec![shift_key(KeyCode::Char('D'))],
                OverlayRoute::DeleteTaskConfirm,
            ),
        ] {
            app.overlay = Some(OverlayState::Detail { scroll: 4 });
            app.dispatch_key(key(KeyCode::Char('t')), (80, 24).into())
                .await
                .unwrap();
            assert_pending(&app, &["t"]);
            assert!(matches!(
                app.overlay,
                Some(OverlayState::Detail { scroll: 4 })
            ));

            for event in events {
                app.dispatch_key(event, (80, 24).into()).await.unwrap();
            }
            match (&app.overlay, expected_route) {
                (Some(OverlayState::TextInput(state)), route) => assert_eq!(state.route, route),
                (Some(OverlayState::MultilineInput(state)), route) => {
                    assert_eq!(state.route, route)
                }
                (Some(OverlayState::Picker(state)), route) => assert_eq!(state.route, route),
                (Some(OverlayState::TagCombobox(state)), route) => assert_eq!(state.route, route),
                (Some(OverlayState::Confirm(state)), route) => assert_eq!(state.route, route),
                (overlay, route) => panic!("expected {route:?}, got {overlay:?}"),
            }
            assert_pending_empty(&app);
            assert!(app.view().detail_underlay);
            assert_eq!(app.view().detail_underlay_scroll, 4);
        }
    }

    #[tokio::test]
    async fn detail_single_key_edit_shortcuts_still_work() {
        let mut app = test_app().await;
        app.store.create_label("Bug".to_string()).await.unwrap();
        create_and_select_task(&mut app, test_task_draft("Detail target")).await;
        app.overlay = Some(OverlayState::Detail { scroll: 3 });

        app.dispatch_key(key(KeyCode::Char('e')), (80, 24).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('l')), (80, 24).into())
            .await
            .unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::TagCombobox(state)) if state.route == OverlayRoute::EditLabels
        ));
        assert_pending_empty(&app);
        assert!(app.view().detail_underlay);
    }

    #[tokio::test]
    async fn invalid_detail_prefix_stays_in_detail() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Detail target")).await;
        app.overlay = Some(OverlayState::Detail { scroll: 5 });

        app.dispatch_key(key(KeyCode::Char('t')), (80, 24).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('z')), (80, 24).into())
            .await
            .unwrap();

        assert_pending_empty(&app);
        assert!(matches!(
            app.overlay,
            Some(OverlayState::Detail { scroll: 5 })
        ));
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("invalid shortcut: t z")
        );
    }

    #[tokio::test]
    async fn detail_prefix_hints_render_above_detail_overlay() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Detail target")).await;
        app.overlay = Some(OverlayState::Detail { scroll: 0 });

        app.dispatch_key(key(KeyCode::Char('t')), (100, 30).into())
            .await
            .unwrap();

        let rendered = render_app_text(&mut app, 100, 30);

        assert!(rendered.contains("t …"));
        assert!(rendered.contains(":detail-edit-title"));
    }

    #[tokio::test]
    async fn ignored_keys_stay_in_detail() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Detail target")).await;
        app.overlay = Some(OverlayState::Detail { scroll: 0 });

        app.dispatch_key(key(KeyCode::Char('a')), (80, 24).into())
            .await
            .unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::Detail { scroll: 0 })
        ));
        assert!(app.authoring.is_idle());
    }

    #[tokio::test]
    async fn detail_toast_renders_above_detail_overlay() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Toast target")).await;
        app.overlay = Some(OverlayState::Detail { scroll: 0 });
        app.set_success("set APP-TEST status=done");

        let rendered = render_app_text(&mut app, 100, 30);

        assert!(rendered.contains("set APP-TEST status=done"));
    }

    #[tokio::test]
    async fn detail_done_shortcut_keeps_detail_and_sets_message() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Next target")).await;
        let selected = create_and_select_task(&mut app, test_task_draft("Done target")).await;
        let selected_task_id = app.store.tasks[selected].task.id.clone();
        let display_ref = app.store.tasks[selected].display_ref.clone();
        app.overlay = Some(OverlayState::Detail { scroll: 7 });

        app.dispatch_key(key(KeyCode::Char('t')), (80, 24).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('d')), (80, 24).into())
            .await
            .unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::Detail { scroll: 7 })
        ));
        assert_eq!(
            toast_message(&app).as_deref(),
            Some(format!("set {display_ref} status=done").as_str())
        );
        let selected = app.widgets.table.selected().unwrap();
        assert_eq!(app.store.tasks[selected].task.id, selected_task_id);
        assert_eq!(app.store.tasks[selected].task.status, "done");
    }

    #[tokio::test]
    async fn detail_status_picker_done_keeps_same_task() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Next target")).await;
        let selected = create_and_select_task(&mut app, test_task_draft("Done target")).await;
        let selected_task_id = app.store.tasks[selected].task.id.clone();
        app.overlay = Some(OverlayState::Detail { scroll: 4 });

        app.dispatch_key(key(KeyCode::Char('s')), (80, 24).into())
            .await
            .unwrap();
        app.handle_overlay_key(key(KeyCode::Char('/')))
            .await
            .unwrap();
        type_chars(&mut app, "done").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::Detail { scroll: 4 })
        ));
        let selected = app.widgets.table.selected().unwrap();
        assert_eq!(app.store.tasks[selected].task.id, selected_task_id);
        assert_eq!(app.store.tasks[selected].task.status, "done");
    }

    #[tokio::test]
    async fn detail_status_mouse_click_opens_menu_and_returns_to_detail() {
        let mut app = test_app().await;
        let selected =
            create_and_select_task(&mut app, test_task_draft("Status click target")).await;
        app.overlay = Some(OverlayState::Detail { scroll: 3 });

        app.dispatch_mouse(
            detail_metadata_click(crate::tui::ui::DetailMetadataTarget::Status),
            (120, 30).into(),
        )
        .await
        .unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::HeaderMenu(state))
                if state.kind == crate::tui::overlay::HeaderMenuKind::Status
                    && state.items.iter().any(|item| item.label == "inbox" && item.selected)
        ));
        assert!(app.view().detail_underlay);

        app.dispatch_mouse(left_click(90, 13), (120, 30).into())
            .await
            .unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::Detail { scroll: 3 })
        ));
        assert_eq!(app.store.tasks[selected].task.status, "active");
    }

    #[tokio::test]
    async fn detail_status_menu_empty_click_returns_to_detail_without_selecting_task() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Hidden task")).await;
        let selected = create_and_select_task(&mut app, test_task_draft("Visible task")).await;
        app.overlay = Some(OverlayState::Detail { scroll: 0 });

        app.dispatch_mouse(
            detail_metadata_click(crate::tui::ui::DetailMetadataTarget::Status),
            (120, 30).into(),
        )
        .await
        .unwrap();
        app.dispatch_mouse(left_click(110, 20), (120, 30).into())
            .await
            .unwrap();

        assert_eq!(app.widgets.table.selected(), Some(selected));
        assert!(matches!(
            app.overlay,
            Some(OverlayState::Detail { scroll: 0 })
        ));
    }

    #[tokio::test]
    async fn detail_priority_mouse_click_opens_menu_and_returns_to_detail() {
        let mut app = test_app().await;
        let selected = create_and_select_task(
            &mut app,
            TaskDraft {
                priority: "medium".to_string(),
                ..test_task_draft("Priority click target")
            },
        )
        .await;
        app.overlay = Some(OverlayState::Detail { scroll: 3 });

        app.dispatch_mouse(
            detail_metadata_click(crate::tui::ui::DetailMetadataTarget::Priority),
            (120, 30).into(),
        )
        .await
        .unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::HeaderMenu(state))
                if state.kind == crate::tui::overlay::HeaderMenuKind::Priority
                    && state.items.iter().any(|item| item.label == "medium" && item.selected)
        ));
        assert!(app.view().detail_underlay);

        app.dispatch_mouse(left_click(90, 17), (120, 30).into())
            .await
            .unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::Detail { scroll: 3 })
        ));
        assert_eq!(app.store.tasks[selected].task.priority, "urgent");
    }

    #[tokio::test]
    async fn detail_undo_shortcut_reverts_last_mutation() {
        let mut app = test_app().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Before")).await;
        app.store
            .update_title(Some(selected), "After".to_string())
            .await
            .unwrap();
        app.overlay = Some(OverlayState::Detail { scroll: 5 });

        app.dispatch_key(key(KeyCode::Char('u')), (80, 24).into())
            .await
            .unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::Detail { scroll: 5 })
        ));
        assert_eq!(app.store.tasks[selected].task.title, "Before");
        assert!(toast_message(&app).is_some_and(|message| message.contains("undid")));
    }

    #[tokio::test]
    async fn detail_undo_after_status_menu_keeps_task_identity() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Other task")).await;
        let selected = create_and_select_task(&mut app, test_task_draft("Undo target")).await;
        let task_id = app.store.tasks[selected].task.id.clone();
        app.overlay = Some(OverlayState::Detail { scroll: 0 });

        app.dispatch_mouse(
            detail_metadata_click(crate::tui::ui::DetailMetadataTarget::Status),
            (120, 30).into(),
        )
        .await
        .unwrap();
        app.dispatch_mouse(left_click(90, 14), (120, 30).into())
            .await
            .unwrap();
        assert!(matches!(
            app.overlay,
            Some(OverlayState::Detail { scroll: 0 })
        ));

        app.dispatch_key(key(KeyCode::Char('u')), (120, 30).into())
            .await
            .unwrap();

        let selected = app.widgets.table.selected().unwrap();
        assert_eq!(app.store.tasks[selected].task.id, task_id);
        assert_eq!(app.store.tasks[selected].task.status, "inbox");
        assert!(matches!(
            app.overlay,
            Some(OverlayState::Detail { scroll: 0 })
        ));
    }

    #[tokio::test]
    async fn cancel_add_note_from_detail_returns_to_detail() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Note target")).await;
        app.overlay = Some(OverlayState::Detail { scroll: 0 });

        app.dispatch_key(key(KeyCode::Char('t')), (80, 24).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('N')), (80, 24).into())
            .await
            .unwrap();
        app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::Detail { scroll: 0 })
        ));
        assert!(app.authoring.is_idle());
    }

    #[tokio::test]
    async fn add_note_blank_body_from_detail_returns_to_detail() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Note target")).await;
        app.overlay = Some(OverlayState::Detail { scroll: 0 });

        app.dispatch_key(key(KeyCode::Char('t')), (80, 24).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('N')), (80, 24).into())
            .await
            .unwrap();
        app.handle_overlay_key(ctrl_s()).await.unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::Detail { scroll: 0 })
        ));
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("note body is required")
        );
    }
}

mod task_editing {
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
        app.widgets.table.select(None);

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
        app.handle_normal_key(KeyCode::Char('n')).await.unwrap();
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

        let selected = app.widgets.table.selected().unwrap();
        assert_eq!(app.store.tasks[selected].task.title, "Old title updated");
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

        let selected = app.widgets.table.selected().unwrap();
        assert_eq!(
            app.store.tasks[selected].task.description,
            "first\nsecond updated"
        );
    }

    #[tokio::test]
    async fn edit_project_picker_uses_existing_projects_only() {
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
        let selected = app.widgets.table.selected().unwrap();
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
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Picker(state))
                if state.title == EDIT_PRIORITY_TITLE
                    && state.items.iter().any(|item| item.value == "high" && item.selected)
        ));

        app.handle_overlay_key(key(KeyCode::Char('/')))
            .await
            .unwrap();
        type_chars(&mut app, "urgent").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        let selected = app.widgets.table.selected().unwrap();
        assert_eq!(app.store.tasks[selected].task.priority, "urgent");
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

        let selected = app.widgets.table.selected().unwrap();
        assert_eq!(app.store.tasks[selected].labels, vec!["docs".to_string()]);
    }

    #[tokio::test]
    async fn status_picker_alias_updates_selected_task() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Status alias")).await;

        app.handle_normal_key(KeyCode::Char('s')).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Picker(state)) if state.title == EDIT_STATUS_TITLE
        ));
        app.handle_overlay_key(key(KeyCode::Char('/')))
            .await
            .unwrap();
        type_chars(&mut app, "todo").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        let selected = app.widgets.table.selected().unwrap();
        assert_eq!(app.store.tasks[selected].task.status, "todo");
    }

    #[tokio::test]
    async fn done_alias_keeps_selected_row_position() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("First")).await;
        create_and_select_task(&mut app, test_task_draft("Second")).await;
        create_and_select_task(&mut app, test_task_draft("Third")).await;
        let selected = 1;
        let next_title = app.store.tasks[selected + 1].task.title.clone();
        app.widgets.table.select(Some(selected));

        app.handle_normal_key(KeyCode::Char('d')).await.unwrap();

        assert_eq!(app.widgets.table.selected(), Some(selected));
        let selected = app.widgets.table.selected().unwrap();
        assert_eq!(app.store.tasks[selected].task.title, next_title);
        assert_eq!(app.store.tasks[selected].task.status, "inbox");
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
        app.widgets.table.select(Some(selected));

        app.handle_normal_key(KeyCode::Char('d')).await.unwrap();

        assert_eq!(app.widgets.table.selected(), Some(0));
        let selected = app.widgets.table.selected().unwrap();
        assert_eq!(app.store.tasks[selected].task.title, "First");
    }

    #[tokio::test]
    async fn done_and_cancel_aliases_update_selected_task() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Status alias")).await;

        app.handle_normal_key(KeyCode::Char('d')).await.unwrap();
        let selected = app.store.show_view(TaskView::Done).await.unwrap();
        app.widgets.table.select(selected);
        let selected = app.widgets.table.selected().unwrap();
        assert_eq!(app.store.tasks[selected].task.status, "done");

        app.handle_normal_key(KeyCode::Char('x')).await.unwrap();
        let selected = app.store.show_view(TaskView::Done).await.unwrap();
        app.widgets.table.select(selected);
        let selected = app.widgets.table.selected().unwrap();
        assert_eq!(app.store.tasks[selected].task.status, "canceled");
    }

    #[tokio::test]
    async fn exact_priority_shortcut_updates_selected_task() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Priority shortcut")).await;

        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('u')).await.unwrap();

        let selected = app.widgets.table.selected().unwrap();
        assert_eq!(app.store.tasks[selected].task.priority, "urgent");
    }

    #[tokio::test]
    async fn edit_shortcuts_require_selected_task() {
        let mut app = test_app().await;
        app.widgets.table.select(None);

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
    async fn copy_requires_selected_task() {
        let mut app = test_app().await;
        app.widgets.table.select(None);

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
        app.widgets.table.select(Some(selected));

        app.handle_normal_key(KeyCode::Char('d')).await.unwrap();
        assert_eq!(app.widgets.table.selected(), Some(selected));

        app.handle_normal_key(KeyCode::Char('u')).await.unwrap();

        assert_eq!(app.widgets.table.selected(), Some(selected));
        assert_eq!(app.store.tasks[selected].task.status, "inbox");
    }

    #[tokio::test]
    async fn undo_command_reverts_last_mutation() {
        let mut app = test_app().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Before")).await;
        app.store
            .update_title(Some(selected), "After".to_string())
            .await
            .unwrap();

        app.begin_command();
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
}

mod delete_and_restore {
    use super::*;

    #[tokio::test]
    async fn delete_task_opens_confirmation_with_task_context() {
        let mut app = test_app().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Delete target")).await;
        let display_ref = app.store.tasks[selected].display_ref.clone();

        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('D')).await.unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::Confirm(ConfirmState {
                route: OverlayRoute::DeleteTaskConfirm,
                ref title,
                ref prompt,
            })) if title == DELETE_TASK_TITLE
                && prompt.contains(&display_ref)
                && prompt.contains("Delete target")
        ));
        assert!(!app.store.tasks[selected].task.deleted);
    }

    #[tokio::test]
    async fn cancel_delete_task_leaves_task_unchanged() {
        let mut app = test_app().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Keep target")).await;

        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('D')).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();

        assert!(app.overlay.is_none());
        assert!(!app.store.tasks[selected].task.deleted);
        assert!(app.notification.is_none());
    }

    #[tokio::test]
    async fn confirm_delete_task_soft_deletes_selected_task() {
        let mut app = test_app().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Delete target")).await;
        let display_ref = app.store.tasks[selected].display_ref.clone();

        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('D')).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Char('y')))
            .await
            .unwrap();

        let selected = app.widgets.table.selected().unwrap();
        assert!(app.store.tasks[selected].task.deleted);
        assert!(!app.store.view_state.filter_modifiers.include_deleted);
        assert_eq!(
            toast_message(&app).as_deref(),
            Some(format!("deleted {display_ref}").as_str())
        );
    }

    #[tokio::test]
    async fn delete_task_from_detail_returns_to_detail() {
        let mut app = test_app().await;
        let selected =
            create_and_select_task(&mut app, test_task_draft("Detail delete target")).await;
        app.overlay = Some(OverlayState::Detail { scroll: 7 });

        app.dispatch_key(key(KeyCode::Char('t')), (80, 24).into())
            .await
            .unwrap();
        app.dispatch_key(key(KeyCode::Char('D')), (80, 24).into())
            .await
            .unwrap();
        assert!(matches!(
            app.overlay,
            Some(OverlayState::Confirm(ConfirmState {
                route: OverlayRoute::DeleteTaskConfirm,
                ..
            }))
        ));
        assert!(app.view().detail_underlay);

        app.handle_overlay_key(key(KeyCode::Char('y')))
            .await
            .unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::Detail { scroll: 7 })
        ));
        assert!(app.store.tasks[selected].task.deleted);
    }

    #[tokio::test]
    async fn rename_project_opens_project_picker_from_task_focus() {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();

        app.execute(Action::BeginRenameProject).await.unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::Picker(PickerState {
                route: OverlayRoute::RenameProjectPicker,
                ..
            }))
        ));
        assert!(app.notification.is_none());
    }

    #[tokio::test]
    async fn delete_project_opens_project_picker_from_task_focus() {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();

        app.execute(Action::BeginDeleteProject).await.unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::Picker(PickerState {
                route: OverlayRoute::DeleteProjectPicker,
                ..
            }))
        ));
        assert!(app.notification.is_none());
    }

    #[tokio::test]
    async fn delete_project_picker_preselects_sidebar_project() {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();
        app.focus = Focus::Sidebar;
        let project_index = app
            .store
            .sidebar_entries
            .iter()
            .position(|entry| {
                entry.target
                    == Some(SidebarEntryTarget::Scope(TaskScopeTarget::Project(
                        "mobile-app".to_string(),
                    )))
            })
            .unwrap();
        app.widgets.sidebar.select(Some(project_index));

        app.execute(Action::BeginDeleteProject).await.unwrap();

        let Some(OverlayState::Picker(state)) = &app.overlay else {
            panic!("expected project picker");
        };
        assert_eq!(state.items[state.selected].value, "mobile-app");
    }

    #[tokio::test]
    async fn rename_project_submission_updates_selected_project() {
        let mut app = test_app().await;
        app.store
            .create_project("Agent Offload".to_string())
            .await
            .unwrap();

        app.execute(Action::BeginRenameProject).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::TextInput(TextInputState {
                route: OverlayRoute::RenameProjectName,
                ..
            }))
        ));
        app.submit_rename_project("sideagent".to_string())
            .await
            .unwrap();

        assert_eq!(
            toast_message(&app).as_deref(),
            Some("renamed project sideagent prefix=SDG")
        );
        assert!(
            app.store
                .projects
                .iter()
                .any(|project| project.key == "sideagent")
        );
        assert!(app.pending_rename_project.is_none());
    }

    #[tokio::test]
    async fn delete_project_confirmation_removes_selected_project() {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();

        app.execute(Action::BeginDeleteProject).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::TextInput(TextInputState {
                route: OverlayRoute::DeleteProjectNameConfirm,
                ..
            }))
        ));
        app.submit_delete_project_name("mobile-app".to_string())
            .await
            .unwrap();
        assert!(matches!(
            app.overlay,
            Some(OverlayState::Confirm(ConfirmState {
                route: OverlayRoute::DeleteProjectConfirm,
                ..
            }))
        ));
        app.handle_overlay_key(key(KeyCode::Char('y')))
            .await
            .unwrap();

        assert_eq!(
            toast_message(&app).as_deref(),
            Some("deleted project mobile-app")
        );
        assert!(
            !app.store
                .projects
                .iter()
                .any(|project| project.key == "mobile-app")
        );
        assert!(app.pending_delete_project.is_none());
    }

    #[tokio::test]
    async fn delete_project_cancel_clears_pending_state() {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();
        app.focus = Focus::Sidebar;
        let project_index = app
            .store
            .sidebar_entries
            .iter()
            .position(|entry| {
                entry.target
                    == Some(SidebarEntryTarget::Scope(TaskScopeTarget::Project(
                        "mobile-app".to_string(),
                    )))
            })
            .unwrap();
        app.widgets.sidebar.select(Some(project_index));

        app.execute(Action::BeginDeleteProject).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        assert_eq!(app.pending_delete_project.as_deref(), Some("mobile-app"));
        assert!(matches!(
            app.overlay,
            Some(OverlayState::TextInput(TextInputState {
                route: OverlayRoute::DeleteProjectNameConfirm,
                ..
            }))
        ));
        app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();

        assert!(app.pending_delete_project.is_none());
    }
}

mod conflicts {
    use super::*;

    #[tokio::test]
    async fn conflict_list_shortcut_applies_conflicts_view() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('l')).await.unwrap();
        assert_eq!(app.store.view_state.view, TaskView::Conflicts);
        assert_eq!(app.store.view_state.view, TaskView::Conflicts);
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("no unresolved conflicts")
        );
    }

    #[tokio::test]
    async fn conflict_show_opens_text_panel_and_esc_closes() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Conflict show")).await;
        insert_title_conflict(&pool, &mut app, selected, "local title", "remote title").await;

        app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('s')).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::TextPanel(state))
                if state.lines.iter().any(|line| line.contains("field=title"))
        ));

        app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
        assert!(app.overlay.is_none());
    }

    #[tokio::test]
    async fn conflict_next_selects_next_conflicted_task() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        create_and_select_task(&mut app, test_task_draft("First")).await;
        create_and_select_task(&mut app, test_task_draft("Second")).await;
        let first_id = app
            .store
            .tasks
            .iter()
            .find(|item| item.task.title == "First")
            .unwrap()
            .task
            .id
            .clone();
        let second_id = app
            .store
            .tasks
            .iter()
            .find(|item| item.task.title == "Second")
            .unwrap()
            .task
            .id
            .clone();
        insert_title_conflict_for_task_id(&pool, &mut app, &first_id, "local one", "remote one")
            .await;
        insert_title_conflict_for_task_id(&pool, &mut app, &second_id, "local two", "remote two")
            .await;
        let first = app
            .store
            .tasks
            .iter()
            .position(|item| item.task.id == first_id)
            .unwrap();
        let second = app
            .store
            .tasks
            .iter()
            .position(|item| item.task.id == second_id)
            .unwrap();
        app.widgets.table.select(Some(first));

        app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('n')).await.unwrap();
        assert_eq!(app.widgets.table.selected(), Some(second));
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("selected next conflict")
        );
    }

    #[tokio::test]
    async fn accept_local_conflict_resolves_after_confirmation() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Before")).await;
        insert_title_conflict(&pool, &mut app, selected, "local title", "remote title").await;

        app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Confirm(state)) if state.title == CONFLICT_CONFIRM_LOCAL_TITLE
        ));

        app.handle_overlay_key(key(KeyCode::Char('y')))
            .await
            .unwrap();
        assert!(app.overlay.is_none());
        assert_eq!(app.store.tasks[selected].task.title, "local title");
        assert!(!app.store.tasks[selected].has_conflict);
        assert!(
            toast_message(&app).is_some_and(
                |message| message.contains("resolved") && message.contains("field=title")
            )
        );
    }

    #[tokio::test]
    async fn accept_remote_conflict_resolves_after_confirmation() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Before")).await;
        insert_title_conflict(&pool, &mut app, selected, "local title", "remote title").await;

        app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('r')).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Char('y')))
            .await
            .unwrap();

        assert_eq!(app.store.tasks[selected].task.title, "remote title");
        assert!(!app.store.tasks[selected].has_conflict);
    }

    #[tokio::test]
    async fn manual_conflict_merge_resolves_with_submitted_value() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Before")).await;
        insert_title_conflict(&pool, &mut app, selected, "local title", "remote title").await;

        app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('m')).await.unwrap();
        type_chars(&mut app, " merged").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert_eq!(app.store.tasks[selected].task.title, "local title merged");
        assert!(!app.store.tasks[selected].has_conflict);
    }

    #[tokio::test]
    async fn manual_conflict_retry_preserves_submitted_text_after_error() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Before")).await;
        let task_id = app.store.tasks[selected].task.id.clone();
        insert_title_conflict(&pool, &mut app, selected, "local title", "remote title").await;

        app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('m')).await.unwrap();
        type_chars(&mut app, " merged").await;

        let mut conn = pool.acquire().await.unwrap();
        sqlx::query("DELETE FROM conflicts WHERE task_id = ? AND field = 'title'")
            .bind(&task_id)
            .execute(&mut *conn)
            .await
            .unwrap();
        drop(conn);

        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(toast_message(&app).is_some_and(|message| message.contains("conflict-not-found")));
        assert_eq!(toast_severity(&app), Some(ToastSeverity::Error));
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::TextInput(state))
                if state.route == OverlayRoute::ConflictManual
                    && state.input.as_str() == "local title merged"
        ));
    }

    #[tokio::test]
    async fn conflict_resolution_without_selected_task_reports_message() {
        let mut app = test_app().await;
        app.widgets.table.select(None);

        app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        assert_eq!(
            toast_message(&app).as_deref(),
            Some("no selected task for conflict resolution")
        );
    }

    #[tokio::test]
    async fn cancel_clears_conflict_flow() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Conflict")).await;
        insert_title_conflict(&pool, &mut app, selected, "local title", "remote title").await;

        app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        assert!(app.conflict_flow.is_active());
        app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
        assert!(app.conflict_flow.is_idle());
    }
}

mod overlay_submit_routes {
    use super::*;

    #[tokio::test]
    async fn generic_text_input_submits_message() {
        let mut app = test_app().await;
        app.overlay = Some(OverlayState::TextInput(TextInputState::new(
            OverlayRoute::MessageOnly,
            "Changed title",
            "Enter title",
            "done".to_string(),
        )));
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        assert!(app.overlay.is_none());
        assert_eq!(toast_message(&app).as_deref(), Some("submitted overlay"));
    }

    #[tokio::test]
    async fn add_project_submit_routes_by_route_not_title() {
        let mut app = test_app().await;
        app.overlay = Some(OverlayState::TextInput(TextInputState::new(
            OverlayRoute::AddProject,
            "Renamed copy",
            "project name:",
            "Mobile App".to_string(),
        )));

        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert_eq!(
            toast_message(&app).as_deref(),
            Some("created project mobile-app")
        );
    }

    #[tokio::test]
    async fn add_task_project_shortcut_routes_by_route_not_title_prefix() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        let Some(OverlayState::AddTask(state)) = &mut app.overlay else {
            panic!("expected add task overlay");
        };
        state.project = "Create item".to_string();

        app.handle_overlay_key(ctrl_p()).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Picker(state))
                if state.route == OverlayRoute::AddTaskTitleProject
        ));
    }

    #[tokio::test]
    async fn add_task_priority_shortcut_routes_by_route_not_title_prefix() {
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
    async fn conflict_confirm_without_active_flow_reports_message() {
        let mut app = test_app().await;
        app.overlay = Some(OverlayState::Confirm(ConfirmState {
            route: OverlayRoute::ConflictConfirm,
            title: CONFLICT_CONFIRM_LOCAL_TITLE.to_string(),
            prompt: "Resolve?".to_string(),
        }));

        app.handle_overlay_key(key(KeyCode::Char('y')))
            .await
            .unwrap();

        assert_eq!(
            toast_message(&app).as_deref(),
            Some("conflict confirmation is not active")
        );
    }

    #[tokio::test]
    async fn delete_project_picker_and_name_confirm_use_distinct_routes() {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();

        app.execute(Action::BeginDeleteProject).await.unwrap();
        assert!(matches!(
            app.overlay,
            Some(OverlayState::Picker(PickerState {
                route: OverlayRoute::DeleteProjectPicker,
                ref title,
                ..
            })) if title == DELETE_PROJECT_TITLE
        ));

        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        assert!(matches!(
            app.overlay,
            Some(OverlayState::TextInput(TextInputState {
                route: OverlayRoute::DeleteProjectNameConfirm,
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
        app.submit_delete_project_name("mobile".to_string())
            .await
            .unwrap();

        assert_eq!(
            toast_message(&app).as_deref(),
            Some("project name does not match")
        );
        assert!(matches!(
            app.overlay,
            Some(OverlayState::TextInput(TextInputState {
                route: OverlayRoute::DeleteProjectNameConfirm,
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

    #[test]
    fn overlay_submit_routes_are_all_handled() {
        for route in OverlayRoute::ALL {
            for kind in route.submit_kinds() {
                assert!(
                    crate::tui::app_overlay_submit::handles_submit_kind(route, kind),
                    "unhandled {kind:?} route {route:?}"
                );
            }
        }
    }

    #[tokio::test]
    async fn generic_confirm_submits_on_y() {
        let mut app = test_app().await;
        app.overlay = Some(OverlayState::Confirm(ConfirmState {
            route: OverlayRoute::MessageOnly,
            title: "Changed title".to_string(),
            prompt: "Continue?".to_string(),
        }));
        app.handle_overlay_key(key(KeyCode::Char('y')))
            .await
            .unwrap();
        assert!(app.overlay.is_none());
        assert_eq!(toast_message(&app).as_deref(), Some("confirmed overlay"));
    }

    #[tokio::test]
    async fn generic_multiline_submit_uses_route_fallback_verb() {
        let mut app = test_app().await;
        app.overlay = Some(OverlayState::MultilineInput(
            MultilineInputState::from_value(
                OverlayRoute::MessageOnly,
                "Changed title",
                "Body",
                "done\nhere".to_string(),
            ),
        ));
        app.handle_overlay_key(ctrl_s()).await.unwrap();
        assert!(app.overlay.is_none());
        assert_eq!(toast_message(&app).as_deref(), Some("submitted overlay"));
    }

    #[tokio::test]
    async fn generic_picker_submit_uses_route_fallback_verb() {
        let mut app = test_app().await;
        app.overlay = Some(OverlayState::Picker(PickerState {
            route: OverlayRoute::MessageOnly,
            title: "Choose".to_string(),
            filter: LineEdit::blank(),
            items: vec![crate::tui::overlay::PickerItem {
                label: "One".to_string(),
                value: "one".to_string(),
                selected: false,
            }],
            selected: 0,
            scroll: 0,
            multi: false,
            mode: PickerMode::Navigate,
        }));
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        assert!(app.overlay.is_none());
        assert_eq!(toast_message(&app).as_deref(), Some("selected overlay"));
    }
}
