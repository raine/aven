use super::*;
use crate::tui::app_edit::{
    EDIT_DESCRIPTION_TITLE, EDIT_LABELS_TITLE, EDIT_PRIORITY_TITLE, EDIT_PROJECT_TITLE,
    EDIT_STATUS_TITLE, EDIT_TITLE_TITLE,
};
use crate::tui::config_overlay::{
    CONFIG_INFO_TITLE, CONFIG_INIT_TITLE, CONFIG_PATHS_TITLE, CONFIG_STATUS_TITLE,
};
use crate::tui::overlay::{
    ConfirmState, MultilineInputState, OverlayRoute, PickerState, TextInputState, TextPanelState,
};
use crate::tui::store::SidebarTarget;

async fn test_app() -> App {
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::db::open_db(&dir.path().join("test.db"))
        .await
        .unwrap();
    reset_default_workspace(&pool).await;
    App::new(pool).await.unwrap()
}

fn test_task_draft(title: &str) -> TaskDraft {
    TaskDraft {
        title: title.to_string(),
        description: String::new(),
        project: None,
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
    let app = App::new(pool.clone()).await.unwrap();
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

fn ctrl_s() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)
}

fn ctrl_c() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
}

fn ctrl_p() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL)
}

fn ctrl_d() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL)
}

fn ctrl_u() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL)
}

async fn type_chars(app: &mut App, input: &str) {
    for ch in input.chars() {
        app.handle_overlay_key(key(KeyCode::Char(ch)))
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn ctrl_c_quits_from_normal_mode() {
    let mut app = test_app().await;
    app.dispatch_key(ctrl_c(), 24).await.unwrap();
    assert!(app.should_quit);
}

#[tokio::test]
async fn ctrl_c_quits_while_overlay_captures_input() {
    let mut app = test_app().await;
    app.begin_search();
    app.dispatch_key(ctrl_c(), 24).await.unwrap();
    assert!(app.should_quit);
}

#[tokio::test]
async fn prefix_key_enters_prefix_mode() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('m')).await.unwrap();
    assert_eq!(app.pending_shortcut, vec![KeyCode::Char('m')]);
}

#[tokio::test]
async fn add_task_alias_executes_immediately() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
    assert!(app.pending_shortcut.is_empty());
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::TextInput(state)) if state.route == OverlayRoute::AddTaskTitle
    ));
}

#[tokio::test]
async fn prefix_is_inactive_while_overlay_captures_input() {
    let mut app = test_app().await;
    app.begin_search();
    app.handle_normal_key(KeyCode::Char('m')).await.unwrap();

    assert!(app.pending_shortcut.is_empty());
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Search { input }) if input.as_str() == "m"
    ));
}

#[tokio::test]
async fn esc_cancels_prefix_before_overlay() {
    let mut app = test_app().await;
    app.overlay = Some(OverlayState::Detail { scroll: 0 });
    app.pending_shortcut.push(KeyCode::Char('m'));
    app.dispatch_key(key(KeyCode::Esc), 24).await.unwrap();
    assert!(app.pending_shortcut.is_empty());
    assert!(matches!(
        app.overlay,
        Some(OverlayState::Detail { scroll: 0 })
    ));

    app.dispatch_key(key(KeyCode::Esc), 24).await.unwrap();
    assert!(app.overlay.is_none());
}

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
    assert_eq!(app.message.as_deref(), Some("ambiguous command: s"));

    app.begin_command();
    for ch in "zzzz".chars() {
        app.handle_overlay_key(key(KeyCode::Char(ch)))
            .await
            .unwrap();
    }
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(matches!(app.overlay, Some(OverlayState::Command { .. })));
    assert_eq!(app.message.as_deref(), Some("unknown command: zzzz"));
}

