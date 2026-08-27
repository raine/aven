use super::*;
use crate::choices::{TaskPriority, TaskSource, TaskStatus};
use crate::operations::TaskDraft;
use crate::query::RecurrenceSeriesLifecycleFilter;
use crate::tui::app_conflicts::CONFLICT_CONFIRM_LOCAL_TITLE;
use crate::tui::app_edit::{
    EDIT_AVAILABILITY_TITLE, EDIT_DESCRIPTION_TITLE, EDIT_DUE_TITLE, EDIT_LABELS_TITLE,
    EDIT_PROJECT_TITLE, EDIT_TITLE_TITLE,
};
use crate::tui::app_filters::{SCOPE_PROJECT_TITLE, SWITCH_WORKSPACE_TITLE};
use crate::tui::app_intake::{IntakeWorkView, NaturalRetry};
use crate::tui::app_projects::{DELETE_PROJECT_TITLE, DELETE_TASK_TITLE};
use crate::tui::app_search::SearchControllerView;
use crate::tui::app_workspaces::{ADD_WORKSPACE_TITLE, RENAME_WORKSPACE_TITLE};
use crate::tui::authoring::{ADD_NOTE_TITLE, AddTaskOrigin, AddTaskStep};
use crate::tui::config_overlay::{CONFIG_INFO_TITLE, CONFIG_INIT_TITLE, CONFIG_PATHS_TITLE};
use crate::tui::detail_session::DetailSnapshot;
use crate::tui::event::Action;
use crate::tui::overlay::{
    AddTaskMode, CommandState, ConfirmIntent, ConfirmState, LineEdit, MultilineInputMode,
    MultilineInputState, MultilineIntent, OverlayState, OverlayTarget, OverlayView, PickerIntent,
    PickerItem, PickerMode, PickerState, SearchIntent, SearchState, TagComboboxIntent,
    TextInputState, TextIntent, TextPanelState,
};
use crate::tui::store::{
    SidebarEntryTarget, TaskLayout, TaskOrder, TaskQuery, TaskScope, TaskScopeTarget, TaskViewState,
};
use crate::tui::toast::ToastSeverity;
use crate::tui::ui::ViewSurface;
use aven_core::recurrence::{RecurrenceDuePolicy, RecurrenceRule, RecurrenceSchedule, TimeZoneId};
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

fn detail_scroll(app: &App) -> u16 {
    app.detail.state().map_or(0, |detail| detail.scroll())
}

async fn test_app() -> App {
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::test_support::open_db(&dir.path().join("test.db"))
        .await
        .unwrap();
    reset_default_workspace(&pool).await;
    let database = aven_core::db::Database::open(&dir.path().join("test.db"))
        .await
        .unwrap();
    let mut app = App::new_for_tests(database).await.unwrap();
    app._test_database_dir = Some(dir);
    app
}

#[path = "app_tests/custom_commands.rs"]
mod custom_commands;

#[path = "app_tests/configuration.rs"]
mod configuration;

#[path = "app_tests/sync.rs"]
mod sync;

#[path = "app_tests/recurrence.rs"]
mod recurrence;
use self::recurrence::add_recurring_series;

fn png_bytes(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image::RgbaImage::new(width, height))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

async fn add_real_attachment(
    app: &mut App,
    _pool: &SqlitePool,
    db_path: &std::path::Path,
    selected: usize,
) -> String {
    app.set_add_task_db_path(db_path.to_path_buf());
    let blob_dir = crate::config::resolve_blob_dir(db_path, app.intake.config()).unwrap();
    let task_id = app.store.tasks[selected].task.id.clone();
    let database = aven_core::db::Database::open(db_path).await.unwrap();
    let outcome = database
        .add_task_attachment(
            &app.store.active_workspace,
            &blob_dir,
            crate::attachments::lifecycle::LifecyclePolicy::default(),
            &task_id,
            crate::operations::AttachmentAddInput {
                filename: Some("viewer-test.png".to_string()),
                alt_text: None,
                declared_media_type: Some("image/png".to_string()),
                bytes: png_bytes(2, 2),
                optimization_policy: crate::attachments::ImageOptimizationPolicy::Preserve,
                dedupe_existing: true,
            },
        )
        .await
        .unwrap();
    app.store.refresh(Some(&task_id)).await.unwrap();
    app.list.select_task(Some(selected));
    outcome.outcome.attachment.attachment_id
}

