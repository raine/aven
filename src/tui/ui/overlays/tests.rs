use super::*;
use crate::query::SyncHistoryStats;
use crate::tui::authoring::{AddTaskStep, PendingTaskAttachmentSummary};
use crate::tui::config_overlay::{CONFIG_STATUS_TITLE, DATABASE_STATS_TITLE};
use crate::tui::overlay::{
    AddTaskAttachmentsView, AddTaskMode, AddTaskView, ConfirmView, LineEdit, MultilineInputKind,
    MultilineInputMode, MultilineInputView, OverlayState, OverlayView, PickerIntent, PickerItem,
    PickerKind, PickerMode, PickerState, PickerView, ScheduleEditorField, ScheduleEditorMode,
    ScheduleEditorState, SearchKind, SearchResultItem, SyncStatusState, SyncStatusView,
    TagComboboxIntent, TagComboboxKind, TagComboboxView, TextInputKind, TextInputView,
    TextPanelView,
};
use crate::tui::store::{
    DatabaseStatsPriorityCounts, DatabaseStatsStatusCounts, SyncStatusCheck, TuiDatabaseStats,
    TuiSyncStatus,
};
use crate::tui::theme::{self, ACCENT, BG_ALT, FG, FG_DIM, ORANGE, RED};
use crate::tui::widgets::priority_icon;
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::Modifier;
use ratatui::text::Line;

fn buffer_text(backend: &TestBackend) -> String {
    backend
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

fn render_non_help_overlay_content(frame: &mut Frame, overlay: &OverlayView) {
    match overlay {
        OverlayView::Onboarding { .. } => render_onboarding(frame),
        OverlayView::Search {
            input,
            cursor,
            results,
            selected,
            total_matches,
            stale,
            no_matches_cached,
            intent,
        } => render_search(
            frame,
            SearchRenderView {
                input,
                cursor: *cursor,
                results,
                selected: *selected,
                total_matches: *total_matches,
                status: SearchRenderStatus {
                    stale: *stale,
                    no_matches_cached: *no_matches_cached,
                },
                intent,
            },
        ),
        OverlayView::AddTask(state) => render_add_task(frame, state),
        OverlayView::TextInput(state) => render_text_input(frame, state),
        OverlayView::MultilineInput(state) => render_multiline_input(frame, state),
        OverlayView::Picker(state) => render_picker(frame, state),
        OverlayView::TagCombobox(state) => render_tag_combobox(frame, state),
        OverlayView::Confirm(state) => render_confirm(frame, state),
        OverlayView::TextPanel(state) => render_text_panel(frame, state),
        OverlayView::Changelog { markdown, scroll } => render_changelog(frame, markdown, *scroll),
        OverlayView::SyncStatus(state) => render_sync_status(frame, state),
        OverlayView::DatabaseStats { stats, scroll } => {
            render_database_stats(frame, stats, *scroll)
        }
        OverlayView::Update(state) => render_update(frame, state),
        OverlayView::Detail { .. } => {}
        _ => unreachable!("test helper only renders non-help overlays"),
    }
}

fn render_overlay_view_at(overlay: OverlayView, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| render_non_help_overlay_content(frame, &overlay))
        .unwrap();
    buffer_text(terminal.backend())
}

fn render_overlay_view(overlay: OverlayView) -> String {
    render_overlay_view_at(overlay, 100, 30)
}

fn add_task_overlay(view: AddTaskView) -> OverlayView {
    OverlayView::AddTask(Box::new(view))
}

fn overlay_buffer(overlay: OverlayView) -> ratatui::buffer::Buffer {
    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| render_non_help_overlay_content(frame, &overlay))
        .unwrap();
    terminal.backend().buffer().clone()
}

fn buffer_row(buffer: &ratatui::buffer::Buffer, row: u16) -> String {
    (0..buffer.area.width)
        .map(|column| buffer[(column, row)].symbol())
        .collect()
}

fn assert_overlay_uses_dialog_chrome(overlay: OverlayView, title: &str) {
    let buffer = overlay_buffer(overlay);
    let title_row = (0..buffer.area.height)
        .map(|row| buffer_row(&buffer, row))
        .find(|row| row.contains(title))
        .unwrap_or_else(|| panic!("missing overlay title {title:?}"));

    assert!(title_row.contains(&format!("╭─ {title} ")), "{title_row}");
    assert!(title_row.contains("─╮"), "{title_row}");
}

fn styled_key_contents(line: Line<'static>) -> Vec<String> {
    line.spans
        .iter()
        .filter(|span| span.style.fg == Some(FG))
        .map(|span| span.content.to_string())
        .collect()
}

