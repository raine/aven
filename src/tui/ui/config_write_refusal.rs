use super::{
    dialog::{Dialog, dialog_hint_line},
    inline_code,
};
use crate::tui::theme::Theme;
use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Text},
    widgets::Paragraph,
};

fn lines(theme: &Theme, message: &str, width: u16) -> Vec<Line<'static>> {
    let mut lines = vec![Line::default()];
    for line in message.lines() {
        lines.extend(inline_code::wrap_with_code(
            theme,
            line,
            if line.starts_with("  ") || line.starts_with("- ") {
                theme.code()
            } else {
                Style::default()
            },
            width.saturating_sub(4).max(1) as usize,
        ));
    }
    lines.push(Line::default());
    lines.push(dialog_hint_line(theme, &[("Enter/Esc", "close")]));
    lines
}

pub(super) fn render(frame: &mut Frame, theme: &Theme, title: &str, message: &str, reason: &str) {
    let width = frame.area().width.saturating_sub(4).min(88);
    let mut rows = lines(theme, message, width);
    if rows.len() + 2 > frame.area().height.saturating_sub(2) as usize {
        let reason = match reason {
            "symlinked" => "This config is a symbolic link.",
            "read-only" => "This config is read-only.",
            _ => "This config is not a regular file.",
        };
        rows = lines(
            theme,
            &format!("{reason}\n\nEnlarge the terminal to view the manual recovery instructions."),
            width,
        );
    }
    let inner = Dialog::new(theme, title, width, (rows.len() + 2) as u16).render_block(frame);
    let footer = rows.pop().expect("dialog lines include a close hint");
    rows.pop();
    if rows.len() > inner.height.saturating_sub(2) as usize {
        rows = inline_code::wrap_with_code(
            theme,
            "Enlarge terminal to view instructions.",
            Style::default(),
            inner.width.max(1) as usize,
        );
    }
    frame.render_widget(
        Paragraph::new(Text::from(rows)).style(Style::default().fg(theme.fg)),
        Rect::new(
            inner.x,
            inner.y,
            inner.width,
            inner.height.saturating_sub(1),
        ),
    );
    if inner.height > 0 {
        frame.render_widget(
            Paragraph::new(footer),
            Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn rendered(width: u16, height: u16, message: &str, reason: &str) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                render(
                    frame,
                    &Theme::dark(),
                    "Could not add project path",
                    message,
                    reason,
                )
            })
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn insufficient_height_shows_notice_and_resize_restores_instructions() {
        let message = format!(
            "refusing to modify symlinked config: /config.yaml\n\n{}\nmanual mapping instructions",
            "long recovery guidance\n".repeat(15)
        );
        let small = rendered(46, 12, &message, "symlinked");
        assert!(small.contains("This config is a symbolic link."));
        assert!(small.contains("Enlarge the terminal"));
        assert!(small.contains("close"));
        assert!(!small.contains("manual mapping instructions"));
        let large = rendered(80, 32, &message, "symlinked");
        assert!(large.contains("manual mapping instructions"));
        assert!(!large.contains("Enlarge the terminal"));
        let tiny = rendered(40, 8, &message, "symlinked");
        assert!(tiny.contains("Enlarge terminal"));
        assert!(tiny.contains("close"));
        let readonly = rendered(46, 12, &message, "read-only");
        assert!(readonly.contains("This config is read-only."));
    }

    #[test]
    fn long_path_recovery_fits_standard_and_compact_terminals() {
        let theme = Theme::dark();
        let message = format!(
            "refusing to modify symlinked config: /home/user/.config/aven/config.yaml\n\nIn your configuration source, merge this entry into the existing `project.overrides` sequence (replace any mapping for this path):\n- workspace_id: 0000000000000000\n  project: app\n  paths:\n    - /home/user/projects/{}/app",
            "nested-directory/".repeat(8)
        );
        for (width, height) in [(80, 24), (60, 20)] {
            let rows = lines(&theme, &message, width - 4);
            assert!(rows.len() + 2 <= height);
        }
    }

    #[test]
    fn recovery_dialog_has_spacing_code_style_and_close_hint() {
        let theme = Theme::dark();
        let message = "refusing to modify symlinked config: /home/user/.config/aven/config.yaml\n\nIn your configuration source, merge this entry into the existing `project.overrides` sequence (replace any mapping for this path):\n- workspace_id: 0000000000000000\n  project: app\n  paths:\n    - /home/user/projects/app";
        let rows = lines(&theme, message, 76);
        assert!(rows.first().unwrap().spans.is_empty());
        assert!(
            rows.iter()
                .flat_map(|line| &line.spans)
                .any(|span| span.content == "project.overrides" && span.style == theme.code())
        );
        assert!(rows.len() + 2 <= 24);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| {
                render(
                    frame,
                    &theme,
                    "Could not add project path",
                    message,
                    "symlinked",
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("Could not add project path"));
        assert!(text.contains("close"));
        assert!(!text.contains("scroll"));
    }
}
