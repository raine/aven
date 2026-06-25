use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};

use super::ViewState;
use super::dialog::Dialog;
use super::input::input_line;
use crate::tui::event::{
    CommandContext, CommandLifecycle, CommandSpec, matching_commands, prefix_hint_commands,
};
use crate::tui::theme::{ACCENT, BG_ALT, BG_PANEL, FG, FG_DIM, FG_MUTED, ORANGE, SELECTED_BG};

struct HelpTopic {
    keys: &'static str,
    description: &'static str,
    section: &'static str,
}

const DETAIL_HELP_TOPICS: &[HelpTopic] = &[
    HelpTopic {
        keys: "Esc/Enter/q",
        description: "return to the task list",
        section: "General",
    },
    HelpTopic {
        keys: "?",
        description: "toggle task detail help",
        section: "General",
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

pub(super) fn render_help(frame: &mut Frame, scroll: u16) {
    let width = frame.area().width.saturating_sub(6).min(112);
    let height = frame.area().height.saturating_sub(4).min(28);
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
    render_help_scrollbar(frame, content, content_height, scroll);
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
        .iter()
        .filter(|command| command.section == section)
        .count()
        + 1
}

fn render_help_column(frame: &mut Frame, area: Rect, sections: &[&'static str], scroll: u16) {
    let lines = help_column_lines(sections);
    let start = help_scroll_start(scroll, lines.len(), area.height as usize);
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
    let start = help_scroll_start(scroll, content_height, visible_rows);
    let visible = lines
        .into_iter()
        .skip(start)
        .take(visible_rows)
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(Text::from(visible)).style(Style::new().fg(FG).bg(BG_ALT)),
        area,
    );
    render_help_scrollbar(frame, area, content_height, scroll);
}

fn render_help_scrollbar(frame: &mut Frame, area: Rect, content_height: usize, scroll: u16) {
    let visible_rows = area.height as usize;
    if content_height > visible_rows {
        let start = help_scroll_start(scroll, content_height, visible_rows);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .style(Style::new().fg(FG_DIM).bg(BG_ALT))
                .thumb_style(Style::new().fg(FG_MUTED)),
            area,
            &mut ScrollbarState::new(content_height)
                .position(help_scrollbar_position(
                    start,
                    content_height,
                    visible_rows.max(1),
                ))
                .viewport_content_length(visible_rows),
        );
    }
}

pub(super) fn render_detail_help(frame: &mut Frame, scroll: u16) {
    let mut dialog = Dialog::new("Task detail shortcuts", 72, 18);
    let visible_rows = dialog.area(frame).height.saturating_sub(2);
    if let Some(title) = detail_help_scroll_title(scroll, visible_rows) {
        dialog = dialog.right_title(Line::from(Span::styled(title, Style::new().fg(FG_MUTED))));
    }
    let content = dialog.render_block(frame);
    render_scrollable_help_lines(frame, content, detail_help_lines(), scroll);
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
            .iter()
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
        lines.extend(commands.into_iter().map(help_command_line));
    }
    lines
}

fn detail_help_line(topic: &HelpTopic) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{:<18}", topic.keys), Style::new().fg(FG_MUTED)),
        Span::styled(topic.description, Style::new().fg(FG_DIM)),
    ])
}

fn detail_help_scroll_title(scroll: u16, visible_rows: u16) -> Option<String> {
    let max_rows = detail_help_lines().len();
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
            .iter()
            .filter(|command| command.section == *section)
        {
            lines.push(help_command_line(command));
        }
    }
    lines
}

