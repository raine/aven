use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::super::dialog::{Dialog, dialog_hint_line};
use super::super::input::{clipped_input_line, cursor_cell};
use super::super::scroll::{clamp_scroll_start, render_vertical_scrollbar};
use super::super::truncate::truncate_chars;
use super::confirm::render_confirm;
use super::multiline::add_task_description_input_line;
use super::picker::{
    picker_filter_line, picker_hint_line, priority_picker_line, project_picker_line,
};
use super::shared::viewport_start_for_cursor;
use super::tag_combobox::tag_combobox_lines;
use crate::tui::authoring::AddTaskStep;
use crate::tui::overlay::{
    AddTaskMode, AddTaskView, ConfirmView, OverlayRoute, PickerView, TAG_COMBOBOX_VIEWPORT_ROWS,
    TagComboboxView, tag_combobox_completion, tag_combobox_matches, visible_picker_indices,
};
use crate::tui::text::cell_width_ranges;
use crate::tui::theme::{self, BG_ALT, BG_PANEL, FG, FG_DIM, FG_MUTED, SELECTED};
use crate::tui::widgets::{priority_short, status_span};

pub(crate) fn add_task_field_at(
    terminal: Rect,
    full_frame: bool,
    column: u16,
    row: u16,
) -> Option<AddTaskStep> {
    let outer = if full_frame {
        terminal
    } else {
        crate::tui::overlay::dialog_area(
            terminal,
            100,
            terminal.height.saturating_sub(2).clamp(14, 22),
        )
    };
    let content = Rect {
        x: outer.x.saturating_add(2),
        y: outer.y.saturating_add(1),
        width: outer.width.saturating_sub(4),
        height: outer.height.saturating_sub(2),
    };
    if column < content.x || column >= content.right() || row < content.y || row >= content.bottom()
    {
        return None;
    }
    let relative_x = column.saturating_sub(content.x);
    let relative_y = row.saturating_sub(content.y);
    let metadata_rows = if content.width >= 120 {
        1
    } else if content.width >= 60 {
        2
    } else {
        6
    };
    if relative_y < metadata_rows {
        let index = if metadata_rows == 1 {
            (relative_x as usize * 6 / content.width.max(1) as usize).min(5)
        } else if metadata_rows == 2 {
            let row_index = relative_y as usize;
            row_index * 3 + (relative_x as usize * 3 / content.width.max(1) as usize).min(2)
        } else {
            relative_y as usize
        };
        return AddTaskStep::ALL.get(index).copied();
    }
    if content.height <= 10 {
        if relative_y == metadata_rows {
            return Some(AddTaskStep::Title);
        }
        if relative_y == metadata_rows + 1 {
            return Some(AddTaskStep::Description);
        }
        return None;
    }
    let title_y = metadata_rows + 1;
    if relative_y == title_y || relative_y == title_y + 1 {
        return Some(AddTaskStep::Title);
    }
    if relative_y >= title_y + 3 {
        return Some(AddTaskStep::Description);
    }
    None
}

pub(in crate::tui::ui) fn render_add_task(frame: &mut Frame, state: &AddTaskView) {
    let height = frame.area().height.saturating_sub(2).clamp(14, 22);
    let dialog = Dialog::new("Add task", 100, height);
    let content = dialog.render_block(frame);
    render_add_task_body(frame, state, content);
}

pub(in crate::tui::ui) fn render_add_task_full_frame(frame: &mut Frame, state: &AddTaskView) {
    let area = frame.area();
    let content = Dialog::new("Add task", area.width, area.height).render_block_at(frame, area);
    render_add_task_body(frame, state, content);
}

