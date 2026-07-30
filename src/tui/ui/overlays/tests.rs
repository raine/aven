use super::*;
use crate::query::SyncHistoryStats;
use crate::tui::authoring::{AddTaskStep, PendingTaskAttachmentSummary};
use crate::tui::config_overlay::{CONFIG_STATUS_TITLE, DATABASE_STATS_TITLE};
use crate::tui::overlay::{
    AddTaskAttachmentsView, AddTaskMode, AddTaskView, ConfirmView, LineEdit, MultilineInputKind,
    MultilineInputMode, MultilineInputView, OverlayState, OverlayView, PickerIntent, PickerItem,
    PickerKind, PickerMode, PickerState, PickerView, ScheduleEditorField, ScheduleEditorMode,
    ScheduleEditorState, SearchKind, SearchResultItem, TagComboboxIntent, TagComboboxKind,
    TagComboboxView, TextInputKind, TextInputView, TextPanelView,
};
use crate::tui::store::{
    DatabaseStatsPriorityCounts, DatabaseStatsStatusCounts, SyncStatusCheck, TuiDatabaseStats,
    TuiSyncStatus,
};
use crate::tui::theme::{self, ACCENT, BG_ALT, FG, FG_DIM, GREEN, RED};
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
        priority: "none".to_string(),
        labels: Vec::new(),
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

mod onboarding {
    use super::*;

    #[test]
    fn welcome_card_renders_at_minimum_tui_size() {
        let rendered = render_overlay_view_at(
            OverlayView::Onboarding {
                splash_underlay: false,
            },
            70,
            18,
        );

        assert!(rendered.contains("Welcome to aven"));
        assert!(rendered.contains("Local-first tasks for power users and coding agents."));
        assert!(rendered.contains("Everyday keys"));
        assert!(rendered.contains("Add a task and capture what's on your mind"));
        assert!(rendered.contains("Learn more"));
        assert!(rendered.contains("Agents guide"));
        assert!(rendered.contains("https://aven.raine.dev/agents/"));
        assert!(rendered.contains("Open the command panel"));
        assert!(rendered.contains("https://aven.raine.dev/tui/"));
        assert!(rendered.contains("a create first task"));
        assert!(rendered.contains("? shortcuts"));
        assert!(rendered.contains("Enter explore"));
    }

    #[test]
    fn welcome_card_uses_shared_dialog_chrome() {
        assert_overlay_uses_dialog_chrome(
            OverlayView::Onboarding {
                splash_underlay: false,
            },
            "Welcome to aven",
        );
    }

    #[test]
    fn welcome_card_styles_action_keys() {
        let keys = onboarding_lines_for_test()
            .into_iter()
            .flat_map(styled_key_contents)
            .collect::<Vec<_>>();

        assert!(keys.iter().any(|key| key.trim() == "a"));
        assert!(keys.iter().any(|key| key.trim() == "Enter"));
        assert!(keys.iter().any(|key| key.trim() == "?"));
    }
}

mod text_panel_and_search {
    use super::*;

    #[test]
    fn overlay_render_includes_text_panel_content_and_hint() {
        let rendered = render_overlay_view(OverlayView::TextPanel(TextPanelView {
            title: "Conflict details".to_string(),
            lines: vec![
                "field=title".to_string(),
                "local a: local title".to_string(),
            ],
            scroll: 0,
        }));
        assert!(rendered.contains("Conflict details"));
        assert!(rendered.contains("field=title"));
        assert!(rendered.contains("Enter/Esc close"));
    }

    #[test]
    fn changelog_renders_markdown_and_reader_controls() {
        let rendered = render_overlay_view(OverlayView::Changelog {
            markdown: "## v1.2.3\n\n- Added **reader** support.".to_string(),
            scroll: 0,
        });

        assert!(rendered.contains("Changelog"));
        assert!(rendered.contains("v1.2.3"));
        assert!(rendered.contains("Added reader support."));
        assert!(rendered.contains("j/k line"));
        assert!(rendered.contains("d/u half"));
        assert!(rendered.contains("PgUp/PgDn page"));
    }

    #[test]
    fn changelog_draws_shared_scrollbar_when_content_overflows() {
        let rendered = render_overlay_view(OverlayView::Changelog {
            markdown: format!("## Unreleased\n\n{}", "- release note\n".repeat(40)),
            scroll: 0,
        });

        assert!(rendered.contains("▲"));
        assert!(rendered.contains("▼"));
    }

    #[test]
    fn overlay_render_includes_search_title_and_input() {
        let rendered = render_overlay_view(OverlayView::Search {
            input: "query".to_string(),
            cursor: 5,
            results: vec![search_result_item("Query result")],
            selected: 0,
            total_matches: 12,
            stale: false,
            no_matches_cached: false,
            intent: SearchKind::Navigate,
        });
        assert!(rendered.contains("Search"));
        assert!(rendered.contains("query"));
        assert!(rendered.contains("Query result"));
        assert!(rendered.contains("1 of 12"));
        assert!(rendered.contains("age="));
    }

    #[test]
    fn search_overlay_shows_empty_result_summary() {
        let rendered = render_overlay_view(OverlayView::Search {
            input: "missing".to_string(),
            cursor: 7,
            results: Vec::new(),
            selected: 0,
            total_matches: 0,
            stale: false,
            no_matches_cached: false,
            intent: SearchKind::Navigate,
        });

        assert!(rendered.contains("0 matches"));
        assert!(!rendered.contains("No matching tasks"));
    }

    #[test]
    fn stale_search_overlay_keeps_empty_state_blank() {
        let rendered = render_overlay_view(OverlayView::Search {
            input: "query".to_string(),
            cursor: 5,
            results: Vec::new(),
            selected: 0,
            total_matches: 0,
            stale: true,
            no_matches_cached: false,
            intent: SearchKind::Navigate,
        });

        assert!(!rendered.contains("searching..."));
        assert!(!rendered.contains("No matching tasks"));
        assert!(!rendered.contains("0 matches"));
    }

    #[test]
    fn stale_search_overlay_preserves_cached_empty_summary() {
        let rendered = render_overlay_view(OverlayView::Search {
            input: "quer".to_string(),
            cursor: 4,
            results: Vec::new(),
            selected: 0,
            total_matches: 0,
            stale: true,
            no_matches_cached: true,
            intent: SearchKind::Navigate,
        });

        assert!(rendered.contains("0 matches"));
        assert!(!rendered.contains("searching..."));
        assert!(!rendered.contains("No matching tasks"));
    }

    #[test]
    fn add_dependency_search_explains_blocker_selection() {
        let rendered = render_overlay_view(OverlayView::Search {
            input: String::new(),
            cursor: 0,
            results: Vec::new(),
            selected: 0,
            total_matches: 0,
            stale: false,
            no_matches_cached: false,
            intent: SearchKind::AddDependency,
        });

        assert!(rendered.contains("Add dependency"));
        assert!(rendered.contains("Search for the task that blocks this task"));
        assert!(rendered.contains("Enter add selected as blocker"));
        assert!(!rendered.contains("Tab open results"));
    }

    #[test]
    fn add_epic_child_create_action_spans_row_and_updates_enter_hint() {
        let mut create = search_result_item("Create a child using \"Ship account security\"...");
        create.display_ref.clear();
        create.create_new = true;
        let intent = SearchKind::AddEpicChild {
            display_ref: "APP-YDKM".to_string(),
        };
        let buffer = overlay_buffer(OverlayView::Search {
            input: "Ship account security".to_string(),
            cursor: 21,
            results: vec![create, search_result_item("Ship account security")],
            selected: 0,
            total_matches: 1,
            stale: false,
            no_matches_cached: false,
            intent,
        });
        let create_row = (0..buffer.area.height)
            .map(|row| buffer_row(&buffer, row))
            .find(|row| row.contains("Create a child using"))
            .unwrap();
        let rendered = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(create_row.contains("▸ Create a child using"));
        assert!(rendered.contains("  Open task authoring"));
        assert!(rendered.contains("Enter create child"));
        assert!(!rendered.contains("Enter add selected child"));
    }

    #[test]
    fn add_epic_child_task_selection_uses_add_enter_hint() {
        let mut create = search_result_item("Create a new child task...");
        create.display_ref.clear();
        create.create_new = true;
        let rendered = render_overlay_view(OverlayView::Search {
            input: String::new(),
            cursor: 0,
            results: vec![create, search_result_item("Existing task")],
            selected: 1,
            total_matches: 1,
            stale: false,
            no_matches_cached: false,
            intent: SearchKind::AddEpicChild {
                display_ref: "APP-YDKM".to_string(),
            },
        });

        assert!(rendered.contains("Enter add selected child"));
        assert!(!rendered.contains("Enter create child"));
    }

    #[test]
    fn search_overlay_marks_epic_results_with_star() {
        let mut result = search_result_item("Query result");
        result.is_epic = true;
        let rendered = render_overlay_view(OverlayView::Search {
            input: "query".to_string(),
            cursor: 5,
            results: vec![result],
            selected: 0,
            total_matches: 1,
            stale: false,
            no_matches_cached: false,
            intent: SearchKind::Navigate,
        });

        assert!(rendered.contains(crate::tui::ui::task_list::EPIC_MARKER));
    }

    #[test]
    fn search_overlay_colors_project_prefix() {
        let buffer = overlay_buffer(OverlayView::Search {
            input: "query".to_string(),
            cursor: 5,
            results: vec![search_result_item("Query result")],
            selected: 0,
            total_matches: 12,
            stale: false,
            no_matches_cached: false,
            intent: SearchKind::Navigate,
        });
        let prefix_cell = buffer
            .content
            .iter()
            .find(|cell| cell.symbol() == "A" && cell.fg == theme::project_color("aven"))
            .unwrap();

        assert_eq!(prefix_cell.fg, theme::project_color("aven"));
    }

    #[test]
    fn search_overlay_vertical_position_ignores_result_count() {
        let empty = overlay_buffer(OverlayView::Search {
            input: "query".to_string(),
            cursor: 5,
            results: Vec::new(),
            selected: 0,
            total_matches: 0,
            stale: false,
            no_matches_cached: false,
            intent: SearchKind::Navigate,
        });
        let populated = overlay_buffer(OverlayView::Search {
            input: "query".to_string(),
            cursor: 5,
            results: vec![
                search_result_item("First result"),
                search_result_item("Second result"),
            ],
            selected: 0,
            total_matches: 12,
            stale: false,
            no_matches_cached: false,
            intent: SearchKind::Navigate,
        });
        let title_row = |buffer: &ratatui::buffer::Buffer| {
            (0..buffer.area.height)
                .find(|row| buffer_row(buffer, *row).contains("Search"))
                .unwrap()
        };

        assert_eq!(title_row(&empty), title_row(&populated));
    }