#[tokio::test]
async fn search_replaces_existing_overlay() {
    let mut app = test_app().await;
    app.overlay = Some(OverlayState::Help { scroll: 0 });
    app.begin_search();
    assert!(matches!(app.overlay, Some(OverlayState::Search { .. })));
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
async fn help_key_opens_detail_help_from_detail_overlay() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Detail help target")).await;
    app.overlay = Some(OverlayState::Detail { scroll: 0 });

    app.dispatch_key(key(KeyCode::Char('?')), 24).await.unwrap();

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

    app.dispatch_key(key(KeyCode::Char('?')), 24).await.unwrap();

    assert!(matches!(
        app.overlay,
        Some(OverlayState::Detail { scroll: 0 })
    ));
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
async fn config_status_opens_text_panel() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('C')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('s')).await.unwrap();

    let Some(OverlayState::TextPanel(panel)) = app.overlay else {
        panic!("expected text panel");
    };
    assert_eq!(panel.title, CONFIG_STATUS_TITLE);
    assert!(
        panel
            .lines
            .iter()
            .any(|line| line.contains("sync enabled:"))
    );
    assert!(
        panel
            .lines
            .iter()
            .any(|line| line.contains("daemon state: not checked from TUI"))
    );
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
    assert!(app.message.is_none());
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

#[tokio::test]
async fn invalid_continuation_shows_message() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('m')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('z')).await.unwrap();
    assert!(app.pending_shortcut.is_empty());
    assert_eq!(app.message.as_deref(), Some("invalid shortcut: m z"));
}

#[tokio::test]
async fn valid_continuation_executes_and_clears() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('m')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
    assert!(app.pending_shortcut.is_empty());
}

#[tokio::test]
async fn order_shortcut_sets_sort() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('o')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('p')).await.unwrap();
    assert_eq!(app.store.sort, TaskSort::Priority);
    assert_eq!(app.message.as_deref(), Some("order priority asc"));
}

#[tokio::test]
async fn order_reverse_shortcut_toggles_direction() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('o')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('r')).await.unwrap();
    assert_eq!(app.store.sort_direction_label(), "desc");
    assert_eq!(app.message.as_deref(), Some("order queue desc"));
}

#[tokio::test]
async fn due_order_shortcut_reports_unsupported() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('o')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('d')).await.unwrap();
    assert_eq!(
        app.message.as_deref(),
        Some(":order-due is disabled: tasks do not have due dates")
    );
}

#[tokio::test]
async fn filter_project_shortcut_opens_project_picker() {
    let mut app = test_app().await;
    app.store
        .create_project("Mobile App".to_string())
        .await
        .unwrap();

    app.handle_normal_key(KeyCode::Char('f')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('p')).await.unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Picker(PickerState { title, .. })) if title == FILTER_PROJECT_TITLE
    ));
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
async fn switch_workspace_changes_active_workspace() {
    let (_dir, pool, mut app) = test_app_with_pool().await;
    create_and_select_task(&mut app, test_task_draft("Default only")).await;

    let mut conn = pool.acquire().await.unwrap();
    crate::workspaces::create_workspace(&mut conn, "Client Work")
        .await
        .unwrap();
    drop(conn);
    app.refresh().await.unwrap();

    app.store.filters.status = Some("todo".to_string());
    app.store.active_view = SidebarTarget::Todo;

    let (message, selected) = app
        .store
        .switch_workspace("client-work".to_string())
        .await
        .unwrap();
    app.apply_filter_selection(selected);
    app.set_message(message);

    assert_eq!(app.store.active_workspace.key, "client-work");
    assert_eq!(app.store.active_view, SidebarTarget::All);
    assert!(app.store.filters.status.is_none());
    assert!(app.store.tasks.is_empty());
    assert!(app.overlay.is_none());
    assert!(
        app.message
            .as_deref()
            .is_some_and(|message| message.contains("switched workspace to client-work"))
    );

    reset_default_workspace(&pool).await;
}

#[tokio::test]
async fn clear_filters_shortcut_resets_default_view() {
    let mut app = test_app().await;
    app.store.filters.status = Some("todo".to_string());
    app.store.active_view = SidebarTarget::Todo;

    app.handle_normal_key(KeyCode::Char('f')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('c')).await.unwrap();

    assert_eq!(app.store.active_view, SidebarTarget::All);
    assert!(app.store.filters.status.is_none());
    assert_eq!(app.message.as_deref(), Some("filters cleared"));
}

#[tokio::test]
async fn go_conflicts_shortcut_sets_conflicts_view() {
    let mut app = test_app().await;

    app.handle_normal_key(KeyCode::Char('g')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('c')).await.unwrap();

    assert_eq!(app.store.active_view, SidebarTarget::Conflicts);
    assert!(app.store.filters.conflicts_only);
}