fn render_add_task_body(frame: &mut Frame, state: &AddTaskView, content: Rect) {
    let mut lines = add_task_metadata_lines(state, content.width);
    if content.height <= 10 {
        lines.push(compact_text_field(
            "Title",
            if state.title.is_empty() {
                ADD_TASK_TITLE_PLACEHOLDER
            } else {
                &state.title
            },
            state.focus == AddTaskStep::Title,
        ));
        lines.push(compact_text_field(
            "Description",
            state
                .description
                .first()
                .map(String::as_str)
                .filter(|value| !value.is_empty())
                .unwrap_or("Optional details..."),
            state.focus == AddTaskStep::Description,
        ));
        while lines.len() + 1 < content.height as usize {
            lines.push(Line::from(""));
        }
        lines.push(add_task_hint_line(
            state.focus,
            state.status_prefix_active,
            state.priority_prefix_active,
        ));
        frame.render_widget(
            Paragraph::new(Text::from(lines))
                .style(Style::new().fg(FG).bg(crate::tui::theme::BG_ALT)),
            content,
        );
        render_add_task_child(frame, state, content);
        return;
    }
    lines.push(Line::from(""));
    lines.push(add_task_field_label(
        "Title",
        state.focus == AddTaskStep::Title,
    ));
    lines.push(indent_add_task_input(add_task_title_input_line(
        &state.title,
        (state.focus == AddTaskStep::Title).then_some(state.title_cursor),
        (content.width as usize).saturating_sub(2),
    )));
    if state.title_error {
        lines.push(Line::from(Span::styled(
            "  Title is required",
            Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
        )));
    } else {
        lines.push(Line::from(""));
    }
    lines.push(add_task_field_label(
        "Description",
        state.focus == AddTaskStep::Description,
    ));
    let reserved = lines.len().saturating_add(1);
    let description_rows = (content.height as usize).saturating_sub(reserved).max(1);
    lines.extend(add_task_description_lines(
        state,
        description_rows,
        content.width as usize,
    ));
    while lines.len() + 1 < content.height as usize {
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
    render_add_task_child(frame, state, content);
}

fn indent_add_task_input(line: Line<'static>) -> Line<'static> {
    let mut spans = Vec::with_capacity(line.spans.len() + 1);
    spans.push(Span::raw("  "));
    spans.extend(line.spans);
    Line::from(spans)
}

fn compact_text_field(label: &str, value: &str, active: bool) -> Line<'static> {
    Line::from(vec![
        Span::styled(if active { "▶ " } else { "  " }, Style::new().fg(FG)),
        Span::styled(
            format!("{label}: "),
            if active {
                Style::new().fg(FG).add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(FG_DIM)
            },
        ),
        Span::raw(value.to_string()),
    ])
}

fn add_task_metadata_lines(state: &AddTaskView, width: u16) -> Vec<Line<'static>> {
    let fields = [
        (AddTaskStep::Project, "Project", state.project.clone()),
        (AddTaskStep::Status, "Status", state.status.clone()),
        (AddTaskStep::Priority, "Priority", state.priority.clone()),
        (AddTaskStep::Labels, "Labels", labels_display(&state.labels)),
    ];
    let mut owned = fields
        .into_iter()
        .map(|(field, label, value)| metadata_field(field, label, &value, state.focus))
        .collect::<Vec<_>>();
    owned.push(availability_metadata_field(state));
    owned.push(due_metadata_field(state));
    if width >= 120 {
        return vec![metadata_row(owned, width as usize)];
    }
    if width >= 60 {
        return vec![
            metadata_row(owned[..3].to_vec(), width as usize),
            metadata_row(owned[3..].to_vec(), width as usize),
        ];
    }
    owned
        .into_iter()
        .map(|line| fit_line_to_width(line, width as usize))
        .collect()
}

fn availability_metadata_field(state: &AddTaskView) -> Line<'static> {
    let value = if state.available_at.is_empty() {
        "Now"
    } else {
        &state.available_at
    };
    let mut line = metadata_field(AddTaskStep::AvailableAt, "Available", value, state.focus);
    if state.focus == AddTaskStep::AvailableAt {
        line.spans.pop();
        line.spans.extend(
            placeholder_input_line(
                &state.available_at,
                Some(state.available_at_cursor),
                48,
                ADD_TASK_AVAILABILITY_PLACEHOLDER,
            )
            .spans,
        );
    }
    line
}

fn due_metadata_field(state: &AddTaskView) -> Line<'static> {
    let value = if state.due_on.is_empty() {
        "None"
    } else {
        &state.due_on
    };
    let mut line = metadata_field(AddTaskStep::Due, "Due", value, state.focus);
    if state.focus == AddTaskStep::Due {
        line.spans.pop();
        line.spans.extend(
            placeholder_input_line(
                &state.due_on,
                Some(state.due_on_cursor),
                40,
                ADD_TASK_DUE_PLACEHOLDER,
            )
            .spans,
        );
    }
    line
}