    #[test]
    fn text_panel_scroll_offset_changes_visible_content() {
        let rendered = render_overlay_view(OverlayView::TextPanel(TextPanelView {
            title: "Long panel".to_string(),
            lines: (0..20).map(|index| format!("Line {index}")).collect(),
            scroll: 8,
        }));
        assert!(rendered.contains("Line 8"));
        assert!(!rendered.contains("Line 0"));
    }
}

mod text_input {
    use super::*;

    #[test]
    fn overlay_render_includes_text_input_prompt_and_hints() {
        let rendered = render_overlay_view(OverlayView::TextInput(TextInputView {
            kind: TextInputKind::EditTitle,
            title: "Edit title".to_string(),
            prompt: "New title".to_string(),
            input: "alpha".to_string(),
            cursor: 5,
        }));
        assert!(rendered.contains("Edit title"));
        assert!(rendered.contains("New title"));
        assert!(rendered.contains("Enter submit"));
    }

    #[test]
    fn mixed_date_edit_uses_keep_and_clear_hints() {
        let rendered = render_overlay_view(OverlayView::TextInput(TextInputView {
            kind: TextInputKind::EditDate,
            title: "Edit due date · 2 marked tasks".to_string(),
            prompt: "Current: varies\nType a date to set it on all tasks".to_string(),
            input: String::new(),
            cursor: 0,
        }));

        assert!(rendered.contains("Type a date to set it on all tasks"));
        assert!(rendered.contains("Enter keep"));
        assert!(rendered.contains("Ctrl+D clear dates"));
        assert!(rendered.contains("Esc cancel"));
    }

    #[test]
    fn overlay_render_omits_empty_text_input_prompt() {
        let rendered = render_overlay_view(OverlayView::TextInput(TextInputView {
            kind: TextInputKind::EditTitle,
            title: "Edit title".to_string(),
            prompt: String::new(),
            input: "alpha".to_string(),
            cursor: 5,
        }));
        assert!(rendered.contains("Edit title"));
        assert!(rendered.contains("alpha"));
        assert!(!rendered.contains("title:"));
        assert!(rendered.contains("Enter submit"));
    }

    #[test]
    fn delete_project_name_confirmation_separates_prompt_and_input() {
        let buffer = overlay_buffer(OverlayView::TextInput(TextInputView {
            kind: TextInputKind::ConfirmDeleteProject,
            title: "Delete project".to_string(),
            prompt: "Type blocked-test to delete project:".to_string(),
            input: "blocked-test".to_string(),
            cursor: 12,
        }));
        let prompt_row = (0..buffer.area.height)
            .find(|row| buffer_row(&buffer, *row).contains("Type blocked-test"))
            .unwrap();
        let input_row = (prompt_row + 1..buffer.area.height)
            .find(|row| buffer_row(&buffer, *row).contains("blocked-test"))
            .unwrap();

        assert_eq!(input_row, prompt_row + 2);
    }

    #[test]
    fn placeholder_text_input_kinds_use_placeholder_style() {
        for (kind, title, prompt, placeholder) in [
            (
                TextInputKind::AddProject,
                "Add project",
                "project name:",
                ADD_PROJECT_NAME_PLACEHOLDER,
            ),
            (
                TextInputKind::AddLabel,
                "Add label",
                "label name:",
                ADD_LABEL_NAME_PLACEHOLDER,
            ),
            (
                TextInputKind::RenameProject,
                "Rename project",
                "new project name:",
                RENAME_PROJECT_NAME_PLACEHOLDER,
            ),
            (
                TextInputKind::ConflictManual,
                "Resolve manually",
                "manual value for field=title:",
                CONFLICT_MANUAL_VALUE_PLACEHOLDER,
            ),
        ] {
            let rendered = render_overlay_view(OverlayView::TextInput(TextInputView {
                kind,
                title: title.to_string(),
                prompt: prompt.to_string(),
                input: String::new(),
                cursor: 0,
            }));
            assert!(rendered.contains(title), "{kind:?}");
            assert!(rendered.contains(placeholder), "{kind:?}");
            assert!(!rendered.contains(prompt), "{kind:?}");
            assert!(rendered.contains("Enter submit"), "{kind:?}");
        }
    }

    #[test]
    fn empty_placeholder_text_input_shows_placeholder() {
        let line = placeholder_text_input_line("", 0, 20, ADD_PROJECT_NAME_PLACEHOLDER);
        assert_eq!(line.spans[0].content.as_ref(), "E");
        assert_eq!(line.spans[0].style.fg, Some(BG_ALT));
        assert_eq!(line.spans[0].style.bg, Some(FG));
        assert_eq!(line.spans[1].content.as_ref(), "nter project name here...");
        assert_eq!(line.spans[1].style.fg, Some(FG_DIM));
        assert_eq!(line.to_string(), ADD_PROJECT_NAME_PLACEHOLDER);
    }
}

mod add_task_overlay {
    use super::*;

    fn dialog_height(state: AddTaskView) -> u16 {
        let buffer = overlay_buffer(add_task_overlay(state));
        let top = (0..buffer.area.height)
            .find(|row| buffer_row(&buffer, *row).contains("╭─ Add task "))
            .expect("add task top border");
        let bottom = (top..buffer.area.height)
            .rev()
            .find(|row| buffer_row(&buffer, *row).contains('╰'))
            .expect("add task bottom border");
        bottom - top + 1
    }

    #[test]
    fn add_task_overlay_starts_compact() {
        assert_eq!(dialog_height(add_task_view()), 14);
    }

    #[test]
    fn add_task_overlay_grows_for_wrapped_description() {
        let compact_height = dialog_height(add_task_view());
        let expanded_height = dialog_height(AddTaskView {
            description: vec!["description ".repeat(40)],
            ..add_task_view()
        });

        assert!(expanded_height > compact_height);
    }

    #[test]
    fn add_task_overlay_keeps_space_above_shortcuts() {
        let buffer = overlay_buffer(add_task_overlay(AddTaskView {
            description: vec!["last description line".to_string()],
            focus: AddTaskStep::Description,
            ..add_task_view()
        }));
        let description_row = (0..buffer.area.height)
            .find(|row| buffer_row(&buffer, *row).contains("last description line"))
            .expect("description row");
        let footer_row = (description_row..buffer.area.height)
            .find(|row| buffer_row(&buffer, *row).contains("Ctrl-Enter"))
            .expect("footer row");

        assert!(footer_row >= description_row + 2);
    }

    #[test]
    fn add_task_overlay_renders_metadata_fields_and_footer() {
        let rendered = render_overlay_view(add_task_overlay(AddTaskView {
            title: "ship dialogs".to_string(),
            title_cursor: 12,
            priority: "high".to_string(),
            ..add_task_view()
        }));
        assert!(rendered.contains("Add task"));
        assert!(rendered.contains("Project: ● aven"));
        assert!(rendered.contains("Status: ▣ inbox"));
        assert!(rendered.contains("Priority: ● high"));
        assert!(rendered.contains("Labels: none"));
        assert!(rendered.contains("Schedule: none"));
        assert!(rendered.contains("Title"));
        assert!(rendered.contains("Description"));
        assert!(rendered.find("Schedule: none").unwrap() < rendered.find("Title").unwrap());
        assert!(rendered.contains("  ship dialogs"));
        assert!(rendered.contains("Optional details, links, or handoff context..."));
        assert!(rendered.contains("Tab next"));
        assert!(rendered.contains("^N create with AI"));
        assert!(rendered.contains("F1 help"));
    }

    #[test]
    fn schedule_fields_align_with_the_metadata_grid() {
        let buffer = overlay_buffer(add_task_overlay(AddTaskView {
            available_at: "tomorrow".to_string(),
            ..add_task_view()
        }));
        let status_row = (0..buffer.area.height)
            .map(|row| buffer_row(&buffer, row))
            .find(|row| row.contains("Status:"))
            .unwrap();
        let schedule_row = (0..buffer.area.height)
            .map(|row| buffer_row(&buffer, row))
            .find(|row| row.contains("Schedule:"))
            .unwrap();

        let cell_position = |row: &str, label: &str| {
            let byte = row.find(label).unwrap();
            unicode_width::UnicodeWidthStr::width(&row[..byte])
        };
        assert_eq!(
            cell_position(&status_row, "Status:"),
            cell_position(&schedule_row, "Schedule:")
        );
    }

    #[test]
    fn composer_help_expands_the_parent_dialog() {
        let compact_height = dialog_height(add_task_view());
        let help_height = dialog_height(AddTaskView {
            mode: Box::new(AddTaskMode::Help { scroll: 0 }),
            ..add_task_view()
        });

        assert!(help_height > compact_height);
    }

    #[test]
    fn structured_schedule_editor_preserves_composer_height() {
        let compact_height = dialog_height(add_task_view());
        let structured_height = dialog_height(AddTaskView {
            mode: Box::new(AddTaskMode::Schedule(schedule_editor(
                ScheduleEditorMode::Once,
            ))),
            ..add_task_view()
        });

        assert_eq!(structured_height, compact_height);
    }

    #[test]
    fn nested_schedule_editor_dims_only_its_underlay() {
        let buffer = overlay_buffer(add_task_overlay(AddTaskView {
            mode: Box::new(AddTaskMode::Schedule(schedule_editor(
                ScheduleEditorMode::Repeat,
            ))),
            ..add_task_view()
        }));
        let text_position = |needle: &str| {
            let chars = needle.chars().collect::<Vec<_>>();
            (0..buffer.area.height)
                .find_map(|row| {
                    (0..buffer.area.width.saturating_sub(chars.len() as u16)).find_map(|column| {
                        chars
                            .iter()
                            .enumerate()
                            .all(|(offset, ch)| {
                                buffer[(column + offset as u16, row)].symbol() == ch.to_string()
                            })
                            .then_some((column, row))
                    })
                })
                .unwrap_or_else(|| panic!("missing {needle:?}"))
        };

        assert!(
            buffer[text_position("Project:")]
                .modifier
                .contains(Modifier::DIM)
        );
        assert!(
            !buffer[text_position("Type")]
                .modifier
                .contains(Modifier::DIM)
        );
    }

