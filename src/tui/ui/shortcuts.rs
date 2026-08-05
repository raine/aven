use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};

use super::ViewState;
use super::dialog::Dialog;
use super::input::input_line;
use super::scroll::{clamp_scroll_start, render_vertical_scrollbar};
use crate::tui::app::{DetailSection, DetailTargetId};
use crate::tui::event::{
    BulkSupport, CommandContext, CommandLifecycle, CommandSpec, matching_commands_for_bulk,
    prefix_hint_commands,
};
use crate::tui::theme::{
    ACCENT, BG_ALT, BG_PANEL, BORDER, FG, FG_DIM, FG_MUTED, ORANGE, SELECTED_BG,
};

struct HelpTopic {
    keys: &'static str,
    description: &'static str,
    section: &'static str,
}

const DETAIL_HELP_TOPICS: &[HelpTopic] = &[
    HelpTopic {
        keys: "Esc",
        description: "back one level",
        section: "General",
    },
    HelpTopic {
        keys: "Enter",
        description: "open focused relationship or attachment",
        section: "General",
    },
    HelpTopic {
        keys: "q",
        description: "close detail",
        section: "General",
    },
    HelpTopic {
        keys: "?",
        description: "toggle task detail help",
        section: "General",
    },
    HelpTopic {
        keys: "Tab/Shift+Tab",
        description: "focus relationships and attachments",
        section: "Task detail",
    },
    HelpTopic {
        keys: "s / t d",
        description: "set focused related task status / done",
        section: "Task detail",
    },
    HelpTopic {
        keys: "t U / t c r",
        description: "unlink focused dependency / epic relationship",
        section: "Task detail",
    },
    HelpTopic {
        keys: "y r / y i / t D",
        description: "copy focused related ref / id or delete task",
        section: "Task detail",
    },
    HelpTopic {
        keys: "C-d C-u",
        description: "scroll one page",
        section: "Task detail",
    },
    HelpTopic {
        keys: "j/k Up/Down",
        description: "scroll one line",
        section: "Task detail",
    },
    HelpTopic {
        keys: "[/]",
        description: "select previous or next task",
        section: "Task detail",
    },
];

const CHILD_DETAIL_HELP_TOPICS: &[HelpTopic] = &[
    HelpTopic {
        keys: "Esc",
        description: "clear child focus",
        section: "General",
    },
    HelpTopic {
        keys: "Enter",
        description: "open focused child",
        section: "General",
    },
    HelpTopic {
        keys: "?",
        description: "toggle child task help",
        section: "General",
    },
    HelpTopic {
        keys: "q",
        description: "close detail",
        section: "General",
    },
    HelpTopic {
        keys: "Tab/Shift+Tab",
        description: "focus next or previous detail row",
        section: "Child task",
    },
    HelpTopic {
        keys: "j/k Up/Down",
        description: "focus next or previous child",
        section: "Child task",
    },
    HelpTopic {
        keys: "s / d / x",
        description: "choose status / mark done / cancel",
        section: "Child task",
    },
    HelpTopic {
        keys: "y t / y r / y i",
        description: "copy title / display ref / durable ID",
        section: "Child task",
    },
    HelpTopic {
        keys: "t c r",
        description: "unlink from epic after confirmation",
        section: "Child task",
    },
    HelpTopic {
        keys: "t D",
        description: "delete task after confirmation",
        section: "Child task",
    },
    HelpTopic {
        keys: "u",
        description: "undo last TUI mutation",
        section: "Child task",
    },
];

const HELP_DIALOG_MAX_WIDTH: u16 = 112;
const HELP_DIALOG_MAX_HEIGHT: u16 = 28;
const COMMAND_DIALOG_MAX_WIDTH: u16 = 112;

fn help_dialog_height(frame_height: u16) -> u16 {
    frame_height.saturating_sub(4).min(HELP_DIALOG_MAX_HEIGHT)
}

fn help_dialog_size(area: Rect) -> (u16, u16) {
    (
        area.width.saturating_sub(6).min(HELP_DIALOG_MAX_WIDTH),
        help_dialog_height(area.height),
    )
}

pub(super) fn render_help(frame: &mut Frame, scroll: u16) {
    let (width, height) = help_dialog_size(frame.area());
    let visible_rows = height.saturating_sub(2);
    let dialog = if let Some(title) = help_scroll_title(scroll, visible_rows) {
        Dialog::new("Shortcuts", width, height)
            .right_title(Line::from(Span::styled(title, Style::new().fg(FG_MUTED))))
    } else {
        Dialog::new("Shortcuts", width, height)
    };
    let content = dialog.render_block(frame);
    let [left, _, right] = Layout::horizontal([
        Constraint::Ratio(1, 2),
        Constraint::Length(4),
        Constraint::Ratio(1, 2),
    ])
    .areas(content);
    let columns = help_columns();
    let content_height = columns
        .iter()
        .map(|sections| help_column_lines(sections).len())
        .max()
        .unwrap_or(0);
    for (column, sections) in [left, right].into_iter().zip(columns.iter()) {
        render_help_column(frame, column, sections, scroll);
    }
    render_vertical_scrollbar(frame, content, content_height, scroll);
}

fn help_columns() -> [Vec<&'static str>; 2] {
    let section_count = CommandContext::Normal.sections().len();
    let section_rows = CommandContext::Normal
        .sections()
        .iter()
        .map(|section| help_section_len(section))
        .collect::<Vec<_>>();
    let total_section_rows = section_rows.iter().sum::<usize>();
    let mut best_mask = 1;
    let mut best_score = (usize::MAX, usize::MAX, usize::MAX);

    for mask in 1usize..(1usize << section_count) - 1 {
        if mask & 1 == 0 {
            continue;
        }
        let left_count = mask.count_ones() as usize;
        let right_count = section_count - left_count;
        let left_rows = section_rows
            .iter()
            .enumerate()
            .filter(|(index, _)| mask & (1usize << index) != 0)
            .map(|(_, rows)| *rows)
            .sum::<usize>()
            + left_count.saturating_sub(1);
        let right_rows = total_section_rows + section_count - 2 - left_rows;
        let tail_left = (section_count.saturating_sub(3)..section_count)
            .filter(|index| mask & (1usize << index) != 0)
            .count();
        let tail_right = 3 - tail_left;
        let score = (
            left_rows.abs_diff(right_rows),
            tail_left.abs_diff(tail_right),
            left_count.abs_diff(right_count),
        );
        if score < best_score {
            best_mask = mask;
            best_score = score;
        }
    }

    let mut left = Vec::new();
    let mut right = Vec::new();
    for (index, section) in CommandContext::Normal.sections().iter().enumerate() {
        if best_mask & (1usize << index) != 0 {
            left.push(*section);
        } else {
            right.push(*section);
        }
    }

    [left, right]
}

