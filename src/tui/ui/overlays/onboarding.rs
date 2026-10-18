use ratatui::Frame;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};

use ratatui::widgets::Paragraph;

use super::super::dialog::{Dialog, dialog_hint_line};
use crate::tui::theme::Theme;

const ONBOARDING_WIDTH: u16 = 66;
const ONBOARDING_HEIGHT: u16 = 16;

pub(in crate::tui::ui) fn render_onboarding(frame: &mut Frame, theme: &Theme) {
    render_onboarding_raised(frame, theme, 1.0);
}

pub(in crate::tui::ui) fn render_onboarding_raised(frame: &mut Frame, theme: &Theme, reveal: f32) {
    if reveal <= 0.0 {
        return;
    }
    let dialog = Dialog::new(
        theme,
        "Welcome to aven",
        ONBOARDING_WIDTH,
        ONBOARDING_HEIGHT,
    );
    let mut area = dialog.area(frame);
    area.y = area
        .y
        .saturating_add(((1.0 - reveal.clamp(0.0, 1.0)) * 2.0).round() as u16);
    let inner = dialog.render_block_at(frame, area);
    frame.render_widget(
        Paragraph::new(Text::from(onboarding_lines(theme)))
            .style(Style::new().fg(theme.fg).bg(theme.bg_raised)),
        inner,
    );
}

fn onboarding_lines(theme: &Theme) -> Vec<Line<'static>> {
    vec![
        Line::from(Span::styled(
            "Local-first tasks for power users and coding agents.",
            theme.muted(),
        )),
        Line::default(),
        section_heading(theme, "Everyday keys"),
        shortcut_line(theme, "a", "Add a task and capture what's on your mind"),
        shortcut_line(theme, "Enter", "Open the selected task and see its details"),
        shortcut_line(
            theme,
            "s / d",
            "Change status or mark the selected task done",
        ),
        shortcut_line(theme, "u", "Undo the last change"),
        shortcut_line(theme, ":", "Open commands; use :sync to join existing sync"),
        Line::default(),
        section_heading(theme, "Learn more"),
        resource_line(theme, "TUI guide", "https://aventasks.dev/tui/"),
        resource_line(theme, "Agents guide", "https://aventasks.dev/agents/"),
        Line::default(),
        dialog_hint_line(
            theme,
            &[
                ("a", "create first task"),
                ("?", "shortcuts"),
                ("Enter", "explore"),
            ],
        ),
    ]
}

fn section_heading(theme: &Theme, title: &'static str) -> Line<'static> {
    Line::from(Span::styled(
        title,
        Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
    ))
}

fn resource_line(theme: &Theme, label: &'static str, value: &'static str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("  {label:<13}"),
            Style::new().fg(theme.fg).add_modifier(Modifier::BOLD),
        ),
        Span::styled(value, theme.muted()),
    ])
}

fn shortcut_line(theme: &Theme, key: &'static str, label: &'static str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("  {key:<7}"),
            Style::new().fg(theme.fg).add_modifier(Modifier::BOLD),
        ),
        Span::styled(label, theme.muted()),
    ])
}

#[cfg(test)]
pub(in crate::tui::ui) fn onboarding_lines_for_test() -> Vec<Line<'static>> {
    onboarding_lines(&Theme::dark())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn completed_entrance_matches_stable_dialog() {
        let theme = Theme::dark();
        let mut stable = Terminal::new(TestBackend::new(100, 30)).unwrap();
        stable
            .draw(|frame| render_onboarding(frame, &theme))
            .unwrap();
        let stable_buffer = stable.backend().buffer().clone();

        let mut animated = Terminal::new(TestBackend::new(100, 30)).unwrap();
        animated
            .draw(|frame| render_onboarding_raised(frame, &theme, 1.0))
            .unwrap();

        assert_eq!(animated.backend().buffer(), &stable_buffer);
    }
}