    #[test]
    fn schedule_editor_keeps_the_composer_compact() {
        let default = overlay_buffer(add_task_overlay(add_task_view()));
        let configured = overlay_buffer(add_task_overlay(AddTaskView {
            schedule_input: "every Friday at 09:00, due same day".to_string(),
            repeat_rule: "every Friday".to_string(),
            repeat_at: "09:00".to_string(),
            ..add_task_view()
        }));
        let title_row = |buffer: &ratatui::buffer::Buffer| {
            (0..buffer.area.height)
                .position(|row| buffer_row(buffer, row).contains("Add task"))
                .unwrap()
        };

        assert_eq!(title_row(&default), title_row(&configured));
    }

    #[test]
    fn schedule_field_renders_natural_language_settings() {
        let rendered = render_overlay_view(add_task_overlay(AddTaskView {
            schedule_input: "available tomorrow, due next Friday".to_string(),
            available_at: "tomorrow".to_string(),
            due_on: "next Friday".to_string(),
            ..add_task_view()
        }));
        assert!(rendered.contains("Schedule: available tomorrow, due next Friday"));
        assert!(!rendered.contains("Available:"));
    }

    #[test]
    fn schedule_hit_testing_uses_the_summary_row() {
        let terminal = ratatui::layout::Rect::new(0, 0, 120, 30);
        let state = add_task_view();
        let outer = crate::tui::overlay::dialog_area(terminal, 100, 14);
        let schedule_row = outer.y + 2;
        for column in [70, 100] {
            assert_eq!(
                add_task_field_at(
                    terminal,
                    false,
                    AddTaskLayout {
                        description: &state.description,
                        mode: &state.mode,
                        has_attachments: false,
                        show_schedule_error: false,
                    },
                    column,
                    schedule_row,
                ),
                Some(AddTaskStep::Schedule)
            );
        }
    }

    #[test]
    fn schedule_focus_footer_explains_direct_input_and_details() {
        let rendered = render_overlay_view(add_task_overlay(AddTaskView {
            focus: AddTaskStep::Schedule,
            ..add_task_view()
        }));

        assert!(rendered.contains("type schedule"));
        assert!(rendered.contains("Enter details"));
        assert!(rendered.contains("^A available"));
        assert!(rendered.contains("^U due"));
    }

    #[test]
    fn focused_schedule_field_teaches_natural_input() {
        let rendered = render_overlay_view(add_task_overlay(AddTaskView {
            focus: AddTaskStep::Schedule,
            ..add_task_view()
        }));

        assert!(rendered.contains("Schedule: type a schedule or press enter"));
        assert!(rendered.contains("Enter details"));
    }

    #[test]
    fn focused_schedule_field_waits_to_show_validation() {
        let draft = AddTaskView {
            focus: AddTaskStep::Schedule,
            schedule_input: "d".to_string(),
            schedule_error: Some("invalid schedule".to_string()),
            ..add_task_view()
        };
        let editing = render_overlay_view(add_task_overlay(draft.clone()));
        let blurred = render_overlay_view(add_task_overlay(AddTaskView {
            schedule_validation_requested: true,
            ..draft
        }));

        assert!(!editing.contains("Schedule: Try tomorrow"));
        assert!(blurred.contains("Schedule: Try tomorrow"));
    }

    #[test]
    fn add_task_metadata_values_use_shared_styles() {
        let project = metadata_field(AddTaskStep::Project, "Project", "aven", AddTaskStep::Title);
        assert_eq!(project.to_string(), "  ^P Project: ● aven");
        assert_eq!(
            project.spans[3].style.fg,
            Some(theme::project_color("aven"))
        );

        let status = metadata_field(AddTaskStep::Status, "Status", "active", AddTaskStep::Title);
        assert_eq!(status.to_string(), "  ^T Status: ● active");
        assert_eq!(status.spans[3].style.fg, theme::status_style("active").fg);

        let priority = metadata_field(
            AddTaskStep::Priority,
            "Priority",
            "medium",
            AddTaskStep::Title,
        );
        assert_eq!(priority.to_string(), "  ^R Priority: ◐ med");
        assert_eq!(
            priority.spans[3].style.fg,
            theme::priority_style("medium").fg
        );
        assert!(!priority.to_string().contains('['));

        let availability = metadata_field(
            AddTaskStep::AvailableAt,
            "Available",
            "Now",
            AddTaskStep::Title,
        );
        assert_eq!(availability.to_string(), "  ^A Available: Now");
    }

    #[test]
    fn add_task_wide_medium_and_narrow_layouts_keep_every_field() {
        for (width, height) in [(120, 30), (80, 24), (42, 24), (50, 12)] {
            let rendered = render_overlay_view_at(
                add_task_overlay(AddTaskView {
                    focus: AddTaskStep::Labels,
                    ..add_task_view()
                }),
                width,
                height,
            );
            for label in [
                "Project",
                "Status",
                "Priority",
                "Labels",
                "Schedule",
                "Title",
                "Description",
            ] {
                assert!(
                    rendered.contains(label),
                    "missing {label} at {width}x{height}"
                );
            }
            assert!(
                rendered.contains("▶ ^L Labels"),
                "missing focus cue at {width}x{height}"
            );
            for shortcut in ["^P", "^T", "^R", "^L"] {
                assert!(
                    rendered.contains(shortcut),
                    "missing {shortcut} at {width}x{height}"
                );
            }
        }
    }

    #[test]
    fn structured_schedule_dialog_stays_coherent_across_responsive_layouts() {
        for (width, height) in [(120, 30), (80, 24), (42, 30)] {
            let rendered = render_overlay_view_at(
                add_task_overlay(AddTaskView {
                    mode: Box::new(AddTaskMode::Schedule(schedule_editor(
                        ScheduleEditorMode::Repeat,
                    ))),
                    ..add_task_view()
                }),
                width,
                height,
            );
            for label in [
                "Schedule",
                "Type",
                "Available",
                "Due",
                "Repeat",
                "Starts",
                "Next",
            ] {
                assert!(
                    rendered.contains(label),
                    "missing {label} at {width}x{height}"
                );
            }
        }
    }

    #[test]
    fn add_task_wide_layout_bounds_long_metadata_values() {
        let rendered = render_overlay_view_at(
            add_task_overlay(AddTaskView {
                project: "telegram-tori-bot-with-a-long-name".to_string(),
                status: "canceled".to_string(),
                labels: vec!["feature".to_string(), "urgent".to_string()],
                ..add_task_view()
            }),
            160,
            30,
        );

        assert!(
            rendered.contains("Project: ● tele"),
            "missing bounded project value:\n{rendered}"
        );
        assert!(rendered.contains('…'));
        assert!(rendered.contains("^T Status:"));
        assert!(rendered.contains("^R Priority:"));
        assert!(rendered.contains("^L Labels: featu"));
    }

    #[test]
    fn add_task_validation_and_help_are_visible() {
        let validation = render_overlay_view(add_task_overlay(AddTaskView {
            title_error: true,
            ..add_task_view()
        }));
        assert!(validation.contains("Title is required"));

        let help = render_overlay_view(add_task_overlay(AddTaskView {
            mode: Box::new(crate::tui::overlay::AddTaskMode::Help { scroll: 0 }),
            ..add_task_view()
        }));
        assert!(help.contains("Composer help"));
        assert!(help.contains("Shift+Tab"));
        assert!(help.contains("create with AI"));
        assert!(help.contains("one-off Available / Due"));
        assert!(help.contains("Schedule editor"));
        assert!(help.contains("↑/↓ move"));
        assert!(help.contains("aven.raine.dev/tui/#capture-tasks"));
    }

    #[test]
    fn inactive_composer_headers_stay_distinct_from_placeholders() {
        let header = add_task_field_label("Title", false);
        let placeholder = add_task_title_input_line("", None, 40);

        assert_eq!(header.spans[1].style.fg, Some(crate::tui::theme::FG_MUTED));
        assert!(
            header.spans[1]
                .style
                .add_modifier
                .contains(ratatui::style::Modifier::BOLD)
        );
        assert_eq!(placeholder.spans[0].style.fg, Some(FG_DIM));
        assert!(
            !placeholder.spans[0]
                .style
                .add_modifier
                .contains(ratatui::style::Modifier::BOLD)
        );
    }

    #[test]
    fn composer_help_scrolls_with_a_stable_dialog_and_scrollbar() {
        let top = render_overlay_view_at(
            add_task_overlay(AddTaskView {
                mode: Box::new(AddTaskMode::Help { scroll: 0 }),
                ..add_task_view()
            }),
            100,
            20,
        );
        let bottom = render_overlay_view_at(
            add_task_overlay(AddTaskView {
                mode: Box::new(AddTaskMode::Help {
                    scroll: composer_help_scroll_cap(20, false, false),
                }),
                ..add_task_view()
            }),
            100,
            20,
        );

        let top_border = top.lines().position(|line| line.contains("Composer help"));
        let bottom_border = bottom
            .lines()
            .position(|line| line.contains("Composer help"));
        let top_end = top.lines().position(|line| line.contains('╰'));
        let bottom_end = bottom.lines().position(|line| line.contains('╰'));
        assert_eq!(top_border, bottom_border);
        assert_eq!(top_end, bottom_end);
        assert!(top.contains('▲'));
        assert!(top.contains('▼'));
        assert!(top.contains("Tab / Shift+Tab"));
        assert!(!top.contains("confirm discard"));
        assert!(!bottom.contains("Tab / Shift+Tab"));
        assert!(bottom.contains("confirm discard"));
        assert!(bottom.contains("j/k scroll"));
    }

    #[test]
    fn composer_help_scroll_cap_matches_add_task_layout() {
        assert_eq!(composer_help_scroll_cap(20, false, false), 9);
        assert_eq!(composer_help_scroll_cap(20, false, true), 7);
        assert_eq!(composer_help_scroll_cap(20, true, false), 5);
        assert_eq!(composer_help_scroll_cap(30, false, false), 4);
    }

    #[test]
    fn composer_help_uses_muted_keys_and_dim_descriptions() {
        let line = composer_help_line("Ctrl-a", "jump to availability");
        let long = composer_help_line("Ctrl-Enter / Ctrl-s", "create from any field");
        let docs = composer_help_line("Docs", "https://aven.raine.dev/tui/#capture-tasks");

        assert_eq!(line.spans[0].style.fg, Some(crate::tui::theme::FG_MUTED));
        assert_eq!(line.spans[1].style.fg, Some(FG_DIM));
        assert!(long.to_string().contains("Ctrl-s   create"));
        assert_eq!(docs.spans[0].style.fg, Some(ACCENT));
        assert_eq!(docs.spans[1].style.fg, Some(ACCENT));
        assert!(
            docs.spans[1]
                .style
                .add_modifier
                .contains(ratatui::style::Modifier::UNDERLINED)
        );
    }

