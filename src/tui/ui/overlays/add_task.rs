use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::super::dialog::{Dialog, dialog_hint_line, dim_rendered_background};
use super::super::input::{clipped_input_line, cursor_cell};
use super::super::scroll::{clamp_scroll_start, render_vertical_scrollbar};
use super::super::truncate::truncate_chars;
use super::confirm::render_confirm;
use super::multiline::add_task_description_input_line;
use super::picker::{
    picker_filter_line, picker_hint_line, priority_picker_line, project_picker_line,
};
use super::shared::viewport_start_for_cursor;
use super::tag_combobox::tag_combobox_lines_with_viewport;
use crate::task_render::human_file_size;
use crate::tui::authoring::AddTaskStep;
use crate::tui::overlay::{
    AddTaskMode, AddTaskView, ConfirmView, PickerKind, PickerView, ScheduleEditorField,
    ScheduleEditorMode, ScheduleEditorState, TAG_COMBOBOX_VIEWPORT_ROWS, TagComboboxView,
    tag_combobox_completion, tag_combobox_matches, visible_picker_indices,
};
use crate::tui::text::cell_width_ranges;
use crate::tui::theme::{self, BG_ALT, BG_PANEL, FG, FG_DIM, FG_MUTED, SELECTED};
use crate::tui::widgets::{priority_short, status_span};

#[derive(Clone, Copy)]
pub(crate) struct AddTaskLayout<'a> {
    pub(crate) description: &'a [String],
    pub(crate) mode: &'a AddTaskMode,
    pub(crate) has_attachments: bool,
    pub(crate) show_schedule_error: bool,
}

pub(crate) fn add_task_field_at(
    terminal: Rect,
    full_frame: bool,
    layout: AddTaskLayout<'_>,
    column: u16,
    row: u16,
) -> Option<AddTaskStep> {
    let outer = if full_frame {
        terminal
    } else {
        crate::tui::overlay::dialog_area(terminal, 100, add_task_dialog_height(terminal, layout))
    };
    let has_attachments = layout.has_attachments;
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
    let steps = metadata_steps();
    let metadata_rows = if content.width >= 80 {
        2
    } else {
        steps.len().div_ceil(metadata_columns(content.width)) as u16
    };
    if relative_y < metadata_rows {
        if content.width >= 80 {
            return match relative_y {
                0 => Some(
                    [
                        AddTaskStep::Project,
                        AddTaskStep::Status,
                        AddTaskStep::Priority,
                    ][(relative_x as usize * 3 / content.width.max(1) as usize).min(2)],
                ),
                1 => Some(
                    [
                        AddTaskStep::Labels,
                        AddTaskStep::Schedule,
                        AddTaskStep::Epic,
                    ][(relative_x as usize * 3 / content.width.max(1) as usize).min(2)],
                ),
                _ => None,
            };
        }
        let columns = metadata_columns(content.width);
        let index = relative_y as usize * columns
            + (relative_x as usize * columns / content.width.max(1) as usize)
                .min(columns.saturating_sub(1));
        return steps.get(index).copied();
    }
    let preview_rows = 0;
    if relative_y == metadata_rows + preview_rows && has_attachments {
        return Some(AddTaskStep::Images);
    }
    if content.height <= 10 {
        let title_y = metadata_rows + preview_rows + u16::from(has_attachments);
        if relative_y == title_y {
            return Some(AddTaskStep::Title);
        }
        if relative_y == title_y + 1 {
            return Some(AddTaskStep::Description);
        }
        return None;
    }
    let title_y = metadata_rows + preview_rows + if has_attachments { 2 } else { 1 };
    if relative_y == title_y || relative_y == title_y + 1 {
        return Some(AddTaskStep::Title);
    }
    if relative_y >= title_y + 3 {
        return Some(AddTaskStep::Description);
    }
    None
}

const ADD_TASK_MAX_HEIGHT: u16 = 26;
const ADD_TASK_DEFAULT_DESCRIPTION_ROWS: usize = 3;

