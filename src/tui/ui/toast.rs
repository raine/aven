use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use super::inline_code::code_spans;
use crate::tui::theme::Theme;
use crate::tui::toast::{Toast, ToastSeverity};

struct ToastTone {
    icon: &'static str,
    color: Color,
}

fn toast_tone(theme: &Theme, severity: ToastSeverity) -> ToastTone {
    match severity {
        ToastSeverity::Info => ToastTone {
            icon: "•",
            color: theme.info,
        },
        ToastSeverity::Warning => ToastTone {
            icon: "!",
            color: theme.warning,
        },
        ToastSeverity::Error => ToastTone {
            icon: "!",
            color: theme.danger,
        },
        ToastSeverity::Success => ToastTone {
            icon: "✓",
            color: theme.success,
        },
    }
}

fn toast_width(content: &Line<'_>, frame_width: u16) -> u16 {
    let content_width = content.width().min(u16::MAX as usize) as u16;
    content_width.clamp(20, frame_width.saturating_sub(5))
}

fn toast_message_spans(theme: &Theme, message: &str, fill: Color) -> Vec<Span<'static>> {
    let message_style = Style::new()
        .fg(theme.fg)
        .bg(fill)
        .add_modifier(Modifier::BOLD);
    let separator_style = theme.dim().bg(fill).add_modifier(Modifier::BOLD);
    let mut spans = Vec::new();
    for (index, part) in message.split(" · ").enumerate() {
        if index > 0 {
            spans.push(Span::styled(" │ ", separator_style));
        }
        spans.extend(code_spans(theme, part, message_style));
    }
    spans
}

pub(super) fn render_toast(frame: &mut Frame, theme: &Theme, toast: &Toast) {
    let tone = toast_tone(theme, toast.severity);
    let fill = theme.bg_panel;
    let mut spans = vec![
        Span::styled(theme.pill_edge(""), Style::new().fg(fill).bg(theme.bg)),
        Span::styled("▌", Style::new().fg(tone.color).bg(fill)),
        Span::styled(" ", Style::new().bg(fill)),
    ];
    if toast.icon {
        spans.extend([
            Span::styled(tone.icon, Style::new().fg(tone.color).bg(fill)),
            Span::styled(" ", Style::new().bg(fill)),
        ]);
    }
    spans.extend(toast_message_spans(theme, toast.message.as_str(), fill));
    spans.extend([
        Span::styled(" ", Style::new().bg(fill)),
        Span::styled(theme.pill_edge(""), Style::new().fg(fill).bg(theme.bg)),
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
        Paragraph::new(content).style(Style::new().fg(theme.fg).bg(theme.bg)),
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
    fn toast_renders_explicit_severity_with_icon_and_color() {
        let theme = Theme::dark();
        for (severity, message, icon, color) in [
            (ToastSeverity::Info, "nothing to undo", "•", theme.info),
            (ToastSeverity::Warning, "saved", "!", theme.warning),
            (ToastSeverity::Error, "saved", "!", theme.danger),
            (
                ToastSeverity::Success,
                "filters cleared",
                "✓",
                theme.success,
            ),
        ] {
            let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
            terminal
                .draw(|frame| render_toast(frame, &theme, &Toast::new(message, severity)))
                .unwrap();
            assert!(buffer_text(terminal.backend()).contains(&format!("{icon} {message}")));
            let buffer = terminal.backend().buffer();
            let icon_cell = buffer
                .content
                .iter()
                .find(|cell| cell.symbol() == icon)
                .unwrap();
            assert_eq!(icon_cell.fg, color);
            let stripe = buffer
                .content
                .iter()
                .find(|cell| cell.symbol() == "▌")
                .unwrap();
            assert_eq!(stripe.fg, color);
        }
    }

    #[test]
    fn iconless_info_toast_omits_dot() {
        let backend = TestBackend::new(60, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render_toast(
                    frame,
                    &Theme::dark(),
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
        let theme = Theme::dark();
        let spans = toast_message_spans(&theme, "saved · u undo · g . return", theme.bg_panel);

        assert_eq!(
            spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>(),
            "saved │ u undo │ g . return"
        );
        assert_eq!(spans[0].style.fg, Some(theme.fg));
        assert_eq!(spans[1].style.fg, Some(theme.fg_dim));
        assert_eq!(spans[2].style.fg, Some(theme.fg));
        assert_eq!(spans[3].style.fg, Some(theme.fg_dim));
    }

    #[test]
    fn toast_width_counts_wide_content_cells() {
        assert_eq!(toast_width(&Line::from("한글"), 80), 20);
        assert_eq!(toast_width(&Line::from("한".repeat(12)), 80), 24);
    }
}
