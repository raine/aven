use ratatui::Frame;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;

use super::dialog::{Dialog, dialog_hint_line};
use super::input::{
    InputWidth, clipped_input_line, cursor_cell, input_cursor_spans, input_line,
    prefixed_input_line,
};
use super::truncate::truncate_chars;
use crate::tui::overlay::{
    ConfirmView, MultilineInputView, PickerItem, PickerView, TextInputView, TextPanelView,
};
use crate::tui::theme::{self, ACCENT, BG_ALT, BG_PANEL, FG, FG_DIM, FG_MUTED, SELECTED};
use crate::tui::widgets::priority_icon;

pub(super) fn render_search(frame: &mut Frame, input: &str, cursor: usize) {
    Dialog::new("Search", 54, 3).render_text(frame, input_line("/", input, cursor));
}

pub(super) fn render_text_input(frame: &mut Frame, state: &TextInputView) {
    if let Some((project, priority)) = add_task_title_metadata(&state.title) {
        let dialog = Dialog::new("Add task", 60, 5);
        let width = dialog.area(frame).width;
        let dialog = dialog.right_title(add_task_metadata_title(project, priority, width));
        let content = dialog.render_block(frame);
        let input = add_task_title_input_line(&state.input, state.cursor, content.width as usize);
        let text = Text::from(vec![input, Line::from(""), add_task_hint_line()]);
        frame.render_widget(
            Paragraph::new(text).style(Style::new().fg(FG).bg(BG_ALT)),
            content,
        );
        return;
    }

    let text = Text::from(vec![
        Line::from(Span::styled(&state.prompt, Style::new().fg(FG_DIM))),
        input_line("", &state.input, state.cursor),
        Line::from(""),
        dialog_hint_line(&[("Enter", "submit"), ("Esc", "cancel")]),
    ]);
    Dialog::new(&state.title, 54, 6).render_text(frame, text);
}

fn add_task_title_metadata(title: &str) -> Option<(&str, &str)> {
    let value = title.strip_prefix("Add task  project=")?;
    value.split_once(" priority=")
}

fn add_task_title_input_line(input: &str, cursor: usize, width: usize) -> Line<'static> {
    if input.is_empty() {
        return Line::from(vec![
            cursor_cell("t"),
            Span::styled("itle", Style::new().fg(FG_DIM)),
        ]);
    }
    clipped_input_line(input, cursor, width)
}

fn add_task_hint_line() -> Line<'static> {
    dialog_hint_line(&[
        ("Enter", "create"),
        ("Tab", "project"),
        ("Ctrl+P", "priority"),
        ("Esc", "cancel"),
    ])
}

fn add_task_metadata_title(project: &str, priority: &str, width: u16) -> Line<'static> {
    let value_width = (width as usize).saturating_sub(24).max(4) / 2;
    let value_style = Style::new().fg(Color::Rgb(194, 174, 255));
    Line::from(vec![
        Span::styled(" project: ", Style::new().fg(FG_MUTED)),
        Span::styled(truncate_chars(project, value_width), value_style),
        Span::styled(" · ", Style::new().fg(FG_DIM)),
        Span::styled("prio: ", Style::new().fg(FG_MUTED)),
        Span::styled(truncate_chars(priority, value_width), value_style),
    ])
}

pub(super) fn render_multiline_input(frame: &mut Frame, state: &MultilineInputView) {
    if state.title == "Add note" {
        render_add_note_input(frame, state);
        return;
    }

    let visible_rows = 10usize;
    let content_rows = state.lines.len().min(visible_rows).max(1);
    let height = (content_rows as u16).saturating_add(5).min(16);
    let start = state.row.saturating_sub(visible_rows.saturating_sub(1));
    let mut lines = vec![Line::from(Span::styled(
        &state.prompt,
        Style::new().fg(FG_DIM),
    ))];
    for (row_index, line) in state
        .lines
        .iter()
        .enumerate()
        .skip(start)
        .take(visible_rows)
    {
        if row_index == state.row {
            lines.push(input_line("", line, state.column));
        } else {
            lines.push(Line::from(line.clone()));
        }
    }
    lines.push(Line::from(""));
    lines.push(multiline_hint_line());
    Dialog::new(&state.title, 60, height.saturating_add(1))
        .wrap()
        .render_text(frame, Text::from(lines));
}