fn help_section_len(section: &str) -> usize {
    CommandContext::Normal
        .commands()
        .filter(|command| command.section == section)
        .count()
        + 1
}

fn render_help_column(frame: &mut Frame, area: Rect, sections: &[&'static str], scroll: u16) {
    let lines = help_column_lines(sections);
    let start = clamp_scroll_start(scroll, lines.len(), area.height as usize);
    let visible = lines
        .into_iter()
        .skip(start)
        .take(area.height as usize)
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(Text::from(visible)).style(Style::new().fg(FG).bg(BG_ALT)),
        area,
    );
}

fn render_scrollable_help_lines(
    frame: &mut Frame,
    area: Rect,
    lines: Vec<Line<'static>>,
    scroll: u16,
) {
    let content_height = lines.len();
    let visible_rows = area.height as usize;
    let start = clamp_scroll_start(scroll, content_height, visible_rows);
    let visible = lines
        .into_iter()
        .skip(start)
        .take(visible_rows)
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(Text::from(visible)).style(Style::new().fg(FG).bg(BG_ALT)),
        area,
    );
    render_vertical_scrollbar(frame, area, content_height, scroll);
}

fn focused_epic_child(target: Option<&DetailTargetId>) -> bool {
    matches!(
        target,
        Some(DetailTargetId::Task {
            section: DetailSection::EpicChildren,
            ..
        })
    )
}

pub(super) fn render_detail_help(
    frame: &mut Frame,
    scroll: u16,
    focused_target: Option<&DetailTargetId>,
) {
    let child_help = focused_epic_child(focused_target);
    let title = if child_help {
        "Child task shortcuts"
    } else {
        "Task detail shortcuts"
    };
    let lines = detail_help_lines_for(focused_target);
    let (width, height) = help_dialog_size(frame.area());
    let mut dialog = Dialog::new(title, width, height);
    let visible_rows = dialog.area(frame).height.saturating_sub(2);
    if let Some(title) = detail_help_scroll_title(scroll, visible_rows, lines.len()) {
        dialog = dialog.right_title(Line::from(Span::styled(title, Style::new().fg(FG_MUTED))));
    }
    let content = dialog.render_block(frame);
    render_scrollable_help_lines(frame, content, lines, scroll);
}

fn detail_help_lines_for(focused_target: Option<&DetailTargetId>) -> Vec<Line<'static>> {
    if focused_epic_child(focused_target) {
        return focused_help_lines(CHILD_DETAIL_HELP_TOPICS);
    }
    detail_help_lines()
}

fn focused_help_lines(topics: &[HelpTopic]) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut sections = Vec::new();
    for section in topics.iter().map(|topic| topic.section) {
        if sections.contains(&section) {
            continue;
        }
        sections.push(section);
        if !lines.is_empty() {
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled(
            section,
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        )));
        lines.extend(
            topics
                .iter()
                .filter(|topic| topic.section == section)
                .map(detail_help_line),
        );
    }
    lines
}

fn detail_help_lines() -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for section in CommandContext::Detail.sections() {
        let fixed = DETAIL_HELP_TOPICS
            .iter()
            .filter(|topic| topic.section == *section)
            .collect::<Vec<_>>();
        let commands = CommandContext::Detail
            .commands()
            .filter(|command| command.section == *section)
            .collect::<Vec<_>>();
        if fixed.is_empty() && commands.is_empty() {
            continue;
        }
        if !lines.is_empty() {
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled(
            *section,
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        )));
        lines.extend(fixed.into_iter().map(detail_help_line));
        lines.extend(
            commands
                .into_iter()
                .map(|command| help_command_line(command, CommandContext::Detail)),
        );
    }
    lines
}

fn detail_help_line(topic: &HelpTopic) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{:<18}", topic.keys), Style::new().fg(FG_MUTED)),
        Span::styled(topic.description, Style::new().fg(FG_DIM)),
    ])
}

fn detail_help_scroll_title(scroll: u16, visible_rows: u16, max_rows: usize) -> Option<String> {
    let visible_rows = visible_rows as usize;
    if max_rows <= visible_rows {
        return None;
    }
    let total = max_rows.saturating_sub(visible_rows).saturating_add(1);
    let current = (scroll as usize).saturating_add(1).min(total);
    Some(format!(" {current}/{total} "))
}

fn help_scroll_title(scroll: u16, visible_rows: u16) -> Option<String> {
    let max_rows = help_columns()
        .iter()
        .map(|sections| help_column_lines(sections).len())
        .max()
        .unwrap_or(0);
    let visible_rows = visible_rows as usize;
    if max_rows <= visible_rows {
        return None;
    }
    let total = max_rows.saturating_sub(visible_rows).saturating_add(1);
    let current = (scroll as usize).saturating_add(1).min(total);
    Some(format!(" {current}/{total} "))
}

fn help_column_lines(sections: &[&'static str]) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for section in sections {
        if !lines.is_empty() {
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled(
            *section,
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        )));
        for command in CommandContext::Normal
            .commands()
            .filter(|command| command.section == *section)
        {
            lines.push(help_command_line(command, CommandContext::Normal));
        }
    }
    lines
}

pub(crate) fn help_scroll_cap(frame_height: u16) -> u16 {
    let visible_rows = help_dialog_height(frame_height).saturating_sub(2) as usize;
    help_columns()
        .iter()
        .map(|sections| {
            help_column_lines(sections)
                .len()
                .saturating_sub(visible_rows)
        })
        .max()
        .unwrap_or(0) as u16
}

pub(crate) fn detail_help_scroll_cap(
    frame_height: u16,
    focused_target: Option<&DetailTargetId>,
) -> u16 {
    let visible_rows = help_dialog_height(frame_height).saturating_sub(2) as usize;
    detail_help_lines_for(focused_target)
        .len()
        .saturating_sub(visible_rows) as u16
}

fn command_name_style(command: &CommandSpec) -> Style {
    match command.lifecycle {
        CommandLifecycle::Implemented => Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        CommandLifecycle::Planned { .. } => Style::new().fg(FG_MUTED),
        CommandLifecycle::Disabled { .. } => Style::new().fg(FG_DIM),
    }
}

