use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;

use super::super::dialog::{Dialog, dialog_hint_line};
use super::super::input::{clipped_input_line, cursor_cell};
use super::super::truncate::truncate_chars;
use super::multiline::add_task_description_input_line;
use super::shared::viewport_start_for_cursor;
use crate::tui::app::LoadingState;
use crate::tui::authoring::AddTaskStep;
use crate::tui::overlay::AddTaskView;
use crate::tui::text::cell_width_ranges;
use crate::tui::theme::{self, FG, FG_DIM, FG_MUTED};

pub(in crate::tui::ui) fn render_add_task(frame: &mut Frame, state: &AddTaskView) {
    let expanded =
        add_task_description_has_content(state) || state.focus == AddTaskStep::Description;
    let height = if expanded {
        frame.area().height.saturating_sub(4).clamp(11, 18)
    } else {
        11
    };
    let dialog = Dialog::new("Add task", 100, height);
    let width = dialog.area(frame).width;
    let content = dialog
        .right_title(add_task_metadata_title(
            &state.project,
            &state.status,
            &state.priority,
            width,
        ))
        .render_block(frame);
    render_add_task_body(frame, state, content, None);
}

pub(in crate::tui::ui) fn render_add_task_full_frame(
    frame: &mut Frame,
    state: &AddTaskView,
    loading: Option<&LoadingState>,
) {
    let area = frame.area();
    let content = Dialog::new("Add task", area.width, area.height)
        .right_title(add_task_metadata_title(
            &state.project,
            &state.status,
            &state.priority,
            area.width,
        ))
        .render_block_at(frame, area);
    render_add_task_body(frame, state, content, loading);
}

fn render_add_task_body(
    frame: &mut Frame,
    state: &AddTaskView,
    content: Rect,
    loading: Option<&LoadingState>,
) {
    let description_rows = (content.height as usize).saturating_sub(5).max(1);
    let mut lines = vec![
        add_task_field_label("Title", state.focus == AddTaskStep::Title),
        add_task_title_input_line(
            &state.title,
            if state.focus == AddTaskStep::Title {
                Some(state.title_cursor)
            } else {
                None
            },
            content.width as usize,
        ),
        Line::from(""),
        add_task_field_label("Description", state.focus == AddTaskStep::Description),
    ];
    lines.extend(add_task_description_lines(
        state,
        description_rows,
        content.width as usize,
    ));
    while lines.len() + 2 < content.height as usize {
        lines.push(Line::from(""));
    }
    if let Some(loading) = loading {
        lines.push(add_task_loading_line(loading));
    } else if lines.len() + 1 < content.height as usize {
        lines.push(Line::from(""));
    }
    lines.push(add_task_hint_line(
        state.focus,
        state.status_prefix_active,
        state.priority_prefix_active,
    ));
    frame.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::new().fg(FG).bg(crate::tui::theme::BG_ALT)),
        content,
    );
}

pub(in crate::tui::ui) fn add_task_title_metadata(title: &str) -> Option<(&str, &str)> {
    let value = title.strip_prefix("Add task  project=")?;
    value.split_once(" priority=")
}

pub(in crate::tui::ui) const ADD_TASK_TITLE_PLACEHOLDER: &str = "Enter title here...";

pub(in crate::tui::ui) fn add_task_title_input_line(
    input: &str,
    cursor: Option<usize>,
    width: usize,
) -> Line<'static> {
    if input.is_empty() {
        if cursor.is_some() {
            return Line::from(vec![
                cursor_cell(&ADD_TASK_TITLE_PLACEHOLDER[..1]),
                Span::styled(&ADD_TASK_TITLE_PLACEHOLDER[1..], Style::new().fg(FG_DIM)),
            ]);
        }
        return Line::from(Span::styled(
            ADD_TASK_TITLE_PLACEHOLDER,
            Style::new().fg(FG_DIM),
        ));
    }
    match cursor {
        Some(cursor) => clipped_input_line(input, cursor, width),
        None => Line::from(input.to_string()),
    }
}

fn add_task_field_label(label: &'static str, active: bool) -> Line<'static> {
    let style = if active {
        Style::new()
            .fg(Color::Rgb(194, 174, 255))
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(FG_DIM)
    };
    Line::from(Span::styled(label, style))
}

pub(in crate::tui::ui) fn add_task_description_has_content(state: &AddTaskView) -> bool {
    state.description.iter().any(|line| !line.is_empty())
}

pub(in crate::tui::ui) fn add_task_description_lines(
    state: &AddTaskView,
    visible_rows: usize,
    width: usize,
) -> Vec<Line<'static>> {
    let show_placeholder = state.description.len() == 1 && state.description[0].is_empty();
    let mut visual_rows = Vec::new();
    for (row_index, line) in state.description.iter().enumerate() {
        visual_rows.extend(add_task_description_visual_lines(
            line,
            if state.focus == AddTaskStep::Description && row_index == state.description_row {
                Some(state.description_column)
            } else {
                None
            },
            show_placeholder && row_index == 0,
            width,
        ));
    }
    let cursor_visual_row = if state.focus == AddTaskStep::Description {
        visual_rows
            .iter()
            .position(|row| row.has_cursor)
            .unwrap_or_else(|| visual_rows.len().saturating_sub(1))
    } else {
        0
    };
    let start = viewport_start_for_cursor(
        cursor_visual_row,
        visible_rows,
        visual_rows.len(),
        state.focus == AddTaskStep::Description,
    );
    let end = start.saturating_add(visible_rows).min(visual_rows.len());
    let hidden_above = start > 0;
    let hidden_below = end < visual_rows.len();
    visual_rows
        .into_iter()
        .enumerate()
        .skip(start)
        .take(visible_rows)
        .map(|(index, row)| {
            let marker = match (
                index == start && hidden_above,
                index + 1 == end && hidden_below,
            ) {
                (true, true) => "↕ ",
                (true, false) => "↑ ",
                (false, true) => "↓ ",
                (false, false) => "  ",
            };
            add_task_description_viewport_line(marker, row.line)
        })
        .collect()
}