// -- Fixture helpers --

fn picker_item(label: &str, value: &str) -> PickerItem {
    PickerItem {
        label: label.to_string(),
        value: value.to_string(),
        selected: false,
    }
}

fn pending_attachment(
    filename: &str,
    byte_size: i64,
    dimensions: Option<(u32, u32)>,
) -> PendingTaskAttachmentSummary {
    PendingTaskAttachmentSummary {
        filename: filename.to_string(),
        byte_size,
        dimensions,
    }
}

fn add_task_view() -> AddTaskView {
    AddTaskView {
        title: String::new(),
        title_cursor: 0,
        description: vec![String::new()],
        description_row: 0,
        description_column: 0,
        focus: AddTaskStep::Title,
        project: "aven".to_string(),
        status: "inbox".to_string(),
        status_automatic: true,
        priority: "none".to_string(),
        labels: Vec::new(),
        is_epic: false,
        create_more: false,
        create_more_available: true,
        available_at: String::new(),
        available_at_cursor: 0,
        due_on: String::new(),
        due_on_cursor: 0,
        schedule_input: String::new(),
        schedule_input_cursor: 0,
        schedule_error: None,
        schedule_validation_requested: false,
        attachments: Box::new(AddTaskAttachmentsView {
            items: Vec::new().into_boxed_slice(),
            selected: 0,
        }),
        recurrence_series_id: None,
        editing_template: false,
        repeat_rule: String::new(),
        repeat_rule_cursor: 0,
        repeat_at: String::new(),
        repeat_at_cursor: 0,
        repeat_due: "same-day".to_string(),
        time_zone: "UTC".to_string(),
        repeat_start_on: "2026-07-20".to_string(),
        repeat_start_on_cursor: 0,
        schedule_expanded: false,
        recurrence_preview: Vec::new(),
        recurrence_error: None,
        mode: Box::new(crate::tui::overlay::AddTaskMode::Compose),
        title_error: false,
        status_prefix_active: false,
        priority_prefix_active: false,
    }
}

fn schedule_editor(mode: ScheduleEditorMode) -> ScheduleEditorState {
    ScheduleEditorState {
        mode,
        focus: ScheduleEditorField::Mode,
        available_at: LineEdit::blank(),
        due_on: LineEdit::blank(),
        repeat_rule: LineEdit::new("every Friday".to_string()),
        repeat_at: LineEdit::new("09:00".to_string()),
        repeat_due: "same-day".to_string(),
        repeat_start_on: LineEdit::new("2026-08-03".to_string()),
        time_zone: "UTC".to_string(),
        template_locked: false,
        preview: vec!["Fri Aug 7".to_string(), "Fri Aug 14".to_string()],
        error: None,
        validation_requested: false,
    }
}

fn picker_view() -> PickerView {
    PickerView {
        kind: PickerKind::Generic,
        title: String::new(),
        filter: String::new(),
        filter_cursor: 0,
        items: vec![],
        selected: 0,
        scroll: 0,
        multi: false,
        mode: PickerMode::Navigate,
        visible_indices: vec![],
    }
}

fn project_picker_view() -> PickerView {
    PickerView {
        kind: PickerKind::ScopeProject,
        title: "Scope: project".to_string(),
        filter: String::new(),
        filter_cursor: 0,
        items: vec![picker_item("CC claude-code", "claude-code")],
        selected: 0,
        scroll: 0,
        multi: false,
        mode: PickerMode::Navigate,
        visible_indices: vec![0],
    }
}

fn search_result_item(title: &str) -> SearchResultItem {
    SearchResultItem {
        task_id: crate::test_support::task_id("task-1"),
        display_ref: "AVN-1".to_string(),
        title: title.to_string(),
        description: "Preview body".to_string(),
        project_key: "aven".to_string(),
        status: "todo".to_string(),
        priority: "high".to_string(),
        created_at: "2026-06-20T00:00:00Z".to_string(),
        labels: vec!["ux".to_string()],
        matched_field: crate::query::SearchMatchedField::Title,
        snippet: None,
        score: 100,
        deleted: false,
        is_epic: false,
        unavailable_reason: None,
        create_new: false,
    }
}

mod onboarding;

mod text_panel_and_search;

mod text_input;

mod add_task_overlay;

mod multiline_overlays;

mod picker_overlays;

mod database_stats_overlay;

mod sync_status_overlay;

mod presentation_kind_rendering;

mod confirm_overlays;
