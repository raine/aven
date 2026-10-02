use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use crate::tui::theme::{self, BG};
use crate::tui::toast::{Toast, ToastSeverity};

struct ToastTone {
    icon: &'static str,
    color: Color,
}

fn toast_tone(severity: ToastSeverity) -> ToastTone {
    match severity {
        ToastSeverity::Info => ToastTone {
            icon: "•",
            color: theme::blue(),
        },
        ToastSeverity::Warning => ToastTone {
            icon: "!",
            color: theme::orange(),
        },
        ToastSeverity::Error => ToastTone {
            icon: "!",
            color: theme::red(),
        },
        ToastSeverity::Success => ToastTone {
            icon: "✓",
            color: theme::green(),
        },
    }
}

fn toast_width(content: &Line<'_>, frame_width: u16) -> u16 {
    let content_width = content.width().min(u16::MAX as usize) as u16;
    content_width.clamp(20, frame_width.saturating_sub(5))
}

fn toast_message_spans(message: &str, fill: Color) -> Vec<Span<'_>> {
    let message_style = Style::new()
        .fg(theme::fg())
        .bg(fill)
        .add_modifier(Modifier::BOLD);
    let separator_style = Style::new()
        .fg(theme::fg_dim())
        .bg(fill)
        .add_modifier(Modifier::BOLD);
    let mut spans = Vec::new();
    for (index, part) in message.split(" · ").enumerate() {
        if index > 0 {
            spans.push(Span::styled(" │ ", separator_style));
        }
        spans.push(Span::styled(part, message_style));
    }
    spans
}

pub(super) fn render_toast(frame: &mut Frame, toast: &Toast) {
    let tone = toast_tone(toast.severity);
    let fill = theme::bg_panel();
    let mut spans = vec![
        Span::styled("", Style::new().fg(fill).bg(BG)),
        Span::styled("▌", Style::new().fg(tone.color).bg(fill)),
        Span::styled(" ", Style::new().bg(fill)),
    ];
    if toast.icon {
        spans.extend([
            Span::styled(tone.icon, Style::new().fg(tone.color).bg(fill)),
            Span::styled(" ", Style::new().bg(fill)),
        ]);
    }
    spans.extend(toast_message_spans(toast.message.as_str(), fill));
    spans.extend([
        Span::styled(" ", Style::new().bg(fill)),
        Span::styled("", Style::new().fg(fill).bg(BG)),
    ]);
    let content = Line::from(spans);
    let width = toast_width(&content, frame.area().width);
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
        Paragraph::new(content).style(Style::new().fg(theme::fg()).bg(BG)),
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
        assert_eq!(toast_tone(ToastSeverity::Info).color, theme::blue());
        assert_eq!(toast_tone(ToastSeverity::Warning).color, theme::orange());
        assert_eq!(toast_tone(ToastSeverity::Error).color, theme::red());
        assert_eq!(toast_tone(ToastSeverity::Success).color, theme::green());
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

    #[test]
    fn iconless_info_toast_omits_dot() {
        let backend = TestBackend::new(60, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render_toast(
                    frame,
                    &Toast::new("⠋ adding task with LLM", ToastSeverity::Info).without_icon(),
                )
            })
            .unwrap();
        let rendered = buffer_text(terminal.backend());
        assert!(rendered.contains("⠋ adding task with LLM"));
        assert!(!rendered.contains("• ⠋ adding task with LLM"));
    }

    #[test]
    fn toast_separators_use_muted_text_color() {
        let spans = toast_message_spans("saved · u undo · g . return", theme::bg_panel());

        assert_eq!(
            spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>(),
            "saved │ u undo │ g . return"
        );
        assert_eq!(spans[0].style.fg, Some(theme::fg()));
        assert_eq!(spans[1].style.fg, Some(theme::fg_dim()));
        assert_eq!(spans[2].style.fg, Some(theme::fg()));
        assert_eq!(spans[3].style.fg, Some(theme::fg_dim()));
    }

    #[test]
    fn toast_width_counts_wide_content_cells() {
        assert_eq!(toast_width(&Line::from("한글"), 80), 20);
        assert_eq!(toast_width(&Line::from("한".repeat(12)), 80), 24);
    }
}