fn test_task_intake_result(title: &str) -> crate::task_intake::TaskIntakeResult {
    crate::task_intake::TaskIntakeResult {
        task: test_task_draft(title),
        recurrence: None,
    }
}

fn test_task_draft(title: &str) -> TaskDraft {
    TaskDraft {
        metadata: Vec::new(),
        title: title.to_string(),
        description: String::new(),
        project: None,
        status: "inbox".to_string(),
        priority: "none".to_string(),
        source: TaskSource::Unknown,
        labels: Vec::new(),
        available_at: None,
        due_on: None,
        is_epic: false,
    }
}

fn test_attachment(
    attachment_id: &str,
    media_type: &str,
    has_blob: bool,
    dimensions: Option<(i64, i64)>,
) -> crate::task_render::AttachmentMetadataJson {
    crate::task_render::AttachmentMetadataJson {
        attachment_id: attachment_id.to_string(),
        task_id: "7KQ9A1X".to_string(),
        sha256: format!("{attachment_id:0<64}"),
        media_type: media_type.to_string(),
        byte_size: 4,
        filename: Some(format!("{attachment_id}.png")),
        alt_text: None,
        width: dimensions.map(|(width, _)| width),
        height: dimensions.map(|(_, height)| height),
        created_at: "2026-06-20T12:00:00Z".to_string(),
        deleted: false,
        deleted_at: None,
        bytes_state: if has_blob {
            crate::attachments::AttachmentBytesState::Present
        } else {
            crate::attachments::AttachmentBytesState::PendingDownload
        },
        has_blob,
    }
}

async fn create_and_select_task(app: &mut App, draft: TaskDraft) -> usize {
    let (_, selected) = app.store.create_task(draft, None).await.unwrap();
    let selected = selected.unwrap();
    app.list.select_task(Some(selected));
    selected
}

async fn create_epic_with_children(
    app: &mut App,
    pool: &SqlitePool,
    parent_title: &str,
    child_titles: &[&str],
) -> (crate::ids::TaskId, Vec<crate::ids::TaskId>) {
    let (_, parent_index) = app
        .store
        .create_task(
            TaskDraft {
                metadata: Vec::new(),
                is_epic: true,
                ..test_task_draft(parent_title)
            },
            None,
        )
        .await
        .unwrap();
    let parent_id = app.store.tasks[parent_index.unwrap()].task.id.clone();
    let mut child_ids = Vec::with_capacity(child_titles.len());
    for title in child_titles {
        let (_, child_index) = app
            .store
            .create_task(test_task_draft(title), None)
            .await
            .unwrap();
        child_ids.push(app.store.tasks[child_index.unwrap()].task.id.clone());
    }

    let mut conn = pool.acquire().await.unwrap();
    for child_id in &child_ids {
        crate::operations::add_task_to_epic(
            &mut conn,
            &app.store.active_workspace,
            child_id,
            &parent_id,
        )
        .await
        .unwrap();
    }
    (parent_id, child_ids)
}

async fn create_blocked_pair(app: &mut App) -> (crate::ids::TaskId, crate::ids::TaskId) {
    let (_, blocker_index) = app
        .store
        .create_task(test_task_draft("Blocker"), None)
        .await
        .unwrap();
    let blocker_index = blocker_index.unwrap();
    let blocker_id = app.store.tasks[blocker_index].task.id.clone();
    let (_, blocked_index) = app
        .store
        .create_task(test_task_draft("Blocked"), None)
        .await
        .unwrap();
    let blocked_index = blocked_index.unwrap();
    let blocked_id = app.store.tasks[blocked_index].task.id.clone();
    app.store
        .add_dependency(Some(blocked_index), &blocker_id)
        .await
        .unwrap()
        .unwrap();
    (blocker_id, blocked_id)
}

async fn test_app_with_pool() -> (tempfile::TempDir, SqlitePool, App) {
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::test_support::open_db(&dir.path().join("test.db"))
        .await
        .unwrap();
    reset_default_workspace(&pool).await;
    let database = aven_core::db::Database::open(&dir.path().join("test.db"))
        .await
        .unwrap();
    let app = App::new_for_tests(database).await.unwrap();
    (dir, pool, app)
}

async fn reset_default_workspace(pool: &SqlitePool) {
    let mut conn = pool.acquire().await.unwrap();
    crate::workspaces::ensure_default_workspace(&mut conn)
        .await
        .unwrap();
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn shift_key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::SHIFT)
}