fn render_add_note_input(frame: &mut Frame, state: &MultilineInputView) {
    let visible_rows = 8usize;
    let content_rows = state.lines.len().min(visible_rows).max(1);
    let height = (content_rows as u16).saturating_add(4).min(13);
    let start = state.row.saturating_sub(visible_rows.saturating_sub(1));
    let mut lines = Vec::new();
    for (row_index, line) in state
        .lines
        .iter()
        .enumerate()
        .skip(start)
        .take(visible_rows)
    {
        lines.push(add_note_input_line(
            line,
            if row_index == state.row {
                Some(state.column)
            } else {
                None
            },
        ));
    }
    lines.push(Line::from(""));
    lines.push(multiline_hint_line());
    Dialog::new("Add note", 60, height)
        .wrap()
        .render_text(frame, Text::from(lines));
}

fn add_note_input_line(line: &str, cursor: Option<usize>) -> Line<'static> {
    if line.is_empty() && cursor.is_some() {
        return Line::from(vec![
            cursor_cell("n"),
            Span::styled("ote body", Style::new().fg(FG_DIM)),
        ]);
    }
    match cursor {
        Some(cursor) => Line::from(input_cursor_spans(line, cursor, InputWidth::Full)),
        None => Line::from(line.to_string()),
    }
}

fn multiline_hint_line() -> Line<'static> {
    dialog_hint_line(&[("Ctrl+S", "submit"), ("Esc", "cancel")])
}

pub(super) fn render_picker(frame: &mut Frame, state: &PickerView) {
    if let Some(submit_label) = project_picker_submit_label(&state.title) {
        render_project_picker(frame, state, submit_label);
        return;
    }

    let visible_count = state.visible_indices.len().max(1);
    let viewport_rows = 8usize;
    let height = (visible_count.min(viewport_rows) as u16).saturating_add(5);
    let selected_position = state
        .visible_indices
        .iter()
        .position(|index| *index == state.selected)
        .unwrap_or(0);
    let start = selected_position.saturating_sub(viewport_rows.saturating_sub(1));
    let mut lines = vec![
        input_line("/", &state.filter, state.filter_cursor),
        Line::from(""),
    ];
    for index in state.visible_indices.iter().skip(start).take(viewport_rows) {
        let item = &state.items[*index];
        let marker = if *index == state.selected {
            "▸ "
        } else {
            "  "
        };
        let check = if state.multi && item.selected {
            " ✓"
        } else {
            ""
        };
        if state.title == "Edit task: priority" {
            lines.push(priority_picker_line(item, *index == state.selected));
        } else {
            lines.push(Line::from(format!("{marker}{}{check}", item.label)));
        }
    }
    lines.push(Line::from(""));
    lines.push(picker_hint_line(state.multi, "submit"));
    Dialog::new(&state.title, 60, height.saturating_add(1)).render_text(frame, Text::from(lines));
}

fn priority_picker_line(item: &PickerItem, selected: bool) -> Line<'static> {
    let marker = if selected { "▸ " } else { "  " };
    Line::from(vec![
        Span::raw(marker),
        Span::styled(
            format!("{} ", priority_icon(&item.value)),
            theme::priority_style(&item.value).add_modifier(Modifier::BOLD),
        ),
        Span::styled(item.label.clone(), theme::priority_style(&item.value)),
    ])
}

fn picker_hint_line(multi: bool, submit_label: &str) -> Line<'static> {
    let mut items = vec![("Up/Down", "move"), ("Ctrl+N/P", "move")];
    if multi {
        items.push(("Space", "toggle"));
    }
    items.extend([("Enter", submit_label), ("Esc", "cancel")]);
    dialog_hint_line(&items)
}

