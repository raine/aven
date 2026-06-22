use ratatui::Frame;
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;

use super::super::dialog::{Dialog, dialog_hint_line};
use super::super::input::{InputWidth, input_cursor_spans, input_line};
use super::shared::{tail_viewport_start, viewport_start_for_cursor};
use crate::tui::overlay::MultilineInputView;
use crate::tui::text::{char_boundary_at_or_before, char_count_ranges, char_count_segment_index};
use crate::tui::theme::{FG, FG_DIM, FG_MUTED};

pub(in crate::tui::ui) fn render_multiline_input(frame: &mut Frame, state: &MultilineInputView) {
    match state.route {
        crate::tui::overlay::OverlayRoute::AddNote => {
            render_add_note_input(frame, state);
            return;
        }
        crate::tui::overlay::OverlayRoute::EditDescription => {
            render_description_input(frame, state);
            return;
        }
        crate::tui::overlay::OverlayRoute::AddTaskDescription => {
            render_add_task_description_input(frame, state);
            return;
        }
        crate::tui::overlay::OverlayRoute::AddTaskNatural => {
            render_add_task_natural_input(frame, state);
            return;
        }
        _ => {}
    }

    let visible_rows = 10usize;
    let content_rows = state.lines.len().min(visible_rows).max(1);
    let prompt_rows = usize::from(!state.prompt.is_empty());
    let height = (content_rows + prompt_rows + 4).min(16) as u16;
    let start = tail_viewport_start(state.row, visible_rows);
    let mut lines = Vec::new();
    if !state.prompt.is_empty() {
        lines.push(Line::from(Span::styled(
            &state.prompt,
            Style::new().fg(FG_DIM),
        )));
    }
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
    Dialog::new(&state.title, 60, height)
        .wrap()
        .render_text(frame, Text::from(lines));
}

fn render_description_input(frame: &mut Frame, state: &MultilineInputView) {
    let frame_area = frame.area();
    let max_height = frame_area.height.saturating_mul(4).saturating_div(5);
    let width = description_editor_width(frame_area.width);
    let line_width = width.saturating_sub(4).max(1) as usize;
    let max_editor_rows = max_height.saturating_sub(4).max(1) as usize;
    let editor_rows = description_visual_row_count(state, line_width).clamp(4, max_editor_rows);
    let height = editor_rows.saturating_add(4) as u16;
    let dialog = Dialog::new(&state.title, width, height);
    let content = dialog.render_block(frame);
    let editor_rows = content.height.saturating_sub(2).max(1) as usize;
    let line_width = content.width.max(1) as usize;
    let (mut lines, cursor_row) = description_editor_lines(state, line_width);
    let start = viewport_start_for_cursor(cursor_row, editor_rows, lines.len(), true);
    lines = lines.into_iter().skip(start).take(editor_rows).collect();
    while lines.len() < editor_rows {
        lines.push(Line::from(""));
    }
    lines.push(Line::from(""));
    lines.push(description_hint_line(state));

    frame.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::new().fg(FG).bg(crate::tui::theme::BG_ALT)),
        content,
    );
}

fn render_add_task_description_input(frame: &mut Frame, state: &MultilineInputView) {
    render_add_task_free_text_input(
        frame,
        state,
        "Optional details, links, or handoff context...",
        add_task_description_hint_line(),
    );
}

fn render_add_task_natural_input(frame: &mut Frame, state: &MultilineInputView) {
    render_add_task_free_text_input(
        frame,
        state,
        "Describe the task in natural language...",
        add_task_natural_hint_line(),
    );
}

fn render_add_task_free_text_input(
    frame: &mut Frame,
    state: &MultilineInputView,
    placeholder: &'static str,
    hint_line: Line<'static>,
) {
    let visible_rows = 8usize;
    let content_rows = state.lines.len().min(visible_rows).max(1);
    let height = (content_rows as u16).saturating_add(4).min(13);
    let start = tail_viewport_start(state.row, visible_rows);
    let mut lines = Vec::new();
    for (row_index, line) in state
        .lines
        .iter()
        .enumerate()
        .skip(start)
        .take(visible_rows)
    {
        lines.push(add_task_free_text_input_line(
            line,
            if row_index == state.row {
                Some(state.column)
            } else {
                None
            },
            line.is_empty() && state.lines.len() == 1,
            placeholder,
        ));
    }
    lines.push(Line::from(""));
    lines.push(hint_line);
    Dialog::new(&state.title, 70, height)
        .wrap()
        .render_text(frame, Text::from(lines));
}

pub(in crate::tui::ui) fn description_editor_width(frame_width: u16) -> u16 {
    let max_width = frame_width.saturating_sub(4).max(1);
    frame_width
        .saturating_mul(4)
        .saturating_div(5)
        .max(80)
        .min(max_width)
}

pub(in crate::tui::ui) fn description_visual_row_count(
    state: &MultilineInputView,
    line_width: usize,
) -> usize {
    state
        .lines
        .iter()
        .map(|line| char_count_ranges(line, line_width).len())
        .sum::<usize>()
        .max(1)
}