    #[test]
    fn add_task_child_modes_use_shared_dialog_and_control_styles() {
        let mut picker = PickerState::new(
            PickerIntent::AddTaskPriority,
            "Add task: priority",
            vec![
                PickerItem {
                    label: "none".to_string(),
                    value: "none".to_string(),
                    selected: false,
                },
                PickerItem {
                    label: "high".to_string(),
                    value: "high".to_string(),
                    selected: true,
                },
            ],
            false,
        );
        picker.filter.text = "hi".to_string();
        picker.filter.cursor = 2;
        let rendered = render_overlay_view(add_task_overlay(AddTaskView {
            mode: Box::new(AddTaskMode::Picker {
                field: AddTaskStep::Priority,
                state: picker,
            }),
            ..add_task_view()
        }));
        assert!(rendered.contains("╭─ Add task: priority"));
        assert!(rendered.contains("▸"));
        assert!(rendered.contains("Enter submit"));
        assert!(!rendered.contains("/hi"));

        let OverlayState::TagCombobox(labels) = OverlayState::tag_combobox(
            TagComboboxIntent::AddTaskLabels,
            "Add task: labels",
            vec!["feature".to_string()],
            vec!["feature".to_string()],
        ) else {
            panic!("expected labels control");
        };
        let rendered = render_overlay_view(add_task_overlay(AddTaskView {
            mode: Box::new(AddTaskMode::Labels(labels)),
            ..add_task_view()
        }));
        assert!(rendered.contains("╭─ Add task: labels"));
        assert!(rendered.contains("feature"));
        assert!(rendered.contains("Enter add/save"));

        let rendered = render_overlay_view(add_task_overlay(AddTaskView {
            mode: Box::new(AddTaskMode::ConfirmDiscard),
            ..add_task_view()
        }));
        assert!(rendered.contains("╭─ Discard draft?"));
        assert!(rendered.contains("Discard this task draft?"));
        assert!(rendered.contains("y yes"));
        assert!(rendered.contains("n no"));
        assert!(rendered.contains("Esc cancel"));
        assert!(!rendered.contains("This draft has content"));
    }

    #[test]
    fn add_task_overlay_shows_pending_images() {
        let empty = render_overlay_view(add_task_overlay(add_task_view()));
        assert!(!empty.contains("Images ("));

        let rendered = render_overlay_view(add_task_overlay(AddTaskView {
            attachments: Box::new(AddTaskAttachmentsView {
                items: vec![
                    pending_attachment("diagram.png", 2048, Some((800, 600))),
                    pending_attachment("screenshot.png", 1536, Some((1440, 900))),
                ]
                .into_boxed_slice(),
                selected: 0,
            }),
            ..add_task_view()
        }));
        assert!(rendered.contains("Images (2)  1/2 diagram.png · 800×600 · 2.0 KiB"));
        assert!(!rendered.contains("D remove"));

        let focused = render_overlay_view(add_task_overlay(AddTaskView {
            attachments: Box::new(AddTaskAttachmentsView {
                items: vec![
                    pending_attachment("diagram.png", 2048, Some((800, 600))),
                    pending_attachment("screenshot.png", 1536, Some((1440, 900))),
                ]
                .into_boxed_slice(),
                selected: 1,
            }),
            focus: AddTaskStep::Images,
            ..add_task_view()
        }));
        assert!(focused.contains("▶ Images (2)  ◀ 2/2 screenshot.png · 1440×900 · 1.5 KiB ▶"));
        assert!(focused.contains("D remove"));

        let buffer = overlay_buffer(add_task_overlay(AddTaskView {
            attachments: Box::new(AddTaskAttachmentsView {
                items: vec![pending_attachment("diagram.png", 2048, Some((800, 600)))]
                    .into_boxed_slice(),
                selected: 0,
            }),
            ..add_task_view()
        }));
        let image_row = (0..buffer.area.height)
            .find(|row| buffer_row(&buffer, *row).contains("Images (1)"))
            .unwrap();
        let image_line = buffer_row(&buffer, image_row);
        assert!(image_line.contains("diagram.png · 800×600 · 2.0 KiB"));
        assert!(!image_line.contains("1/1"));
        let title_row = (0..buffer.area.height)
            .find(|row| buffer_row(&buffer, *row).contains("Title"))
            .unwrap();
        assert_eq!(title_row, image_row + 2);
    }

    #[test]
    fn add_task_overlay_pins_footer_to_bottom() {
        let buffer = overlay_buffer(add_task_overlay(AddTaskView {
            focus: AddTaskStep::Description,
            ..add_task_view()
        }));
        let hint_row = (0..buffer.area.height)
            .find(|row| buffer_row(&buffer, *row).contains("Ctrl-Enter / ^S create"))
            .unwrap();
        let bottom_border_row = (0..buffer.area.height)
            .rev()
            .find(|row| buffer_row(&buffer, *row).contains("╰"))
            .unwrap();
        assert_eq!(hint_row + 1, bottom_border_row);
    }

    #[test]
    fn add_task_overlay_does_not_truncate_title_hints() {
        let rendered = render_overlay_view(add_task_overlay(AddTaskView { ..add_task_view() }));
        assert!(rendered.contains("Esc cancel"));
    }

    #[test]
    fn add_task_overlay_does_not_truncate_description_hints() {
        let rendered = render_overlay_view(add_task_overlay(AddTaskView {
            focus: AddTaskStep::Description,
            ..add_task_view()
        }));
        assert!(rendered.contains("Esc cancel"));
    }

    #[test]
    fn add_task_overlay_replaces_footer_when_status_prefix_is_active() {
        let rendered = render_overlay_view(add_task_overlay(AddTaskView {
            status_prefix_active: true,
            ..add_task_view()
        }));
        assert!(rendered.contains("i inbox"));
        assert!(rendered.contains("a active"));
        assert!(rendered.contains("Esc cancel"));
        assert!(!rendered.contains("Enter create"));
        assert!(!rendered.contains("^P project"));
    }

    #[test]
    fn add_task_overlay_replaces_footer_when_priority_prefix_is_active() {
        let rendered = render_overlay_view(add_task_overlay(AddTaskView {
            priority_prefix_active: true,
            ..add_task_view()
        }));
        assert!(rendered.contains("n none"));
        assert!(rendered.contains("h high"));
        assert!(rendered.contains("Esc cancel"));
        assert!(!rendered.contains("Enter create"));
        assert!(!rendered.contains("^P project"));
    }

    #[test]
    fn add_task_overlay_omits_title_placeholder_cursor_when_description_focused() {
        let buffer = overlay_buffer(add_task_overlay(AddTaskView {
            description: vec!["details".to_string()],
            description_column: 7,
            focus: AddTaskStep::Description,
            ..add_task_view()
        }));
        let title_row = (0..buffer.area.height)
            .find(|row| buffer_row(&buffer, *row).contains(ADD_TASK_TITLE_PLACEHOLDER))
            .unwrap();
        let row = buffer_row(&buffer, title_row);
        assert!(row.contains(ADD_TASK_TITLE_PLACEHOLDER));
        for column in 0..buffer.area.width {
            assert_ne!(buffer[(column, title_row)].style().bg, Some(FG));
        }
    }

    #[test]
    fn add_task_description_wraps_and_marks_hidden_rows() {
        let lines = add_task_description_lines(
            &AddTaskView {
                description: vec!["abcdefghijklmnopqrstuvwxyz".to_string()],
                description_column: 25,
                focus: AddTaskStep::Description,
                ..add_task_view()
            },
            2,
            12,
        );

        assert_eq!(lines.len(), 2);
        assert!(lines[0].to_string().starts_with("↑ "));
        assert!(lines[0].to_string().contains("klmnopqrst"));
        assert!(lines[1].to_string().contains("uvwxyz"));
        assert!(!lines[0].to_string().contains("abcdefghij"));
    }

    #[test]
    fn add_task_description_unfocused_preview_starts_at_top() {
        let lines = add_task_description_lines(
            &AddTaskView {
                description: vec!["abcdefghijklmnopqrstuvwxyz".to_string()],
                description_column: 25,
                ..add_task_view()
            },
            2,
            12,
        );

        assert!(lines[0].to_string().contains("abcdefghij"));
        assert!(lines[1].to_string().starts_with("↓ "));
    }

    #[test]
    fn hint_lines_style_keys() {
        let add_task_keys =
            styled_key_contents(add_task_hint_line(AddTaskStep::Title, false, false, false));
        assert_eq!(
            add_task_keys,
            vec!["Enter", "↑/↓", "Tab", "^N", "F1", "Esc"]
        );

        let multiline_keys = styled_key_contents(multiline_hint_line());
        assert_eq!(multiline_keys, vec!["Ctrl-Enter / ^S", "Esc"]);

        let add_task_description_keys = styled_key_contents(add_task_hint_line(
            AddTaskStep::Description,
            false,
            false,
            false,
        ));
        assert_eq!(
            add_task_description_keys,
            vec!["Ctrl-Enter / ^S", "Tab", "^N", "F1", "Esc"]
        );

        let add_task_description_editor_keys =
            styled_key_contents(add_task_description_hint_line());
        assert_eq!(
            add_task_description_editor_keys,
            vec!["Ctrl-Enter / ^S", "Enter", "^P", "^R", "Esc"]
        );

        let add_task_natural_keys = styled_key_contents(add_task_natural_hint_line());
        assert_eq!(
            add_task_natural_keys,
            vec!["Ctrl-Enter / ^S", "Enter", "Esc"]
        );

        let status_keys = styled_key_contents(add_task_status_hint_line());
        assert_eq!(status_keys, vec!["i", "b", "t", "a", "d", "x", "Esc"]);

        let priority_keys = styled_key_contents(add_task_priority_hint_line());
        assert_eq!(priority_keys, vec!["n", "l", "m", "h", "u", "Esc"]);

        let confirm_keys = styled_key_contents(confirm_hint_line());
        assert_eq!(confirm_keys, vec!["y", "n", "Esc"]);
    }

    #[test]
    fn add_task_empty_title_input_shows_placeholder() {
        let line = add_task_title_input_line("", Some(0), 20);
        assert_eq!(line.spans[0].content.as_ref(), "E");
        assert_eq!(line.spans[0].style.fg, Some(BG_ALT));
        assert_eq!(line.spans[0].style.bg, Some(FG));
        assert_eq!(line.spans[1].content.as_ref(), "nter title here...");
        assert_eq!(line.spans[1].style.fg, Some(FG_DIM));
        assert_eq!(line.to_string(), ADD_TASK_TITLE_PLACEHOLDER);
    }