pub(crate) fn help_scroll_cap(frame_height: u16) -> u16 {
    let visible_rows = frame_height.saturating_sub(4).min(28).saturating_sub(2) as usize;
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

pub(crate) fn detail_help_scroll_cap(frame_height: u16) -> u16 {
    let visible_rows = frame_height.min(18).saturating_sub(2) as usize;
    detail_help_lines().len().saturating_sub(visible_rows) as u16
}

fn help_scroll_start(scroll: u16, content_height: usize, visible: usize) -> usize {
    let max_start = content_height.saturating_sub(visible);
    (scroll as usize).min(max_start)
}

fn help_scrollbar_position(start: usize, content_height: usize, visible: usize) -> usize {
    let max_start = content_height.saturating_sub(visible);
    start
        .saturating_mul(content_height.saturating_sub(1))
        .checked_div(max_start)
        .unwrap_or(0)
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

fn command_hint_line(leading: Span<'static>, command: &CommandSpec) -> Line<'static> {
    let mut spans = vec![
        leading,
        Span::styled(
            format!(":{:<18}", command.name),
            command_name_style(command),
        ),
    ];
    if let Some(badge) = lifecycle_badge(command.lifecycle) {
        spans.push(badge);
    }
    spans.push(Span::styled(command.description, Style::new().fg(FG_DIM)));
    Line::from(spans)
}

fn help_command_line(command: &CommandSpec) -> Line<'static> {
    let keys = command
        .keys
        .iter()
        .map(|key| key.label)
        .collect::<Vec<_>>()
        .join("/");
    let mut spans = vec![Span::styled(
        format!("{keys:<14}"),
        Style::new().fg(FG_MUTED),
    )];
    if let Some(badge) = lifecycle_badge(command.lifecycle) {
        spans.push(badge);
    }
    spans.push(Span::styled(command.description, Style::new().fg(FG_DIM)));
    Line::from(spans)
}

fn command_line(command: &CommandSpec) -> Line<'static> {
    command_line_with_highlight(command, false)
}

fn command_line_with_highlight(command: &CommandSpec, highlighted: bool) -> Line<'static> {
    let keys = command
        .keys
        .iter()
        .map(|key| key.label)
        .collect::<Vec<_>>()
        .join("/");
    let mut line = command_hint_line(
        Span::styled(format!("{keys:<10}"), Style::new().fg(FG_MUTED)),
        command,
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

pub(super) fn render_command(
    frame: &mut Frame,
    input: &str,
    cursor: usize,
    cycle_input: Option<&str>,
    highlighted: Option<&str>,
) {
    let matches = matching_commands(cycle_input.unwrap_or(input));
    let height = (matches.len().min(8) as u16).saturating_add(3);

    let mut lines = vec![input_line(":", input, cursor)];
    for command in matches.into_iter().take(8) {
        let line = if highlighted == Some(command.name) {
            command_line_with_highlight(command, true)
        } else {
            command_line(command)
        };
        lines.push(line);
    }

    Dialog::new("Command", 72, height).render_text(frame, Text::from(lines));
}

fn prefix_hint_lines(context: CommandContext, pending: &[String]) -> Vec<Line<'static>> {
    prefix_hint_commands(context, pending)
        .into_iter()
        .map(|(command, _, next_key)| {
            command_hint_line(
                Span::styled(
                    format!(" {:<6} ", next_key),
                    Style::new().fg(FG_MUTED).bg(BG_PANEL),
                ),
                command,
            )
        })
        .collect()
}

pub(super) fn render_prefix_hints(frame: &mut Frame, view: &ViewState) {
    let context = if view.detail_underlay {
        CommandContext::Detail
    } else {
        CommandContext::Normal
    };
    let lines = prefix_hint_lines(context, &view.pending_shortcut);
    if lines.is_empty() {
        return;
    }
    let visible_rows = prefix_hint_visible_rows(frame.area().height, lines.len());
    let title = format!("{} …", view.pending_shortcut.join(" "));
    let content = Dialog::new(&title, 72, visible_rows.saturating_add(2)).render_block(frame);
    render_scrollable_help_lines(frame, content, lines, 0);
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
            .draw(|frame| render_detail_help(frame, scroll))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn render_detail_help_overlay(scroll: u16) -> String {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_detail_help(frame, scroll))
            .unwrap();
        buffer_text(terminal.backend())
    }

    fn render_command_overlay(input: &str, cursor: usize) -> String {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_command(frame, input, cursor, None, None))
            .unwrap();
        buffer_text(terminal.backend())
    }

    fn render_command_buffer(
        input: &str,
        cursor: usize,
        cycle_input: Option<&str>,
        highlighted: Option<&str>,
    ) -> ratatui::buffer::Buffer {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_command(frame, input, cursor, cycle_input, highlighted))
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
    fn prefix_hint_lines_use_shared_catalog() {
        let lines = prefix_hint_lines(CommandContext::Normal, &["m".to_string()]);
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
        let lines = prefix_hint_lines(CommandContext::Detail, &["e".to_string()]);
        let rendered = lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains(":detail-edit-title"));
        assert!(rendered.contains(" t "));
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
        let line = command_line(command);
        let rendered = line.to_string();
        assert!(rendered.contains("m a"));
    }

    #[test]
    fn command_line_marks_planned_actions() {
        let command = COMMANDS
            .iter()
            .find(|command| command.name == "add-project-path")
            .unwrap();
        let rendered = command_line(command).to_string();
        assert!(rendered.contains("planned"));
    }

    #[test]
    fn prefix_hint_lines_mark_planned_actions() {
        let rendered = prefix_hint_lines(CommandContext::Normal, &["A".to_string()])
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains(":add-project-path"));
        assert!(rendered.contains("planned"));
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
        assert!(rendered.contains(":config-init"));
        assert!(!rendered.contains("planned"));
    }

    #[test]
    fn command_line_marks_disabled_actions() {
        let command = COMMANDS
            .iter()
            .find(|command| command.name == "order-due")
            .unwrap();
        assert!(command_line(command).to_string().contains("disabled"));
    }

    #[test]
    fn prefix_hint_lines_mark_disabled_actions() {
        let rendered = prefix_hint_lines(CommandContext::Normal, &["o".to_string()])
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains(":order-due"));
        assert!(rendered.contains("disabled"));
    }

    #[test]
    fn overlay_render_includes_command_title_and_input() {
        let rendered = render_command_overlay("ref", 3);
        assert!(rendered.contains("Command"));
        assert!(rendered.contains(":ref"));
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
    fn detail_help_overlay_shows_detail_shortcuts() {
        let rendered = render_detail_help_overlay(0);
        assert!(rendered.contains("Task detail shortcuts"));
        assert!(rendered.contains("return to the task list"));
        assert!(rendered.contains("scroll one page"));
        assert!(rendered.contains("select previous or next task"));
        assert!(rendered.contains("add a note to selected task"));
        assert!(!rendered.contains("view updated"));
    }

    #[test]
    fn detail_help_includes_fixed_overlay_rows_and_catalog_commands() {
        let rendered = detail_help_lines()
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("return to the task list"));
        assert!(rendered.contains("scroll one page"));
        assert!(rendered.contains("select previous or next task"));

        for command in CommandContext::Detail.commands() {
            let keys = command
                .keys
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
    fn detail_help_scroll_cap_uses_detail_rows() {
        assert!(detail_help_scroll_cap(10) > 0);
    }

    #[test]
    fn help_scrollbar_position_reaches_end_at_last_visible_row() {
        assert_eq!(help_scrollbar_position(0, 20, 5), 0);
        assert_eq!(help_scrollbar_position(15, 20, 5), 19);
        assert_eq!(help_scrollbar_position(0, 3, 5), 0);
    }

    #[test]
    fn help_scroll_start_stops_at_last_available_row() {
        assert_eq!(help_scroll_start(50, 20, 5), 15);
        assert_eq!(help_scroll_start(50, 3, 5), 0);
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
            let rendered = command_line(command).to_string();
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

        let tail_right = ["Order", "Conflict", "Config"]
            .into_iter()
            .filter(|section| columns[1].contains(section))
            .count();

        assert!(row_counts[0].abs_diff(row_counts[1]) <= 3);
        assert!(tail_right < 3);
    }

    fn assert_prefix_hints_cover_context(context: CommandContext) {
        let mut prefixes: Vec<Vec<String>> = Vec::new();

        for command in context.commands() {
            for key in command.keys {
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
                for key in command.keys {
                    let labels = key
                        .codes
                        .iter()
                        .map(|code| key_label(*code))
                        .collect::<Vec<_>>();
                    if labels.len() > prefix.len() && labels.starts_with(&prefix) {
                        assert!(
                            rendered.contains(&format!(":{}", command.name)),
                            "prefix {} missing :{}",
                            prefix.join(" "),
                            command.name
                        );
                        assert!(
                            rendered.contains(&format!(" {:<6} ", labels[prefix.len()])),
                            "prefix {} missing next key {}",
                            prefix.join(" "),
                            labels[prefix.len()]
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