pub(in crate::tui::ui) fn metadata_field(
    field: AddTaskStep,
    label: &str,
    value: &str,
    focus: AddTaskStep,
) -> Line<'static> {
    let marker = if field == focus { "▶ " } else { "  " };
    let shortcut = match field {
        AddTaskStep::Project => "^P ",
        AddTaskStep::Status => "^T ",
        AddTaskStep::Priority => "^R ",
        AddTaskStep::Labels => "^L ",
        AddTaskStep::AvailableAt => "^A ",
        AddTaskStep::Due => "^U ",
        _ => "",
    };
    let mut spans = vec![
        Span::styled(marker, Style::new().fg(FG)),
        Span::styled(shortcut, Style::new().fg(FG).add_modifier(Modifier::BOLD)),
        Span::styled(
            format!("{label}: "),
            if field == focus {
                Style::new().fg(FG).add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(FG_DIM)
            },
        ),
    ];
    spans.extend(metadata_value_spans(field, value));
    Line::from(spans)
}

fn metadata_value_spans(field: AddTaskStep, value: &str) -> Vec<Span<'static>> {
    match field {
        AddTaskStep::Project => vec![
            Span::styled("● ", Style::new().fg(theme::project_color(value))),
            Span::styled(
                value.to_string(),
                Style::new().fg(FG).add_modifier(Modifier::BOLD),
            ),
        ],
        AddTaskStep::Status => vec![status_span(value)],
        AddTaskStep::Priority => vec![Span::styled(
            priority_short(value).to_string(),
            theme::priority_style(value).add_modifier(Modifier::BOLD),
        )],
        AddTaskStep::Labels if value == "none" => {
            vec![Span::styled("none", Style::new().fg(FG_DIM))]
        }
        AddTaskStep::Labels => vec![Span::styled(label_summary(value), Style::new().fg(FG_DIM))],
        _ => vec![Span::raw(value.to_string())],
    }
}

fn label_summary(value: &str) -> String {
    let mut labels = value.split(',');
    let first = labels.next().unwrap_or_default();
    let more = labels.count();
    if more == 0 {
        first.to_string()
    } else {
        format!("{first} +{more}")
    }
}

const METADATA_SEPARATOR: &str = "   ";

fn metadata_row(lines: Vec<Line<'static>>, width: usize) -> Line<'static> {
    let separator_width = METADATA_SEPARATOR.width();
    let fields_width = width.saturating_sub(separator_width * lines.len().saturating_sub(1));
    let base_width = fields_width / lines.len().max(1);
    let remainder = fields_width % lines.len().max(1);
    let fitted = lines
        .into_iter()
        .enumerate()
        .map(|(index, line)| fit_line_to_width(line, base_width + usize::from(index < remainder)))
        .collect();
    join_lines(fitted, METADATA_SEPARATOR)
}

fn fit_line_to_width(line: Line<'static>, width: usize) -> Line<'static> {
    if line.width() <= width {
        let padding = width.saturating_sub(line.width());
        let mut spans = line.spans;
        spans.push(Span::raw(" ".repeat(padding)));
        return Line::from(spans);
    }
    if width == 0 {
        return Line::default();
    }

    let content_width = width.saturating_sub(1);
    let mut remaining = content_width;
    let mut spans = Vec::new();
    let mut ellipsis_style = Style::new().fg(FG_DIM);
    for span in line.spans {
        ellipsis_style = span.style;
        let mut content = String::new();
        let mut truncated = false;
        for ch in span.content.chars() {
            let char_width = ch.width().unwrap_or(0);
            if char_width > remaining {
                truncated = true;
                break;
            }
            content.push(ch);
            remaining = remaining.saturating_sub(char_width);
        }
        if !content.is_empty() {
            spans.push(Span::styled(content, span.style));
        }
        if truncated || remaining == 0 {
            break;
        }
    }
    spans.push(Span::styled("…", ellipsis_style));
    Line::from(spans)
}

fn join_lines(lines: Vec<Line<'static>>, separator: &'static str) -> Line<'static> {
    let mut spans = Vec::new();
    for (index, line) in lines.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw(separator));
        }
        spans.extend(line.spans);
    }
    Line::from(spans)
}

