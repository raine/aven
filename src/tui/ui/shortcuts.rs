use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;

use super::ViewState;
use super::dialog::Dialog;
use super::input::input_line;
use crate::tui::event::{COMMANDS, CommandLifecycle, CommandSpec, key_label, matching_commands};
use crate::tui::theme::{ACCENT, BG_ALT, BG_PANEL, FG, FG_DIM, FG_MUTED, ORANGE};

const HELP_COLUMNS: &[&[&str]] = &[
    &["General", "Navigation", "Tasks", "Status", "Priority"],
    &[
        "Views",
        "Add/Create",
        "Metadata",
        "Edit",
        "Filters",
        "Order",
        "Conflict",
        "Config",
    ],
];

const DETAIL_HELP_SECTIONS: &[&str] = &["General", "Task detail", "Edit", "Status", "Priority"];

struct HelpTopic {
    keys: &'static str,
    description: &'static str,
    section: &'static str,
}

const DETAIL_HELP_TOPICS: &[HelpTopic] = &[
    HelpTopic {
        keys: "Esc/Enter",
        description: "return to the task list",
        section: "General",
    },
    HelpTopic {
        keys: "?",
        description: "toggle task detail help",
        section: "General",
    },
    HelpTopic {
        keys: "j/k Up/Down",
        description: "scroll one line",
        section: "Task detail",
    },
    HelpTopic {
        keys: "C-d C-u",
        description: "scroll one page",
        section: "Task detail",
    },
    HelpTopic {
        keys: "[/]",
        description: "select previous or next task",
        section: "Task detail",
    },
    HelpTopic {
        keys: "n",
        description: "add a note to this task",
        section: "Task detail",
    },
    HelpTopic {
        keys: "y/Y",
        description: "copy display ref or task id",
        section: "Task detail",
    },
    HelpTopic {
        keys: "D",
        description: "confirm deleting this task",
        section: "Task detail",
    },
    HelpTopic {
        keys: "e t/d/p/l",
        description: "edit title, description, project, labels",
        section: "Edit",
    },
    HelpTopic {
        keys: "s",
        description: "open status picker",
        section: "Status",
    },
    HelpTopic {
        keys: "d/x",
        description: "set status to done or canceled",
        section: "Status",
    },
    HelpTopic {
        keys: "m i/b/t/a",
        description: "set inbox, backlog, todo, active",
        section: "Status",
    },
    HelpTopic {
        keys: "p",
        description: "open priority picker",
        section: "Priority",
    },
    HelpTopic {
        keys: "m 0/l/m/h/u",
        description: "set none, low, medium, high, urgent",
        section: "Priority",
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
    for (column, sections) in [left, right].into_iter().zip(HELP_COLUMNS.iter()) {
        render_help_column(frame, column, sections, scroll);
    }
}

fn render_help_column(frame: &mut Frame, area: Rect, sections: &[&'static str], scroll: u16) {
    let lines = help_column_lines(sections);
    let visible = lines
        .into_iter()
        .skip(scroll as usize)
        .take(area.height as usize)
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(Text::from(visible)).style(Style::new().fg(FG).bg(BG_ALT)),
        area,
    );
}

pub(super) fn render_detail_help(frame: &mut Frame, scroll: u16) {
    let mut dialog = Dialog::new("Task detail shortcuts", 72, 18);
    let visible_rows = dialog.area(frame).height.saturating_sub(2);
    if let Some(title) = detail_help_scroll_title(scroll, visible_rows) {
        dialog = dialog.right_title(Line::from(Span::styled(title, Style::new().fg(FG_MUTED))));
    }
    let content = dialog.render_block(frame);
    let lines = detail_help_lines();
    let visible = lines
        .into_iter()
        .skip(scroll as usize)
        .take(content.height as usize)
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(Text::from(visible)).style(Style::new().fg(FG).bg(BG_ALT)),
        content,
    );
}

fn detail_help_lines() -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for section in DETAIL_HELP_SECTIONS {
        let mut section_lines = DETAIL_HELP_TOPICS
            .iter()
            .filter(|topic| topic.section == *section)
            .peekable();
        if section_lines.peek().is_none() {
            continue;
        }
        if !lines.is_empty() {
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled(
            *section,
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        )));
        lines.extend(section_lines.map(detail_help_line));
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
    let max_rows = HELP_COLUMNS
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
        for command in COMMANDS
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
    HELP_COLUMNS
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
    let keys = command
        .keys
        .iter()
        .map(|key| key.label)
        .collect::<Vec<_>>()
        .join("/");
    command_hint_line(
        Span::styled(format!("{keys:<10}"), Style::new().fg(FG_MUTED)),
        command,
    )
}