    #[test]
    fn add_task_empty_title_input_without_focus_omits_cursor() {
        let line = add_task_title_input_line("", None, 20);
        assert_eq!(line.to_string(), ADD_TASK_TITLE_PLACEHOLDER);
        assert_eq!(line.spans.len(), 1);
        assert_eq!(line.spans[0].style.fg, Some(FG_DIM));
        assert_eq!(line.spans[0].style.bg, None);
    }

    #[test]
    fn add_task_title_input_draws_cursor_as_cell() {
        let line = add_task_title_input_line("abc", Some(1), 20);
        assert_eq!(line.spans[0].content.as_ref(), "a");
        assert_eq!(line.spans[1].content.as_ref(), "b");
        assert_eq!(line.spans[1].style.fg, Some(BG_ALT));
        assert_eq!(line.spans[1].style.bg, Some(FG));
        assert_eq!(line.spans[2].content.as_ref(), "c");
    }

    #[test]
    fn add_task_title_input_draws_end_cursor_as_blank_cell() {
        let line = add_task_title_input_line("abc", Some(3), 20);
        assert_eq!(line.spans[0].content.as_ref(), "abc");
        assert_eq!(line.spans[1].content.as_ref(), " ");
        assert_eq!(line.spans[1].style.bg, Some(FG));
    }

    #[test]
    fn add_task_title_input_scrolls_to_cursor_cell() {
        let line = add_task_title_input_line("abcdef", Some(5), 4);
        assert_eq!(line.spans[0].content.as_ref(), "cde");
        assert_eq!(line.spans[1].content.as_ref(), "f");
    }

    #[test]
    fn add_task_metadata_title_labels_values() {
        let line = add_task_metadata_title(
            "aven",
            "todo",
            "none",
            &["feature".to_string(), "ui".to_string()],
            120,
        );
        let rendered = line.to_string();
        assert!(rendered.contains("project: aven"));
        assert!(rendered.contains("status: todo"));
        assert!(rendered.contains("prio: none"));
        assert!(rendered.contains("labels: feature,ui"));
        assert!(rendered.contains(" · "));
        assert!(!rendered.contains("Tab"));
        assert!(!rendered.contains("^P"));
        let project = line
            .spans
            .iter()
            .find(|span| span.content == "aven")
            .unwrap();
        assert_eq!(project.style.fg, Some(theme::project_color("aven")));
        let status = line
            .spans
            .iter()
            .find(|span| span.content == "todo")
            .unwrap();
        assert_eq!(status.style.fg, theme::status_style("todo").fg);
        let priority = line
            .spans
            .iter()
            .find(|span| span.content == "none")
            .unwrap();
        assert_eq!(priority.style.fg, theme::priority_style("none").fg);
    }

    #[test]
    fn add_task_description_empty_input_shows_placeholder() {
        let line = add_task_description_input_line("", Some(0), true);
        assert_eq!(line.spans[0].content.as_ref(), "O");
        assert_eq!(line.spans[0].style.fg, Some(BG_ALT));
        assert_eq!(line.spans[0].style.bg, Some(FG));
        assert_eq!(
            line.spans[1].content.as_ref(),
            "ptional details, links, or handoff context..."
        );
        assert_eq!(line.spans[1].style.fg, Some(FG_DIM));
    }

    #[test]
    fn add_task_description_empty_unfocused_shows_placeholder() {
        let line = add_task_description_input_line("", None, true);
        assert_eq!(
            line.to_string(),
            "Optional details, links, or handoff context..."
        );
        assert_eq!(line.spans[0].style.fg, Some(FG_DIM));
    }

    #[test]
    fn recurring_schedule_dialog_renders_fields_and_preview() {
        let rendered = render_overlay_view_at(
            add_task_overlay(AddTaskView {
                mode: Box::new(AddTaskMode::Schedule(schedule_editor(
                    ScheduleEditorMode::Repeat,
                ))),
                ..add_task_view()
            }),
            120,
            30,
        );
        for expected in [
            " Repeating ",
            "Repeat    every Friday",
            "Available 09:00",
            "Same day - due on the occurrence day",
            "Starts    2026-08-03",
            "Next      Fri Aug 7, Fri Aug 14",
        ] {
            assert!(rendered.contains(expected), "missing {expected}");
        }
    }

    #[test]
    fn recurring_schedule_dialog_keeps_validation_guidance_visible() {
        let mut editor = schedule_editor(ScheduleEditorMode::Repeat);
        editor.repeat_rule = LineEdit::new("sometimes".to_string());
        editor.error = Some(crate::recurrence_input::rule_guidance().to_string());
        let rendered = render_overlay_view_at(
            add_task_overlay(AddTaskView {
                mode: Box::new(AddTaskMode::Schedule(editor)),
                ..add_task_view()
            }),
            100,
            28,
        );
        assert!(rendered.contains("Repeat"));
        assert!(rendered.contains("Repeat: use daily"));
        assert!(rendered.contains("←/→ choose"));
        assert!(rendered.contains("↑/↓ move"));
    }

    #[test]
    fn once_schedule_dialog_explains_available_and_due_inputs() {
        let mut editor = schedule_editor(ScheduleEditorMode::Once);
        editor.focus = ScheduleEditorField::Available;
        let rendered = render_overlay_view_at(
            add_task_overlay(AddTaskView {
                mode: Box::new(AddTaskMode::Schedule(editor)),
                ..add_task_view()
            }),
            100,
            28,
        );
        assert!(rendered.contains(" One-off "));
        assert!(!rendered.contains(" None "));
        assert!(rendered.contains("Available tomorrow or next monday at 9am"));
        assert!(rendered.contains("Due       next friday or none"));
    }

    #[test]
    fn once_schedule_dialog_aligns_type_and_field_values() {
        let buffer = overlay_buffer(add_task_overlay(AddTaskView {
            mode: Box::new(AddTaskMode::Schedule(schedule_editor(
                ScheduleEditorMode::Once,
            ))),
            ..add_task_view()
        }));
        let rows = (0..buffer.area.height)
            .map(|row| buffer_row(&buffer, row))
            .collect::<Vec<_>>();
        let type_row_index = rows
            .iter()
            .position(|row| row.contains(" One-off "))
            .unwrap();
        let type_row = &rows[type_row_index];
        let available_row = rows
            .iter()
            .find(|row| row.contains("tomorrow or next monday at 9am"))
            .unwrap();

        let type_start = type_row.find(" One-off ").unwrap();
        let available_start = available_row
            .find("tomorrow or next monday at 9am")
            .unwrap();
        let type_column = type_row[..type_start].chars().count();
        assert_eq!(
            type_column,
            available_row[..available_start].chars().count()
        );
        assert_eq!(
            buffer[(type_column as u16, type_row_index as u16)]
                .style()
                .bg,
            Some(ACCENT)
        );
    }

    #[test]
    fn repeating_schedule_dialog_aligns_field_values() {
        let buffer = overlay_buffer(add_task_overlay(AddTaskView {
            mode: Box::new(AddTaskMode::Schedule(schedule_editor(
                ScheduleEditorMode::Repeat,
            ))),
            ..add_task_view()
        }));
        let rows = (0..buffer.area.height)
            .map(|row| buffer_row(&buffer, row))
            .collect::<Vec<_>>();
        let value_columns = [
            " One-off ",
            "every Friday",
            "09:00",
            "Same day",
            "2026-08-03",
            "Fri Aug 7",
        ]
        .map(|value| {
            let row = rows.iter().find(|row| row.contains(value)).unwrap();
            let start = row.find(value).unwrap();
            row[..start].chars().count()
        });

        assert!(
            value_columns
                .iter()
                .all(|column| *column == value_columns[0])
        );
    }

    #[test]
    fn repeating_schedule_dialog_waits_to_validate_an_empty_rule() {
        let mut editor = schedule_editor(ScheduleEditorMode::Repeat);
        editor.repeat_rule = LineEdit::blank();
        editor.refresh();
        let rendered = render_overlay_view(add_task_overlay(AddTaskView {
            mode: Box::new(AddTaskMode::Schedule(editor)),
            ..add_task_view()
        }));

        assert!(!rendered.contains("Repeat: use daily"));
    }

    #[test]
    fn composer_shows_configured_natural_schedule_without_detail_fields() {
        let rendered = render_overlay_view_at(
            add_task_overlay(AddTaskView {
                schedule_input: "daily at 09:00, no due, starting 2026-08-03".to_string(),
                repeat_rule: "daily".to_string(),
                repeat_at: "09:00".to_string(),
                repeat_due: "none".to_string(),
                repeat_start_on: "2026-08-03".to_string(),
                ..add_task_view()
            }),
            100,
            30,
        );
        assert!(rendered.contains("Schedule: daily at 09:00, no due, starting 2026-08-03"));
        assert!(!rendered.contains("Available:"));
        assert!(!rendered.contains("Starts:"));
    }

    #[test]
    fn add_task_template_shows_natural_schedule_summary() {
        let rendered = render_overlay_view_at(
            add_task_overlay(AddTaskView {
                editing_template: true,
                schedule_input:
                    "Every 4 weeks on Monday and Thursday, due same day, starting 2026-07-20"
                        .to_string(),
                repeat_rule: "Every 4 weeks on Monday and Thursday".to_string(),
                repeat_start_on: "2026-07-20".to_string(),
                ..add_task_view()
            }),
            100,
            30,
        );
        assert!(rendered.contains("Edit recurring template"));
        assert!(rendered.contains("Schedule: Every 4 weeks"));
        assert!(!rendered.contains("Europe/Stockholm"));
    }

    #[test]
    fn add_task_description_blank_later_line_omits_placeholder() {
        let line = add_task_description_input_line("", Some(0), false);
        assert_eq!(line.to_string(), " ");
        assert!(!line.to_string().contains("Optional details"));
    }
}

mod multiline_overlays {
    use super::*;

    #[test]
    fn overlay_render_includes_multiline_submit_hints() {
        let rendered = render_overlay_view(OverlayView::MultilineInput(MultilineInputView {
            kind: MultilineInputKind::EditDescription,
            title: "Description".to_string(),
            prompt: "Body".to_string(),
            lines: vec!["line one".to_string()],
            row: 0,
            column: 4,
            mode: MultilineInputMode::Compose,
        }));
        assert!(rendered.contains("Description"));
        assert!(rendered.contains("line one"));
        assert!(rendered.contains("Ctrl-Enter / ^S submit"));
    }

