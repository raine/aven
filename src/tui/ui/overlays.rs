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
use crate::tui::authoring::AddTaskStep;
use crate::tui::overlay::{
    AddTaskView, ConfirmView, MultilineInputView, OverlayRoute, PickerItem, PickerMode, PickerView,
    TextInputView, TextPanelView,
};
use crate::tui::text::{
    cell_width_ranges, char_boundary_at_or_before, char_count_ranges, char_count_segment_index,
};
use crate::tui::theme::{self, ACCENT, BG_ALT, BG_PANEL, FG, FG_DIM, FG_MUTED, SELECTED};
use crate::tui::widgets::priority_icon;

pub(super) fn render_search(frame: &mut Frame, input: &str, cursor: usize) {
    Dialog::new("Search", 54, 3).render_text(frame, input_line("/", input, cursor));
}

pub(super) fn render_add_task(frame: &mut Frame, state: &AddTaskView) {
    let expanded =
        add_task_description_has_content(state) || state.focus == AddTaskStep::Description;
    let height = if expanded {
        frame.area().height.saturating_sub(4).clamp(11, 18)
    } else {
        11
    };
    let dialog = Dialog::new("Add task", 100, height);
    let width = dialog.area(frame).width;
    let dialog = dialog.right_title(add_task_metadata_title(
        &state.project,
        &state.priority,
        width,
    ));
    let content = dialog.render_block(frame);
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
    while lines.len() + 1 < content.height as usize {
        lines.push(Line::from(""));
    }
    lines.push(add_task_hint_line(state.focus));
    frame.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::new().fg(FG).bg(BG_ALT)),
        content,
    );
}

pub(super) fn render_text_input(frame: &mut Frame, state: &TextInputView) {
    if let Some((project, priority)) = add_task_title_metadata(&state.title) {
        let dialog = Dialog::new("Add task", 74, 5);
        let width = dialog.area(frame).width;
        let dialog = dialog.right_title(add_task_metadata_title(project, priority, width));
        let content = dialog.render_block(frame);
        let input =
            add_task_title_input_line(&state.input, Some(state.cursor), content.width as usize);
        let text = Text::from(vec![
            input,
            Line::from(""),
            add_task_hint_line(AddTaskStep::Title),
        ]);
        frame.render_widget(
            Paragraph::new(text).style(Style::new().fg(FG).bg(BG_ALT)),
            content,
        );
        return;
    }

    if state.prompt.is_empty() {
        let dialog = Dialog::new(&state.title, 54, 5);
        let content = dialog.render_block(frame);
        let input = clipped_input_line(&state.input, state.cursor, content.width as usize);
        let text = Text::from(vec![
            input,
            Line::from(""),
            dialog_hint_line(&[("Enter", "submit"), ("Esc", "cancel")]),
        ]);
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

const ADD_TASK_TITLE_PLACEHOLDER: &str = "Enter title here...";

fn add_task_title_input_line(input: &str, cursor: Option<usize>, width: usize) -> Line<'static> {
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

fn add_task_description_has_content(state: &AddTaskView) -> bool {
    state.description.iter().any(|line| !line.is_empty())
}

fn add_task_description_lines(
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
    let start = add_task_description_viewport_start(
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

fn add_task_description_viewport_start(
    cursor_row: usize,
    visible_rows: usize,
    row_count: usize,
    focused: bool,
) -> usize {
    if row_count <= visible_rows {
        return 0;
    }
    if !focused {
        return 0;
    }
    cursor_row
        .saturating_sub(visible_rows / 2)
        .min(row_count.saturating_sub(visible_rows))
}

fn add_task_description_viewport_line(marker: &'static str, line: Line<'static>) -> Line<'static> {
    let mut spans = Vec::with_capacity(line.spans.len() + 1);
    spans.push(Span::styled(marker, Style::new().fg(FG_DIM)));
    spans.extend(line.spans);
    Line::from(spans)
}

struct AddTaskDescriptionVisualLine {
    line: Line<'static>,
    has_cursor: bool,
}

fn add_task_description_visual_lines(
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

fn add_task_hint_line(focus: AddTaskStep) -> Line<'static> {
    match focus {
        AddTaskStep::Title => dialog_hint_line(&[
            ("Enter", "create"),
            ("Tab", "description"),
            ("Ctrl+P", "project"),
            ("Ctrl+R", "priority"),
            ("Esc", "cancel"),
        ]),
        AddTaskStep::Description => dialog_hint_line(&[
            ("Ctrl+S", "create"),
            ("Ctrl+X Ctrl+E", "editor"),
            ("Tab", "title"),
            ("Ctrl+P", "project"),
            ("Ctrl+R", "priority"),
            ("Esc", "cancel"),
        ]),
    }
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
    match state.route {
        OverlayRoute::AddNote => {
            render_add_note_input(frame, state);
            return;
        }
        OverlayRoute::EditDescription => {
            render_description_input(frame, state);
            return;
        }
        OverlayRoute::AddTaskDescription => {
            render_add_task_description_input(frame, state);
            return;
        }
        _ => {}
    }

    let visible_rows = 10usize;
    let content_rows = state.lines.len().min(visible_rows).max(1);
    let prompt_rows = usize::from(!state.prompt.is_empty());
    let height = (content_rows + prompt_rows + 4).min(16) as u16;
    let start = state.row.saturating_sub(visible_rows.saturating_sub(1));
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
    let start = cursor_row.saturating_sub(editor_rows.saturating_sub(1));
    lines = lines.into_iter().skip(start).take(editor_rows).collect();
    while lines.len() < editor_rows {
        lines.push(Line::from(""));
    }
    lines.push(Line::from(""));
    lines.push(description_hint_line(state));

    frame.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::new().fg(FG).bg(BG_ALT)),
        content,
    );
}

fn render_add_task_description_input(frame: &mut Frame, state: &MultilineInputView) {
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
        lines.push(add_task_description_input_line(
            line,
            if row_index == state.row {
                Some(state.column)
            } else {
                None
            },
            line.is_empty() && state.lines.len() == 1,
        ));
    }
    lines.push(Line::from(""));
    lines.push(add_task_description_hint_line());
    Dialog::new(&state.title, 70, height)
        .wrap()
        .render_text(frame, Text::from(lines));
}