fn lifecycle_badge(lifecycle: CommandLifecycle) -> Option<Span<'static>> {
    match lifecycle {
        CommandLifecycle::Implemented => None,
        CommandLifecycle::Planned { .. } => {
            Some(Span::styled(" planned ", Style::new().fg(ORANGE)))
        }
        CommandLifecycle::Disabled { .. } => {
            Some(Span::styled(" disabled ", Style::new().fg(FG_DIM)))
        }
    }
}

fn command_hint_line(
    leading: Span<'static>,
    command: &CommandSpec,
    command_name_width: usize,
) -> Line<'static> {
    let mut spans = vec![
        leading,
        Span::styled(
            format!(":{:<command_name_width$}", command.name),
            command_name_style(command),
        ),
    ];
    if let Some(badge) = lifecycle_badge(command.lifecycle) {
        spans.push(badge);
    }
    spans.push(Span::styled(command.description, Style::new().fg(FG_DIM)));
    Line::from(spans)
}

fn command_name_width(commands: &[&CommandSpec]) -> usize {
    commands
        .iter()
        .map(|command| command.name.len())
        .max()
        .unwrap_or(18)
        .max(18)
        .saturating_add(2)
}

fn help_command_line(command: &CommandSpec, context: CommandContext) -> Line<'static> {
    let keys = command
        .keys(context)
        .iter()
        .map(|key| key.label)
        .collect::<Vec<_>>()
        .join("/");
    let key_width = match context {
        CommandContext::Normal => 14,
        CommandContext::Detail => 18,
    };
    let mut spans = vec![Span::styled(
        format!("{keys:<key_width$}"),
        Style::new().fg(FG_MUTED),
    )];
    if let Some(badge) = lifecycle_badge(command.lifecycle) {
        spans.push(badge);
    }
    spans.push(Span::styled(command.description, Style::new().fg(FG_DIM)));
    Line::from(spans)
}

#[cfg(test)]
fn command_line(command: &CommandSpec, context: CommandContext) -> Line<'static> {
    command_line_with_highlight(command, context, false)
}

#[cfg(test)]
fn command_line_with_highlight(
    command: &CommandSpec,
    context: CommandContext,
    highlighted: bool,
) -> Line<'static> {
    let keys = command
        .keys(context)
        .iter()
        .map(|key| key.label)
        .collect::<Vec<_>>()
        .join("/");
    let mut line = command_hint_line(
        Span::styled(format!("{keys:<10}"), Style::new().fg(FG_MUTED)),
        command,
        18,
    );
    if highlighted {
        line.style = line.style.bg(SELECTED_BG);
        for span in &mut line.spans {
            span.style = span.style.bg(SELECTED_BG);
        }
        line.spans
            .push(Span::styled(" ".repeat(80), Style::new().bg(SELECTED_BG)));
    }
    line
}

fn command_palette_line(
    command: &CommandSpec,
    context: CommandContext,
    command_name_width: usize,
    line_width: usize,
    highlighted: bool,
    annotation: Option<String>,
    unavailable_reason: Option<&str>,
) -> Line<'static> {
    let keys = command
        .keys(context)
        .iter()
        .min_by_key(|key| unicode_width::UnicodeWidthStr::width(key.label))
        .map_or("", |key| key.label);
    let mut spans = vec![
        Span::styled(format!("{keys:<10}"), Style::new().fg(FG_MUTED)),
        Span::styled(
            format!(":{:<command_name_width$}", command.name),
            command_name_style(command),
        ),
    ];
    if let Some(badge) = lifecycle_badge(command.lifecycle) {
        spans.push(badge);
    }
    if let Some(annotation) = annotation {
        spans.push(Span::styled(annotation, Style::new().fg(FG_MUTED)));
    }
    if let Some(reason) = unavailable_reason {
        spans.push(Span::styled(
            format!("disabled: {reason} · "),
            Style::new().fg(FG_DIM),
        ));
    }
    let used_width = spans
        .iter()
        .map(|span| unicode_width::UnicodeWidthStr::width(span.content.as_ref()))
        .sum::<usize>();
    let description =
        super::truncate::truncate_width(command.description, line_width.saturating_sub(used_width));
    spans.push(Span::styled(description, Style::new().fg(FG_DIM)));
    if unavailable_reason.is_some() {
        for span in &mut spans {
            span.style = span.style.fg(FG_DIM);
        }
    }
    let mut line = Line::from(spans);
    if highlighted {
        line.style = line.style.bg(SELECTED_BG);
        for span in &mut line.spans {
            span.style = span.style.bg(SELECTED_BG);
        }
        line.spans
            .push(Span::styled(" ".repeat(80), Style::new().bg(SELECTED_BG)));
    }
    line
}

pub(super) struct CommandRenderContext<'a> {
    pub(super) unavailable: &'a [crate::tui::overlay::CommandAvailabilityOverride],
    pub(super) command_context: CommandContext,
    pub(super) marked_task_count: usize,
}