    #[test]
    fn edit_description_empty_input_shows_placeholder() {
        let line = description_input_line("", 0, true);
        assert_eq!(line.spans[0].content.as_ref(), "E");
        assert_eq!(line.spans[0].style.fg, Some(BG_ALT));
        assert_eq!(line.spans[0].style.bg, Some(FG));
        assert_eq!(
            line.spans[1].content.as_ref(),
            "nter task description here..."
        );
        assert_eq!(line.spans[1].style.fg, Some(FG_DIM));
    }

    #[test]
    fn edit_description_blank_line_does_not_show_placeholder() {
        let state = MultilineInputView {
            kind: MultilineInputKind::EditDescription,
            title: "Edit description".to_string(),
            prompt: String::new(),
            lines: vec!["body".to_string(), String::new()],
            row: 1,
            column: 0,
            mode: MultilineInputMode::Compose,
        };
        let (lines, _) = description_editor_lines(&state, 80);
        assert!(!lines[1].to_string().contains("Enter task description here"));
        assert_eq!(lines[1].spans[1].content.as_ref(), " ");
        assert_eq!(lines[1].spans[1].style.bg, Some(FG));
    }

    #[test]
    fn edit_description_overlay_wraps_long_lines() {
        let overlay = OverlayView::MultilineInput(MultilineInputView {
            kind: MultilineInputKind::EditDescription,
            title: "Edit description".to_string(),
            prompt: String::new(),
            lines: vec!["a".repeat(160)],
            row: 0,
            column: 150,
            mode: MultilineInputMode::Compose,
        });
        let rendered = render_overlay_view(overlay);
        assert!(rendered.contains("Edit description"));
        assert!(rendered.contains("Ctrl-Enter / ^S submit"));
        assert!(rendered.contains("Ctrl+X Ctrl+E editor"));
        assert!(rendered.contains("line 1/1"));
        assert!(!rendered.contains(&"a".repeat(160)));
    }

    #[test]
    fn edit_description_overlay_sizes_height_to_wrapped_content() {
        let short = description_overlay_metrics(100, vec!["body".to_string()], 0, 4);
        let long = description_overlay_metrics(
            100,
            (0..16).map(|index| format!("line {index}")).collect(),
            15,
            7,
        );
        let wrapped = description_overlay_metrics(100, vec!["a".repeat(400)], 0, 390);
        assert!(short.rows < long.rows, "expected content-sized height");
        assert!(short.rows < wrapped.rows, "expected wrapped line height");
        assert!(
            short.rows >= 4,
            "expected useful minimum height, got {}",
            short.rows
        );
        assert!(
            long.rows <= 24,
            "expected terminal-relative cap, got {}",
            long.rows
        );
    }

    #[test]
    fn edit_description_overlay_width_tracks_terminal_size() {
        let normal = description_overlay_metrics(100, vec!["body".to_string()], 0, 4);
        let wide = description_overlay_metrics(160, vec!["body".to_string()], 0, 4);
        assert!(wide.columns > normal.columns);
    }

    #[test]
    fn edit_description_cursor_row_tracks_wrapped_segment() {
        let state = MultilineInputView {
            kind: MultilineInputKind::EditDescription,
            title: "Edit description".to_string(),
            prompt: String::new(),
            lines: vec!["abcdefghij".to_string()],
            row: 0,
            column: 8,
            mode: MultilineInputMode::Compose,
        };
        let (lines, cursor_row) = description_editor_lines(&state, 4);
        assert_eq!(lines.len(), 3);
        assert_eq!(cursor_row, 2);
    }

    #[test]
    fn overlay_render_omits_empty_multiline_prompt() {
        let rendered = render_overlay_view(OverlayView::MultilineInput(MultilineInputView {
            kind: MultilineInputKind::EditDescription,
            title: "Edit description".to_string(),
            prompt: String::new(),
            lines: vec!["line one".to_string()],
            row: 0,
            column: 4,
            mode: MultilineInputMode::Compose,
        }));
        assert!(rendered.contains("Edit description"));
        assert!(rendered.contains("line one"));
        assert!(!rendered.contains("description:"));
        assert!(rendered.contains("Ctrl-Enter / ^S submit"));
    }

    #[test]
    fn add_note_empty_input_shows_placeholder() {
        let line = add_note_input_line("", Some(0), true);
        assert_eq!(line.spans[0].content.as_ref(), "E");
        assert_eq!(line.spans[0].style.fg, Some(BG_ALT));
        assert_eq!(line.spans[0].style.bg, Some(FG));
        assert_eq!(line.spans[1].content.as_ref(), "nter note body here...");
        assert_eq!(line.spans[1].style.fg, Some(FG_DIM));
    }

    #[test]
    fn add_task_natural_overlay_uses_kind_and_add_task_free_text_style() {
        let rendered = render_overlay_view(OverlayView::MultilineInput(MultilineInputView {
            kind: MultilineInputKind::AddTaskNatural,
            title: "Anything".to_string(),
            prompt: "wrong prompt".to_string(),
            lines: vec![String::new()],
            row: 0,
            column: 0,
            mode: MultilineInputMode::Compose,
        }));
        assert!(rendered.contains("Anything"));
        assert!(rendered.contains("Describe the task in natural language..."));
        assert!(rendered.contains("Ctrl-Enter / ^S parse"));
        assert!(rendered.contains("Enter newline"));
        assert!(!rendered.contains("wrong prompt"));
    }

    #[test]
    fn edit_description_kind_does_not_use_natural_style_by_title() {
        let rendered = render_overlay_view(OverlayView::MultilineInput(MultilineInputView {
            kind: MultilineInputKind::EditDescription,
            title: "Add task: natural language".to_string(),
            prompt: "body:".to_string(),
            lines: vec![String::new()],
            row: 0,
            column: 0,
            mode: MultilineInputMode::Compose,
        }));
        assert!(rendered.contains("Add task: natural language"));
        assert!(rendered.contains("Enter task description here"));
        assert!(rendered.contains("Ctrl-Enter / ^S submit"));
        assert!(!rendered.contains("Ctrl-Enter / ^S parse"));
        assert!(!rendered.contains("Describe the task in natural language..."));
    }

    #[test]
    fn conflict_manual_multiline_uses_placeholder_style() {
        let rendered = render_overlay_view(OverlayView::MultilineInput(MultilineInputView {
            kind: MultilineInputKind::ConflictManual,
            title: "Resolve manually".to_string(),
            prompt: "manual value for field=description:".to_string(),
            lines: vec![String::new()],
            row: 0,
            column: 0,
            mode: MultilineInputMode::Compose,
        }));
        assert!(rendered.contains("Resolve manually"));
        assert!(rendered.contains(CONFLICT_MANUAL_BODY_PLACEHOLDER));
        assert!(!rendered.contains("manual value for field=description:"));
        assert!(rendered.contains("Ctrl-Enter / ^S submit"));
    }

    #[test]
    fn add_note_overlay_uses_placeholder_key_styles_and_spacing() {
        let overlay = OverlayView::MultilineInput(MultilineInputView {
            kind: MultilineInputKind::AddNote,
            title: "Add note".to_string(),
            prompt: "note body:".to_string(),
            lines: vec![String::new()],
            row: 0,
            column: 0,
            mode: MultilineInputMode::Compose,
        });
        let rendered = render_overlay_view(overlay.clone());
        assert!(rendered.contains("Add note"));
        assert!(rendered.contains("Enter note body here..."));
        assert!(rendered.contains("Ctrl-Enter / ^S submit"));

        let buffer = overlay_buffer(overlay);
        let hint_row = (0..buffer.area.height)
            .find(|row| buffer_row(&buffer, *row).contains("Ctrl-Enter / ^S submit"))
            .unwrap();
        let blank_row = buffer_row(&buffer, hint_row.saturating_sub(1));
        assert!(
            blank_row
                .trim_matches(|ch| ch == ' ' || ch == '│')
                .is_empty(),
            "expected blank row above key hints: {blank_row:?}"
        );
    }

    #[test]
    fn add_note_discard_confirmation_replaces_editor_with_explicit_controls() {
        let rendered = render_overlay_view(OverlayView::MultilineInput(MultilineInputView {
            kind: MultilineInputKind::AddNote,
            title: "Add note".to_string(),
            prompt: "note body:".to_string(),
            lines: vec!["draft note text".to_string()],
            row: 0,
            column: 5,
            mode: MultilineInputMode::ConfirmDiscard,
        }));

        assert!(rendered.contains("Discard note draft?"));
        assert!(rendered.contains("The note text will be lost."));
        assert!(rendered.contains("y discard"));
        assert!(rendered.contains("n keep editing"));
        assert!(rendered.contains("Esc keep editing"));
        assert!(!rendered.contains("Add note"));
        assert!(!rendered.contains("draft note text"));
        assert!(!rendered.contains("Ctrl-Enter / ^S submit"));
    }

    #[test]
    fn add_note_overlay_hides_placeholder_on_later_empty_lines() {
        let rendered = render_overlay_view(OverlayView::MultilineInput(MultilineInputView {
            kind: MultilineInputKind::AddNote,
            title: "Add note".to_string(),
            prompt: "note body:".to_string(),
            lines: vec!["existing note text".to_string(), String::new()],
            row: 1,
            column: 0,
            mode: MultilineInputMode::Compose,
        }));

        assert!(rendered.contains("existing note text"));
        assert!(!rendered.contains("Enter note body here..."));
        assert!(rendered.contains("Ctrl-Enter / ^S submit"));
    }

    #[test]
    fn add_note_overlay_keeps_hints_visible_when_input_wraps() {
        let body = "wrapped note text ".repeat(12);
        let rendered = render_overlay_view(OverlayView::MultilineInput(MultilineInputView {
            kind: MultilineInputKind::AddNote,
            title: "Add note".to_string(),
            prompt: "note body:".to_string(),
            lines: vec![body.clone()],
            row: 0,
            column: body.len(),
            mode: MultilineInputMode::Compose,
        }));

        assert!(rendered.contains("wrapped note text"));
        assert!(rendered.contains("Ctrl-Enter / ^S submit"));
        assert!(rendered.contains("Esc cancel"));
    }

    struct DescriptionOverlayMetrics {
        rows: usize,
        columns: usize,
    }

