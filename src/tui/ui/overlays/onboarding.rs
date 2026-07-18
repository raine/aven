use ratatui::Frame;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};

use super::super::dialog::{Dialog, dialog_hint_line};
use crate::tui::theme::{ACCENT, FG, FG_MUTED};

const ONBOARDING_WIDTH: u16 = 66;
const ONBOARDING_HEIGHT: u16 = 16;

pub(in crate::tui::ui) fn render_onboarding(frame: &mut Frame) {
    Dialog::new("Welcome to aven", ONBOARDING_WIDTH, ONBOARDING_HEIGHT)
        .render_text(frame, Text::from(onboarding_lines()));
}

fn onboarding_lines() -> Vec<Line<'static>> {
    vec![
        Line::from(Span::styled(
            "Local-first tasks for power users and coding agents.",
            Style::new().fg(FG_MUTED),
        )),
        Line::default(),
        section_heading("Everyday keys"),
        shortcut_line("a", "Add a task and capture what's on your mind"),
        shortcut_line("Enter", "Open the selected task and see its details"),
        shortcut_line("s / d", "Change status or mark the selected task done"),
        shortcut_line("u", "Undo the last change"),
        shortcut_line(":", "Open the command panel"),
        Line::default(),
        section_heading("Learn more"),
        resource_line("TUI guide", "https://aven.raine.dev/tui/"),
        resource_line("Agents guide", "https://aven.raine.dev/agents/"),
        Line::default(),
        dialog_hint_line(&[
            ("a", "create first task"),
            ("?", "shortcuts"),
            ("Enter", "explore"),
        ]),
    ]
}

fn section_heading(title: &'static str) -> Line<'static> {
    Line::from(Span::styled(
        title,
        Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
    ))
}

fn resource_line(label: &'static str, value: &'static str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("  {label:<13}"),
            Style::new().fg(FG).add_modifier(Modifier::BOLD),
        ),
        Span::styled(value, Style::new().fg(FG_MUTED)),
    ])
}

fn shortcut_line(key: &'static str, label: &'static str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("  {key:<7}"),
            Style::new().fg(FG).add_modifier(Modifier::BOLD),
        ),
        Span::styled(label, Style::new().fg(FG_MUTED)),
    ])
}

#[cfg(test)]
pub(in crate::tui::ui) fn onboarding_lines_for_test() -> Vec<Line<'static>> {
    onboarding_lines()
}