#[tokio::test]
async fn add_task_shortcut_opens_title_prompt() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('A')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('t')).await.unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::TextInput(state))
            if state.route == OverlayRoute::AddTaskTitle && state.prompt.is_empty()
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
    assert_eq!(task.task.priority, "none");
    assert_eq!(task.task.description, "");
    assert!(task.labels.is_empty());
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
        .show_view(SidebarTarget::Project("mobile-app".to_string()))
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
    assert_eq!(app.store.filters.project.as_deref(), Some("mobile-app"));
}

#[tokio::test]
async fn add_task_flow_configures_project_and_priority_from_title() {
    let mut app = test_app().await;
    app.store
        .create_project("Mobile App".to_string())
        .await
        .unwrap();

    app.handle_normal_key(KeyCode::Char('A')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
    type_chars(&mut app, "Write docs").await;
    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Picker(state)) if state.route == OverlayRoute::AddTaskTitleProject
    ));
    type_chars(&mut app, "mobile").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    app.handle_overlay_key(ctrl_p()).await.unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Picker(state)) if state.route == OverlayRoute::AddTaskTitlePriority
    ));
    type_chars(&mut app, "high").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
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
async fn add_task_flow_cancels_at_title_step() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('A')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
    assert!(app.overlay.is_none());
}

#[tokio::test]
async fn add_task_blank_title_is_rejected() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('A')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert_eq!(app.message.as_deref(), Some("task title is required"));
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::TextInput(state)) if state.route == OverlayRoute::AddTaskTitle
    ));
}

#[tokio::test]
async fn add_note_requires_selected_task() {
    let mut app = test_app().await;
    app.widgets.table.select(None);
    app.handle_normal_key(KeyCode::Char('A')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('n')).await.unwrap();

    assert!(app.overlay.is_none());
    assert_eq!(app.message.as_deref(), Some("no selected task for note"));
}

#[tokio::test]
async fn add_note_alias_requires_selected_task() {
    let mut app = test_app().await;
    app.widgets.table.select(None);
    app.handle_normal_key(KeyCode::Char('n')).await.unwrap();

    assert!(app.overlay.is_none());
    assert_eq!(app.message.as_deref(), Some("no selected task for note"));
}

#[tokio::test]
async fn add_note_flow_creates_note_for_selected_task() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Note target")).await;

    app.handle_normal_key(KeyCode::Char('A')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('n')).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::MultilineInput(state)) if state.title == ADD_NOTE_TITLE
    ));

    type_chars(&mut app, "Important detail").await;
    app.handle_overlay_key(ctrl_s()).await.unwrap();

    assert!(app.overlay.is_none());
    assert!(
        app.message
            .as_deref()
            .is_some_and(|message| message.starts_with("added note "))
    );
}

#[tokio::test]
async fn detail_scroll_keys_update_detail_offset() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Scroll target")).await;
    app.overlay = Some(OverlayState::Detail { scroll: 0 });

    app.dispatch_key(ctrl_d(), 24).await.unwrap();
    assert!(matches!(
        app.overlay,
        Some(OverlayState::Detail { scroll: 18 })
    ));

    app.dispatch_key(key(KeyCode::PageDown), 24).await.unwrap();
    assert!(matches!(
        app.overlay,
        Some(OverlayState::Detail { scroll: 36 })
    ));

    app.dispatch_key(ctrl_u(), 24).await.unwrap();
    assert!(matches!(
        app.overlay,
        Some(OverlayState::Detail { scroll: 18 })
    ));

    app.dispatch_key(key(KeyCode::Char('k')), 24).await.unwrap();
    assert!(matches!(
        app.overlay,
        Some(OverlayState::Detail { scroll: 17 })
    ));

    app.dispatch_key(key(KeyCode::PageUp), 24).await.unwrap();
    assert!(matches!(
        app.overlay,
        Some(OverlayState::Detail { scroll: 0 })
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

    app.dispatch_key(key(KeyCode::Char(']')), 24).await.unwrap();
    assert_eq!(app.widgets.table.selected(), Some(second));
    assert!(matches!(
        app.overlay,
        Some(OverlayState::Detail { scroll: 0 })
    ));
    assert_eq!(app.message.as_deref(), Some("selected next task"));

    app.dispatch_key(key(KeyCode::Char('[')), 24).await.unwrap();
    assert_eq!(app.widgets.table.selected(), Some(first));
    assert!(matches!(
        app.overlay,
        Some(OverlayState::Detail { scroll: 0 })
    ));
    assert_eq!(app.message.as_deref(), Some("selected previous task"));
}

#[tokio::test]
async fn add_note_from_detail_returns_to_detail() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Note target")).await;
    app.overlay = Some(OverlayState::Detail { scroll: 0 });

    app.dispatch_key(key(KeyCode::Char('n')), 24).await.unwrap();
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

    app.dispatch_key(key(KeyCode::Char('p')), 24).await.unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Picker(PickerState { title, .. })) if title == EDIT_PRIORITY_TITLE
    ));
    assert!(app.view().detail_underlay);
}