fn description_editor_width(frame_width: u16) -> u16 {
    let max_width = frame_width.saturating_sub(4).max(1);
    frame_width
        .saturating_mul(4)
        .saturating_div(5)
        .max(80)
        .min(max_width)
}

fn description_visual_row_count(state: &MultilineInputView, line_width: usize) -> usize {
    state
        .lines
        .iter()
        .map(|line| char_count_ranges(line, line_width).len())
        .sum::<usize>()
        .max(1)
}

fn description_editor_lines(
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

fn description_input_line(line: &str, cursor: usize, show_placeholder: bool) -> Line<'static> {
    if show_placeholder && line.is_empty() && cursor == 0 {
        return Line::from(vec![
            cursor_cell("E"),
            Span::styled("nter task description here...", Style::new().fg(FG_DIM)),
        ]);
    }
    input_line("", line, cursor)
}

fn add_task_description_input_line(
    line: &str,
    cursor: Option<usize>,
    show_placeholder: bool,
) -> Line<'static> {
    if show_placeholder {
        let placeholder = "Optional details, links, or handoff context...";
        if cursor.is_some() {
            return Line::from(vec![
                cursor_cell(&placeholder[..1]),
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

fn add_task_description_hint_line() -> Line<'static> {
    dialog_hint_line(&[
        ("Ctrl+S", "create"),
        ("Enter", "newline"),
        ("Ctrl+P", "project"),
        ("Ctrl+R", "priority"),
        ("Esc", "cancel"),
    ])
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
    Dialog::new(&state.title, 60, height)
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

fn description_hint_line(state: &MultilineInputView) -> Line<'static> {
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

pub(super) fn render_picker(frame: &mut Frame, state: &PickerView) {
    if let Some(submit_label) = project_picker_submit_label(state.route) {
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
        if priority_picker_submit_label(state.route).is_some() {
            lines.push(priority_picker_line(item, *index == state.selected));
        } else {
            lines.push(Line::from(format!("{marker}{}{check}", item.label)));
        }
    }
    lines.push(Line::from(""));
    lines.push(picker_hint_line(
        state.mode,
        state.multi,
        priority_picker_submit_label(state.route).unwrap_or("submit"),
    ));
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

fn picker_hint_line(mode: PickerMode, multi: bool, submit_label: &str) -> Line<'static> {
    let mut items = match mode {
        PickerMode::Navigate => vec![("j/k", "move"), ("/", "filter")],
        PickerMode::Filter => vec![("type", "filter"), ("Up/Down", "move")],
    };
    if multi {
        items.push(("Space", "toggle"));
    }
    let esc_label = match mode {
        PickerMode::Navigate => "cancel",
        PickerMode::Filter => "normal",
    };
    items.extend([("Enter", submit_label), ("Esc", esc_label)]);
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
    lines.push(project_picker_hint_line(state.mode, submit_label));
    Dialog::new(&state.title, 70, height).render_text(frame, Text::from(lines));
}

fn project_picker_submit_label(route: OverlayRoute) -> Option<&'static str> {
    match route {
        OverlayRoute::ViewProject => Some("open"),
        OverlayRoute::EditProject | OverlayRoute::AddTaskTitleProject => Some("submit"),
        OverlayRoute::DeleteProjectPicker => Some("delete"),
        _ => None,
    }
}

fn priority_picker_submit_label(route: OverlayRoute) -> Option<&'static str> {
    match route {
        OverlayRoute::EditPriority | OverlayRoute::AddTaskTitlePriority => Some("submit"),
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

fn project_picker_hint_line(mode: PickerMode, submit_label: &'static str) -> Line<'static> {
    picker_hint_line(mode, false, submit_label)
}

pub(super) fn render_confirm(frame: &mut Frame, state: &ConfirmView) {
    let width = confirm_width(frame.area().width, &state.prompt);
    let prompt_rows = char_count_ranges(&state.prompt, width.saturating_sub(4) as usize).len();
    let height = prompt_rows.saturating_add(4) as u16;
    let text = Text::from(vec![
        Line::from(state.prompt.as_str()),
        Line::from(""),
        confirm_hint_line(),
    ]);
    Dialog::new(&state.title, width, height)
        .wrap()
        .render_text(frame, text);
}

fn confirm_width(frame_width: u16, prompt: &str) -> u16 {
    let prompt_width = prompt.chars().count().saturating_add(4) as u16;
    prompt_width
        .clamp(32, 80)
        .min(frame_width.saturating_sub(4).max(32))
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
    use crate::tui::overlay::{OverlayRoute, OverlayView};
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
            OverlayView::AddTask(state) => render_add_task(frame, state),
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
    fn add_task_overlay_renders_metadata_fields_and_footer() {
        let rendered = render_overlay_view(OverlayView::AddTask(AddTaskView {
            title: "ship dialogs".to_string(),
            title_cursor: 12,
            description: vec![String::new()],
            description_row: 0,
            description_column: 0,
            focus: AddTaskStep::Title,
            project: "aven".to_string(),
            priority: "high".to_string(),
        }));
        assert!(rendered.contains("Add task"));
        assert!(rendered.contains("project: aven"));
        assert!(rendered.contains("prio: high"));
        assert!(rendered.contains("Title"));
        assert!(rendered.contains("Description"));
        assert!(rendered.contains("ship dialogs"));
        assert!(rendered.contains("Optional details, links, or handoff context..."));
        assert!(rendered.contains("Tab description"));
        assert!(rendered.contains("Ctrl+P project"));
        assert!(rendered.contains("Ctrl+R priority"));
    }

    #[test]
    fn add_task_overlay_pins_footer_to_bottom() {
        let buffer = overlay_buffer(OverlayView::AddTask(AddTaskView {
            title: String::new(),
            title_cursor: 0,
            description: vec![String::new()],
            description_row: 0,
            description_column: 0,
            focus: AddTaskStep::Description,
            project: "aven".to_string(),
            priority: "none".to_string(),
        }));
        let hint_row = (0..buffer.area.height)
            .find(|row| buffer_row(&buffer, *row).contains("Ctrl+S create"))
            .unwrap();
        let bottom_border_row = (0..buffer.area.height)
            .rev()
            .find(|row| buffer_row(&buffer, *row).contains("╰"))
            .unwrap();
        assert_eq!(hint_row + 1, bottom_border_row);
    }

    #[test]
    fn add_task_overlay_does_not_truncate_title_hints() {
        let rendered = render_overlay_view(OverlayView::AddTask(AddTaskView {
            title: String::new(),
            title_cursor: 0,
            description: vec![String::new()],
            description_row: 0,
            description_column: 0,
            focus: AddTaskStep::Title,
            project: "aven".to_string(),
            priority: "none".to_string(),
        }));
        assert!(rendered.contains("Esc cancel"));
    }

    #[test]
    fn add_task_overlay_does_not_truncate_description_hints() {
        let rendered = render_overlay_view(OverlayView::AddTask(AddTaskView {
            title: String::new(),
            title_cursor: 0,
            description: vec![String::new()],
            description_row: 0,
            description_column: 0,
            focus: AddTaskStep::Description,
            project: "aven".to_string(),
            priority: "none".to_string(),
        }));
        assert!(rendered.contains("Esc cancel"));
    }

    #[test]
    fn add_task_overlay_omits_title_placeholder_cursor_when_description_focused() {
        let buffer = overlay_buffer(OverlayView::AddTask(AddTaskView {
            title: String::new(),
            title_cursor: 0,
            description: vec!["details".to_string()],
            description_row: 0,
            description_column: 7,
            focus: AddTaskStep::Description,
            project: "aven".to_string(),
            priority: "none".to_string(),
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
                title: String::new(),
                title_cursor: 0,
                description: vec!["abcdefghijklmnopqrstuvwxyz".to_string()],
                description_row: 0,
                description_column: 25,
                focus: AddTaskStep::Description,
                project: "aven".to_string(),
                priority: "none".to_string(),
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
                title: String::new(),
                title_cursor: 0,
                description: vec!["abcdefghijklmnopqrstuvwxyz".to_string()],
                description_row: 0,
                description_column: 25,
                focus: AddTaskStep::Title,
                project: "aven".to_string(),
                priority: "none".to_string(),
            },
            2,
            12,
        );

        assert!(lines[0].to_string().contains("abcdefghij"));
        assert!(lines[1].to_string().starts_with("↓ "));
    }

    #[test]
    fn hint_lines_style_keys() {
        let add_task_keys = styled_key_contents(add_task_hint_line(AddTaskStep::Title));
        assert_eq!(
            add_task_keys,
            vec!["Enter", "Tab", "Ctrl+P", "Ctrl+R", "Esc"]
        );

        let multiline_keys = styled_key_contents(multiline_hint_line());
        assert_eq!(multiline_keys, vec!["Ctrl+S", "Esc"]);

        let add_task_description_keys =
            styled_key_contents(add_task_hint_line(AddTaskStep::Description));
        assert_eq!(
            add_task_description_keys,
            vec!["Ctrl+S", "Ctrl+X Ctrl+E", "Tab", "Ctrl+P", "Ctrl+R", "Esc"]
        );

        let add_task_description_editor_keys =
            styled_key_contents(add_task_description_hint_line());
        assert_eq!(
            add_task_description_editor_keys,
            vec!["Ctrl+S", "Enter", "Ctrl+P", "Ctrl+R", "Esc"]
        );

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

    #[test]
    fn overlay_render_includes_picker_filter_and_hints() {
        let rendered = render_overlay_view(OverlayView::Picker(PickerView {
            route: OverlayRoute::MessageOnly,
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
            mode: PickerMode::Navigate,
            visible_indices: vec![0],
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
            route: OverlayRoute::MessageOnly,
            title: "Project".to_string(),
            filter: "app".to_string(),
            filter_cursor: 3,
            items: vec![PickerItem {
                label: "APP app".to_string(),
                value: "app".to_string(),
                selected: false,
            }],
            selected: 0,
            multi: false,
            mode: PickerMode::Filter,
            visible_indices: vec![0],
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
                filter: String::new(),
                filter_cursor: 0,
                items: vec![PickerItem {
                    label: "urgent".to_string(),
                    value: "urgent".to_string(),
                    selected: false,
                }],
                selected: 0,
                multi: false,
                mode: PickerMode::Navigate,
                visible_indices: vec![0],
            }));
            assert!(rendered.contains(priority_icon("urgent")));
            assert!(rendered.contains("urgent"));
            assert!(rendered.contains("Enter"));
            assert!(rendered.contains("submit"));
        }
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
            route: OverlayRoute::MessageOnly,
            title: "Project".to_string(),
            filter: String::new(),
            filter_cursor: 0,
            items,
            selected: 10,
            multi: false,
            mode: PickerMode::Navigate,
            visible_indices: (0..12).collect(),
        }));
        assert!(rendered.contains("▸ Item 10"));
        assert!(!rendered.contains("Item 0"));
    }

    #[test]
    fn project_picker_uses_structured_columns() {
        let rendered = render_overlay_view(OverlayView::Picker(PickerView {
            route: OverlayRoute::ViewProject,
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
            mode: PickerMode::Navigate,
            visible_indices: vec![0],
        }));
        assert!(rendered.contains("PREFIX"));
        assert!(rendered.contains("PROJECT"));
        assert!(rendered.contains("CC"));
        assert!(rendered.contains("claude-code"));
        assert!(rendered.contains("Enter open"));
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
                items: vec![PickerItem {
                    label: "CC claude-code".to_string(),
                    value: "claude-code".to_string(),
                    selected: false,
                }],
                selected: 0,
                multi: false,
                mode: PickerMode::Navigate,
                visible_indices: vec![0],
            }));
            assert!(rendered.contains("PREFIX"));
            assert!(rendered.contains("PROJECT"));
            assert!(rendered.contains("CC"));
            assert!(rendered.contains("claude-code"));
            assert!(rendered.contains("Enter submit"));
            assert!(rendered.contains(title));
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
                OverlayRoute::ViewProject,
                "Changed view title",
                "Enter open",
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
                filter: String::new(),
                filter_cursor: 0,
                items: vec![PickerItem {
                    label: "AVN aven".to_string(),
                    value: "aven".to_string(),
                    selected: false,
                }],
                selected: 0,
                multi: false,
                mode: PickerMode::Navigate,
                visible_indices: vec![0],
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
            filter: String::new(),
            filter_cursor: 0,
            items: vec![PickerItem {
                label: "urgent".to_string(),
                value: "urgent".to_string(),
                selected: false,
            }],
            selected: 0,
            multi: false,
            mode: PickerMode::Navigate,
            visible_indices: vec![0],
        }));
        assert!(rendered.contains("Changed priority title"));
        assert!(rendered.contains(priority_icon("urgent")));
    }

    #[test]
    fn add_task_priority_route_uses_priority_renderer() {
        let rendered = render_overlay_view(OverlayView::Picker(PickerView {
            route: OverlayRoute::AddTaskTitlePriority,
            title: "Changed add task priority".to_string(),
            filter: String::new(),
            filter_cursor: 0,
            items: vec![PickerItem {
                label: "urgent".to_string(),
                value: "urgent".to_string(),
                selected: false,
            }],
            selected: 0,
            multi: false,
            mode: PickerMode::Navigate,
            visible_indices: vec![0],
        }));
        assert!(rendered.contains("Changed add task priority"));
        assert!(rendered.contains(priority_icon("urgent")));
        assert!(rendered.contains("urgent"));
        assert!(rendered.contains("Enter submit"));
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

    fn buffer_text_from_rows(buffer: &ratatui::buffer::Buffer) -> String {
        (0..buffer.area.height)
            .map(|row| buffer_row(buffer, row))
            .collect::<Vec<_>>()
            .join("\n")
    }
}
