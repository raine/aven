use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::tui::theme::{BG, BG_PANEL, BORDER, FG, FG_DIM, FG_MUTED};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum FooterMode {
    List,
    Detail,
}

pub(super) fn footer_bar(mode: FooterMode, width: u16) -> Paragraph<'static> {
    let mut spans = Vec::new();
    for (keys, label) in footer_hints(mode, width) {
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

fn footer_hints(mode: FooterMode, width: u16) -> &'static [(&'static str, &'static str)] {
    match mode {
        FooterMode::List if width >= 148 => &[
            ("j/k", "move"),
            ("Enter", "detail"),
            ("a", "add"),
            ("n", "note"),
            ("s", "status"),
            ("p", "projects"),
            ("d", "done"),
            ("x", "cancel"),
            ("g", "scope"),
            ("v", "views"),
            ("f", "filter"),
            ("o", "order"),
            ("?", "more"),
            ("q", "quit"),
        ],
        FooterMode::List if width >= 96 => &[
            ("j/k", "move"),
            ("Enter", "detail"),
            ("a", "add"),
            ("s", "status"),
            ("p", "projects"),
            ("g/v/f/o", "menus"),
            ("?", "more"),
            ("q", "quit"),
        ],
        FooterMode::List => &[
            ("j/k", "move"),
            ("Enter", "detail"),
            ("a", "add"),
            ("?", "more"),
            ("q", "quit"),
        ],
        FooterMode::Detail if width >= 128 => &[
            ("j/k Pg", "scroll"),
            ("[/]", "task"),
            ("t e", "edit"),
            ("t s", "status"),
            ("t P", "priority"),
            ("t N", "note"),
            ("t d", "done"),
            ("t y/Y", "copy"),
            ("?", "more"),
            ("Esc", "back"),
        ],
        FooterMode::Detail if width >= 72 => &[
            ("j/k Pg", "scroll"),
            ("[/]", "task"),
            ("t e", "edit"),
            ("t s/t P", "edit"),
            ("t N", "note"),
            ("?", "more"),
            ("Esc", "back"),
        ],
        FooterMode::Detail => &[
            ("j/k", "scroll"),
            ("[/]", "task"),
            ("e", "edit"),
            ("?", "more"),
            ("Esc", "back"),
        ],
    }
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
        let backend = TestBackend::new(128, 3);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| frame.render_widget(footer_bar(FooterMode::Detail, 128), frame.area()))
            .unwrap();
        let rendered = buffer_text(terminal.backend());

        assert!(rendered.contains("j/k Pg"));
        assert!(rendered.contains("[/]"));
        assert!(rendered.contains("task"));
        assert!(rendered.contains("t s"));
        assert!(rendered.contains("t P"));
        assert!(rendered.contains("more"));
    }

    #[test]
    fn list_footer_expands_intent_labels_when_wide() {
        let hints = footer_hints(FooterMode::List, 148);

        assert!(hints.contains(&("a", "add")));
        assert!(hints.contains(&("s", "status")));
        assert!(hints.contains(&("p", "projects")));
        assert!(hints.contains(&("d", "done")));
        assert!(hints.contains(&("x", "cancel")));
        assert!(hints.contains(&("g", "scope")));
        assert!(hints.contains(&("v", "views")));
        assert!(hints.contains(&("?", "more")));
        assert!(!hints.contains(&("p", "priority")));
        assert!(!hints.contains(&("g", "views")));
        assert!(!hints.iter().any(|(_, label)| *label == "prefixes"));
    }

    #[test]
    fn footer_collapses_to_core_hints_when_narrow() {
        let hints = footer_hints(FooterMode::List, 60);

        assert_eq!(
            hints,
            &[
                ("j/k", "move"),
                ("Enter", "detail"),
                ("a", "add"),
                ("?", "more"),
                ("q", "quit"),
            ]
        );
    }
}