fn add_task_dialog_height(terminal: Rect, layout: AddTaskLayout<'_>) -> u16 {
    let available_height = terminal
        .height
        .saturating_sub(4)
        .clamp(1, ADD_TASK_MAX_HEIGHT);
    if layout.mode.expands_composer() {
        return available_height;
    }

    let outer = crate::tui::overlay::dialog_area(terminal, 100, available_height);
    let content_width = outer.width.saturating_sub(4) as usize;
    let metadata_rows = if content_width >= 80 {
        2
    } else {
        metadata_steps()
            .len()
            .div_ceil(metadata_columns(content_width as u16))
    };
    let schedule_error_rows = usize::from(layout.show_schedule_error);
    let attachment_rows = if layout.has_attachments { 2 } else { 1 };
    let description_rows = layout
        .description
        .iter()
        .enumerate()
        .map(|(row, line)| {
            add_task_description_visual_lines(
                line,
                None,
                layout.description.len() == 1 && line.is_empty() && row == 0,
                content_width,
            )
            .len()
        })
        .sum::<usize>()
        .max(ADD_TASK_DEFAULT_DESCRIPTION_ROWS);
    let fixed_content_rows = metadata_rows + schedule_error_rows + attachment_rows + 6;
    let desired_height = fixed_content_rows
        .saturating_add(description_rows)
        .saturating_add(2);
    desired_height.min(available_height as usize) as u16
}

pub(in crate::tui::ui) fn render_add_task(frame: &mut Frame, state: &AddTaskView) {
    let height = add_task_dialog_height(
        frame.area(),
        AddTaskLayout {
            description: &state.description,
            mode: &state.mode,
            has_attachments: !state.attachments.items.is_empty(),
            show_schedule_error: state.schedule_error.is_some()
                && state.schedule_validation_requested,
        },
    );
    let title = if state.editing_template {
        "Edit recurring template"
    } else {
        "Add task"
    };
    let dialog = Dialog::new(title, 100, height);
    let content = dialog.render_block(frame);
    render_add_task_body(frame, state, content);
}

pub(in crate::tui::ui) fn render_add_task_full_frame(frame: &mut Frame, state: &AddTaskView) {
    let area = frame.area();
    let title = if state.editing_template {
        "Edit recurring template"
    } else {
        "Add task"
    };
    let content = Dialog::new(title, area.width, area.height).render_block_at(frame, area);
    render_add_task_body(frame, state, content);
}