const COMPOSER_HELP_TOPICS: &[(&str, &str)] = &[
    ("Tab / Shift+Tab", "next / previous field"),
    ("Arrows", "move fields; edit cursor in date fields"),
    (
        "Enter",
        "open metadata, create title, or newline description",
    ),
    (
        "Available",
        "tomorrow · next mon at 9am · 2w · in 2 months · next week",
    ),
    ("Available", "YYYY-MM-DD · UTC timestamp · epoch seconds"),
    ("Available", "local time; empty or now = immediate"),
    ("Due", "same dates without times · YYYY-MM-DD · none clears"),
    ("Ctrl-p/t/r/l/a/u", "jump to metadata fields"),
    ("Ctrl-Enter", "create from any field"),
    ("Ctrl-s", "portable create fallback"),
    ("Ctrl-n", "create with AI"),
    ("F1", "open this help"),
    ("Ctrl-x Ctrl-e", "edit description externally"),
    ("Esc", "cancel or confirm discard"),
];
const COMPOSER_HELP_HEIGHT: u16 = 18;
const COMPOSER_HELP_FIXED_ROWS: u16 = 4;

pub(crate) fn composer_help_scroll_cap(frame_height: u16, full_frame: bool) -> u16 {
    let frame_padding = if full_frame { 2 } else { 4 };
    let add_task_content_height = frame_height.saturating_sub(frame_padding).min(20);
    let dialog_height = COMPOSER_HELP_HEIGHT.min(add_task_content_height);
    let visible_rows = dialog_height.saturating_sub(COMPOSER_HELP_FIXED_ROWS) as usize;
    COMPOSER_HELP_TOPICS.len().saturating_sub(visible_rows) as u16
}

fn render_add_task_child(frame: &mut Frame, state: &AddTaskView, content: Rect) {
    if matches!(&state.mode, AddTaskMode::ConfirmDiscard) {
        render_confirm(
            frame,
            &ConfirmView {
                route: OverlayRoute::MessageOnly,
                title: "Discard draft?".to_string(),
                prompt: "Discard this task draft?".to_string(),
            },
        );
        return;
    }
    if let AddTaskMode::Help { scroll } = &state.mode {
        render_composer_help(frame, content, *scroll);
        return;
    }

    let (title, lines, width, background) = match &state.mode {
        AddTaskMode::Compose => return,
        AddTaskMode::Picker { state, .. } => {
            let view = PickerView {
                route: state.route,
                title: state.title.clone(),
                filter: state.filter.text.clone(),
                filter_cursor: state.filter.cursor,
                items: state.items.clone(),
                selected: state.selected,
                scroll: state.scroll,
                multi: state.multi,
                mode: state.mode,
                visible_indices: visible_picker_indices(state),
            };
            (view.title.clone(), add_task_picker_lines(&view), 54, BG_ALT)
        }
        AddTaskMode::Labels(state) => {
            let visible_indices = tag_combobox_matches(state);
            let view = TagComboboxView {
                route: state.route,
                title: state.title.clone(),
                input: state.input.text.clone(),
                input_cursor: state.input.cursor,
                completion: tag_combobox_completion(state),
                options: state.options.clone(),
                selected: state.selected.clone(),
                highlighted: state.highlighted,
                visible_start: visible_indices
                    .iter()
                    .position(|index| *index == state.highlighted)
                    .unwrap_or(0)
                    .saturating_sub(TAG_COMBOBOX_VIEWPORT_ROWS.saturating_sub(1)),
                visible_indices,
            };
            (view.title.clone(), tag_combobox_lines(&view), 64, BG_PANEL)
        }
        AddTaskMode::Help { .. } => unreachable!("composer help renders separately"),
        AddTaskMode::ConfirmDiscard => unreachable!("discard confirmation renders above"),
    };
    let width = width.min(content.width.saturating_sub(2)).max(1);
    let desired_height = (lines.len() as u16).saturating_add(2);
    let height = desired_height.min(content.height).max(1);
    let area = Rect {
        x: content.x + content.width.saturating_sub(width) / 2,
        y: content.y + content.height.saturating_sub(height) / 2,
        width,
        height,
    };
    let inner = Dialog::new(&title, width, height).render_block_at(frame, area);
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: false })
            .style(Style::new().fg(FG).bg(background)),
        inner,
    );
}