#[tokio::test]
async fn ignored_keys_stay_in_detail() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Detail target")).await;
    app.overlay = Some(OverlayState::Detail { scroll: 0 });

    app.dispatch_key(key(KeyCode::Char('a')), 24).await.unwrap();

    assert!(matches!(
        app.overlay,
        Some(OverlayState::Detail { scroll: 0 })
    ));
    assert!(app.authoring.is_idle());
}

#[tokio::test]
async fn cancel_add_note_from_detail_returns_to_detail() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Note target")).await;
    app.overlay = Some(OverlayState::Detail { scroll: 0 });

    app.dispatch_key(key(KeyCode::Char('n')), 24).await.unwrap();
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

    app.dispatch_key(key(KeyCode::Char('n')), 24).await.unwrap();
    app.handle_overlay_key(ctrl_s()).await.unwrap();

    assert!(matches!(
        app.overlay,
        Some(OverlayState::Detail { scroll: 0 })
    ));
    assert_eq!(app.message.as_deref(), Some("note body is required"));
}

#[tokio::test]
async fn add_note_blank_body_is_rejected() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Note target")).await;

    app.handle_normal_key(KeyCode::Char('A')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('n')).await.unwrap();
    app.handle_overlay_key(ctrl_s()).await.unwrap();

    assert!(app.overlay.is_none());
    assert_eq!(app.message.as_deref(), Some("note body is required"));
}

#[tokio::test]
async fn planned_and_disabled_shortcut_and_command_report_non_executing() {
    let mut app = test_app().await;

    app.handle_normal_key(KeyCode::Char('g')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('x')).await.unwrap();
    assert_eq!(
        app.message.as_deref(),
        Some(":view-deleted is not yet implemented: not yet implemented")
    );
    assert!(app.overlay.is_none());

    app.begin_command();
    type_chars(&mut app, "view-deleted").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert_eq!(
        app.message.as_deref(),
        Some(":view-deleted is not yet implemented: not yet implemented")
    );
    assert!(app.overlay.is_none());

    app.handle_normal_key(KeyCode::Char('o')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('d')).await.unwrap();
    assert_eq!(
        app.message.as_deref(),
        Some(":order-due is disabled: tasks do not have due dates")
    );

    app.begin_command();
    type_chars(&mut app, "order-due").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert_eq!(
        app.message.as_deref(),
        Some(":order-due is disabled: tasks do not have due dates")
    );
    assert!(app.overlay.is_none());
}

#[tokio::test]
async fn no_selected_mutating_shortcuts_report_failure() {
    let mut app = test_app().await;
    app.widgets.table.select(None);

    for sequence in [
        [KeyCode::Char('m'), KeyCode::Char('i')],
        [KeyCode::Char('m'), KeyCode::Char('h')],
        [KeyCode::Char('m'), KeyCode::Char('D')],
        [KeyCode::Char('m'), KeyCode::Char('r')],
    ] {
        app.message = None;
        app.handle_normal_key(sequence[0]).await.unwrap();
        app.handle_normal_key(sequence[1]).await.unwrap();
        assert_eq!(app.message.as_deref(), Some("no selected task to edit"));
    }
}