pub(super) fn render_command(
    frame: &mut Frame,
    input: &str,
    cursor: usize,
    cycle_input: Option<&str>,
    highlighted: Option<&str>,
    render_context: CommandRenderContext<'_>,
) {
    let CommandRenderContext {
        unavailable,
        command_context,
        marked_task_count,
    } = render_context;
    let matches = matching_commands_for_bulk(
        command_context,
        cycle_input.unwrap_or(input),
        marked_task_count,
    );
    let match_count = matches.len();
    let selected = highlighted
        .and_then(|highlighted| {
            matches
                .iter()
                .position(|command| command.name == highlighted)
        })
        .unwrap_or(0);
    let offset = selected.saturating_sub(7);
    let visible_end = offset.saturating_add(8).min(match_count);
    let command_name_width = command_name_width(&matches[offset..visible_end]);
    let height = (visible_end.saturating_sub(offset) as u16)
        .saturating_add(3)
        .saturating_add(u16::from(match_count > 0));
    let title = if marked_task_count == 0 {
        "Command".to_string()
    } else {
        format!("Command · {}", marked_task_label(marked_task_count))
    };
    let dialog_width = frame
        .area()
        .width
        .saturating_sub(2)
        .min(COMMAND_DIALOG_MAX_WIDTH);
    let mut dialog = Dialog::new(&title, dialog_width, height);
    if match_count > 0 {
        let position = highlighted.map_or_else(
            || format!("{match_count} commands"),
            |_| format!("{}/{match_count}", selected + 1),
        );
        dialog = dialog.right_title(Line::from(Span::styled(
            position,
            Style::new().fg(FG_MUTED),
        )));
    }
    let content = dialog.render_block(frame);
    let line_width = (content.width as usize).saturating_sub(usize::from(match_count > 8));

    let mut lines = vec![input_line(":", input, cursor)];
    for command in matches.into_iter().skip(offset).take(8) {
        let unavailable_reason = unavailable
            .iter()
            .find(|override_| override_.action == command.action)
            .map(|override_| override_.reason);
        let is_highlighted = highlighted == Some(command.name);
        let annotation = match command.bulk_support() {
            BulkSupport::Batch if marked_task_count > 0 => {
                let noun = if marked_task_count == 1 {
                    "task"
                } else {
                    "tasks"
                };
                Some(format!("{marked_task_count} {noun} · "))
            }
            BulkSupport::Focused if marked_task_count > 0 => Some("focused task · ".to_string()),
            BulkSupport::SingleOnly(_) | BulkSupport::BulkControl | BulkSupport::NotTaskScoped => {
                None
            }
            BulkSupport::Batch | BulkSupport::Focused => None,
        };
        lines.push(command_palette_line(
            command,
            command_context,
            command_name_width,
            line_width,
            is_highlighted,
            annotation,
            unavailable_reason,
        ));
    }
    if match_count > 0 {
        lines.push(Line::from(Span::styled(
            "  ↑/↓ browse · Enter run · Tab complete",
            Style::new().fg(FG_MUTED),
        )));
    }

    frame.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::new().fg(FG).bg(BG_ALT)),
        content,
    );
    render_command_scrollbar(frame, content, match_count, offset);
}

fn render_command_scrollbar(frame: &mut Frame, content: Rect, match_count: usize, offset: usize) {
    const VISIBLE_COMMANDS: usize = 8;
    if match_count <= VISIBLE_COMMANDS {
        return;
    }
    let area = Rect {
        y: content.y.saturating_add(1),
        height: (match_count.min(VISIBLE_COMMANDS)) as u16,
        ..content
    };
    let position = super::scroll::scrollbar_thumb_position(offset, match_count, VISIBLE_COMMANDS);
    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None)
        .thumb_symbol("┃")
        .track_symbol(Some("│"))
        .thumb_style(Style::new().fg(ACCENT).bg(BG_ALT))
        .track_style(Style::new().fg(BORDER).bg(BG_ALT));
    let mut state = ScrollbarState::new(match_count)
        .position(position)
        .viewport_content_length(VISIBLE_COMMANDS);
    frame.render_stateful_widget(scrollbar, area, &mut state);
}

fn marked_task_label(count: usize) -> String {
    let noun = if count == 1 { "task" } else { "tasks" };
    format!("{count} marked {noun}")
}

fn prefix_hint_lines(context: CommandContext, pending: &[String]) -> Vec<Line<'static>> {
    prefix_hint_lines_with_availability(context, pending, true, true, 0)
}

fn prefix_hint_lines_with_availability(
    context: CommandContext,
    pending: &[String],
    copy_description_available: bool,
    copy_notes_available: bool,
    marked_task_count: usize,
) -> Vec<Line<'static>> {
    let matches = prefix_hint_commands(context, pending);
    let command_name_width = command_name_width(
        &matches
            .iter()
            .map(|(command, _, _)| *command)
            .collect::<Vec<_>>(),
    );
    matches
        .into_iter()
        .map(|(command, _, key_hint)| {
            let support = command.bulk_support();
            let copy_mark_limit = command.action.copy_requires_single_task()
                && marked_task_count > 0
                && context == CommandContext::Normal;
            let unavailable = matches!(
                command.action,
                crate::tui::event::Action::CopyTaskDescription
            ) && !copy_description_available
                || matches!(command.action, crate::tui::event::Action::CopyTaskNotes)
                    && !copy_notes_available
                || copy_mark_limit
                || matches!(support, BulkSupport::SingleOnly(_)) && marked_task_count > 1;
            let mut line = command_hint_line(
                Span::styled(
                    format!(" {:<6} ", key_hint),
                    Style::new().fg(FG_MUTED).bg(BG_PANEL),
                ),
                command,
                command_name_width,
            );
            if copy_mark_limit {
                line.spans
                    .push(Span::styled(" · 1 task only", Style::new().fg(FG_DIM)));
            } else {
                match support {
                    BulkSupport::SingleOnly(_) if marked_task_count > 1 => line
                        .spans
                        .push(Span::styled(" · 1 task only", Style::new().fg(FG_DIM))),
                    BulkSupport::Batch if marked_task_count > 0 => line.spans.push(Span::styled(
                        format!(" · {}", marked_task_label(marked_task_count)),
                        Style::new().fg(FG_MUTED),
                    )),
                    BulkSupport::Focused if marked_task_count > 0 => line
                        .spans
                        .push(Span::styled(" · focused task", Style::new().fg(FG_MUTED))),
                    BulkSupport::Batch
                    | BulkSupport::SingleOnly(_)
                    | BulkSupport::Focused
                    | BulkSupport::BulkControl
                    | BulkSupport::NotTaskScoped => {}
                }
            }
            if unavailable {
                for span in &mut line.spans {
                    span.style = span.style.fg(FG_DIM);
                }
            }
            line
        })
        .collect()
}

pub(super) fn render_prefix_hints(frame: &mut Frame, view: &ViewState) {
    let context = if view.detail_underlay {
        CommandContext::Detail
    } else {
        CommandContext::Normal
    };
    let lines = prefix_hint_lines_with_availability(
        context,
        &view.pending_shortcut,
        view.copy_description_available,
        view.copy_notes_available,
        view.visible_marked_task_count,
    );
    if lines.is_empty() {
        return;
    }
    let visible_rows = prefix_hint_visible_rows(frame.area().height, lines.len());
    let title = if view.pending_shortcut == ["e"] && view.visible_marked_task_count > 0 {
        let noun = if view.visible_marked_task_count == 1 {
            "task"
        } else {
            "tasks"
        };
        format!("Edit · {} marked {noun}", view.visible_marked_task_count)
    } else {
        format!("{} …", view.pending_shortcut.join(" "))
    };
    let content = Dialog::new(&title, 72, visible_rows.saturating_add(2)).render_block(frame);
    render_scrollable_help_lines(frame, content, lines, view.pending_shortcut_scroll);
}