fn render_composer_help(frame: &mut Frame, content: Rect, scroll: u16) {
    let width = 66.min(content.width.saturating_sub(2)).max(1);
    let height = COMPOSER_HELP_HEIGHT.min(content.height).max(1);
    let area = Rect {
        x: content.x + content.width.saturating_sub(width) / 2,
        y: content.y + content.height.saturating_sub(height) / 2,
        width,
        height,
    };
    let inner = Dialog::new("Composer help", width, height).render_block_at(frame, area);
    let help_area = Rect {
        height: inner.height.saturating_sub(2),
        ..inner
    };
    let visible_rows = help_area.height as usize;
    let start = clamp_scroll_start(scroll, COMPOSER_HELP_TOPICS.len(), visible_rows);
    let lines = COMPOSER_HELP_TOPICS
        .iter()
        .skip(start)
        .take(visible_rows)
        .map(|(keys, description)| composer_help_line(keys, description))
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::new().fg(FG).bg(BG_ALT)),
        help_area,
    );
    if help_area.height > 0 {
        render_vertical_scrollbar(frame, help_area, COMPOSER_HELP_TOPICS.len(), scroll);
    }
    if inner.height > 0 {
        let hint_area = Rect {
            y: inner.bottom().saturating_sub(1),
            height: 1,
            ..inner
        };
        frame.render_widget(
            Paragraph::new(dialog_hint_line(&[("j/k", "scroll"), ("Esc", "close")]))
                .style(Style::new().fg(FG).bg(BG_ALT)),
            hint_area,
        );
    }
}

pub(in crate::tui::ui) fn composer_help_line(
    keys: &'static str,
    description: &'static str,
) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{keys:<18}"), Style::new().fg(FG_MUTED)),
        Span::styled(description, Style::new().fg(FG_DIM)),
    ])
}

fn add_task_picker_lines(state: &PickerView) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if matches!(state.mode, crate::tui::overlay::PickerMode::Filter) {
        lines.push(picker_filter_line(
            Span::raw("/"),
            &state.filter,
            state.filter_cursor,
        ));
        lines.push(Line::from(""));
    }
    let visible = state
        .visible_indices
        .iter()
        .skip(state.scroll)
        .take(8)
        .copied()
        .collect::<Vec<_>>();
    for index in visible {
        let item = &state.items[index];
        let selected = index == state.selected;
        let line = match state.route {
            crate::tui::overlay::OverlayRoute::AddTaskTitleProject => {
                project_picker_line(item, selected)
            }
            crate::tui::overlay::OverlayRoute::AddTaskTitlePriority => {
                priority_picker_line(item, selected)
            }
            _ => {
                let marker = if selected { "▸ " } else { "  " };
                let style = if selected {
                    SELECTED
                } else {
                    Style::new().bg(BG_ALT)
                };
                Line::from(Span::styled(format!("{marker}{}", item.label), style))
            }
        };
        lines.push(line);
    }
    if state.visible_indices.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no matching options",
            Style::new().fg(FG_DIM),
        )));
    }
    lines.push(Line::from(""));
    lines.push(picker_hint_line(state.mode, state.multi, "submit"));
    lines
}

pub(in crate::tui::ui) fn add_task_title_metadata(title: &str) -> Option<(&str, &str)> {
    let value = title.strip_prefix("Add task  project=")?;
    value.split_once(" priority=")
}

pub(in crate::tui::ui) const ADD_TASK_TITLE_PLACEHOLDER: &str = "Enter title here...";
const ADD_TASK_AVAILABILITY_PLACEHOLDER: &str = "tomorrow, in 2 weeks, or next monday at 9am";
const ADD_TASK_DUE_PLACEHOLDER: &str = "tomorrow, in 2 weeks, or next monday";

pub(in crate::tui::ui) fn add_task_title_input_line(
    input: &str,
    cursor: Option<usize>,
    width: usize,
) -> Line<'static> {
    placeholder_input_line(input, cursor, width, ADD_TASK_TITLE_PLACEHOLDER)
}

fn placeholder_input_line(
    input: &str,
    cursor: Option<usize>,
    width: usize,
    placeholder: &'static str,
) -> Line<'static> {
    if input.is_empty() {
        if cursor.is_some() {
            return Line::from(vec![
                cursor_cell(&placeholder[..1]),
                Span::styled(&placeholder[1..], Style::new().fg(FG_DIM)),
            ]);
        }
        return Line::from(Span::styled(placeholder, Style::new().fg(FG_DIM)));
    }
    match cursor {
        Some(cursor) => clipped_input_line(input, cursor, width),
        None => Line::from(input.to_string()),
    }
}

