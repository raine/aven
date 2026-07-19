use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::tui::theme::{self, BG, BG_PANEL, BORDER, FG, FG_DIM, FG_MUTED, YELLOW};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum FooterMode {
    List,
    Columns,
    Detail,
    DetailChildren,
    DetailAttachment,
    AttachmentPreview,
    DetailSelection,
    StatusChoice,
    PriorityChoice,
}

pub(super) fn footer_bar(
    mode: FooterMode,
    width: u16,
    marked_task_count: usize,
) -> Paragraph<'static> {
    let mut spans = marked_task_indicator(marked_task_count);
    let indicator_width = spans
        .iter()
        .map(|span| span.content.chars().count() as u16)
        .sum::<u16>();
    for (keys, label) in footer_hints(mode, width.saturating_sub(indicator_width)) {
        spans.extend(key(keys));
        spans.push(cmd(mode, label));
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

fn marked_task_indicator(count: usize) -> Vec<Span<'static>> {
    if count == 0 {
        return Vec::new();
    }
    let mut spans = vec![Span::styled(
        format!(" ● {count} marked  "),
        Style::new().fg(YELLOW).add_modifier(Modifier::BOLD),
    )];
    spans.extend(key("t C"));
    spans.push(Span::styled(" clear all  ", Style::new().fg(FG_DIM)));
    spans
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
            ("Space", "mark"),
            ("e l", "labels"),
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
        FooterMode::Columns if width >= 120 => &[
            ("h/j/k/l", "select"),
            ("</>", "move"),
            ("m", "lane"),
            ("Space", "mark"),
            ("u", "undo"),
            ("Enter", "detail"),
            ("?", "more"),
            ("q", "quit"),
        ],
        FooterMode::Columns if width >= 80 => &[
            ("h/j/k/l", "select"),
            ("</>", "move"),
            ("m", "lane"),
            ("Space", "mark"),
            ("?", "more"),
        ],
        FooterMode::Columns => &[
            ("h/l", "select"),
            ("</>", "move"),
            ("m", "lane"),
            ("?", "more"),
        ],
        FooterMode::DetailChildren => &[
            ("j/k", "select child"),
            ("Enter", "open"),
            ("Tab/Esc", "leave"),
        ],
        FooterMode::DetailAttachment => &[
            ("j/k", "select image"),
            ("Enter", "preview/open"),
            ("o", "system viewer"),
            ("Tab/Esc", "leave"),
        ],
        FooterMode::AttachmentPreview => &[
            ("j/k", "switch image"),
            ("o", "system viewer"),
            ("Esc", "back"),
        ],
        FooterMode::DetailSelection if width >= 72 => &[
            ("y", "copy selection"),
            ("Esc", "clear selection"),
            ("j/k Pg", "scroll"),
        ],
        FooterMode::DetailSelection => &[("y", "copy"), ("Esc", "clear")],
        FooterMode::Detail if width >= 128 => &[
            ("j/k Pg", "scroll"),
            ("[/]", "task"),
            ("e", "edit"),
            ("s", "status"),
            ("e p", "priority"),
            ("n", "note"),
            ("t d", "done"),
            ("t y/Y", "copy"),
            ("?", "more"),
            ("Esc", "back"),
        ],
        FooterMode::Detail if width >= 72 => &[
            ("j/k Pg", "scroll"),
            ("[/]", "task"),
            ("e", "edit"),
            ("s/e p", "status/priority"),
            ("n", "note"),
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
        FooterMode::StatusChoice => &[
            ("i", "inbox"),
            ("b", "backlog"),
            ("t", "todo"),
            ("a", "active"),
            ("d", "done"),
            ("x", "canceled"),
            ("Esc", "cancel"),
        ],
        FooterMode::PriorityChoice => &[
            ("n", "none"),
            ("l", "low"),
            ("m", "medium"),
            ("h", "high"),
            ("u", "urgent"),
            ("Esc", "cancel"),
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

fn cmd(mode: FooterMode, label: &str) -> Span<'static> {
    let style = match mode {
        FooterMode::StatusChoice => theme::status_style(label),
        FooterMode::PriorityChoice => theme::priority_style(label),
        FooterMode::List
        | FooterMode::Columns
        | FooterMode::Detail
        | FooterMode::DetailChildren
        | FooterMode::DetailAttachment
        | FooterMode::AttachmentPreview
        | FooterMode::DetailSelection => Style::new().fg(FG_DIM),
    };
    Span::styled(format!(" {label}  "), style)
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
            .draw(|frame| frame.render_widget(footer_bar(FooterMode::Detail, 128, 0), frame.area()))
            .unwrap();
        let rendered = buffer_text(terminal.backend());

        assert!(rendered.contains("j/k Pg"));
        assert!(rendered.contains("[/]"));
        assert!(rendered.contains("task"));
        assert!(rendered.contains("s"));
        assert!(rendered.contains("e p"));
        assert!(rendered.contains("more"));
    }

    #[test]
    fn footer_shows_marked_task_scope() {
        let backend = TestBackend::new(100, 3);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| frame.render_widget(footer_bar(FooterMode::List, 100, 3), frame.area()))
            .unwrap();
        let rendered = buffer_text(terminal.backend());

        assert!(rendered.contains("● 3 marked"));
        assert!(rendered.contains("t C"));
        assert!(rendered.contains("clear all"));
    }

    #[test]
    fn footer_hides_marked_task_scope_when_marks_are_empty() {
        let backend = TestBackend::new(100, 3);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| frame.render_widget(footer_bar(FooterMode::List, 100, 0), frame.area()))
            .unwrap();
        let rendered = buffer_text(terminal.backend());

        assert!(!rendered.contains("marked"));
    }

    #[test]
    fn detail_children_footer_advertises_selection_controls() {
        let hints = footer_hints(FooterMode::DetailChildren, 80);

        assert_eq!(
            hints,
            &[
                ("j/k", "select child"),
                ("Enter", "open"),
                ("Tab/Esc", "leave"),
            ]
        );
    }

    #[test]
    fn detail_attachment_footer_advertises_preview_controls() {
        assert_eq!(
            footer_hints(FooterMode::DetailAttachment, 80),
            &[
                ("j/k", "select image"),
                ("Enter", "preview/open"),
                ("o", "system viewer"),
                ("Tab/Esc", "leave"),
            ]
        );
        assert_eq!(
            footer_hints(FooterMode::AttachmentPreview, 80),
            &[
                ("j/k", "switch image"),
                ("o", "system viewer"),
                ("Esc", "back"),
            ]
        );
    }

    #[test]
    fn detail_selection_footer_advertises_copy_and_clear() {
        let hints = footer_hints(FooterMode::DetailSelection, 80);

        assert_eq!(
            hints,
            &[
                ("y", "copy selection"),
                ("Esc", "clear selection"),
                ("j/k Pg", "scroll"),
            ]
        );
    }

    #[test]
    fn list_footer_expands_intent_labels_when_wide() {
        let hints = footer_hints(FooterMode::List, 148);

        assert!(hints.contains(&("a", "add")));
        assert!(hints.contains(&("s", "status")));
        assert!(hints.contains(&("p", "projects")));
        assert!(hints.contains(&("d", "done")));
        assert!(hints.contains(&("x", "cancel")));
        assert!(hints.contains(&("Space", "mark")));
        assert!(hints.contains(&("e l", "labels")));
        assert!(hints.contains(&("g", "scope")));
        assert!(hints.contains(&("v", "views")));
        assert!(hints.contains(&("?", "more")));
        assert!(!hints.contains(&("p", "priority")));
        assert!(!hints.contains(&("g", "views")));
        assert!(!hints.iter().any(|(_, label)| *label == "prefixes"));
    }

    #[test]
    fn columns_footer_prioritizes_selection_and_move_keys() {
        let hints = footer_hints(FooterMode::Columns, 120);

        assert!(hints.contains(&("h/j/k/l", "select")));
        assert!(hints.contains(&("</>", "move")));
        assert!(hints.contains(&("m", "lane")));
        assert!(hints.contains(&("Space", "mark")));
        assert!(hints.contains(&("u", "undo")));
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