#[tokio::test]
async fn esc_closes_every_overlay_variant() {
    let overlays = vec![
        OverlayState::Help { scroll: 0 },
        OverlayState::Detail { scroll: 0 },
        OverlayState::DetailHelp { scroll: 0 },
        OverlayState::Search {
            input: LineEdit::new("q".to_string()),
        },
        OverlayState::Command {
            input: LineEdit::new("ref".to_string()),
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
            multi: false,
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
    ];

    for overlay in overlays {
        let detail_help = matches!(overlay, OverlayState::DetailHelp { .. });
        let mut app = test_app().await;
        app.overlay = Some(overlay);
        app.dispatch_key(key(KeyCode::Esc), 24).await.unwrap();
        if detail_help {
            assert!(matches!(
                app.overlay,
                Some(OverlayState::Detail { scroll: 0 })
            ));
        } else {
            assert!(app.overlay.is_none());
        }
        assert!(app.pending_shortcut.is_empty());
    }
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
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query(
        "INSERT INTO conflicts(task_id, field, base_version, local_value, remote_value,
         local_change_id, remote_change_id, variant_a, variant_b, created_at, resolved)
         VALUES (?, 'title', NULL, ?, ?, NULL, ?, 'a', 'b', ?, 0)",
    )
    .bind(task_id)
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

#[tokio::test]
async fn conflict_list_shortcut_applies_conflicts_view() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('l')).await.unwrap();
    assert_eq!(app.store.active_view, SidebarTarget::Conflicts);
    assert!(app.store.filters.conflicts_only);
    assert_eq!(app.message.as_deref(), Some("no unresolved conflicts"));
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
    insert_title_conflict_for_task_id(&pool, &mut app, &first_id, "local one", "remote one").await;
    insert_title_conflict_for_task_id(&pool, &mut app, &second_id, "local two", "remote two").await;
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
    assert_eq!(app.message.as_deref(), Some("selected next conflict"));
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
        app.message
            .as_deref()
            .is_some_and(|message| message.contains("resolved") && message.contains("field=title"))
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

    assert!(
        app.message
            .as_deref()
            .is_some_and(|message| message.contains("conflict-not-found"))
    );
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
        app.message.as_deref(),
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

#[tokio::test]
async fn generic_text_input_submits_message() {
    let mut app = test_app().await;
    app.overlay = Some(OverlayState::TextInput(TextInputState::new(
        OverlayRoute::MessageOnly,
        "Title",
        "Enter title",
        "done".to_string(),
    )));
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert!(app.overlay.is_none());
    assert_eq!(app.message.as_deref(), Some("submitted Title"));
}

#[tokio::test]
async fn add_project_shortcut_opens_prompt_and_creates_project() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('A')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('p')).await.unwrap();
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
    assert_eq!(app.message.as_deref(), Some("created project mobile-app"));
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
    app.handle_normal_key(KeyCode::Char('A')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('l')).await.unwrap();
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
    assert_eq!(app.message.as_deref(), Some("created label needs-review"));
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
    app.handle_normal_key(KeyCode::Char('p')).await.unwrap();
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
    app.handle_normal_key(KeyCode::Char('r')).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Picker(state))
            if state.title == EDIT_PRIORITY_TITLE
                && state.items.iter().any(|item| item.value == "high" && item.selected)
    ));

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

    app.handle_normal_key(KeyCode::Char('e')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('l')).await.unwrap();
    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Picker(state))
            if state.title == EDIT_LABELS_TITLE
                && state.items.iter().any(|item| item.value == "bug" && item.selected)
    ));

    type_chars(&mut app, "bug").await;
    app.handle_overlay_key(key(KeyCode::Char(' ')))
        .await
        .unwrap();
    app.handle_overlay_key(key(KeyCode::Backspace))
        .await
        .unwrap();
    app.handle_overlay_key(key(KeyCode::Backspace))
        .await
        .unwrap();
    app.handle_overlay_key(key(KeyCode::Backspace))
        .await
        .unwrap();
    type_chars(&mut app, "docs").await;
    app.handle_overlay_key(key(KeyCode::Char(' ')))
        .await
        .unwrap();
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
    type_chars(&mut app, "todo").await;
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

    let selected = app.widgets.table.selected().unwrap();
    assert_eq!(app.store.tasks[selected].task.status, "todo");
}

