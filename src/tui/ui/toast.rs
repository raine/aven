use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use crate::tui::theme::{BG, BG_PANEL, FG, GREEN, ORANGE, RED};

struct ToastTone {
    icon: &'static str,
    color: Color,
}

fn toast_tone(message: &str) -> ToastTone {
    let lower = message.to_ascii_lowercase();
    if lower.contains("error")
        || lower.contains("failed")
        || lower.contains("invalid")
        || lower.contains("unknown")
        || lower.contains("required")
        || lower.starts_with("no ")
        || lower.starts_with("nothing")
    {
        ToastTone {
            icon: "!",
            color: RED,
        }
    } else if lower.contains("ambiguous") || lower.contains("conflict") {
        ToastTone {
            icon: "•",
            color: ORANGE,
        }
    } else {
        ToastTone {
            icon: "✓",
            color: GREEN,
        }
    }
}

pub(super) fn render_toast(frame: &mut Frame, message: &str) {
    let tone = toast_tone(message);
    let fill = BG_PANEL;
    let content = Line::from(vec![
        Span::styled("", Style::new().fg(fill).bg(BG)),
        Span::styled("▌", Style::new().fg(tone.color).bg(fill)),
        Span::styled(" ", Style::new().bg(fill)),
        Span::styled(tone.icon, Style::new().fg(tone.color).bg(fill)),
        Span::styled(" ", Style::new().bg(fill)),
        Span::styled(
            message.to_string(),
            Style::new().fg(FG).bg(fill).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ", Style::new().bg(fill)),
        Span::styled("", Style::new().fg(fill).bg(BG)),
    ]);
    let width = (message.chars().count() as u16)
        .saturating_add(7)
        .clamp(20, frame.area().width.saturating_sub(5));
    let height = 1.min(frame.area().height);
    let x = frame.area().right().saturating_sub(width.saturating_add(3));
    let y = frame
        .area()
        .bottom()
        .saturating_sub(height.saturating_add(3));
    let area = Rect {
        x,
        y,
        width,
        height,
    };
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(content).style(Style::new().fg(FG).bg(BG)),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn toast_uses_icon_and_message() {
        let backend = TestBackend::new(60, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_toast(frame, "filters cleared"))
            .unwrap();
        let rendered = buffer_text(terminal.backend());
        assert!(rendered.contains("✓ filters cleared"));
    }

    #[test]
    fn toast_tone_detects_error_messages() {
        assert_eq!(toast_tone("Error: failed").icon, "!");
        assert_eq!(toast_tone("nothing found").icon, "!");
    }

    #[test]
    fn toast_tone_detects_ambiguous_messages() {
        assert_eq!(toast_tone("ambiguous match").icon, "•");
    }
}
