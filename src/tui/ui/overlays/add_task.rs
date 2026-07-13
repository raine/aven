use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use super::super::dialog::{Dialog, dialog_hint_line};
use super::super::input::{clipped_input_line, cursor_cell};
use super::super::truncate::truncate_chars;
use super::multiline::add_task_description_input_line;
use super::shared::viewport_start_for_cursor;
use crate::tui::authoring::AddTaskStep;
use crate::tui::overlay::{AddTaskMode, AddTaskView};
use crate::tui::text::cell_width_ranges;
use crate::tui::theme::{self, FG, FG_DIM, FG_MUTED};

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
    let metadata_rows = if content.width >= 96 {
        1
    } else if content.width >= 60 {
        2
    } else {
        4
    };
    if relative_y < metadata_rows {
        let index = if metadata_rows == 1 {
            (relative_x as usize * 4 / content.width.max(1) as usize).min(3)
        } else if metadata_rows == 2 {
            (relative_y as usize * 2)
                + (relative_x as usize * 2 / content.width.max(1) as usize).min(1)
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
    lines.push(add_task_title_input_line(
        &state.title,
        (state.focus == AddTaskStep::Title).then_some(state.title_cursor),
        content.width as usize,
    ));
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
    let owned = fields
        .into_iter()
        .map(|(field, label, value)| metadata_field(field, label, &value, state.focus))
        .collect::<Vec<_>>();
    if width >= 96 {
        return vec![join_lines(owned, "   ")];
    }
    if width >= 60 {
        return vec![
            join_lines(owned[..2].to_vec(), "   "),
            join_lines(owned[2..].to_vec(), "   "),
        ];
    }
    owned
}

fn metadata_field(
    field: AddTaskStep,
    label: &str,
    value: &str,
    focus: AddTaskStep,
) -> Line<'static> {
    let marker = if field == focus { "▶ " } else { "  " };
    Line::from(vec![
        Span::styled(marker, Style::new().fg(FG)),
        Span::styled(
            format!("{label}: "),
            if field == focus {
                Style::new().fg(FG).add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(FG_DIM)
            },
        ),
        Span::styled(format!("[{value}]"), Style::new().fg(FG)),
    ])
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

fn render_add_task_child(frame: &mut Frame, state: &AddTaskView, content: Rect) {
    let (title, lines) = match &state.mode {
        AddTaskMode::Compose => return,
        AddTaskMode::Picker { state, .. } => {
            let mut lines = vec![format!("Filter: {}", state.filter.text)];
            lines.extend(
                state
                    .items
                    .iter()
                    .enumerate()
                    .skip(state.scroll)
                    .take(8)
                    .map(|(index, item)| {
                        format!(
                            "{} {}",
                            if index == state.selected { "▶" } else { " " },
                            item.label
                        )
                    }),
            );
            (state.title.clone(), lines)
        }
        AddTaskMode::Labels(state) => {
            let mut lines = vec![
                format!("Filter: {}", state.input.text),
                format!("Selected: {}", labels_display(&state.selected)),
            ];
            lines.extend(
                crate::tui::overlay::tag_combobox_matches(state)
                    .into_iter()
                    .take(6)
                    .filter_map(|index| state.options.get(index).map(|label| (index, label)))
                    .map(|(index, label)| {
                        format!(
                            "{} {}{}",
                            if index == state.highlighted {
                                "▶"
                            } else {
                                " "
                            },
                            if state.selected.contains(label) {
                                "[x]"
                            } else {
                                "[ ]"
                            },
                            label
                        )
                    }),
            );
            lines.push("Enter accepts, Esc returns".to_string());
            (state.title.clone(), lines)
        }
        AddTaskMode::Help { scroll } => {
            let all = vec![
                "Tab / Shift+Tab   next / previous field".to_string(),
                "Enter             open metadata or create from title".to_string(),
                "Enter             newline in description".to_string(),
                "Ctrl-s            create from any field".to_string(),
                "Ctrl-n            create with AI".to_string(),
                "F1                open this help".to_string(),
                "Ctrl-x Ctrl-e     edit description externally".to_string(),
                "Esc               cancel or confirm discard".to_string(),
            ];
            (
                "Composer help".to_string(),
                all.into_iter().skip(*scroll as usize).collect(),
            )
        }
        AddTaskMode::ConfirmDiscard => (
            "Discard draft?".to_string(),
            vec![
                "This draft has content.".to_string(),
                "y discard   n/Esc keep editing".to_string(),
            ],
        ),
    };
    let width = if content.width < 20 {
        content.width
    } else {
        content.width.saturating_sub(4).clamp(20, 62)
    };
    let desired_height = (lines.len() as u16).saturating_add(2);
    let height = if content.height < 4 {
        content.height
    } else {
        desired_height.clamp(4, content.height)
    };
    let area = Rect {
        x: content.x + content.width.saturating_sub(width) / 2,
        y: content.y + content.height.saturating_sub(height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines.join("\n"))
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title(title))
            .style(Style::new().fg(FG).bg(crate::tui::theme::BG_ALT)),
        area,
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
            ("Tab", "next"),
            ("^S", "create"),
            ("^N", "create with AI"),
            ("F1", "help"),
            ("Esc", "cancel"),
        ]),
        AddTaskStep::Title => dialog_hint_line(&[
            ("Enter", "create"),
            ("Tab", "next"),
            ("^N", "create with AI"),
            ("F1", "help"),
            ("Esc", "cancel"),
        ]),
        AddTaskStep::Description => dialog_hint_line(&[
            ("^S", "create"),
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