pub(super) fn render_command(frame: &mut Frame, input: &str, cursor: usize) {
    let matches = matching_commands(input);
    let height = (matches.len().min(8) as u16).saturating_add(3);

    let mut lines = vec![input_line(":", input, cursor)];
    for command in matches.into_iter().take(8) {
        lines.push(command_line(command));
    }

    Dialog::new("Command", 72, height).render_text(frame, Text::from(lines));
}

fn prefix_hint_lines(pending: &[String]) -> Vec<Line<'static>> {
    COMMANDS
        .iter()
        .flat_map(|command| {
            command.keys.iter().filter_map(move |key| {
                if key.codes.len() <= pending.len() {
                    return None;
                }
                let labels: Vec<String> = key.codes.iter().map(|code| key_label(*code)).collect();
                if labels.len() <= pending.len()
                    || !labels
                        .iter()
                        .zip(pending.iter())
                        .all(|(actual, expected)| actual == expected)
                {
                    return None;
                }
                let next_key = labels[pending.len()].clone();
                Some(command_hint_line(
                    Span::styled(
                        format!(" {:<6} ", next_key),
                        Style::new().fg(FG_MUTED).bg(BG_PANEL),
                    ),
                    command,
                ))
            })
        })
        .collect()
}

pub(super) fn render_prefix_hints(frame: &mut Frame, view: &ViewState) {
    let lines = prefix_hint_lines(&view.pending_shortcut);
    if lines.is_empty() {
        return;
    }
    let height = (lines.len().min(8) as u16).saturating_add(2);
    let title = format!("{} …", view.pending_shortcut.join(" "));
    Dialog::new(&title, 72, height).render_text(
        frame,
        Text::from(lines.into_iter().take(8).collect::<Vec<_>>()),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::event::{COMMANDS, CommandLifecycle, key_label};
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
            .draw(|frame| render_command(frame, input, cursor))
            .unwrap();
        buffer_text(terminal.backend())
    }

    #[test]
    fn prefix_hint_lines_use_shared_catalog() {
        let lines = prefix_hint_lines(&["m".to_string()]);
        let rendered = lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains(":status-active"));
        assert!(rendered.contains(" a "));
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
            .find(|command| command.name == "view-deleted")
            .unwrap();
        let rendered = command_line(command).to_string();
        assert!(rendered.contains("planned"));
    }

    #[test]
    fn prefix_hint_lines_mark_planned_actions() {
        let rendered = prefix_hint_lines(&["g".to_string()])
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains(":view-deleted"));
        assert!(rendered.contains("planned"));
    }

    #[test]
    fn prefix_hint_lines_show_config_shortcuts_without_planned_badge() {
        let rendered = prefix_hint_lines(&["C".to_string()])
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
        let rendered = prefix_hint_lines(&["o".to_string()])
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
    fn overlay_render_includes_help_title() {
        let rendered = render_help_overlay(0);
        assert!(rendered.contains("Shortcuts"));
    }

    #[test]
    fn detail_help_overlay_shows_detail_shortcuts() {
        let rendered = render_detail_help_overlay(0);
        assert!(rendered.contains("Task detail shortcuts"));
        assert!(rendered.contains("return to the task list"));
        assert!(rendered.contains("scroll one page"));
        assert!(rendered.contains("select previous or next task"));
        assert!(rendered.contains("add a note to this task"));
        assert!(!rendered.contains("view updated"));
    }

    #[test]
    fn detail_help_compacts_repeated_prefixes() {
        let rendered = detail_help_lines()
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("e t/d/p/l"));
        assert!(rendered.contains("m i/b/t/a"));
        assert!(rendered.contains("m 0/l/m/h/u"));
        assert!(!rendered.contains("m 0/m l/m m/m h/m u"));
    }

    #[test]
    fn detail_help_scroll_cap_uses_detail_rows() {
        assert!(detail_help_scroll_cap(10) > 0);
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
        let sections = HELP_COLUMNS
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
    fn prefix_hint_lines_include_every_catalog_continuation() {
        let mut prefixes: Vec<Vec<String>> = Vec::new();

        for command in COMMANDS {
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
            let rendered = prefix_hint_lines(&prefix)
                .iter()
                .map(|line| line.to_string())
                .collect::<Vec<_>>()
                .join("\n");

            for command in COMMANDS {
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
}