#[tokio::test]
async fn done_and_cancel_aliases_update_selected_task() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Status alias")).await;

    app.handle_normal_key(KeyCode::Char('d')).await.unwrap();
    let selected = app.store.show_view(SidebarTarget::Done).await.unwrap();
    app.widgets.table.select(selected);
    let selected = app.widgets.table.selected().unwrap();
    assert_eq!(app.store.tasks[selected].task.status, "done");

    app.handle_normal_key(KeyCode::Char('x')).await.unwrap();
    let selected = app
        .store
        .filter_status("canceled".to_string())
        .await
        .unwrap();
    app.widgets.table.select(selected);
    let selected = app.widgets.table.selected().unwrap();
    assert_eq!(app.store.tasks[selected].task.status, "canceled");
}

#[tokio::test]
async fn exact_priority_shortcut_updates_selected_task() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Priority shortcut")).await;

    app.handle_normal_key(KeyCode::Char('m')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('u')).await.unwrap();

    let selected = app.widgets.table.selected().unwrap();
    assert_eq!(app.store.tasks[selected].task.priority, "urgent");
}

#[tokio::test]
async fn priority_alias_opens_picker() {
    let mut app = test_app().await;
    create_and_select_task(&mut app, test_task_draft("Priority alias")).await;

    app.handle_normal_key(KeyCode::Char('p')).await.unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Picker(state)) if state.title == EDIT_PRIORITY_TITLE
    ));
}

#[tokio::test]
async fn edit_shortcuts_require_selected_task() {
    let mut app = test_app().await;
    app.widgets.table.select(None);

    app.handle_normal_key(KeyCode::Char('e')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('t')).await.unwrap();

    assert!(app.overlay.is_none());
    assert_eq!(app.message.as_deref(), Some("no selected task to edit"));
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

    app.handle_normal_key(KeyCode::Char('e')).await.unwrap();
    app.handle_normal_key(KeyCode::Char('d')).await.unwrap();
    type_chars(&mut app, " updated").await;
    app.handle_overlay_key(ctrl_s()).await.unwrap();

    assert!(
        app.message
            .as_deref()
            .is_some_and(|message| message.contains("conflicted-field"))
    );
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

    assert_eq!(app.message.as_deref(), Some("no selected task to copy"));
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
    assert!(app.message.as_ref().unwrap().contains("undid"));
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
    assert_eq!(app.message.as_deref(), Some("nothing to undo"));
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
    assert!(app.message.is_none());
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
        .position(|entry| entry.target == Some(SidebarTarget::Project("mobile-app".to_string())))
        .unwrap();
    app.widgets.sidebar.select(Some(project_index));

    app.execute(Action::BeginDeleteProject).await.unwrap();

    let Some(OverlayState::Picker(state)) = &app.overlay else {
        panic!("expected project picker");
    };
    assert_eq!(state.items[state.selected].value, "mobile-app");
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
        Some(OverlayState::Confirm(ConfirmState {
            route: OverlayRoute::DeleteProjectConfirm,
            ..
        }))
    ));
    app.handle_overlay_key(key(KeyCode::Char('y')))
        .await
        .unwrap();

    assert_eq!(app.message.as_deref(), Some("deleted project mobile-app"));
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
        .position(|entry| entry.target == Some(SidebarTarget::Project("mobile-app".to_string())))
        .unwrap();
    app.widgets.sidebar.select(Some(project_index));

    app.execute(Action::BeginDeleteProject).await.unwrap();
    app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
    assert_eq!(app.pending_delete_project.as_deref(), Some("mobile-app"));
    app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();

    assert!(app.pending_delete_project.is_none());
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

    assert_eq!(app.message.as_deref(), Some("created project mobile-app"));
}

#[tokio::test]
async fn add_task_project_shortcut_routes_by_route_not_title_prefix() {
    let mut app = test_app().await;
    app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
    let Some(OverlayState::TextInput(state)) = &mut app.overlay else {
        panic!("expected add task title overlay");
    };
    state.title = "Create item".to_string();

    app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();

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
    let Some(OverlayState::TextInput(state)) = &mut app.overlay else {
        panic!("expected add task title overlay");
    };
    state.title = "Create item".to_string();

    app.handle_overlay_key(ctrl_p()).await.unwrap();

    assert!(matches!(
        &app.overlay,
        Some(OverlayState::Picker(state))
            if state.route == OverlayRoute::AddTaskTitlePriority
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
        app.message.as_deref(),
        Some("conflict confirmation is not active")
    );
}

#[tokio::test]
async fn delete_project_picker_and_confirm_use_distinct_routes() {
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
        Some(OverlayState::Confirm(ConfirmState {
            route: OverlayRoute::DeleteProjectConfirm,
            ref title,
            ..
        })) if title == DELETE_PROJECT_TITLE
    ));
}