fn render_project_picker(frame: &mut Frame, state: &PickerView, submit_label: &'static str) {
    let viewport_rows = 10usize;
    let height = (viewport_rows as u16).saturating_add(6);
    let selected_position = state
        .visible_indices
        .iter()
        .position(|index| *index == state.selected)
        .unwrap_or(0);
    let start = selected_position.saturating_sub(viewport_rows.saturating_sub(1));
    let mut lines = vec![
        prefixed_input_line(
            Span::styled("/", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
            &state.filter,
            state.filter_cursor,
        ),
        Line::from(vec![
            Span::styled("  PREFIX ", Style::new().fg(FG_DIM).bg(BG_PANEL)),
            Span::styled("PROJECT", Style::new().fg(FG_DIM).bg(BG_PANEL)),
        ]),
    ];
    let list_start = lines.len();
    for index in state.visible_indices.iter().skip(start).take(viewport_rows) {
        lines.push(project_picker_line(
            &state.items[*index],
            *index == state.selected,
        ));
    }
    if state.visible_indices.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no matching projects",
            Style::new().fg(FG_DIM),
        )));
    }
    while lines.len().saturating_sub(list_start) < viewport_rows {
        lines.push(Line::from(""));
    }
    lines.push(Line::from(""));
    lines.push(project_picker_hint_line(submit_label));
    Dialog::new(&state.title, 70, height).render_text(frame, Text::from(lines));
}

fn project_picker_submit_label(title: &str) -> Option<&'static str> {
    match title {
        "Go: project" => Some("open"),
        "Delete project" => Some("delete"),
        _ => None,
    }
}

fn project_picker_line(item: &PickerItem, selected: bool) -> Line<'static> {
    let (prefix, name) = item
        .label
        .split_once(' ')
        .unwrap_or((item.label.as_str(), item.value.as_str()));
    let marker = if selected { "▸" } else { " " };
    let row_style = if selected {
        SELECTED
    } else {
        Style::new().bg(BG_ALT)
    };
    let project_style = Style::new()
        .fg(theme::project_color(&item.value))
        .add_modifier(Modifier::BOLD)
        .bg(row_style.bg.unwrap_or(BG_ALT));
    let name_style = Style::new()
        .fg(if selected { FG } else { FG_MUTED })
        .bg(row_style.bg.unwrap_or(BG_ALT));
    Line::from(vec![
        Span::styled(format!("{marker} "), row_style),
        Span::styled(format!("{prefix:<7}"), project_style),
        Span::styled(" ", row_style),
        Span::styled(name.to_string(), name_style),
    ])
}

fn project_picker_hint_line(submit_label: &'static str) -> Line<'static> {
    picker_hint_line(false, submit_label)
}

pub(super) fn render_confirm(frame: &mut Frame, state: &ConfirmView) {
    let width = state.prompt.chars().count().saturating_add(4).max(32) as u16;
    let text = Text::from(vec![
        Line::from(state.prompt.as_str()),
        Line::from(""),
        confirm_hint_line(),
    ]);
    Dialog::new(&state.title, width, 5).render_text(frame, text);
}

fn confirm_hint_line() -> Line<'static> {
    dialog_hint_line(&[("y", "yes"), ("n", "no"), ("Esc", "cancel")])
}