fn detail_attachment_hit_id(
    item: &crate::query::TaskListItem,
    width: u16,
    height: u16,
    scroll: u16,
    context: &crate::tui::ui::DetailInlineImageContext,
) -> Option<String> {
    (0..height).find_map(|row| {
        (0..width).find_map(|column| {
            crate::tui::ui::detail_attachment_at_position(
                item, width, height, column, row, scroll, context,
            )
            .map(|hit| hit.attachment_id)
        })
    })
}

fn ctrl_s() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)
}

fn ctrl_g() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL)
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

fn ctrl_a() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL)
}

fn ctrl_p() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL)
}

fn ctrl_r() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)
}

fn ctrl_l() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL)
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

fn left_drag(column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

fn left_release(column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

fn right_click(column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Right),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

fn task_list_area(size: (u16, u16)) -> ratatui::layout::Rect {
    let (width, height) = size;
    let body = ratatui::layout::Rect::new(0, 2, width, height.saturating_sub(4));
    if body.width < 100 {
        body
    } else {
        let sidebar_width = body.width.min(26);
        ratatui::layout::Rect::new(
            sidebar_width,
            body.y,
            body.width.saturating_sub(sidebar_width),
            body.height,
        )
    }
}

fn recurrence_right_click_event(app: &App, size: (u16, u16), series_index: usize) -> MouseEvent {
    let task_area = task_list_area(size);
    for row in task_area.y..task_area.y.saturating_add(task_area.height) {
        for column in task_area.x..task_area.x.saturating_add(task_area.width) {
            if crate::tui::ui::recurrence_series_at_position(
                &app.store,
                app.list.table_state(),
                task_area,
                column,
                row,
            )
            .is_some_and(|hit| hit.series_index == series_index)
            {
                return right_click(column, row);
            }
        }
    }
    panic!("expected recurrence series hit target");
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

fn detail_metadata_click(target: crate::tui::ui::DetailMetadataTarget) -> MouseEvent {
    let row = match target {
        crate::tui::ui::DetailMetadataTarget::Project => 5,
        crate::tui::ui::DetailMetadataTarget::Status => 8,
        crate::tui::ui::DetailMetadataTarget::Priority => 11,
        crate::tui::ui::DetailMetadataTarget::Labels => 14,
        crate::tui::ui::DetailMetadataTarget::Availability => 17,
        crate::tui::ui::DetailMetadataTarget::Due => 20,
    };
    left_click(88, row)
}

fn render_app_buffer(app: &mut App, width: u16, height: u16) -> ratatui::buffer::Buffer {
    let backend = ratatui::backend::TestBackend::new(width, height);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    let view = app.view();
    terminal
        .draw(|frame| {
            crate::tui::ui::render(frame, &app.store, &mut app.widgets, &mut app.list, &view)
        })
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

#[path = "app_tests/navigation.rs"]
mod navigation;

async fn type_chars(app: &mut App, input: &str) {
    for ch in input.chars() {
        app.handle_overlay_key(key(KeyCode::Char(ch)))
            .await
            .unwrap();
    }
}

async fn settle_search_preview(app: &mut App) {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while app.search_preview_work_pending() {
            app.poll_search_preview().await.unwrap();
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("search preview settled");
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

#[path = "app_tests/onboarding.rs"]
mod onboarding;

#[path = "app_tests/theme_background.rs"]
mod theme_background;

#[path = "app_tests/keyboard_dispatch.rs"]
mod keyboard_dispatch;

#[path = "app_tests/attachment_save.rs"]
mod attachment_save;

#[path = "app_tests/attachment_paste.rs"]
mod attachment_paste;

#[path = "app_tests/command_and_config_overlays.rs"]
mod command_and_config_overlays;

#[path = "app_tests/filters_and_workspaces.rs"]
mod filters_and_workspaces;

#[path = "app_tests/task_row_mouse.rs"]
mod task_row_mouse;

#[path = "app_tests/authoring.rs"]
mod authoring;

#[path = "app_tests/detail_mode.rs"]
mod detail_mode;

#[path = "app_tests/task_editing.rs"]
mod task_editing;

#[path = "app_tests/delete_and_restore.rs"]
mod delete_and_restore;

#[path = "app_tests/conflicts.rs"]
mod conflicts;

#[path = "app_tests/typed_overlay_submissions.rs"]
mod typed_overlay_submissions;