#[test]
fn overlay_submit_routes_are_all_handled() {
    use crate::tui::overlay::OverlaySubmitKind;

    fn handled(submit: OverlaySubmit) -> bool {
        matches!(
            submit,
            OverlaySubmit::Text {
                route: OverlayRoute::AddTaskTitle
                    | OverlayRoute::AddProject
                    | OverlayRoute::AddLabel
                    | OverlayRoute::EditTitle
                    | OverlayRoute::ConflictManual,
                ..
            } | OverlaySubmit::Multiline {
                route: OverlayRoute::AddNote
                    | OverlayRoute::EditDescription
                    | OverlayRoute::ConflictManual,
                ..
            } | OverlaySubmit::Picker {
                route: OverlayRoute::AddTaskTitleProject
                    | OverlayRoute::AddTaskTitlePriority
                    | OverlayRoute::EditStatus
                    | OverlayRoute::EditProject
                    | OverlayRoute::EditPriority
                    | OverlayRoute::EditLabels
                    | OverlayRoute::FilterProject
                    | OverlayRoute::FilterLabel
                    | OverlayRoute::FilterStatus
                    | OverlayRoute::FilterPriority
                    | OverlayRoute::ViewProject
                    | OverlayRoute::DeleteProjectPicker
                    | OverlayRoute::SwitchWorkspace
                    | OverlayRoute::ConflictField
                    | OverlayRoute::ConflictManual,
                ..
            } | OverlaySubmit::Confirm {
                route: OverlayRoute::ConflictConfirm
                    | OverlayRoute::ConfigInit
                    | OverlayRoute::DeleteProjectConfirm,
                ..
            }
        )
    }

    let routes = [
        OverlayRoute::MessageOnly,
        OverlayRoute::AddTaskTitle,
        OverlayRoute::AddTaskTitleProject,
        OverlayRoute::AddTaskTitlePriority,
        OverlayRoute::AddNote,
        OverlayRoute::AddProject,
        OverlayRoute::AddLabel,
        OverlayRoute::EditStatus,
        OverlayRoute::EditTitle,
        OverlayRoute::EditDescription,
        OverlayRoute::EditProject,
        OverlayRoute::EditPriority,
        OverlayRoute::EditLabels,
        OverlayRoute::FilterProject,
        OverlayRoute::FilterLabel,
        OverlayRoute::FilterStatus,
        OverlayRoute::FilterPriority,
        OverlayRoute::ViewProject,
        OverlayRoute::DeleteProjectPicker,
        OverlayRoute::DeleteProjectConfirm,
        OverlayRoute::SwitchWorkspace,
        OverlayRoute::ConflictField,
        OverlayRoute::ConflictConfirm,
        OverlayRoute::ConflictManual,
        OverlayRoute::ConfigInit,
    ];

    for route in routes {
        for kind in route.submit_kinds() {
            let submit = match kind {
                OverlaySubmitKind::Text => OverlaySubmit::Text {
                    route,
                    title: "Title".to_string(),
                    value: "value".to_string(),
                },
                OverlaySubmitKind::Multiline => OverlaySubmit::Multiline {
                    route,
                    title: "Title".to_string(),
                    value: "value".to_string(),
                },
                OverlaySubmitKind::Picker => OverlaySubmit::Picker {
                    route,
                    title: "Title".to_string(),
                    values: vec!["value".to_string()],
                },
                OverlaySubmitKind::Confirm => OverlaySubmit::Confirm {
                    route,
                    title: "Title".to_string(),
                },
            };
            assert!(handled(submit), "unhandled {kind:?} route {route:?}");
        }
    }
}

#[tokio::test]
async fn generic_confirm_submits_on_y() {
    let mut app = test_app().await;
    app.overlay = Some(OverlayState::Confirm(ConfirmState {
        route: OverlayRoute::MessageOnly,
        title: "Delete".to_string(),
        prompt: "Continue?".to_string(),
    }));
    app.handle_overlay_key(key(KeyCode::Char('y')))
        .await
        .unwrap();
    assert!(app.overlay.is_none());
    assert_eq!(app.message.as_deref(), Some("confirmed Delete"));
}
