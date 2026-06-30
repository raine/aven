use super::*;
use crate::tui::authoring::AddTaskStep;
use crate::tui::config_overlay::{CONFIG_STATUS_TITLE, DATABASE_STATS_TITLE};
use crate::tui::overlay::{
    AddTaskView, ConfirmView, MultilineInputView, OverlayRoute, OverlayView, PickerItem,
    PickerMode, PickerView, SearchPurpose, SearchResultItem, TagComboboxView, TextInputView,
    TextPanelView,
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
        OverlayView::Search {
            input,
            cursor,
            results,
            selected,
            total_matches,
            stale,
            no_matches_cached,
            purpose,
        } => render_search(
            frame,
            input,
            *cursor,
            results,
            *selected,
            *total_matches,
            SearchRenderStatus {
                stale: *stale,
                no_matches_cached: *no_matches_cached,
            },
            purpose,
        ),
        OverlayView::AddTask(state) => render_add_task(frame, state),
        OverlayView::TextInput(state) => render_text_input(frame, state),
        OverlayView::MultilineInput(state) => render_multiline_input(frame, state),
        OverlayView::Picker(state) => render_picker(frame, state),
        OverlayView::TagCombobox(state) => render_tag_combobox(frame, state),
        OverlayView::Confirm(state) => render_confirm(frame, state),
        OverlayView::TextPanel(state) => render_text_panel(frame, state),
        OverlayView::SyncStatus(state) => render_sync_status(frame, state),
        OverlayView::DatabaseStats { stats, scroll } => {
            render_database_stats(frame, stats, *scroll)
        }
        OverlayView::Detail { .. } => {}
        _ => unreachable!("test helper only renders non-help overlays"),
    }
}

fn render_overlay_view(overlay: OverlayView) -> String {
    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| render_non_help_overlay_content(frame, &overlay))
        .unwrap();
    buffer_text(terminal.backend())
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
        status_prefix_active: false,
        priority_prefix_active: false,
    }
}

