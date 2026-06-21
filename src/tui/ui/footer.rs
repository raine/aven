use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::tui::theme::{BG, BG_PANEL, BORDER, FG, FG_DIM, FG_MUTED};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum FooterMode {
    List,
    Detail,
}

pub(super) fn footer_bar(mode: FooterMode) -> Paragraph<'static> {
    let mut spans = Vec::new();
    let hints: &[(&str, &str)] = match mode {
        FooterMode::List => &[
            ("j/k", "navigate"),
            ("Enter", "detail"),
            ("a/s/p/l/n/d/x/y", "task"),
            ("g/e/m/f/o/c/C", "prefixes"),
            ("/", "search"),
            (":", "command"),
            ("?", "help"),
            ("q", "quit"),
        ],
        FooterMode::Detail => &[
            ("j/k Pg", "scroll"),
            ("[/]", "prev/next"),
            ("e", "edit field"),
            ("n", "add note"),
            ("d", "done"),
            ("s/p/l", "edit"),
            ("y/Y", "copy"),
            ("?", "help"),
            ("Esc", "back"),
        ],
    };
    for (keys, label) in hints {
        spans.extend(key(keys));
        spans.push(cmd(label));
    }
    let hints = Line::from(spans);
    Paragraph::new(hints)
        .block(
            Block::new()
                .borders(Borders::TOP)
                .border_style(Style::new().fg(BORDER)),
        )
        .style(Style::new().fg(FG).bg(BG))
}

fn key(label: &str) -> Vec<Span<'static>> {
    let key_style = Style::new()
        .fg(FG_MUTED)
        .bg(BG_PANEL)
        .add_modifier(Modifier::BOLD);
    let separator_style = Style::new().fg(FG_DIM).bg(BG_PANEL);
    let edge_style = Style::new().fg(BG_PANEL).bg(BG);
    let mut spans = vec![Span::styled("".to_string(), edge_style)];
    for (index, part) in label.split('/').enumerate() {
        if index > 0 {
            spans.push(Span::styled("/".to_string(), separator_style));
        }
        spans.push(Span::styled(part.to_string(), key_style));
    }
    spans.push(Span::styled("".to_string(), edge_style));
    spans
}

fn cmd(label: &str) -> Span<'static> {
    Span::styled(format!(" {label}  "), Style::new().fg(FG_DIM))
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
    fn detail_footer_lists_scroll_and_task_navigation_keys() {
        let backend = TestBackend::new(100, 3);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| frame.render_widget(footer_bar(FooterMode::Detail), frame.area()))
            .unwrap();
        let rendered = buffer_text(terminal.backend());

        assert!(rendered.contains("j/k Pg"));
        assert!(rendered.contains("[/]"));
        assert!(rendered.contains("prev/next"));
    }
}