    fn description_overlay_metrics(
        terminal_width: u16,
        lines: Vec<String>,
        row: usize,
        column: usize,
    ) -> DescriptionOverlayMetrics {
        let backend = TestBackend::new(terminal_width, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render_multiline_input(
                    frame,
                    &MultilineInputView {
                        kind: MultilineInputKind::EditDescription,
                        title: "Edit description".to_string(),
                        prompt: String::new(),
                        lines,
                        row,
                        column,
                        mode: MultilineInputMode::Compose,
                    },
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let rows = (0..buffer.area.height)
            .filter(|row| buffer_row(buffer, *row).contains("│"))
            .count();
        let top_row = (0..buffer.area.height)
            .map(|row| buffer_row(buffer, row))
            .find(|row| row.contains('╭'))
            .unwrap();
        let columns = top_row.chars().filter(|ch| *ch == '─').count();
        DescriptionOverlayMetrics { rows, columns }
    }
}

mod picker_overlays {
    use super::*;

    #[test]
    fn picker_navigation_mode_hides_filter_input() {
        let rendered = render_overlay_view(OverlayView::Picker(PickerView {
            title: "Project".to_string(),
            filter: "needle".to_string(),
            filter_cursor: 6,
            items: vec![picker_item("APP app", "app")],
            multi: true,
            visible_indices: vec![0],
            ..picker_view()
        }));
        assert!(rendered.contains("Project"));
        assert!(!rendered.contains("needle"));
        assert!(rendered.contains("j/k"));
        assert!(rendered.contains("/ filter"));
        assert!(rendered.contains("Space"));
        assert!(rendered.contains("toggle"));
    }

    #[test]
    fn picker_filter_mode_hints_show_text_entry() {
        let rendered = render_overlay_view(OverlayView::Picker(PickerView {
            title: "Project".to_string(),
            filter: "app".to_string(),
            filter_cursor: 3,
            items: vec![picker_item("APP app", "app")],
            mode: PickerMode::Filter,
            visible_indices: vec![0],
            ..picker_view()
        }));
        assert!(rendered.contains("/app"));
        assert!(rendered.contains("type filter"));
        assert!(rendered.contains("Esc normal"));
    }

    #[test]
    fn priority_picker_shows_priority_icons() {
        for (kind, title) in [
            (PickerKind::EditPriority, "Edit task: priority"),
            (PickerKind::AddTaskPriority, "Add task: priority"),
        ] {
            let rendered = render_overlay_view(OverlayView::Picker(PickerView {
                kind,
                title: title.to_string(),
                items: vec![picker_item("urgent", "urgent")],
                visible_indices: vec![0],
                ..picker_view()
            }));
            assert!(rendered.contains(priority_icon("urgent")));
            assert!(rendered.contains("urgent"));
            assert!(rendered.contains("Enter"));
            assert!(rendered.contains("submit"));
        }
    }

    #[test]
    fn picker_viewport_uses_scroll_position() {
        let items = (0..12)
            .map(|index| PickerItem {
                label: format!("Item {index}"),
                value: index.to_string(),
                selected: false,
            })
            .collect::<Vec<_>>();
        let rendered = render_overlay_view(OverlayView::Picker(PickerView {
            title: "Project".to_string(),
            items,
            selected: 10,
            scroll: 3,
            visible_indices: (0..12).collect(),
            ..picker_view()
        }));
        assert!(rendered.contains("▸ Item 10"));
        assert!(rendered.contains("Item 3"));
        assert!(!rendered.contains("Item 0"));
    }

    #[test]
    fn project_picker_uses_structured_columns() {
        let rendered = render_overlay_view(OverlayView::Picker(PickerView {
            filter: "claude".to_string(),
            filter_cursor: 6,
            ..project_picker_view()
        }));
        assert!(rendered.contains("PREFIX"));
        assert!(rendered.contains("PROJECT"));
        assert!(rendered.contains("CC"));
        assert!(rendered.contains("claude-code"));
        assert!(!rendered.contains("/claude"));
        assert!(rendered.contains("Enter scope"));
    }

    #[test]
    fn tag_combobox_shows_selected_labels_input_completion_and_matches() {
        let rendered = render_overlay_view(OverlayView::TagCombobox(Box::new(TagComboboxView {
            kind: TagComboboxKind::EditLabels,
            title: "Edit task: labels".to_string(),
            input: "bu".to_string(),
            input_cursor: 2,
            completion: Some("g".to_string()),
            options: vec!["bug".to_string(), "feature".to_string()],
            selected: vec!["feature".to_string()],
            partial: Vec::new(),
            highlighted: 0,
            visible_indices: vec![0],
            visible_start: 0,
        })));

        assert!(rendered.contains("Edit task: labels"));
        assert!(rendered.contains("feature"));
        assert!(rendered.contains("bu"));
        assert!(rendered.contains("bug"));
        assert!(rendered.contains("Tab/Space add"));
        assert!(rendered.contains("Enter add/save"));
        assert!(rendered.contains("^S save"));
    }

    #[test]
    fn tag_combobox_shows_partial_label_membership() {
        let rendered = render_overlay_view(OverlayView::TagCombobox(Box::new(TagComboboxView {
            kind: TagComboboxKind::EditLabelsMulti,
            title: "Edit labels · 2 marked tasks".to_string(),
            input: String::new(),
            input_cursor: 0,
            completion: None,
            options: vec!["urgent".to_string()],
            selected: Vec::new(),
            partial: vec!["urgent".to_string()],
            highlighted: 0,
            visible_indices: vec![0],
            visible_start: 0,
        })));

        assert!(rendered.contains("~ urgent"));
    }

    #[test]
    fn edit_project_uses_structured_project_picker() {
        for (kind, title) in [
            (PickerKind::EditProject, "Edit project"),
            (PickerKind::AddTaskProject, "Add task: project"),
        ] {
            let rendered = render_overlay_view(OverlayView::Picker(PickerView {
                kind,
                title: title.to_string(),
                filter: "claude".to_string(),
                filter_cursor: 6,
                ..project_picker_view()
            }));
            assert!(rendered.contains("PREFIX"));
            assert!(rendered.contains("PROJECT"));
            assert!(rendered.contains("CC"));
            assert!(rendered.contains("claude-code"));
            assert!(rendered.contains("Enter submit"));
            assert!(rendered.contains(title));
        }
    }
}

mod database_stats_overlay {
    use super::*;

    #[test]
    fn database_stats_overlay_renders_like_sync_status() {
        let rendered = render_overlay_view(OverlayView::DatabaseStats {
            stats: Box::new(database_stats()),
            scroll: 0,
        });

        assert!(rendered.contains(DATABASE_STATS_TITLE));
        assert!(rendered.contains("WORKSPACE"));
        assert!(rendered.contains("TASKS"));
        assert!(rendered.contains("SYNC HISTORY"));
        assert!(rendered.contains("change rows"));
        assert!(rendered.contains("min server_seq"));
        assert!(rendered.contains("payload bytes"));
        assert!(rendered.contains("1234"));
        assert!(rendered.contains("Enter/Esc close"));
    }

    #[test]
    fn database_stats_overlay_scroll_changes_visible_content() {
        let backend = TestBackend::new(100, 18);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render_non_help_overlay_content(
                    frame,
                    &OverlayView::DatabaseStats {
                        stats: Box::new(database_stats()),
                        scroll: 14,
                    },
                )
            })
            .unwrap();
        let rendered = buffer_text(terminal.backend());

        assert!(rendered.contains("LATEST TASK TIMESTAMPS"));
        assert!(rendered.contains("Enter/Esc close"));
        assert!(!rendered.contains("WORKSPACE"));
    }

    fn database_stats() -> TuiDatabaseStats {
        TuiDatabaseStats {
            workspace_name: "Default".to_string(),
            workspace_key: "default".to_string(),
            total_tasks: 3,
            open_tasks: 1,
            statuses: DatabaseStatsStatusCounts {
                inbox: 1,
                done: 2,
                ..DatabaseStatsStatusCounts::default()
            },
            priorities: DatabaseStatsPriorityCounts {
                urgent: 1,
                ..DatabaseStatsPriorityCounts::default()
            },
            projects: 1,
            labels: 2,
            notes: 3,
            task_labels: 2,
            sync_history: SyncHistoryStats {
                total_change_rows: 9,
                pending_change_rows: 4,
                synced_change_rows: 5,
                min_server_seq: Some(11),
                max_server_seq: Some(15),
                payload_bytes: 1234,
            },
            sqlite_page_size: 4096,
            sqlite_page_count: 1024,
            ..TuiDatabaseStats::default()
        }
    }
}

mod sync_status_overlay {
    use super::*;

    #[test]
    fn sync_status_overlay_renders_key_sections_without_scrollbar() {
        let rendered = render_overlay_view(OverlayView::SyncStatus(Box::new(sync_status())));

        assert!(rendered.contains(CONFIG_STATUS_TITLE));
        assert!(rendered.contains("CONNECTION"));
        assert!(rendered.contains("STATE"));
        assert!(rendered.contains("LAST SYNC"));
        assert!(rendered.contains("server reach"));
        assert!(rendered.contains("last sync reached server"));
        assert!(rendered.contains("last synced"));
        assert!(!rendered.contains("2026-06-25T10:20:00Z"));
        assert!(rendered.contains("Enter/Esc close"));
        assert!(!rendered.contains('▲'));
        assert!(!rendered.contains('▼'));
    }

    #[test]
    fn sync_status_lines_style_sections_successes_and_errors() {
        let mut status = sync_status();
        status.last_error = Some("connection refused".to_string());
        let lines = sync_status_lines_for_test(&status);

        let section = lines
            .iter()
            .find(|line| line.to_string() == "CONNECTION")
            .unwrap();
        assert_eq!(section.spans[0].style.fg, Some(ACCENT));

        assert_eq!(row_value_fg(&lines, "last synced"), Some(GREEN));
        assert_eq!(row_value_fg(&lines, "last error"), Some(RED));
        assert_eq!(row_value_fg(&lines, "configured server"), Some(GREEN));
        assert_eq!(row_value_fg(&lines, "daemon server"), Some(RED));
    }

    fn sync_status() -> TuiSyncStatus {
        TuiSyncStatus {
            enabled: true,
            configured_server: Some(SyncStatusCheck::new(true, "https://sync.example")),
            pinned_server: Some("https://sync.example".to_string()),
            server_match: Some(SyncStatusCheck::new(true, "yes")),
            daemon_server: Some(SyncStatusCheck::new(false, "not configured")),
            auth_token_configured: true,
            interval_seconds: 60,
            daemon_wake: SyncStatusCheck::new(true, "127.0.0.1:3554"),
            pending_changes: 2,
            conflicts: 0,
            sync_cursor: Some("42".to_string()),
            local_sequence: Some("45".to_string()),
            last_attempt: Some("2026-06-25T10:20:00Z".to_string()),
            last_success: Some("2026-06-25T10:20:00Z".to_string()),
            last_pushed: Some("2".to_string()),
            last_pulled: Some("3".to_string()),
            last_cursor: Some("44".to_string()),
            ..TuiSyncStatus::default()
        }
    }