pub(crate) fn prefix_hint_scroll_cap(
    frame_height: u16,
    detail_underlay: bool,
    pending: &[String],
) -> u16 {
    let context = if detail_underlay {
        CommandContext::Detail
    } else {
        CommandContext::Normal
    };
    let line_count = prefix_hint_lines(context, pending).len();
    let visible_rows = prefix_hint_visible_rows(frame_height, line_count) as usize;
    line_count.saturating_sub(visible_rows) as u16
}

fn prefix_hint_visible_rows(frame_height: u16, line_count: usize) -> u16 {
    let available_rows = frame_height.saturating_sub(4).max(1);
    (line_count as u16).min(available_rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::event::{COMMANDS, CommandContext, CommandLifecycle, key_label};
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

    fn render_help_overlay(scroll: u16) -> String {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render_help(frame, scroll)).unwrap();
        buffer_text(terminal.backend())
    }

    fn buffer_row(buffer: &ratatui::buffer::Buffer, row: u16) -> String {
        (0..buffer.area.width)
            .map(|column| buffer[(column, row)].symbol())
            .collect()
    }

    fn dialog_corners(buffer: &ratatui::buffer::Buffer) -> Vec<(u16, u16, String)> {
        let mut corners = Vec::new();
        for row in 0..buffer.area.height {
            for column in 0..buffer.area.width {
                let symbol = buffer[(column, row)].symbol();
                if matches!(symbol, "╭" | "╮" | "╰" | "╯") {
                    corners.push((column, row, symbol.to_string()));
                }
            }
        }
        corners
    }

    fn render_help_buffer(scroll: u16) -> ratatui::buffer::Buffer {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render_help(frame, scroll)).unwrap();
        terminal.backend().buffer().clone()
    }

    fn render_detail_help_buffer(scroll: u16) -> ratatui::buffer::Buffer {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_detail_help(frame, scroll, None))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn render_detail_help_overlay(scroll: u16) -> String {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_detail_help(frame, scroll, None))
            .unwrap();
        buffer_text(terminal.backend())
    }

    fn render_command_overlay(input: &str, cursor: usize) -> String {
        render_command_overlay_with_marks(input, cursor, 0)
    }

    fn render_command_overlay_with_marks(input: &str, cursor: usize, marked: usize) -> String {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render_command(
                    frame,
                    input,
                    cursor,
                    None,
                    None,
                    CommandRenderContext {
                        unavailable: &[],
                        command_context: CommandContext::Normal,
                        marked_task_count: marked,
                    },
                )
            })
            .unwrap();
        buffer_text(terminal.backend())
    }

    fn render_command_buffer(
        input: &str,
        cursor: usize,
        cycle_input: Option<&str>,
        highlighted: Option<&str>,
    ) -> ratatui::buffer::Buffer {
        render_command_buffer_at_width(input, cursor, cycle_input, highlighted, 100)
    }

    fn render_command_buffer_at_width(
        input: &str,
        cursor: usize,
        cycle_input: Option<&str>,
        highlighted: Option<&str>,
        width: u16,
    ) -> ratatui::buffer::Buffer {
        let backend = TestBackend::new(width, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render_command(
                    frame,
                    input,
                    cursor,
                    cycle_input,
                    highlighted,
                    CommandRenderContext {
                        unavailable: &[],
                        command_context: CommandContext::Normal,
                        marked_task_count: 0,
                    },
                )
            })
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn buffer_text_from_rows(buffer: &ratatui::buffer::Buffer) -> String {
        (0..buffer.area.height)
            .map(|row| buffer_row(buffer, row))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn count_marker(rendered: &str, marker: &str) -> usize {
        rendered.matches(marker).count()
    }

    #[test]
    fn view_prefix_hints_include_upcoming() {
        let rendered = prefix_hint_lines(CommandContext::Normal, &["v".to_string()])
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains(":view-upcoming"));
        assert!(rendered.contains(" p "));
        assert!(rendered.contains("show upcoming task view"));
    }

    #[test]
    fn prefix_hint_lines_use_shared_catalog() {
        let lines = prefix_hint_lines(CommandContext::Normal, &["t".to_string()]);
        let rendered = lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains(":status-active"));
        assert!(rendered.contains(":priority-medium"));
        assert!(rendered.contains(" a "));
        assert!(rendered.contains(" m "));
    }

    #[test]
    fn detail_prefix_hint_lines_use_detail_catalog() {
        let lines = prefix_hint_lines(CommandContext::Detail, &["t".to_string()]);
        let rendered = lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains(":edit-title"));
        assert!(rendered.contains(" e t "));
    }

    #[test]
    fn prefix_hint_lines_show_remaining_chord_sequence() {
        let rendered = prefix_hint_lines(CommandContext::Normal, &["t".to_string()])
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains(" e t "));
        assert!(rendered.contains(":edit-title"));
        assert!(rendered.contains(" e d "));
        assert!(rendered.contains(":edit-description"));
        assert!(rendered.contains(" e j "));
        assert!(rendered.contains(":edit-project"));
        assert!(rendered.contains(" e p "));
        assert!(rendered.contains(":edit-priority"));
    }

    #[test]
    fn detail_prefix_hint_lines_align_command_descriptions() {
        let lines = prefix_hint_lines(CommandContext::Detail, &["t".to_string()]);
        let rendered = lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        let title = rendered
            .lines()
            .find(|line| line.contains(":edit-title"))
            .unwrap();
        let description = rendered
            .lines()
            .find(|line| line.contains(":edit-description"))
            .unwrap();
        assert_eq!(
            title.find("edit selected task title"),
            description.find("edit selected task description")
        );
    }

    #[test]
    fn copy_prefix_groups_every_copy_action() {
        for (context, command_prefix) in [
            (CommandContext::Normal, ":copy-"),
            (CommandContext::Detail, ":copy-"),
        ] {
            let rendered = prefix_hint_lines(context, &["y".to_string()])
                .iter()
                .map(|line| line.to_string())
                .collect::<Vec<_>>()
                .join("\n");

            for name in ["ref", "id", "title", "description", "text", "notes"] {
                assert!(
                    rendered.contains(&format!("{command_prefix}{name}")),
                    "{context:?} copy menu missing {name}"
                );
            }
        }
    }

    #[test]
    fn unavailable_description_and_note_copy_hints_are_dimmed() {
        let lines = prefix_hint_lines_with_availability(
            CommandContext::Normal,
            &["y".to_string()],
            false,
            false,
            0,
        );

        for command_name in [":copy-description", ":copy-notes"] {
            let line = lines
                .iter()
                .find(|line| line.to_string().contains(command_name))
                .unwrap();
            assert!(line.spans.iter().all(|span| span.style.fg == Some(FG_DIM)));
        }

        let title = lines
            .iter()
            .find(|line| line.to_string().contains(":copy-title"))
            .unwrap();
        assert!(title.spans.iter().any(|span| span.style.fg != Some(FG_DIM)));
    }

    #[test]
    fn available_description_and_note_copy_hints_keep_normal_styles() {
        let lines = prefix_hint_lines_with_availability(
            CommandContext::Detail,
            &["y".to_string()],
            true,
            true,
            0,
        );

        for command_name in [":copy-description", ":copy-notes"] {
            let line = lines
                .iter()
                .find(|line| line.to_string().contains(command_name))
                .unwrap();
            assert!(line.spans.iter().any(|span| span.style.fg != Some(FG_DIM)));
        }
    }

    #[test]
    fn marked_copy_hints_keep_bulk_actions_and_dim_single_task_actions() {
        let lines = prefix_hint_lines_with_availability(
            CommandContext::Normal,
            &["y".to_string()],
            true,
            true,
            2,
        );

        for command_name in [
            ":copy-description",
            ":copy-text",
            ":copy-notes",
            ":copy-markdown",
        ] {
            let line = lines
                .iter()
                .find(|line| line.to_string().contains(command_name))
                .unwrap();
            assert!(line.to_string().contains("1 task only"));
            assert!(line.spans.iter().all(|span| span.style.fg == Some(FG_DIM)));
        }

        for command_name in [":copy-ref", ":copy-id", ":copy-title"] {
            let line = lines
                .iter()
                .find(|line| line.to_string().contains(command_name))
                .unwrap();
            assert!(line.spans.iter().any(|span| span.style.fg != Some(FG_DIM)));
        }
    }

    #[test]
    fn marked_edit_hints_show_scope_and_single_task_limits() {
        let rendered = prefix_hint_lines_with_availability(
            CommandContext::Normal,
            &["e".to_string()],
            true,
            true,
            3,
        )
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

        assert!(rendered.contains(":edit-title"));
        assert!(rendered.contains("1 task only"));
        assert!(rendered.contains(":edit-project"));
        assert!(rendered.contains("3 marked tasks"));
    }

    #[test]
    fn prefix_hint_visible_rows_uses_available_terminal_height() {
        assert_eq!(prefix_hint_visible_rows(30, 20), 20);
        assert_eq!(prefix_hint_visible_rows(10, 20), 6);
        assert_eq!(prefix_hint_visible_rows(2, 20), 1);
    }

    #[test]
    fn command_line_includes_multi_key_label() {
        let command = COMMANDS
            .iter()
            .find(|command| command.name == "status-active")
            .unwrap();
        let line = command_line(command, CommandContext::Normal);
        let rendered = line.to_string();
        assert!(rendered.contains("t a"));
    }

    #[test]
    fn command_line_omits_planned_badge_for_project_paths() {
        let command = COMMANDS
            .iter()
            .find(|command| command.name == "add-project-path")
            .unwrap();
        let rendered = command_line(command, CommandContext::Normal).to_string();
        assert!(!rendered.contains("planned"));
    }

    #[test]
    fn prefix_hint_lines_omit_planned_badge_for_project_paths() {
        let rendered = prefix_hint_lines(CommandContext::Normal, &["p".to_string()])
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains(":add-project-path"));
        assert!(!rendered.contains("planned"));
    }

    #[test]
    fn prefix_hint_lines_show_config_shortcuts_without_planned_badge() {
        let rendered = prefix_hint_lines(CommandContext::Normal, &["C".to_string()])
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains(":config-status"));
        assert!(rendered.contains(":config-show"));
        assert!(rendered.contains(":config-paths"));
        assert!(rendered.contains(":database-stats"));
        assert!(rendered.contains(":config-init"));
        assert!(!rendered.contains("planned"));
    }

    #[test]
    fn command_line_marks_disabled_actions() {
        let command = CommandSpec::disabled(
            "disabled-test",
            "disabled test command",
            "Test",
            &[],
            "test reason",
        );
        assert!(
            command_line(&command, CommandContext::Normal)
                .to_string()
                .contains("disabled")
        );
    }

    #[test]
    fn disabled_lifecycle_badge_is_labeled() {
        let badge = lifecycle_badge(CommandLifecycle::Disabled {
            reason: "test reason",
        })
        .unwrap();
        assert!(badge.content.contains("disabled"));
    }

    #[test]
    fn overlay_render_includes_command_title_and_input() {
        let rendered = render_command_overlay("ref", 3);
        assert!(rendered.contains("Command"));
        assert!(rendered.contains(":ref"));
    }

    #[test]
    fn bulk_command_overlay_shows_target_scope() {
        let rendered = render_command_overlay_with_marks("edit-project", 12, 3);

        assert!(rendered.contains("Command · 3 marked tasks"));
        assert!(rendered.contains("e j       :edit-project"));
        assert!(!rendered.contains("/t e j"));
        assert!(rendered.contains("3 tasks · edit selected task project"));
        assert!(!rendered.contains(" · 3 tasks"));
    }

    #[test]
    fn bulk_command_overlay_discloses_navigation_and_hidden_commands() {
        let rendered = render_command_overlay_with_marks("", 0, 3);

        assert!(rendered.contains("↑/↓ browse"));
        assert!(rendered.contains("Enter run"));
        assert!(rendered.contains("Tab complete"));
        assert!(rendered.contains("commands"));
        assert!(rendered.contains("│"));
    }

    #[test]
    fn bulk_command_overlay_identifies_focused_commands() {
        let rendered = render_command_overlay_with_marks("create-gist", 11, 3);

        assert!(rendered.contains("focused task"));
    }

    #[test]
    fn command_overlay_keeps_arrow_selection_visible() {
        let buffer = render_command_buffer("", 0, None, Some("search"));
        let rendered = buffer_text_from_rows(&buffer);

        assert!(rendered.contains(":search"));
        let commands = matching_commands_for_bulk(CommandContext::Normal, "", 0);
        let position = commands
            .iter()
            .position(|command| command.name == "search")
            .unwrap()
            + 1;
        assert!(rendered.contains(&format!("{position}/{}", commands.len())));
        assert!(rendered.contains("┃"));
        assert!((0..buffer.area.height).any(|row| {
            buffer_row(&buffer, row).contains(":search")
                && (0..buffer.area.width)
                    .any(|column| buffer[(column, row)].style().bg == Some(SELECTED_BG))
        }));
    }

    #[test]
    fn command_overlay_sizes_name_column_for_visible_commands() {
        let buffer = render_command_buffer("", 0, None, Some("config-init"));
        let rendered = buffer_text_from_rows(&buffer);

        assert!(rendered.contains(":conflict-manual-merge  resolve with manual value"));
        assert!(!rendered.contains("mergeresolve"));
    }

    #[test]
    fn command_overlay_shows_one_compact_shortcut() {
        let buffer = render_command_buffer("", 0, None, Some("return-to-change"));
        let rendered = buffer_text_from_rows(&buffer);

        assert!(rendered.contains("Tab       :focus"));
        assert!(!rendered.contains("Tab/S-Tab"));
    }

    #[test]
    fn command_overlay_uses_available_width_up_to_its_maximum() {
        let buffer = render_command_buffer_at_width("", 0, None, None, 120);
        let corners = dialog_corners(&buffer);
        let left = corners.iter().map(|(column, _, _)| *column).min().unwrap();
        let right = corners.iter().map(|(column, _, _)| *column).max().unwrap();

        assert_eq!(right - left + 1, COMMAND_DIALOG_MAX_WIDTH);
    }

    #[test]
    fn command_overlay_truncates_descriptions_with_an_ellipsis() {
        let buffer = render_command_buffer_at_width("", 0, None, Some("filter-priority"), 72);
        let rendered = buffer_text_from_rows(&buffer);

        assert!(rendered.contains(":task-child-remove"));
        assert!(rendered.contains('…'));
    }

    #[test]
    fn command_overlay_highlights_cycled_command_row() {
        let buffer = render_command_buffer("status-todo", 11, Some(":stat"), Some("status-todo"));
        assert!((0..buffer.area.height).any(|row| {
            buffer_row(&buffer, row).contains(":status-todo")
                && (0..buffer.area.width).any(|column| {
                    let cell = &buffer[(column, row)];
                    cell.symbol() == " " && cell.style().bg == Some(SELECTED_BG)
                })
        }));
    }

    #[test]
    fn command_overlay_does_not_highlight_without_cycle() {
        let buffer = render_command_buffer("stat", 4, None, None);
        assert!((0..buffer.area.height).all(|row| {
            !buffer_row(&buffer, row).contains(":status-todo")
                || (0..buffer.area.width).all(|column| {
                    let cell = &buffer[(column, row)];
                    cell.symbol() == " " || cell.style().bg != Some(SELECTED_BG)
                })
        }));
    }

    #[test]
    fn overlay_render_includes_help_title() {
        let rendered = render_help_overlay(0);
        assert!(rendered.contains("Shortcuts"));
    }

    #[test]
    fn help_overlays_use_matching_bounds() {
        assert_eq!(
            dialog_corners(&render_help_buffer(0)),
            dialog_corners(&render_detail_help_buffer(0))
        );
    }

    #[test]
    fn help_overlays_render_title_edge_lines() {
        for (buffer, title) in [
            (render_help_buffer(0), "Shortcuts"),
            (render_detail_help_buffer(0), "Task detail shortcuts"),
        ] {
            let title_row = (0..buffer.area.height)
                .map(|row| buffer_row(&buffer, row))
                .find(|row| row.contains(title))
                .unwrap();

            assert!(title_row.contains(&format!("╭─ {title} ")), "{title_row}");
            assert!(title_row.contains("─╮"), "{title_row}");
        }
    }

    #[test]
    fn global_help_includes_upcoming_view_route() {
        let rendered = help_columns()
            .iter()
            .flat_map(|sections| help_column_lines(sections))
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("v p"));
        assert!(rendered.contains("show upcoming task view"));
    }

    #[test]
    fn detail_help_overlay_shows_detail_shortcuts() {
        let rendered = render_detail_help_overlay(0);
        assert!(rendered.contains("Task detail shortcuts"));
        assert!(rendered.contains("back one level"));
        assert!(rendered.contains("open focused relationship or attachment"));
        assert!(rendered.contains("close detail"));
        assert!(rendered.contains("focus relationships and attachments"));
        assert!(!rendered.contains("view updated"));
    }

    #[test]
    fn focused_child_help_only_lists_child_actions() {
        let target = DetailTargetId::Task {
            section: DetailSection::EpicChildren,
            task_id: crate::test_support::task_id("focused-child-help"),
        };
        let rendered = detail_help_lines_for(Some(&target))
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("open focused child"));
        assert!(rendered.contains("choose status / mark done / cancel"));
        assert!(rendered.contains("copy title / display ref / durable ID"));
        assert!(rendered.contains("unlink from epic after confirmation"));
        assert!(rendered.contains("delete task after confirmation"));
        assert!(!rendered.contains("edit selected task title"));
        assert!(!rendered.contains("add a child to the selected epic"));
        assert!(!rendered.contains("select previous or next task"));
    }

    #[test]
    fn detail_help_includes_fixed_overlay_rows_and_catalog_commands() {
        let rendered = detail_help_lines()
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("back one level"));
        assert!(rendered.contains("open focused relationship or attachment"));
        assert!(rendered.contains("close detail"));
        assert!(rendered.contains("set focused related task status / done"));
        assert!(rendered.contains("scroll one page"));
        assert!(rendered.contains("select previous or next task"));
        assert!(rendered.contains("copy selected task title"));
        assert!(rendered.contains("copy selected task description"));
        assert!(rendered.contains("copy selected task title and description"));
        assert!(rendered.contains("copy selected task notes"));

        for command in CommandContext::Detail.commands() {
            let keys = command
                .keys(CommandContext::Detail)
                .iter()
                .map(|key| key.label)
                .collect::<Vec<_>>()
                .join("/");
            assert!(
                rendered.contains(command.description),
                ":{} missing",
                command.name
            );
            assert!(rendered.contains(&keys), ":{} keys missing", command.name);
        }
    }

    #[test]
    fn detail_help_lists_recurrence_lifecycle_shortcuts() {
        let rendered = detail_help_lines()
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        for (keys, description) in [
            ("t r p", "pause the selected recurring series"),
            ("t r r", "resume the selected recurring series"),
            ("t r s", "stop future occurrences after the current task"),
        ] {
            assert!(rendered.contains(keys));
            assert!(rendered.contains(description));
        }
    }

    #[test]
    fn detail_help_aligns_description_columns() {
        let key_widths = detail_help_lines()
            .iter()
            .filter(|line| line.spans.len() > 1)
            .map(|line| line.spans[0].content.chars().count())
            .collect::<Vec<_>>();

        assert!(!key_widths.is_empty());
        assert!(key_widths.iter().all(|width| *width == 18));
    }

    #[test]
    fn detail_help_scroll_cap_uses_detail_rows() {
        assert!(detail_help_scroll_cap(10, None) > 0);
    }

    #[test]
    fn help_overlays_draw_scrollbars_when_content_overflows() {
        let help = buffer_text_from_rows(&render_help_buffer(0));
        let detail = buffer_text_from_rows(&render_detail_help_buffer(0));

        for (rendered, title) in [(help, "Shortcuts"), (detail, "Task detail shortcuts")] {
            assert!(rendered.contains("▲"), "{title} missing scrollbar begin");
            assert!(rendered.contains("▼"), "{title} missing scrollbar end");
        }
    }

    #[test]
    fn global_help_overlay_draws_one_scrollbar() {
        let rendered = buffer_text_from_rows(&render_help_buffer(0));

        assert_eq!(count_marker(&rendered, "▲"), 1);
        assert_eq!(count_marker(&rendered, "▼"), 1);
    }

    #[test]
    fn help_overlay_scrollbar_moves_with_scroll_offset() {
        let top = render_help_buffer(0);
        let scrolled = render_help_buffer(1);

        assert_ne!(
            buffer_text_from_rows(&top),
            buffer_text_from_rows(&scrolled)
        );
    }

    #[test]
    fn detail_help_overlay_scrollbar_moves_with_scroll_offset() {
        let top = render_detail_help_buffer(0);
        let scrolled = render_detail_help_buffer(1);

        assert_ne!(
            buffer_text_from_rows(&top),
            buffer_text_from_rows(&scrolled)
        );
    }

    #[test]
    fn help_overlay_omits_command_names() {
        let rendered = render_help_overlay(0);
        assert!(rendered.contains("quit the TUI"));
        assert!(!rendered.contains(":quit"));
    }

    #[test]
    fn help_overlay_shows_scroll_position() {
        let rendered = render_help_overlay(1);
        assert!(rendered.contains("2/"));
    }

    #[test]
    fn command_rows_render_all_lifecycle_badges_from_catalog() {
        for command in COMMANDS {
            let rendered = command_line(command, CommandContext::Normal).to_string();
            assert!(rendered.contains(command.name));
            match command.lifecycle {
                CommandLifecycle::Implemented => {
                    assert!(!rendered.contains("planned"));
                    assert!(!rendered.contains("disabled"));
                }
                CommandLifecycle::Planned { .. } => assert!(rendered.contains("planned")),
                CommandLifecycle::Disabled { .. } => assert!(rendered.contains("disabled")),
            }
        }
    }

    #[test]
    fn recurrence_palette_renders_disabled_reason() {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let unavailable = [crate::tui::overlay::CommandAvailabilityOverride {
            action: crate::tui::event::Action::PauseRecurrence,
            reason: "series is already paused",
        }];
        terminal
            .draw(|frame| {
                render_command(
                    frame,
                    "recurrence-pause",
                    "recurrence-pause".len(),
                    None,
                    None,
                    CommandRenderContext {
                        unavailable: &unavailable,
                        command_context: CommandContext::Normal,
                        marked_task_count: 0,
                    },
                )
            })
            .unwrap();
        let rendered = buffer_text(terminal.backend());

        assert!(rendered.contains("disabled"));
        assert!(rendered.contains("series is already paused"));
    }

    #[test]
    fn help_columns_cover_every_command_section() {
        let sections = help_columns()
            .iter()
            .flat_map(|column| column.iter().copied())
            .collect::<Vec<_>>();
        for command in COMMANDS {
            assert!(
                sections.contains(&command.section),
                ":{} section {} is not rendered by help",
                command.name,
                command.section
            );
        }
    }

    #[test]
    fn help_columns_balance_section_rows() {
        let columns = help_columns();
        let row_counts = columns
            .iter()
            .map(|sections| help_column_lines(sections).len())
            .collect::<Vec<_>>();

        let tail_right = ["Order", "Conflicts", "Config"]
            .into_iter()
            .filter(|section| columns[1].contains(section))
            .count();

        assert!(row_counts[0].abs_diff(row_counts[1]) <= 3);
        assert!(tail_right < 3);
    }

    fn assert_prefix_hints_cover_context(context: CommandContext) {
        let mut prefixes: Vec<Vec<String>> = Vec::new();

        for command in context.commands() {
            for key in command.keys(context) {
                for len in 1..key.codes.len() {
                    let prefix = key.codes[..len]
                        .iter()
                        .map(|code| key_label(*code))
                        .collect::<Vec<_>>();
                    if !prefixes.contains(&prefix) {
                        prefixes.push(prefix);
                    }
                }
            }
        }

        for prefix in prefixes {
            let rendered = prefix_hint_lines(context, &prefix)
                .iter()
                .map(|line| line.to_string())
                .collect::<Vec<_>>()
                .join("\n");

            for command in context.commands() {
                for key in command.keys(context) {
                    let labels = key
                        .codes
                        .iter()
                        .map(|code| key_label(*code))
                        .collect::<Vec<_>>();
                    if labels.len() > prefix.len() && labels.starts_with(&prefix) {
                        let remaining = labels[prefix.len()..].join(" ");
                        assert!(
                            rendered.contains(&format!(":{}", command.name)),
                            "prefix {} missing :{}",
                            prefix.join(" "),
                            command.name
                        );
                        assert!(
                            rendered.contains(&format!(" {:<6} ", remaining)),
                            "prefix {} missing remaining keys {}",
                            prefix.join(" "),
                            remaining
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn prefix_hint_lines_include_every_catalog_continuation() {
        assert_prefix_hints_cover_context(CommandContext::Normal);
        assert_prefix_hints_cover_context(CommandContext::Detail);
    }
}