fn picker_view() -> PickerView {
    PickerView {
        route: OverlayRoute::MessageOnly,
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
        route: OverlayRoute::ScopeProject,
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
        task_id: "task-1".to_string(),
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
    fn overlay_render_includes_search_title_and_input() {
        let rendered = render_overlay_view(OverlayView::Search {
            input: "query".to_string(),
            cursor: 5,
            results: vec![search_result_item("Query result")],
            selected: 0,
            total_matches: 12,
            stale: false,
            no_matches_cached: false,
            purpose: SearchPurpose::Navigate,
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
            purpose: SearchPurpose::Navigate,
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
            purpose: SearchPurpose::Navigate,
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
            purpose: SearchPurpose::Navigate,
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
            purpose: SearchPurpose::AddDependency {
                task_id: "task-1".to_string(),
                display_ref: "AVN-1".to_string(),
            },
        });

        assert!(rendered.contains("Add dependency"));
        assert!(rendered.contains("Search for the task that blocks this task"));
        assert!(rendered.contains("Enter add selected as blocker"));
        assert!(!rendered.contains("Tab open results"));
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
            purpose: SearchPurpose::Navigate,
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
            purpose: SearchPurpose::Navigate,
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
            purpose: SearchPurpose::Navigate,
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
            route: OverlayRoute::MessageOnly,
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
    fn overlay_render_omits_empty_text_input_prompt() {
        let rendered = render_overlay_view(OverlayView::TextInput(TextInputView {
            route: OverlayRoute::MessageOnly,
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
            route: OverlayRoute::DeleteProjectNameConfirm,
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
    fn placeholder_text_input_routes_use_placeholder_style() {
        for (route, title, prompt, placeholder) in [
            (
                OverlayRoute::AddProject,
                "Add project",
                "project name:",
                ADD_PROJECT_NAME_PLACEHOLDER,
            ),
            (
                OverlayRoute::AddLabel,
                "Add label",
                "label name:",
                ADD_LABEL_NAME_PLACEHOLDER,
            ),
            (
                OverlayRoute::RenameProjectName,
                "Rename project",
                "new project name:",
                RENAME_PROJECT_NAME_PLACEHOLDER,
            ),
            (
                OverlayRoute::ConflictManual,
                "Resolve manually",
                "manual value for field=title:",
                CONFLICT_MANUAL_VALUE_PLACEHOLDER,
            ),
        ] {
            let rendered = render_overlay_view(OverlayView::TextInput(TextInputView {
                route,
                title: title.to_string(),
                prompt: prompt.to_string(),
                input: String::new(),
                cursor: 0,
            }));
            assert!(rendered.contains(title), "{route:?}");
            assert!(rendered.contains(placeholder), "{route:?}");
            assert!(!rendered.contains(prompt), "{route:?}");
            assert!(rendered.contains("Enter submit"), "{route:?}");
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

    #[test]
    fn add_task_overlay_renders_metadata_fields_and_footer() {
        let rendered = render_overlay_view(OverlayView::AddTask(AddTaskView {
            title: "ship dialogs".to_string(),
            title_cursor: 12,
            priority: "high".to_string(),
            ..add_task_view()
        }));
        assert!(rendered.contains("Add task"));
        assert!(rendered.contains("project: aven"));
        assert!(rendered.contains("status: inbox"));
        assert!(rendered.contains("prio: high"));
        assert!(rendered.contains("Title"));
        assert!(rendered.contains("Description"));
        assert!(rendered.contains("ship dialogs"));
        assert!(rendered.contains("Optional details, links, or handoff context..."));
        assert!(rendered.contains("Tab description"));
        assert!(rendered.contains("^P project"));
        assert!(rendered.contains("^R priority"));
    }

    #[test]
    fn add_task_overlay_pins_footer_to_bottom() {
        let buffer = overlay_buffer(OverlayView::AddTask(AddTaskView {
            focus: AddTaskStep::Description,
            ..add_task_view()
        }));
        let hint_row = (0..buffer.area.height)
            .find(|row| buffer_row(&buffer, *row).contains("^S create"))
            .unwrap();
        let bottom_border_row = (0..buffer.area.height)
            .rev()
            .find(|row| buffer_row(&buffer, *row).contains("╰"))
            .unwrap();
        assert_eq!(hint_row + 1, bottom_border_row);
    }

    #[test]
    fn add_task_overlay_does_not_truncate_title_hints() {
        let rendered = render_overlay_view(OverlayView::AddTask(AddTaskView { ..add_task_view() }));
        assert!(rendered.contains("Esc cancel"));
    }

    #[test]
    fn add_task_overlay_does_not_truncate_description_hints() {
        let rendered = render_overlay_view(OverlayView::AddTask(AddTaskView {
            focus: AddTaskStep::Description,
            ..add_task_view()
        }));
        assert!(rendered.contains("Esc cancel"));
    }

    #[test]
    fn add_task_overlay_replaces_footer_when_status_prefix_is_active() {
        let rendered = render_overlay_view(OverlayView::AddTask(AddTaskView {
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
        let rendered = render_overlay_view(OverlayView::AddTask(AddTaskView {
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
        let buffer = overlay_buffer(OverlayView::AddTask(AddTaskView {
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
            styled_key_contents(add_task_hint_line(AddTaskStep::Title, false, false));
        assert_eq!(
            add_task_keys,
            vec!["Enter", "Tab", "^N", "^T", "^P", "^R", "Esc"]
        );

        let multiline_keys = styled_key_contents(multiline_hint_line());
        assert_eq!(multiline_keys, vec!["Ctrl+S", "Esc"]);

        let add_task_description_keys =
            styled_key_contents(add_task_hint_line(AddTaskStep::Description, false, false));
        assert_eq!(
            add_task_description_keys,
            vec!["^S", "^N", "^T", "Tab", "^P", "^R", "Esc"]
        );

        let add_task_description_editor_keys =
            styled_key_contents(add_task_description_hint_line());
        assert_eq!(
            add_task_description_editor_keys,
            vec!["^S", "Enter", "^P", "^R", "Esc"]
        );

        let add_task_natural_keys = styled_key_contents(add_task_natural_hint_line());
        assert_eq!(add_task_natural_keys, vec!["^S", "Enter", "Esc"]);

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
        let line = add_task_metadata_title("aven", "todo", "none", 60);
        let rendered = line.to_string();
        assert!(rendered.contains("project: aven"));
        assert!(rendered.contains("status: todo"));
        assert!(rendered.contains("prio: none"));
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
    fn add_task_description_blank_later_line_omits_placeholder() {
        let line = add_task_description_input_line("", Some(0), false);
        assert_eq!(line.to_string(), " ");
        assert!(!line.to_string().contains("Optional details"));
    }
}

mod multiline_overlays {
    use super::*;

    #[test]
    fn overlay_render_includes_multiline_ctrl_s_hint() {
        let rendered = render_overlay_view(OverlayView::MultilineInput(MultilineInputView {
            route: OverlayRoute::MessageOnly,
            title: "Description".to_string(),
            prompt: "Body".to_string(),
            lines: vec!["line one".to_string()],
            row: 0,
            column: 4,
        }));
        assert!(rendered.contains("Description"));
        assert!(rendered.contains("Body"));
        assert!(rendered.contains("Ctrl+S submit"));
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
            route: OverlayRoute::EditDescription,
            title: "Edit description".to_string(),
            prompt: String::new(),
            lines: vec!["body".to_string(), String::new()],
            row: 1,
            column: 0,
        };
        let (lines, _) = description_editor_lines(&state, 80);
        assert!(!lines[1].to_string().contains("Enter task description here"));
        assert_eq!(lines[1].spans[1].content.as_ref(), " ");
        assert_eq!(lines[1].spans[1].style.bg, Some(FG));
    }

    #[test]
    fn edit_description_overlay_wraps_long_lines() {
        let overlay = OverlayView::MultilineInput(MultilineInputView {
            route: OverlayRoute::EditDescription,
            title: "Edit description".to_string(),
            prompt: String::new(),
            lines: vec!["a".repeat(160)],
            row: 0,
            column: 150,
        });
        let rendered = render_overlay_view(overlay);
        assert!(rendered.contains("Edit description"));
        assert!(rendered.contains("Ctrl+S submit"));
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
            route: OverlayRoute::EditDescription,
            title: "Edit description".to_string(),
            prompt: String::new(),
            lines: vec!["abcdefghij".to_string()],
            row: 0,
            column: 8,
        };
        let (lines, cursor_row) = description_editor_lines(&state, 4);
        assert_eq!(lines.len(), 3);
        assert_eq!(cursor_row, 2);
    }

    #[test]
    fn overlay_render_omits_empty_multiline_prompt() {
        let rendered = render_overlay_view(OverlayView::MultilineInput(MultilineInputView {
            route: OverlayRoute::EditDescription,
            title: "Edit description".to_string(),
            prompt: String::new(),
            lines: vec!["line one".to_string()],
            row: 0,
            column: 4,
        }));
        assert!(rendered.contains("Edit description"));
        assert!(rendered.contains("line one"));
        assert!(!rendered.contains("description:"));
        assert!(rendered.contains("Ctrl+S submit"));
    }

    #[test]
    fn add_note_empty_input_shows_placeholder() {
        let line = add_note_input_line("", Some(0));
        assert_eq!(line.spans[0].content.as_ref(), "n");
        assert_eq!(line.spans[0].style.fg, Some(BG_ALT));
        assert_eq!(line.spans[0].style.bg, Some(FG));
        assert_eq!(line.spans[1].content.as_ref(), "ote body");
        assert_eq!(line.spans[1].style.fg, Some(FG_DIM));
    }

    #[test]
    fn add_task_natural_overlay_uses_route_and_add_task_free_text_style() {
        let rendered = render_overlay_view(OverlayView::MultilineInput(MultilineInputView {
            route: OverlayRoute::AddTaskNatural,
            title: "Anything".to_string(),
            prompt: "wrong prompt".to_string(),
            lines: vec![String::new()],
            row: 0,
            column: 0,
        }));
        assert!(rendered.contains("Anything"));
        assert!(rendered.contains("Describe the task in natural language..."));
        assert!(rendered.contains("^S parse"));
        assert!(rendered.contains("Enter newline"));
        assert!(!rendered.contains("wrong prompt"));
    }

    #[test]
    fn generic_multiline_does_not_use_natural_style_by_title() {
        let rendered = render_overlay_view(OverlayView::MultilineInput(MultilineInputView {
            route: OverlayRoute::MessageOnly,
            title: "Add task: natural language".to_string(),
            prompt: "body:".to_string(),
            lines: vec![String::new()],
            row: 0,
            column: 0,
        }));
        assert!(rendered.contains("Add task: natural language"));
        assert!(rendered.contains("body:"));
        assert!(rendered.contains("Ctrl+S submit"));
        assert!(!rendered.contains("^S parse"));
        assert!(!rendered.contains("Describe the task in natural language..."));
    }

    #[test]
    fn conflict_manual_multiline_uses_placeholder_style() {
        let rendered = render_overlay_view(OverlayView::MultilineInput(MultilineInputView {
            route: OverlayRoute::ConflictManual,
            title: "Resolve manually".to_string(),
            prompt: "manual value for field=description:".to_string(),
            lines: vec![String::new()],
            row: 0,
            column: 0,
        }));
        assert!(rendered.contains("Resolve manually"));
        assert!(rendered.contains(CONFLICT_MANUAL_BODY_PLACEHOLDER));
        assert!(!rendered.contains("manual value for field=description:"));
        assert!(rendered.contains("Ctrl+S submit"));
    }

    #[test]
    fn add_note_overlay_uses_placeholder_key_styles_and_spacing() {
        let overlay = OverlayView::MultilineInput(MultilineInputView {
            route: OverlayRoute::AddNote,
            title: "Add note".to_string(),
            prompt: "note body:".to_string(),
            lines: vec![String::new()],
            row: 0,
            column: 0,
        });
        let rendered = render_overlay_view(overlay.clone());
        assert!(rendered.contains("Add note"));
        assert!(rendered.contains("note body"));
        assert!(rendered.contains("Ctrl+S submit"));

        let buffer = overlay_buffer(overlay);
        let hint_row = (0..buffer.area.height)
            .find(|row| buffer_row(&buffer, *row).contains("Ctrl+S submit"))
            .unwrap();
        let blank_row = buffer_row(&buffer, hint_row.saturating_sub(1));
        assert!(
            blank_row
                .trim_matches(|ch| ch == ' ' || ch == '│')
                .is_empty(),
            "expected blank row above key hints: {blank_row:?}"
        );
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
                        route: OverlayRoute::EditDescription,
                        title: "Edit description".to_string(),
                        prompt: String::new(),
                        lines,
                        row,
                        column,
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
    fn overlay_render_includes_picker_filter_and_hints() {
        let rendered = render_overlay_view(OverlayView::Picker(PickerView {
            title: "Project".to_string(),
            filter: "app".to_string(),
            filter_cursor: 3,
            items: vec![picker_item("APP app", "app")],
            multi: true,
            visible_indices: vec![0],
            ..picker_view()
        }));
        assert!(rendered.contains("Project"));
        assert!(rendered.contains("/app"));
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
        assert!(rendered.contains("type filter"));
        assert!(rendered.contains("Esc normal"));
    }

    #[test]
    fn priority_picker_shows_priority_icons() {
        for (route, title) in [
            (OverlayRoute::EditPriority, "Edit task: priority"),
            (OverlayRoute::AddTaskTitlePriority, "Add task: priority"),
        ] {
            let rendered = render_overlay_view(OverlayView::Picker(PickerView {
                route,
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
        assert!(rendered.contains("Enter scope"));
    }

    #[test]
    fn tag_combobox_shows_selected_labels_input_completion_and_matches() {
        let rendered = render_overlay_view(OverlayView::TagCombobox(TagComboboxView {
            route: OverlayRoute::EditLabels,
            title: "Edit task: labels".to_string(),
            input: "bu".to_string(),
            input_cursor: 2,
            completion: Some("g".to_string()),
            options: vec!["bug".to_string(), "feature".to_string()],
            selected: vec!["feature".to_string()],
            highlighted: 0,
            visible_indices: vec![0],
            visible_start: 0,
        }));

        assert!(rendered.contains("Edit task: labels"));
        assert!(rendered.contains("feature"));
        assert!(rendered.contains("bu"));
        assert!(rendered.contains("bug"));
    }

    #[test]
    fn edit_project_uses_structured_project_picker() {
        for (route, title) in [
            (OverlayRoute::EditProject, "Edit project"),
            (OverlayRoute::AddTaskTitleProject, "Add task: project"),
        ] {
            let rendered = render_overlay_view(OverlayView::Picker(PickerView {
                route,
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
        assert!(rendered.contains("main db size"));
        assert!(rendered.contains("4.0 MiB"));
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
                        scroll: 8,
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
            pending_changes: 4,
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
        assert!(rendered.contains("2026-06-25T10:20:00Z"));
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

mod route_specific_rendering {
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
                purpose: SearchPurpose::Navigate,
            },
            OverlayView::AddTask(AddTaskView {
                title: "ship dialogs".to_string(),
                title_cursor: 12,
                priority: "high".to_string(),
                ..add_task_view()
            }),
            OverlayView::TextInput(TextInputView {
                route: OverlayRoute::MessageOnly,
                title: "Edit title".to_string(),
                prompt: "New title".to_string(),
                input: "alpha".to_string(),
                cursor: 5,
            }),
            OverlayView::MultilineInput(MultilineInputView {
                route: OverlayRoute::MessageOnly,
                title: "Description".to_string(),
                prompt: "Body".to_string(),
                lines: vec!["line one".to_string()],
                row: 0,
                column: 4,
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
            OverlayView::TagCombobox(TagComboboxView {
                route: OverlayRoute::EditLabels,
                title: "Labels".to_string(),
                input: String::new(),
                input_cursor: 0,
                completion: None,
                options: vec!["bug".to_string()],
                selected: Vec::new(),
                highlighted: 0,
                visible_indices: vec![0],
                visible_start: 0,
            }),
            OverlayView::Confirm(ConfirmView {
                route: OverlayRoute::MessageOnly,
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
    fn add_note_route_uses_specialized_renderer_with_changed_title() {
        let rendered = render_overlay_view(OverlayView::MultilineInput(MultilineInputView {
            route: OverlayRoute::AddNote,
            title: "Changed note title".to_string(),
            prompt: "note body:".to_string(),
            lines: vec![String::new()],
            row: 0,
            column: 0,
        }));
        assert!(rendered.contains("Changed note title"));
        assert!(rendered.contains("note body"));
        assert!(rendered.contains("Ctrl+S submit"));
        assert!(rendered.contains("ote body"));
    }

    #[test]
    fn edit_description_route_uses_specialized_renderer_with_changed_title() {
        let rendered = render_overlay_view(OverlayView::MultilineInput(MultilineInputView {
            route: OverlayRoute::EditDescription,
            title: "Changed description title".to_string(),
            prompt: String::new(),
            lines: vec!["a".repeat(160)],
            row: 0,
            column: 150,
        }));
        assert!(rendered.contains("Changed description title"));
        assert!(rendered.contains("Ctrl+X Ctrl+E editor"));
        assert!(rendered.contains("line 1/1"));
    }

    #[test]
    fn add_task_description_route_uses_specialized_renderer_with_changed_title() {
        let rendered = render_overlay_view(OverlayView::MultilineInput(MultilineInputView {
            route: OverlayRoute::AddTaskDescription,
            title: "Changed add task description".to_string(),
            prompt: String::new(),
            lines: vec![String::new()],
            row: 0,
            column: 0,
        }));
        assert!(rendered.contains("Changed add task description"));
        assert!(rendered.contains("Optional details, links, or handoff context..."));
        assert!(rendered.contains("Enter newline"));
    }

    #[test]
    fn project_picker_routes_control_submit_hints_with_changed_titles() {
        for (route, title, hint) in [
            (
                OverlayRoute::ScopeProject,
                "Changed scope title",
                "Enter scope",
            ),
            (
                OverlayRoute::EditProject,
                "Changed edit title",
                "Enter submit",
            ),
            (
                OverlayRoute::AddTaskTitleProject,
                "Changed add-task project title",
                "Enter submit",
            ),
            (
                OverlayRoute::DeleteProjectPicker,
                "Changed delete title",
                "Enter delete",
            ),
        ] {
            let rendered = render_overlay_view(OverlayView::Picker(PickerView {
                route,
                title: title.to_string(),
                items: vec![picker_item("AVN aven", "aven")],
                ..project_picker_view()
            }));
            assert!(rendered.contains(title), "{route:?}");
            assert!(rendered.contains("PREFIX"), "{route:?}");
            assert!(rendered.contains(hint), "{route:?}");
        }
    }

    #[test]
    fn priority_picker_route_controls_icon_rendering_with_changed_title() {
        let rendered = render_overlay_view(OverlayView::Picker(PickerView {
            route: OverlayRoute::EditPriority,
            title: "Changed priority title".to_string(),
            items: vec![picker_item("urgent", "urgent")],
            visible_indices: vec![0],
            ..picker_view()
        }));
        assert!(rendered.contains("Changed priority title"));
        assert!(rendered.contains(priority_icon("urgent")));
    }

    #[test]
    fn add_task_priority_route_uses_priority_renderer() {
        let rendered = render_overlay_view(OverlayView::Picker(PickerView {
            route: OverlayRoute::AddTaskTitlePriority,
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
            route: OverlayRoute::MessageOnly,
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
            route: OverlayRoute::MessageOnly,
            title: "Delete task".to_string(),
            prompt: prompt.clone(),
        });
        let buffer = overlay_buffer(overlay);

        for row in 0..buffer.area.height {
            assert!(!buffer_row(&buffer, row).contains(&prompt));
        }
        assert!(buffer_text_from_rows(&buffer).contains("y yes"));
    }

    fn buffer_text_from_rows(buffer: &ratatui::buffer::Buffer) -> String {
        (0..buffer.area.height)
            .map(|row| buffer_row(buffer, row))
            .collect::<Vec<_>>()
            .join("\n")
    }
}