fn add_task_description_viewport_line(marker: &'static str, line: Line<'static>) -> Line<'static> {
    let mut spans = Vec::with_capacity(line.spans.len() + 1);
    spans.push(Span::styled(marker, Style::new().fg(FG_DIM)));
    spans.extend(line.spans);
    Line::from(spans)
}

pub(in crate::tui::ui) struct AddTaskDescriptionVisualLine {
    line: Line<'static>,
    has_cursor: bool,
}

pub(in crate::tui::ui) fn add_task_description_visual_lines(
    line: &str,
    cursor: Option<usize>,
    show_placeholder: bool,
    width: usize,
) -> Vec<AddTaskDescriptionVisualLine> {
    if show_placeholder {
        return vec![AddTaskDescriptionVisualLine {
            line: add_task_description_input_line(line, cursor, true),
            has_cursor: cursor.is_some(),
        }];
    }
    let width = width.saturating_sub(2).max(1);
    let chunks = cell_width_ranges(line, width);
    chunks
        .into_iter()
        .map(|(start, end)| {
            let cursor = cursor.filter(|cursor| *cursor >= start && *cursor <= end);
            AddTaskDescriptionVisualLine {
                line: add_task_description_input_line(
                    &line[start..end],
                    cursor.map(|cursor| cursor - start),
                    false,
                ),
                has_cursor: cursor.is_some(),
            }
        })
        .collect()
}

fn add_task_loading_line(loading: &LoadingState) -> Line<'static> {
    let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let frame_symbol = frames[loading.frame() % frames.len()];
    Line::from(vec![
        Span::styled(frame_symbol, Style::new().fg(Color::Rgb(194, 174, 255))),
        Span::styled(format!(" {}", loading.message), Style::new().fg(FG_MUTED)),
    ])
}

pub(in crate::tui::ui) fn add_task_status_hint_line() -> Line<'static> {
    dialog_hint_line(&[
        ("i", "inbox"),
        ("b", "backlog"),
        ("t", "todo"),
        ("a", "active"),
        ("d", "done"),
        ("x", "canceled"),
        ("Esc", "cancel"),
    ])
}

pub(in crate::tui::ui) fn add_task_priority_hint_line() -> Line<'static> {
    dialog_hint_line(&[
        ("n", "none"),
        ("l", "low"),
        ("m", "medium"),
        ("h", "high"),
        ("u", "urgent"),
        ("Esc", "cancel"),
    ])
}

pub(in crate::tui::ui) fn add_task_hint_line(
    focus: AddTaskStep,
    status_prefix_active: bool,
    priority_prefix_active: bool,
) -> Line<'static> {
    if status_prefix_active {
        return add_task_status_hint_line();
    }
    if priority_prefix_active {
        return add_task_priority_hint_line();
    }

    match focus {
        AddTaskStep::Title => dialog_hint_line(&[
            ("Enter", "create"),
            ("Tab", "description"),
            ("Ctrl+T", "status"),
            ("Ctrl+P", "project"),
            ("Ctrl+R", "priority"),
            ("Esc", "cancel"),
        ]),
        AddTaskStep::Description => dialog_hint_line(&[
            ("Ctrl+S", "create"),
            ("Ctrl+T", "status"),
            ("Tab", "title"),
            ("Ctrl+P", "project"),
            ("Ctrl+R", "priority"),
            ("Esc", "cancel"),
        ]),
    }
}

pub(in crate::tui::ui) fn add_task_metadata_title(
    project: &str,
    status: &str,
    priority: &str,
    width: u16,
) -> Line<'static> {
    let status_style = theme::status_style(status);
    let priority_style = theme::priority_style(priority);
    if width < 60 {
        return Line::from(vec![
            Span::styled(" status: ", Style::new().fg(FG_MUTED)),
            Span::styled(truncate_chars(status, 8), status_style),
            Span::styled(" · ", Style::new().fg(FG_DIM)),
            Span::styled("prio: ", Style::new().fg(FG_MUTED)),
            Span::styled(truncate_chars(priority, 6), priority_style),
        ]);
    }
    let value_width = (width as usize).saturating_sub(34).max(6) / 3;
    Line::from(vec![
        Span::styled(" project: ", Style::new().fg(FG_MUTED)),
        Span::styled(
            truncate_chars(project, value_width),
            Style::new().fg(theme::project_color(project)),
        ),
        Span::styled(" · ", Style::new().fg(FG_DIM)),
        Span::styled("status: ", Style::new().fg(FG_MUTED)),
        Span::styled(truncate_chars(status, value_width), status_style),
        Span::styled(" · ", Style::new().fg(FG_DIM)),
        Span::styled("prio: ", Style::new().fg(FG_MUTED)),
        Span::styled(truncate_chars(priority, value_width), priority_style),
    ])
}