    fn row_value_fg(lines: &[Line<'static>], label: &str) -> Option<ratatui::style::Color> {
        lines
            .iter()
            .find(|line| line.to_string().starts_with(label))
            .and_then(|line| line.spans.get(1))
            .and_then(|span| span.style.fg)
    }
}

mod presentation_kind_rendering {
    use super::*;

    #[test]
    fn overlay_kinds_use_shared_dialog_chrome() {
        let overlays = [
            OverlayView::Search {
                input: "query".to_string(),
                cursor: 5,
                results: Vec::new(),
                selected: 0,
                total_matches: 12,
                stale: false,
                no_matches_cached: false,
                intent: SearchKind::Navigate,
            },
            add_task_overlay(AddTaskView {
                title: "ship dialogs".to_string(),
                title_cursor: 12,
                priority: "high".to_string(),
                ..add_task_view()
            }),
            OverlayView::TextInput(TextInputView {
                kind: TextInputKind::EditTitle,
                title: "Edit title".to_string(),
                prompt: "New title".to_string(),
                input: "alpha".to_string(),
                cursor: 5,
            }),
            OverlayView::MultilineInput(MultilineInputView {
                kind: MultilineInputKind::EditDescription,
                title: "Description".to_string(),
                prompt: "Body".to_string(),
                lines: vec!["line one".to_string()],
                row: 0,
                column: 4,
                mode: MultilineInputMode::Compose,
            }),
            OverlayView::Picker(PickerView {
                title: "Project".to_string(),
                filter: "app".to_string(),
                filter_cursor: 3,
                items: vec![picker_item("APP app", "app")],
                multi: true,
                visible_indices: vec![0],
                ..picker_view()
            }),
            OverlayView::TagCombobox(Box::new(TagComboboxView {
                kind: TagComboboxKind::EditLabels,
                title: "Labels".to_string(),
                input: String::new(),
                input_cursor: 0,
                completion: None,
                options: vec!["bug".to_string()],
                selected: Vec::new(),
                partial: Vec::new(),
                highlighted: 0,
                visible_indices: vec![0],
                visible_start: 0,
            })),
            OverlayView::Confirm(ConfirmView {
                title: "Delete".to_string(),
                prompt: "Delete task?".to_string(),
            }),
            OverlayView::TextPanel(TextPanelView {
                title: "Conflict details".to_string(),
                lines: vec!["field=title".to_string()],
                scroll: 0,
            }),
            OverlayView::SyncStatus(Box::default()),
        ];

        for (overlay, title) in overlays.into_iter().zip([
            "Search",
            "Add task",
            "Edit title",
            "Description",
            "Project",
            "Labels",
            "Delete",
            "Conflict details",
            CONFIG_STATUS_TITLE,
        ]) {
            assert_overlay_uses_dialog_chrome(overlay, title);
        }
    }

    #[test]
    fn add_note_kind_uses_specialized_renderer_with_changed_title() {
        let rendered = render_overlay_view(OverlayView::MultilineInput(MultilineInputView {
            kind: MultilineInputKind::AddNote,
            title: "Changed note title".to_string(),
            prompt: "note body:".to_string(),
            lines: vec![String::new()],
            row: 0,
            column: 0,
            mode: MultilineInputMode::Compose,
        }));
        assert!(rendered.contains("Changed note title"));
        assert!(rendered.contains("Enter note body here..."));
        assert!(rendered.contains("Ctrl-Enter / ^S submit"));
    }

    #[test]
    fn edit_description_kind_uses_specialized_renderer_with_changed_title() {
        let rendered = render_overlay_view(OverlayView::MultilineInput(MultilineInputView {
            kind: MultilineInputKind::EditDescription,
            title: "Changed description title".to_string(),
            prompt: String::new(),
            lines: vec!["a".repeat(160)],
            row: 0,
            column: 150,
            mode: MultilineInputMode::Compose,
        }));
        assert!(rendered.contains("Changed description title"));
        assert!(rendered.contains("Ctrl+X Ctrl+E editor"));
        assert!(rendered.contains("line 1/1"));
    }

    #[test]
    fn add_task_description_kind_uses_specialized_renderer_with_changed_title() {
        let rendered = render_overlay_view(OverlayView::MultilineInput(MultilineInputView {
            kind: MultilineInputKind::AddTaskDescription,
            title: "Changed add task description".to_string(),
            prompt: String::new(),
            lines: vec![String::new()],
            row: 0,
            column: 0,
            mode: MultilineInputMode::Compose,
        }));
        assert!(rendered.contains("Changed add task description"));
        assert!(rendered.contains("Optional details, links, or handoff context..."));
        assert!(rendered.contains("Enter newline"));
    }

    #[test]
    fn project_picker_kinds_control_submit_hints_with_changed_titles() {
        for (kind, title, hint) in [
            (
                PickerKind::ScopeProject,
                "Changed scope title",
                "Enter scope",
            ),
            (
                PickerKind::EditProject,
                "Changed edit title",
                "Enter submit",
            ),
            (
                PickerKind::AddTaskProject,
                "Changed add-task project title",
                "Enter submit",
            ),
            (
                PickerKind::DeleteProject,
                "Changed delete title",
                "Enter delete",
            ),
        ] {
            let rendered = render_overlay_view(OverlayView::Picker(PickerView {
                kind,
                title: title.to_string(),
                items: vec![picker_item("AVN aven", "aven")],
                ..project_picker_view()
            }));
            assert!(rendered.contains(title), "{kind:?}");
            assert!(rendered.contains("PREFIX"), "{kind:?}");
            assert!(rendered.contains(hint), "{kind:?}");
        }
    }

    #[test]
    fn priority_picker_kind_controls_icon_rendering_with_changed_title() {
        let rendered = render_overlay_view(OverlayView::Picker(PickerView {
            kind: PickerKind::EditPriority,
            title: "Changed priority title".to_string(),
            items: vec![picker_item("urgent", "urgent")],
            visible_indices: vec![0],
            ..picker_view()
        }));
        assert!(rendered.contains("Changed priority title"));
        assert!(rendered.contains(priority_icon("urgent")));
    }

    #[test]
    fn add_task_priority_kind_uses_priority_renderer() {
        let rendered = render_overlay_view(OverlayView::Picker(PickerView {
            kind: PickerKind::AddTaskPriority,
            title: "Changed add task priority".to_string(),
            items: vec![picker_item("urgent", "urgent")],
            visible_indices: vec![0],
            ..picker_view()
        }));
        assert!(rendered.contains("Changed add task priority"));
        assert!(rendered.contains(priority_icon("urgent")));
        assert!(rendered.contains("urgent"));
        assert!(rendered.contains("Enter submit"));
    }
}

mod confirm_overlays {
    use super::*;

    #[test]
    fn overlay_render_includes_confirm_prompt_and_hints() {
        let rendered = render_overlay_view(OverlayView::Confirm(ConfirmView {
            title: "Delete".to_string(),
            prompt: "Delete task?".to_string(),
        }));
        assert!(rendered.contains("Delete"));
        assert!(rendered.contains("Delete task?"));
        assert!(rendered.contains("y yes"));
    }

    #[test]
    fn confirm_overlay_wraps_long_prompt() {
        let prompt =
            "Delete WI-2ZB3 Option to track treadmill sessions as HealthKit workouts ".repeat(2);
        let overlay = OverlayView::Confirm(ConfirmView {
            title: "Delete task".to_string(),
            prompt: prompt.clone(),
        });
        let buffer = overlay_buffer(overlay);

        for row in 0..buffer.area.height {
            assert!(!buffer_row(&buffer, row).contains(&prompt));
        }
        assert!(buffer_text_from_rows(&buffer).contains("y yes"));
    }

    #[test]
    fn available_update_combines_release_notes_and_actions() {
        let rendered = render_overlay_view(OverlayView::Update(
            crate::tui::overlay::UpdateOverlayState::Available {
                plan: crate::update::InstallPlan {
                    release: crate::update::Release {
                        version: semver::Version::new(1, 2, 3),
                        tag: "v1.2.3".to_string(),
                        archive_name: "aven-test.tar.gz".to_string(),
                        archive_url: "https://example.com/aven-test.tar.gz".to_string(),
                        checksum_url: "https://example.com/aven-test.sha256".to_string(),
                    },
                    method: crate::update::InstallMethod::Direct {
                        target: "/usr/local/bin/aven".into(),
                    },
                },
                notes: crate::tui::overlay::UpdateNotesState::Ready(
                    "## v1.2.3\n\n- Faster updates\n\n## v1.1.0\n\n- Earlier changes".to_string(),
                ),
                scroll: 0,
                focus: crate::tui::overlay::UpdateActionFocus::Primary,
                cached: false,
            },
        ));

        assert!(rendered.contains("Software Update"));
        assert!(rendered.contains("Aven v1.2.3 is available"));
        assert!(rendered.contains("You have v"));
        assert!(rendered.contains("Changelog"));
        assert!(rendered.contains("Faster updates"));
        assert!(rendered.contains("Earlier changes"));
        assert!(rendered.contains("Later"));
        assert!(rendered.contains("Update"));
    }

    #[test]
    fn update_overlay_explains_restart_and_cancellation() {
        let success = render_overlay_view(OverlayView::Update(
            crate::tui::overlay::UpdateOverlayState::Success {
                version: "1.2.3".to_string(),
            },
        ));
        assert!(success.contains("Installed aven v1.2.3"));
        assert!(success.contains("Restart aven"));
        assert!(success.contains("q quit"));

        let lines = update_lines_for_test(&crate::tui::overlay::UpdateOverlayState::Cancelled);
        assert!(lines[0].to_string().contains("cancelled"));

        let current = update_lines_for_test(&crate::tui::overlay::UpdateOverlayState::Current {
            version: "1.2.3".to_string(),
            cached: false,
        });
        assert_eq!(current.len(), 3);
        assert!(current[1].to_string().is_empty());
        assert!(current[2].to_string().contains("Esc close"));
    }

    fn buffer_text_from_rows(buffer: &ratatui::buffer::Buffer) -> String {
        (0..buffer.area.height)
            .map(|row| buffer_row(buffer, row))
            .collect::<Vec<_>>()
            .join("\n")
    }
}