pub(in crate::tui::ui) fn add_task_field_label(label: &'static str, active: bool) -> Line<'static> {
    let style = if active {
        Style::new()
            .fg(Color::Rgb(194, 174, 255))
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(FG_MUTED).add_modifier(Modifier::BOLD)
    };
    Line::from(vec![
        Span::styled(if active { "▶ " } else { "  " }, style),
        Span::styled(label, style),
    ])
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

pub(in crate::tui::ui) fn add_task_status_hint_line() -> Line<'static> {
    colored_add_task_hint_line(
        &[
            ("i", "inbox"),
            ("b", "backlog"),
            ("t", "todo"),
            ("a", "active"),
            ("d", "done"),
            ("x", "canceled"),
        ],
        theme::status_style,
    )
}

pub(in crate::tui::ui) fn add_task_priority_hint_line() -> Line<'static> {
    colored_add_task_hint_line(
        &[
            ("n", "none"),
            ("l", "low"),
            ("m", "medium"),
            ("h", "high"),
            ("u", "urgent"),
        ],
        theme::priority_style,
    )
}

fn colored_add_task_hint_line(
    items: &[(&str, &str)],
    label_style: fn(&str) -> Style,
) -> Line<'static> {
    let mut spans = Vec::new();
    for (index, (key, label)) in items.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled("  ", Style::new().fg(FG_MUTED)));
        }
        spans.push(Span::styled(
            key.to_string(),
            Style::new().fg(FG).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(format!(" {label}"), label_style(label)));
    }
    spans.push(Span::styled("  ", Style::new().fg(FG_MUTED)));
    spans.push(Span::styled(
        "Esc".to_string(),
        Style::new().fg(FG).add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::styled(" cancel", Style::new().fg(FG_MUTED)));
    Line::from(spans)
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
        AddTaskStep::Project
        | AddTaskStep::Status
        | AddTaskStep::Priority
        | AddTaskStep::Labels => dialog_hint_line(&[
            ("Enter", "choose"),
            ("←/→", "field"),
            ("Tab", "next"),
            ("^S", "create"),
            ("^N", "create with AI"),
            ("F1", "help"),
            ("Esc", "cancel"),
        ]),
        AddTaskStep::Title => dialog_hint_line(&[
            ("Enter", "create"),
            ("↑/↓", "field"),
            ("Tab", "next"),
            ("^N", "create with AI"),
            ("F1", "help"),
            ("Esc", "cancel"),
        ]),
        AddTaskStep::AvailableAt => dialog_hint_line(&[
            ("←/→", "cursor"),
            ("empty/now", "immediate"),
            ("Tab", "next"),
            ("^S", "create"),
            ("F1", "formats"),
            ("Esc", "cancel"),
        ]),
        AddTaskStep::Due => dialog_hint_line(&[
            ("←/→", "cursor"),
            ("empty/none", "no due date"),
            ("Tab", "next"),
            ("^S", "create"),
            ("F1", "formats"),
            ("Esc", "cancel"),
        ]),
        AddTaskStep::Description => dialog_hint_line(&[
            ("Ctrl-Enter / ^S", "create"),
            ("Tab", "next"),
            ("^N", "create with AI"),
            ("F1", "help"),
            ("Esc", "cancel"),
        ]),
    }
}

pub(in crate::tui::ui) fn add_task_metadata_title(
    project: &str,
    status: &str,
    priority: &str,
    labels: &[String],
    width: u16,
) -> Line<'static> {
    let status_style = theme::status_style(status);
    let priority_style = theme::priority_style(priority);
    let labels = labels_display(labels);
    let label_style = Style::new().fg(Color::Rgb(133, 222, 255));
    if width < 60 {
        return Line::from(vec![
            Span::styled(" status: ", Style::new().fg(FG_MUTED)),
            Span::styled(truncate_chars(status, 8), status_style),
            Span::styled(" · ", Style::new().fg(FG_DIM)),
            Span::styled("prio: ", Style::new().fg(FG_MUTED)),
            Span::styled(truncate_chars(priority, 6), priority_style),
        ]);
    }
    let value_width = (width as usize).saturating_sub(44).max(6) / 4;
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
        Span::styled(" · ", Style::new().fg(FG_DIM)),
        Span::styled("labels: ", Style::new().fg(FG_MUTED)),
        Span::styled(truncate_chars(&labels, value_width), label_style),
    ])
}

fn labels_display(labels: &[String]) -> String {
    if labels.is_empty() {
        "none".to_string()
    } else {
        labels.join(",")
    }
}