fn render_add_task_body(frame: &mut Frame, state: &AddTaskView, content: Rect) {
    let mut lines = add_task_metadata_lines(state, content.width);
    if state.schedule_error.is_some() && state.schedule_validation_requested {
        lines.push(fit_line_to_width(
            Line::from(Span::styled(
                format!("  Schedule: {}", crate::schedule_input::schedule_guidance()),
                Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
            )),
            content.width as usize,
        ));
    }
    if content.height <= 10 {
        if !state.attachments.items.is_empty() && lines.len() + 3 < content.height as usize {
            lines.push(add_task_attachment_line(state, content.width as usize));
        }
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
            !state.schedule_expanded,
        ));
        frame.render_widget(
            Paragraph::new(Text::from(lines))
                .style(Style::new().fg(FG).bg(crate::tui::theme::BG_ALT)),
            content,
        );
        render_add_task_child(frame, state, content);
        return;
    }
    if state.attachments.items.is_empty() {
        lines.push(Line::from(""));
    } else {
        lines.push(add_task_attachment_line(state, content.width as usize));
        lines.push(Line::from(""));
    }
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
    let reserved = lines.len().saturating_add(2);
    let description_rows = (content.height as usize).saturating_sub(reserved).max(1);
    lines.extend(add_task_description_lines(
        state,
        description_rows,
        content.width as usize,
    ));
    while lines.len() + 2 < content.height as usize {
        lines.push(Line::from(""));
    }
    lines.push(Line::from(""));
    lines.push(add_task_hint_line(
        state.focus,
        state.status_prefix_active,
        state.priority_prefix_active,
        !state.schedule_expanded,
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

fn add_task_attachment_line(state: &AddTaskView, width: usize) -> Line<'static> {
    let active = state.focus == AddTaskStep::Images;
    let count = state.attachments.items.len();
    let selected = state.attachments.selected.min(count.saturating_sub(1));
    let mut spans = vec![
        Span::styled(if active { "▶ " } else { "  " }, Style::new().fg(FG)),
        Span::styled(
            format!("Images ({count})"),
            if active {
                Style::new().fg(FG).add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(FG_DIM)
            },
        ),
    ];
    if let Some(attachment) = state.attachments.items.get(selected) {
        spans.push(Span::raw("  "));
        if count > 1 {
            if active {
                spans.push(Span::styled("◀ ", Style::new().fg(FG_MUTED)));
            }
            spans.push(Span::styled(
                format!("{}/{} ", selected + 1, count),
                Style::new().fg(FG_DIM),
            ));
        }
        spans.push(Span::raw(attachment.filename.clone()));
        if let Some((width, height)) = attachment.dimensions {
            spans.push(Span::styled(" · ", Style::new().fg(FG_DIM)));
            spans.push(Span::styled(
                format!("{width}×{height}"),
                Style::new().fg(FG_MUTED),
            ));
        }
        spans.push(Span::styled(" · ", Style::new().fg(FG_DIM)));
        spans.push(Span::styled(
            human_file_size(attachment.byte_size),
            Style::new().fg(FG_MUTED),
        ));
        if active && count > 1 {
            spans.push(Span::styled(" ▶", Style::new().fg(FG_MUTED)));
        }
    }
    fit_line_to_width(Line::from(spans), width)
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

fn metadata_steps() -> [AddTaskStep; 6] {
    [
        AddTaskStep::Project,
        AddTaskStep::Status,
        AddTaskStep::Priority,
        AddTaskStep::Labels,
        AddTaskStep::Schedule,
        AddTaskStep::Epic,
    ]
}

fn metadata_columns(width: u16) -> usize {
    match width {
        100.. => 4,
        60.. => 3,
        40.. => 2,
        _ => 1,
    }
}

fn add_task_metadata_lines(state: &AddTaskView, width: u16) -> Vec<Line<'static>> {
    let owned = [
        metadata_field(AddTaskStep::Project, "Project", &state.project, state.focus),
        metadata_field(AddTaskStep::Status, "Status", &state.status, state.focus),
        metadata_field(
            AddTaskStep::Priority,
            "Priority",
            &state.priority,
            state.focus,
        ),
        metadata_field(
            AddTaskStep::Labels,
            "Labels",
            &labels_display(&state.labels),
            state.focus,
        ),
        schedule_metadata_field(state),
        metadata_field(
            AddTaskStep::Epic,
            "Epic",
            if state.is_epic { "yes" } else { "no" },
            state.focus,
        ),
    ];
    if width >= 80 {
        return vec![
            metadata_row(owned[..3].to_vec(), width as usize),
            metadata_row(owned[3..].to_vec(), width as usize),
        ];
    }
    let columns = metadata_columns(width);
    owned
        .chunks(columns)
        .map(|chunk| metadata_row(chunk.to_vec(), width as usize))
        .collect()
}

fn schedule_metadata_field(state: &AddTaskView) -> Line<'static> {
    let mut line = metadata_field(
        AddTaskStep::Schedule,
        "Schedule",
        if state.schedule_input.is_empty() {
            "none"
        } else {
            &state.schedule_input
        },
        state.focus,
    );
    if state.focus == AddTaskStep::Schedule && !state.editing_template {
        line.spans.pop();
        line.spans.extend(
            placeholder_input_line(
                &state.schedule_input,
                Some(state.schedule_input_cursor),
                64,
                "type a schedule or press enter",
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
        AddTaskStep::Epic => "",
        AddTaskStep::Schedule | AddTaskStep::RepeatRule => "   ",
        AddTaskStep::AvailableAt | AddTaskStep::RepeatAt => "^A ",
        AddTaskStep::Due | AddTaskStep::RepeatDue => "^U ",
        _ => "   ",
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
    ("Tab / Shift+Tab", "next / previous; validate edited field"),
    ("Arrows", "move fields or edit the text cursor"),
    ("Enter", "open focused control; create from title"),
    (
        "Schedule",
        "type naturally; Enter opens the schedule editor",
    ),
    (
        "Schedule examples",
        "tomorrow · due Friday · every Friday at 09:00",
    ),
    ("Ctrl-a / Ctrl-u", "edit one-off Available / Due"),
    (
        "Schedule editor",
        "↑/↓ move · ←/→ choose · Enter apply · Esc cancel",
    ),
    ("Docs", "https://aven.raine.dev/tui/#capture-tasks"),
    (
        "One-off Available",
        "when the task becomes actionable; empty = now",
    ),
    ("One-off Due", "due date; empty = no due date"),
    ("Repeat", "daily · weekdays · every Friday · every 3 weeks"),
    (
        "Repeat Available",
        "time each occurrence appears; empty = start of day",
    ),
    ("Repeat Due", "same day or no due date"),
    ("Ctrl-p/t/r/l", "jump to other metadata fields"),
    ("Ctrl-Enter / Ctrl-s", "create from any field"),
    ("Ctrl-n", "create with AI"),
    ("Images", "Left/Right select; D removes selected image"),
    ("Ctrl-x Ctrl-e", "edit description externally"),
    ("Esc", "cancel or confirm discard"),
];
const COMPOSER_HELP_HEIGHT: u16 = 19;
const COMPOSER_HELP_FIXED_ROWS: u16 = 4;

pub(crate) fn composer_help_scroll_cap(
    frame_height: u16,
    full_frame: bool,
    schedule_expanded: bool,
) -> u16 {
    let frame_padding = if full_frame {
        2
    } else if schedule_expanded {
        4
    } else {
        6
    };
    let add_task_content_height = frame_height.saturating_sub(frame_padding).min(20);
    let dialog_height = COMPOSER_HELP_HEIGHT.min(add_task_content_height);
    let visible_rows = dialog_height.saturating_sub(COMPOSER_HELP_FIXED_ROWS) as usize;
    COMPOSER_HELP_TOPICS.len().saturating_sub(visible_rows) as u16
}

fn schedule_editor_lines(editor: &ScheduleEditorState) -> Vec<Line<'static>> {
    let active = Style::new().fg(FG).add_modifier(Modifier::BOLD);
    let inactive = Style::new().fg(FG_DIM);
    let mode = |label: &'static str, selected| {
        Span::styled(
            format!(" {label} "),
            if selected {
                Style::new()
                    .fg(theme::INVERSE_FG)
                    .bg(theme::ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(FG).bg(theme::BORDER)
            },
        )
    };
    let mut lines = vec![Line::from(vec![
        Span::styled(
            if editor.focus == ScheduleEditorField::Mode {
                "▶ Type      "
            } else {
                "  Type      "
            },
            if editor.focus == ScheduleEditorField::Mode {
                active
            } else {
                inactive
            },
        ),
        mode("One-off", editor.mode == ScheduleEditorMode::Once),
        Span::raw("  "),
        mode("Repeating", editor.mode == ScheduleEditorMode::Repeat),
    ])];

    match editor.mode {
        ScheduleEditorMode::Once => {
            lines.push(schedule_editor_input_line(
                "Available",
                &editor.available_at.text,
                editor.available_at.cursor,
                editor.focus == ScheduleEditorField::Available,
                "tomorrow or next monday at 9am",
            ));
            lines.push(schedule_editor_input_line(
                "Due",
                &editor.due_on.text,
                editor.due_on.cursor,
                editor.focus == ScheduleEditorField::Due,
                "next friday or none",
            ));
        }
        ScheduleEditorMode::Repeat => {
            lines.push(schedule_editor_input_line(
                "Repeat",
                &editor.repeat_rule.text,
                editor.repeat_rule.cursor,
                editor.focus == ScheduleEditorField::Repeat && !editor.template_locked,
                "daily or every Friday",
            ));
            lines.push(schedule_editor_input_line(
                "Available",
                &editor.repeat_at.text,
                editor.repeat_at.cursor,
                editor.focus == ScheduleEditorField::Time,
                "09:00; empty means start of day",
            ));
            lines.push(Line::from(vec![
                Span::styled(
                    if editor.focus == ScheduleEditorField::DuePolicy {
                        "▶ Due       "
                    } else {
                        "  Due       "
                    },
                    if editor.focus == ScheduleEditorField::DuePolicy {
                        active
                    } else {
                        inactive
                    },
                ),
                Span::raw(if editor.repeat_due == "none" {
                    "None - occurrences have no due date"
                } else {
                    "Same day - due on the occurrence day"
                }),
            ]));
            lines.push(schedule_editor_input_line(
                "Starts",
                &editor.repeat_start_on.text,
                editor.repeat_start_on.cursor,
                editor.focus == ScheduleEditorField::Starts && !editor.template_locked,
                "YYYY-MM-DD",
            ));
            if !editor.preview.is_empty() {
                lines.push(Line::from(Span::styled(
                    format!("  Next      {}", editor.preview.join(", ")),
                    Style::new().fg(FG_DIM),
                )));
            }
        }
    }

    if let Some(error) = &editor.error {
        let message = if error.contains("invalid-repeat-at") {
            "Available: use 09:00 or leave empty"
        } else if error.contains("invalid-recurrence-date") {
            "Starts: use a date in YYYY-MM-DD form"
        } else if error.contains("invalid-available-at") {
            "Available: try tomorrow or next Monday at 9am"
        } else if error.contains("invalid-due") {
            "Due: try next Friday or leave empty"
        } else if editor.mode == ScheduleEditorMode::Repeat {
            "Repeat: use daily, weekdays, every Friday, or every 3 weeks"
        } else {
            crate::schedule_input::schedule_guidance()
        };
        lines.push(Line::from(Span::styled(
            format!("  {message}"),
            Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
        )));
    }
    lines.push(Line::from(""));
    lines.push(dialog_hint_line(&[
        ("↑/↓", "move"),
        ("←/→", "choose"),
        ("Enter", "apply"),
        ("Esc", "cancel"),
    ]));
    lines
}

fn schedule_editor_input_line(
    label: &'static str,
    value: &str,
    cursor: usize,
    focused: bool,
    placeholder: &'static str,
) -> Line<'static> {
    let mut spans = vec![Span::styled(
        if focused {
            format!("▶ {label:<10}")
        } else {
            format!("  {label:<10}")
        },
        if focused {
            Style::new().fg(FG).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(FG_DIM)
        },
    )];
    spans.extend(placeholder_input_line(value, focused.then_some(cursor), 48, placeholder).spans);
    Line::from(spans)
}

fn render_add_task_child(frame: &mut Frame, state: &AddTaskView, content: Rect) {
    if matches!(state.mode.as_ref(), AddTaskMode::Compose) {
        return;
    }
    dim_rendered_background(frame);

    if matches!(state.mode.as_ref(), AddTaskMode::ConfirmDiscard) {
        render_confirm(
            frame,
            &ConfirmView {
                title: "Discard draft?".to_string(),
                prompt: "Discard this task draft?".to_string(),
            },
        );
        return;
    }
    if let AddTaskMode::Help { scroll } = state.mode.as_ref() {
        render_composer_help(frame, content, *scroll);
        return;
    }

    let (title, lines, width, background) = match state.mode.as_ref() {
        AddTaskMode::Compose => unreachable!("composer has no child dialog"),
        AddTaskMode::Schedule(editor) => (
            "Schedule".to_string(),
            schedule_editor_lines(editor),
            68,
            BG_ALT,
        ),
        AddTaskMode::Picker { state, .. } => {
            let view = PickerView {
                kind: (&state.intent).into(),
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
            (
                view.title.clone(),
                add_task_picker_lines(&view, content.height.saturating_sub(2) as usize),
                54,
                BG_ALT,
            )
        }
        AddTaskMode::Labels(state) => {
            let visible_indices = tag_combobox_matches(state);
            let view = TagComboboxView {
                kind: (&state.intent).into(),
                title: state.title.clone(),
                input: state.input.text.clone(),
                input_cursor: state.input.cursor,
                completion: tag_combobox_completion(state),
                options: state.options.clone(),
                selected: state.selected.clone(),
                partial: state.partial.clone(),
                highlighted: state.highlighted,
                visible_start: visible_indices
                    .iter()
                    .position(|index| *index == state.highlighted)
                    .unwrap_or(0)
                    .saturating_sub(TAG_COMBOBOX_VIEWPORT_ROWS.saturating_sub(1)),
                visible_indices,
            };
            let viewport_rows = content.height.saturating_sub(2).saturating_sub(4).max(1) as usize;
            (
                view.title.clone(),
                tag_combobox_lines_with_viewport(&view, viewport_rows),
                64,
                BG_PANEL,
            )
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
    let width = 82.min(content.width.saturating_sub(2)).max(1);
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
    let (key_style, description_style) = if keys == "Docs" {
        (
            Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD),
            Style::new()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::UNDERLINED),
        )
    } else {
        (Style::new().fg(FG_MUTED), Style::new().fg(FG_DIM))
    };
    Line::from(vec![
        Span::styled(format!("{keys:<22}"), key_style),
        Span::styled(description, description_style),
    ])
}

fn add_task_picker_lines(state: &PickerView, available_rows: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let filter_rows = if matches!(state.mode, crate::tui::overlay::PickerMode::Filter) {
        lines.push(picker_filter_line(
            Span::raw("/"),
            &state.filter,
            state.filter_cursor,
        ));
        lines.push(Line::from(""));
        2
    } else {
        0
    };
    let viewport_rows = available_rows.saturating_sub(filter_rows + 2).max(1);
    let selected_position = state
        .visible_indices
        .iter()
        .position(|index| *index == state.selected);
    let scroll = selected_position
        .map(|position| {
            state
                .scroll
                .max(position.saturating_add(1).saturating_sub(viewport_rows))
        })
        .unwrap_or(state.scroll)
        .min(state.visible_indices.len().saturating_sub(viewport_rows));
    let visible = state
        .visible_indices
        .iter()
        .skip(scroll)
        .take(viewport_rows)
        .copied()
        .collect::<Vec<_>>();
    for index in visible {
        let item = &state.items[index];
        let selected = index == state.selected;
        let line = match state.kind {
            PickerKind::AddTaskProject => project_picker_line(item, selected),
            PickerKind::AddTaskPriority => priority_picker_line(item, selected),
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
    show_repeat_shortcut: bool,
) -> Line<'static> {
    if status_prefix_active {
        return add_task_status_hint_line();
    }
    if priority_prefix_active {
        return add_task_priority_hint_line();
    }

    match focus {
        AddTaskStep::Schedule => dialog_hint_line(&[
            ("type", "schedule"),
            ("Enter", "details"),
            ("^A", "available"),
            ("^U", "due"),
            ("Tab", "next"),
            ("F1", "help"),
            ("Esc", "cancel"),
        ]),
        AddTaskStep::Epic => dialog_hint_line(&[
            ("Enter", "toggle"),
            ("←/→", "field"),
            ("Tab", "next"),
            ("^S", "create"),
            ("^N", "create with AI"),
            ("F1", "help"),
            ("Esc", "cancel"),
        ]),
        AddTaskStep::Project
        | AddTaskStep::Status
        | AddTaskStep::Priority
        | AddTaskStep::Labels
        | AddTaskStep::RepeatDue => dialog_hint_line(&[
            ("Enter", "choose"),
            ("←/→", "field"),
            ("Tab", "next"),
            ("^S", "create"),
            ("^N", "create with AI"),
            ("F1", "help"),
            ("Esc", "cancel"),
        ]),
        AddTaskStep::Images => dialog_hint_line(&[
            ("←/→", "image"),
            ("D", "remove"),
            ("Tab", "next"),
            ("^S", "create"),
            ("F1", "help"),
            ("Esc", "cancel"),
        ]),
        AddTaskStep::Title if show_repeat_shortcut => dialog_hint_line(&[
            ("Enter", "create"),
            ("↑/↓", "field"),
            ("Tab", "next"),
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
        AddTaskStep::RepeatAt => dialog_hint_line(&[
            ("←/→", "cursor"),
            ("HH:MM", "availability time"),
            ("none", "start of day"),
            ("Tab", "next"),
            ("^S", "create"),
            ("F1", "formats"),
            ("Esc", "cancel"),
        ]),
        AddTaskStep::AvailableAt
        | AddTaskStep::RepeatRule
        | AddTaskStep::TimeZone
        | AddTaskStep::RepeatStartOn => dialog_hint_line(&[
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
        AddTaskStep::Description if show_repeat_shortcut => dialog_hint_line(&[
            ("Ctrl-Enter / ^S", "create"),
            ("Tab", "next"),
            ("^N", "create with AI"),
            ("F1", "help"),
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
