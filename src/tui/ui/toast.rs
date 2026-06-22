use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use crate::tui::theme::{BG, BG_PANEL, BLUE, FG, GREEN, ORANGE, RED};
use crate::tui::toast::{Toast, ToastSeverity};

struct ToastTone {
    icon: &'static str,
    color: Color,
}

fn toast_tone(severity: ToastSeverity) -> ToastTone {
    match severity {
        ToastSeverity::Info => ToastTone {
            icon: "•",
            color: BLUE,
        },
        ToastSeverity::Warning => ToastTone {
            icon: "!",
            color: ORANGE,
        },
        ToastSeverity::Error => ToastTone {
            icon: "!",
            color: RED,
        },
        ToastSeverity::Success => ToastTone {
            icon: "✓",
            color: GREEN,
        },
    }
}

pub(super) fn render_toast(frame: &mut Frame, toast: &Toast) {
    let tone = toast_tone(toast.severity);
    let fill = BG_PANEL;
    let content = Line::from(vec![
        Span::styled("", Style::new().fg(fill).bg(BG)),
        Span::styled("▌", Style::new().fg(tone.color).bg(fill)),
        Span::styled(" ", Style::new().bg(fill)),
        Span::styled(tone.icon, Style::new().fg(tone.color).bg(fill)),
        Span::styled(" ", Style::new().bg(fill)),
        Span::styled(
            toast.message.as_str(),
            Style::new().fg(FG).bg(fill).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ", Style::new().bg(fill)),
        Span::styled("", Style::new().fg(fill).bg(BG)),
    ]);
    let width = (toast.message.chars().count() as u16)
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
            .draw(|frame| {
                render_toast(
                    frame,
                    &Toast::new("filters cleared", ToastSeverity::Success),
                )
            })
            .unwrap();
        let rendered = buffer_text(terminal.backend());
        assert!(rendered.contains("✓ filters cleared"));
    }

    #[test]
    fn toast_tone_uses_explicit_severity() {
        assert_eq!(toast_tone(ToastSeverity::Info).icon, "•");
        assert_eq!(toast_tone(ToastSeverity::Warning).icon, "!");
        assert_eq!(toast_tone(ToastSeverity::Error).icon, "!");
        assert_eq!(toast_tone(ToastSeverity::Success).icon, "✓");
    }

    #[test]
    fn toast_tone_uses_severity_colors() {
        assert_eq!(toast_tone(ToastSeverity::Info).color, BLUE);
        assert_eq!(toast_tone(ToastSeverity::Warning).color, ORANGE);
        assert_eq!(toast_tone(ToastSeverity::Error).color, RED);
        assert_eq!(toast_tone(ToastSeverity::Success).color, GREEN);
    }

    #[test]
    fn neutral_message_text_can_render_as_info() {
        let backend = TestBackend::new(60, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_toast(frame, &Toast::new("nothing to undo", ToastSeverity::Info)))
            .unwrap();
        let rendered = buffer_text(terminal.backend());
        assert!(rendered.contains("• nothing to undo"));
    }
}