pub(in crate::tui::ui) fn description_editor_lines(
    state: &MultilineInputView,
    line_width: usize,
) -> (Vec<Line<'static>>, usize) {
    let mut lines = Vec::new();
    let mut cursor_row = 0;
    let show_placeholder = state.lines.len() == 1 && state.lines[0].is_empty();
    for (row_index, line) in state.lines.iter().enumerate() {
        let ranges = char_count_ranges(line, line_width);
        if row_index == state.row {
            let cursor = char_boundary_at_or_before(line, state.column);
            let cursor_segment = char_count_segment_index(line, cursor, line_width);
            cursor_row = lines.len().saturating_add(cursor_segment);
            for (range_index, (start, end)) in ranges.into_iter().enumerate() {
                if range_index == cursor_segment {
                    lines.push(description_input_line(
                        &line[start..end],
                        cursor - start,
                        show_placeholder,
                    ));
                } else {
                    lines.push(Line::from(line[start..end].to_string()));
                }
            }
        } else {
            for (start, end) in ranges {
                lines.push(Line::from(line[start..end].to_string()));
            }
        }
    }
    (lines, cursor_row)
}

pub(in crate::tui::ui) fn description_input_line(
    line: &str,
    cursor: usize,
    show_placeholder: bool,
) -> Line<'static> {
    if show_placeholder && line.is_empty() && cursor == 0 {
        return Line::from(vec![
            super::super::input::cursor_cell("E"),
            Span::styled("nter task description here...", Style::new().fg(FG_DIM)),
        ]);
    }
    input_line("", line, cursor)
}

pub(in crate::tui::ui) fn add_task_description_input_line(
    line: &str,
    cursor: Option<usize>,
    show_placeholder: bool,
) -> Line<'static> {
    add_task_free_text_input_line(
        line,
        cursor,
        show_placeholder,
        "Optional details, links, or handoff context...",
    )
}

fn add_task_free_text_input_line(
    line: &str,
    cursor: Option<usize>,
    show_placeholder: bool,
    placeholder: &'static str,
) -> Line<'static> {
    if show_placeholder {
        if cursor.is_some() {
            return Line::from(vec![
                super::super::input::cursor_cell(&placeholder[..1]),
                Span::styled(&placeholder[1..], Style::new().fg(FG_DIM)),
            ]);
        }
        return Line::from(Span::styled(placeholder, Style::new().fg(FG_DIM)));
    }
    match cursor {
        Some(cursor) => Line::from(input_cursor_spans(line, cursor, InputWidth::Full)),
        None => Line::from(line.to_string()),
    }
}

pub(in crate::tui::ui) fn add_task_description_hint_line() -> Line<'static> {
    dialog_hint_line(&[
        ("Ctrl+S", "create"),
        ("Enter", "newline"),
        ("Ctrl+P", "project"),
        ("Ctrl+R", "priority"),
        ("Esc", "cancel"),
    ])
}

pub(in crate::tui::ui) fn add_task_natural_hint_line() -> Line<'static> {
    dialog_hint_line(&[
        ("Ctrl+S", "parse"),
        ("Enter", "newline"),
        ("Esc", "cancel"),
    ])
}

pub(in crate::tui::ui) fn render_add_note_input(frame: &mut Frame, state: &MultilineInputView) {
    let visible_rows = 8usize;
    let content_rows = state.lines.len().min(visible_rows).max(1);
    let height = (content_rows as u16).saturating_add(4).min(13);
    let start = tail_viewport_start(state.row, visible_rows);
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
    Dialog::new(&state.title, 60, height)
        .wrap()
        .render_text(frame, Text::from(lines));
}

pub(in crate::tui::ui) fn add_note_input_line(line: &str, cursor: Option<usize>) -> Line<'static> {
    if line.is_empty() && cursor.is_some() {
        return Line::from(vec![
            super::super::input::cursor_cell("n"),
            Span::styled("ote body", Style::new().fg(FG_DIM)),
        ]);
    }
    match cursor {
        Some(cursor) => Line::from(input_cursor_spans(line, cursor, InputWidth::Full)),
        None => Line::from(line.to_string()),
    }
}

pub(in crate::tui::ui) fn multiline_hint_line() -> Line<'static> {
    dialog_hint_line(&[("Ctrl+S", "submit"), ("Esc", "cancel")])
}

pub(in crate::tui::ui) fn description_hint_line(state: &MultilineInputView) -> Line<'static> {
    let position = format!(
        "  line {}/{}",
        state.row.saturating_add(1),
        state.lines.len().max(1)
    );
    let mut line = dialog_hint_line(&[
        ("Ctrl+S", "submit"),
        ("Ctrl+X Ctrl+E", "editor"),
        ("Esc", "cancel"),
    ]);
    line.spans
        .push(Span::styled(position, Style::new().fg(FG_MUTED)));
    line
}