pub(super) fn render_text_panel(frame: &mut Frame, state: &TextPanelView) {
    let visible_rows = 12usize;
    let content_rows = state.lines.len().min(visible_rows).max(1);
    let height = (content_rows as u16).saturating_add(4).min(16);
    let start = (state.scroll as usize).min(state.lines.len().saturating_sub(1));
    let mut lines = state
        .lines
        .iter()
        .skip(start)
        .take(visible_rows)
        .map(|line| Line::from(line.as_str()))
        .collect::<Vec<_>>();
    lines.push(dialog_hint_line(&[
        ("j/k", "scroll"),
        ("Enter/Esc", "close"),
    ]));
    Dialog::new(&state.title, 60, height)
        .wrap()
        .render_text(frame, Text::from(lines));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::overlay::OverlayView;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

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
            OverlayView::Search { input, cursor } => render_search(frame, input, *cursor),
            OverlayView::TextInput(state) => render_text_input(frame, state),
            OverlayView::MultilineInput(state) => render_multiline_input(frame, state),
            OverlayView::Picker(state) => render_picker(frame, state),
            OverlayView::Confirm(state) => render_confirm(frame, state),
            OverlayView::TextPanel(state) => render_text_panel(frame, state),
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
        });
        assert!(rendered.contains("Search"));
        assert!(rendered.contains("/query"));
    }

    #[test]
    fn overlay_render_includes_text_input_prompt_and_hints() {
        let rendered = render_overlay_view(OverlayView::TextInput(TextInputView {
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
    fn add_task_overlay_renders_metadata_title_and_footer() {
        let rendered = render_overlay_view(OverlayView::TextInput(TextInputView {
            title: "Add task  project=aven priority=high".to_string(),
            prompt: "Title".to_string(),
            input: "ship dialogs".to_string(),
            cursor: 12,
        }));
        assert!(rendered.contains("Add task"));
        assert!(rendered.contains("project: aven"));
        assert!(rendered.contains("prio: high"));
        assert!(rendered.contains("ship dialogs"));
        assert!(rendered.contains("Ctrl+P priority"));
    }

    #[test]
    fn hint_lines_style_keys() {
        let add_task_keys = styled_key_contents(add_task_hint_line());
        assert_eq!(add_task_keys, vec!["Enter", "Tab", "Ctrl+P", "Esc"]);

        let multiline_keys = styled_key_contents(multiline_hint_line());
        assert_eq!(multiline_keys, vec!["Ctrl+S", "Esc"]);

        let confirm_keys = styled_key_contents(confirm_hint_line());
        assert_eq!(confirm_keys, vec!["y", "n", "Esc"]);
    }

    fn styled_key_contents(line: Line<'static>) -> Vec<String> {
        line.spans
            .iter()
            .filter(|span| span.style.fg == Some(FG))
            .map(|span| span.content.to_string())
            .collect()
    }

    #[test]
    fn add_task_empty_title_input_shows_placeholder() {
        let line = add_task_title_input_line("", 0, 20);
        assert_eq!(line.spans[0].content.as_ref(), "t");
        assert_eq!(line.spans[0].style.fg, Some(BG_ALT));
        assert_eq!(line.spans[0].style.bg, Some(FG));
        assert_eq!(line.spans[1].content.as_ref(), "itle");
        assert_eq!(line.spans[1].style.fg, Some(FG_DIM));
    }

    #[test]
    fn add_task_title_input_draws_cursor_as_cell() {
        let line = add_task_title_input_line("abc", 1, 20);
        assert_eq!(line.spans[0].content.as_ref(), "a");
        assert_eq!(line.spans[1].content.as_ref(), "b");
        assert_eq!(line.spans[1].style.fg, Some(BG_ALT));
        assert_eq!(line.spans[1].style.bg, Some(FG));
        assert_eq!(line.spans[2].content.as_ref(), "c");
    }

    #[test]
    fn add_task_title_input_draws_end_cursor_as_blank_cell() {
        let line = add_task_title_input_line("abc", 3, 20);
        assert_eq!(line.spans[0].content.as_ref(), "abc");
        assert_eq!(line.spans[1].content.as_ref(), " ");
        assert_eq!(line.spans[1].style.bg, Some(FG));
    }

    #[test]
    fn add_task_title_input_scrolls_to_cursor_cell() {
        let line = add_task_title_input_line("abcdef", 5, 4);
        assert_eq!(line.spans[0].content.as_ref(), "cde");
        assert_eq!(line.spans[1].content.as_ref(), "f");
    }

    #[test]
    fn add_task_metadata_title_labels_values() {
        let rendered = add_task_metadata_title("aven", "none", 60).to_string();
        assert!(rendered.contains("project: aven"));
        assert!(rendered.contains("prio: none"));
        assert!(rendered.contains(" · "));
        assert!(!rendered.contains("Tab"));
        assert!(!rendered.contains("Ctrl+P"));
    }

    #[test]
    fn overlay_render_includes_multiline_ctrl_s_hint() {
        let rendered = render_overlay_view(OverlayView::MultilineInput(MultilineInputView {
            title: "Description".to_string(),
            prompt: "Body".to_string(),
            lines: vec!["line one".to_string()],
            row: 0,
            column: 4,
        }));
        assert!(rendered.contains("Description"));
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
    fn multiline_hint_styles_keys() {
        let line = multiline_hint_line();
        let keys = line
            .spans
            .iter()
            .filter(|span| span.style.fg == Some(FG))
            .map(|span| span.content.as_ref())
            .collect::<Vec<_>>();
        assert_eq!(keys, vec!["Ctrl+S", "Esc"]);
    }

    #[test]
    fn add_note_overlay_uses_placeholder_key_styles_and_spacing() {
        let overlay = OverlayView::MultilineInput(MultilineInputView {
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

    #[test]
    fn overlay_render_includes_picker_filter_and_hints() {
        let rendered = render_overlay_view(OverlayView::Picker(PickerView {
            title: "Project".to_string(),
            filter: "app".to_string(),
            filter_cursor: 3,
            items: vec![PickerItem {
                label: "APP app".to_string(),
                value: "app".to_string(),
                selected: false,
            }],
            selected: 0,
            multi: true,
            visible_indices: vec![0],
        }));
        assert!(rendered.contains("Project"));
        assert!(rendered.contains("/app"));
        assert!(rendered.contains("Ctrl+N/P"));
        assert!(rendered.contains("Space"));
        assert!(rendered.contains("toggle"));
    }

    #[test]
    fn priority_picker_shows_priority_icons() {
        let rendered = render_overlay_view(OverlayView::Picker(PickerView {
            title: "Edit task: priority".to_string(),
            filter: String::new(),
            filter_cursor: 0,
            items: vec![PickerItem {
                label: "urgent".to_string(),
                value: "urgent".to_string(),
                selected: false,
            }],
            selected: 0,
            multi: false,
            visible_indices: vec![0],
        }));
        assert!(rendered.contains(priority_icon("urgent")));
        assert!(rendered.contains("urgent"));
        assert!(rendered.contains("Enter"));
        assert!(rendered.contains("submit"));
    }

    #[test]
    fn picker_viewport_keeps_selected_item_visible() {
        let items = (0..12)
            .map(|index| PickerItem {
                label: format!("Item {index}"),
                value: index.to_string(),
                selected: false,
            })
            .collect::<Vec<_>>();
        let rendered = render_overlay_view(OverlayView::Picker(PickerView {
            title: "Project".to_string(),
            filter: String::new(),
            filter_cursor: 0,
            items,
            selected: 10,
            multi: false,
            visible_indices: (0..12).collect(),
        }));
        assert!(rendered.contains("▸ Item 10"));
        assert!(!rendered.contains("Item 0"));
    }

    #[test]
    fn project_picker_uses_structured_columns() {
        let rendered = render_overlay_view(OverlayView::Picker(PickerView {
            title: "Go: project".to_string(),
            filter: "claude".to_string(),
            filter_cursor: 6,
            items: vec![PickerItem {
                label: "CC claude-code".to_string(),
                value: "claude-code".to_string(),
                selected: false,
            }],
            selected: 0,
            multi: false,
            visible_indices: vec![0],
        }));
        assert!(rendered.contains("PREFIX"));
        assert!(rendered.contains("PROJECT"));
        assert!(rendered.contains("CC"));
        assert!(rendered.contains("claude-code"));
        assert!(rendered.contains("Enter open"));
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
}
