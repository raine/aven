use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::tui::event::{Action, CommandContext};

use crate::tui::theme::Theme;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum FooterMode {
    Bulk,
    List,
    Columns,
    Detail,
    DetailDeleted,
    DetailNested,
    DetailNestedDeleted,
    RecurrenceDetailActive,
    RecurrenceDetailPaused,
    RecurrenceDetailStopped,
    DetailLinks,
    DetailNote,
    DetailAttachment,
    DetailEpicChild,
    AttachmentPreview,
    DetailSelection,
    StatusChoice,
    PriorityChoice,
}

pub(super) fn footer_bar(
    theme: &Theme,
    mode: FooterMode,
    width: u16,
    marked_task_count: usize,
) -> Paragraph<'static> {
    let mode = if marked_task_count > 0 && matches!(mode, FooterMode::List | FooterMode::Columns) {
        FooterMode::Bulk
    } else {
        mode
    };
    let spans = if mode == FooterMode::Bulk {
        visible_bulk_footer_segments(theme, width, marked_task_count)
            .into_iter()
            .flat_map(|segment| segment.spans)
            .collect()
    } else {
        let mut spans = if matches!(mode, FooterMode::StatusChoice | FooterMode::PriorityChoice) {
            marked_task_indicator(theme, marked_task_count)
        } else {
            Vec::new()
        };
        let indicator_width = spans_width(&spans);
        for (keys, label) in footer_hints(mode, width.saturating_sub(indicator_width)) {
            spans.extend(key(theme, keys));
            spans.push(cmd(theme, mode, label));
        }
        spans
    };
    Paragraph::new(Line::from(spans))
        .block(
            Block::new()
                .borders(Borders::TOP)
                .border_style(Style::new().fg(theme.border)),
        )
        .style(Style::new().fg(theme.fg).bg(theme.bg))
}

struct BulkFooterSegment {
    spans: Vec<Span<'static>>,
    action: Option<Action>,
}

fn bulk_footer_segments(theme: &Theme, count: usize) -> Vec<BulkFooterSegment> {
    vec![
        BulkFooterSegment {
            spans: vec![Span::styled(
                format!(" ● {count} marked  "),
                Style::new()
                    .fg(theme.attention)
                    .add_modifier(Modifier::BOLD),
            )],
            action: None,
        },
        bulk_action_segment(theme, "actions", Action::BeginCommand),
        bulk_action_segment(theme, "clear", Action::ClearMarks),
        bulk_action_segment(theme, "undo", Action::Undo),
    ]
}

fn bulk_action_segment(theme: &Theme, label: &str, action: Action) -> BulkFooterSegment {
    let keys = if action == Action::ClearMarks {
        "Esc/t C"
    } else {
        CommandContext::Normal
            .commands()
            .find(|command| command.action == action)
            .and_then(|command| command.keys(CommandContext::Normal).first())
            .map_or("", |keys| keys.label)
    };
    let mut spans = key(theme, keys);
    spans.push(Span::styled(format!(" {label}  "), theme.dim()));
    BulkFooterSegment {
        spans,
        action: Some(action),
    }
}

fn visible_bulk_footer_segments(theme: &Theme, width: u16, count: usize) -> Vec<BulkFooterSegment> {
    let mut used = 0_u16;
    let mut visible = Vec::new();
    for segment in bulk_footer_segments(theme, count) {
        let segment_width = spans_width(&segment.spans);
        if !visible.is_empty() && used.saturating_add(segment_width) > width {
            break;
        }
        used = used.saturating_add(segment_width);
        visible.push(segment);
    }
    visible
}

fn spans_width(spans: &[Span<'_>]) -> u16 {
    spans
        .iter()
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()) as u16)
        .sum()
}

pub(crate) fn bulk_footer_action_at(
    theme: &Theme,
    area: Rect,
    marked_task_count: usize,
    column: u16,
    row: u16,
) -> Option<Action> {
    let in_area = column >= area.x
        && column < area.x.saturating_add(area.width)
        && row >= area.y
        && row < area.y.saturating_add(area.height);
    if marked_task_count == 0 || row != area.y.saturating_add(1) || !in_area {
        return None;
    }
    let mut start = area.x;
    for segment in visible_bulk_footer_segments(theme, area.width, marked_task_count) {
        let end = start.saturating_add(spans_width(&segment.spans));
        if start <= column && column < end {
            return segment.action;
        }
        start = end;
    }
    None
}

fn marked_task_indicator(theme: &Theme, count: usize) -> Vec<Span<'static>> {
    if count == 0 {
        return Vec::new();
    }
    vec![Span::styled(
        format!(" ● {count} marked  "),
        Style::new()
            .fg(theme.attention)
            .add_modifier(Modifier::BOLD),
    )]
}

fn footer_hints(mode: FooterMode, width: u16) -> &'static [(&'static str, &'static str)] {
    match mode {
        FooterMode::Bulk => &[],
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
        FooterMode::RecurrenceDetailActive if width >= 128 => &[
            ("j/k", "scroll"),
            ("Enter", "task"),
            ("t r p", "pause series"),
            ("t r s", "stop series"),
            ("t r h", "history"),
            ("?", "more"),
            ("Esc", "close"),
        ],
        FooterMode::RecurrenceDetailPaused if width >= 128 => &[
            ("j/k", "scroll"),
            ("Enter", "task"),
            ("t r r", "resume series"),
            ("t r s", "stop series"),
            ("t r h", "history"),
            ("?", "more"),
            ("Esc", "close"),
        ],
        FooterMode::RecurrenceDetailStopped if width >= 128 => &[
            ("j/k", "scroll"),
            ("Enter", "task"),
            ("t r h", "history"),
            ("?", "more"),
            ("Esc", "close"),
        ],
        FooterMode::RecurrenceDetailActive if width >= 72 => &[
            ("Enter", "task"),
            ("t r p", "pause"),
            ("t r s", "stop"),
            ("t r h", "history"),
            ("?", "more"),
        ],
        FooterMode::RecurrenceDetailPaused if width >= 72 => &[
            ("Enter", "task"),
            ("t r r", "resume"),
            ("t r s", "stop"),
            ("t r h", "history"),
            ("?", "more"),
        ],
        FooterMode::RecurrenceDetailStopped if width >= 72 => &[
            ("Enter", "task"),
            ("t r h", "history"),
            ("?", "more"),
            ("Esc", "close"),
        ],
        FooterMode::RecurrenceDetailActive
        | FooterMode::RecurrenceDetailPaused
        | FooterMode::RecurrenceDetailStopped => &[
            ("Enter", "occurrence"),
            ("t r h", "history"),
            ("?", "more"),
            ("Esc", "task list"),
        ],
        FooterMode::DetailLinks => &[
            ("j/k", "select link"),
            ("Tab", "next section"),
            ("Enter", "open"),
            ("Esc", "browse"),
            ("q", "task list"),
        ],
        FooterMode::DetailNote => &[
            ("j/k", "select note"),
            ("e", "edit"),
            ("D", "delete"),
            ("Tab", "next section"),
            ("Esc", "browse"),
            ("q", "task list"),
        ],
        FooterMode::DetailAttachment => &[
            ("j/k", "select image"),
            ("Enter", "preview"),
            ("o", "system viewer"),
            ("s", "save as"),
            ("D", "remove"),
            ("Tab", "next section"),
            ("Esc", "browse"),
        ],
        FooterMode::DetailEpicChild => &[
            ("j/k", "select"),
            ("Enter", "open"),
            ("t c r", "remove"),
            ("Tab", "next section"),
            ("Esc", "browse"),
            ("q", "task list"),
        ],
        FooterMode::AttachmentPreview => &[
            ("j/k", "switch image"),
            ("o", "system viewer"),
            ("s", "save as"),
            ("D", "remove"),
            ("Esc", "detail"),
            ("q", "task list"),
        ],
        FooterMode::DetailSelection if width >= 72 => &[
            ("y", "copy selection"),
            ("Esc", "clear selection"),
            ("j/k Pg", "scroll"),
        ],
        FooterMode::DetailSelection => &[("y", "copy"), ("Esc", "clear")],
        FooterMode::DetailNested if width >= 128 => &[
            ("j/k Pg", "scroll"),
            ("[/]", "task"),
            ("e", "edit"),
            ("s", "status"),
            ("e p", "priority"),
            ("n", "note"),
            ("d/x", "done/cancel"),
            ("?", "more"),
            ("Esc", "parent"),
            ("q", "task list"),
        ],
        FooterMode::DetailNested if width >= 72 => &[
            ("j/k Pg", "scroll"),
            ("[/]", "task"),
            ("e", "edit"),
            ("s/e p", "status/priority"),
            ("d/x", "done/cancel"),
            ("?", "more"),
            ("Esc", "parent"),
            ("q", "task list"),
        ],
        FooterMode::DetailNested => &[
            ("j/k", "scroll"),
            ("?", "more"),
            ("Esc", "parent"),
            ("q", "list"),
        ],
        FooterMode::DetailNestedDeleted if width >= 128 => &[
            ("j/k Pg", "scroll"),
            ("[/]", "task"),
            ("e", "edit"),
            ("s", "status"),
            ("e p", "priority"),
            ("d/x", "done/cancel"),
            ("t R", "restore deleted"),
            ("y r", "copy ref"),
            ("?", "more"),
            ("Esc", "parent"),
            ("q", "task list"),
        ],
        FooterMode::DetailNestedDeleted if width >= 72 => &[
            ("j/k Pg", "scroll"),
            ("[/]", "task"),
            ("s/e p", "status/priority"),
            ("d/x", "done/cancel"),
            ("t R", "restore deleted"),
            ("?", "more"),
            ("Esc", "parent"),
            ("q", "task list"),
        ],
        FooterMode::DetailNestedDeleted => &[
            ("j/k", "scroll"),
            ("t R", "restore deleted"),
            ("?", "more"),
            ("Esc", "parent"),
            ("q", "list"),
        ],
        FooterMode::DetailDeleted if width >= 128 => &[
            ("j/k Pg", "scroll"),
            ("[/]", "task"),
            ("e", "edit"),
            ("s", "status"),
            ("e p", "priority"),
            ("d/x", "done/cancel"),
            ("t R", "restore deleted"),
            ("y r", "copy ref"),
            ("?", "more"),
            ("Esc", "task list"),
            ("q", "close"),
        ],
        FooterMode::DetailDeleted if width >= 72 => &[
            ("j/k Pg", "scroll"),
            ("[/]", "task"),
            ("e", "edit"),
            ("s/e p", "status/priority"),
            ("d/x", "done/cancel"),
            ("t R", "restore deleted"),
            ("?", "more"),
            ("Esc", "task list"),
            ("q", "close"),
        ],
        FooterMode::DetailDeleted => &[
            ("j/k", "scroll"),
            ("t R", "restore deleted"),
            ("?", "more"),
            ("Esc", "task list"),
            ("q", "close"),
        ],
        FooterMode::Detail if width >= 128 => &[
            ("j/k Pg", "scroll"),
            ("[/]", "task"),
            ("e", "edit"),
            ("s", "status"),
            ("e p", "priority"),
            ("n", "note"),
            ("d/x", "done/cancel"),
            ("y r", "copy ref"),
            ("?", "more"),
            ("Esc", "task list"),
            ("q", "close"),
        ],
        FooterMode::Detail if width >= 72 => &[
            ("j/k Pg", "scroll"),
            ("[/]", "task"),
            ("e", "edit"),
            ("s/e p", "status/priority"),
            ("n", "note"),
            ("d/x", "done/cancel"),
            ("?", "more"),
            ("Esc", "task list"),
            ("q", "close"),
        ],
        FooterMode::Detail => &[
            ("j/k", "scroll"),
            ("[/]", "task"),
            ("e", "edit"),
            ("?", "more"),
            ("Esc", "task list"),
            ("q", "close"),
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

/// Key hints in the footer's chip style for surfaces outside the task list.
pub(super) fn key_hint_spans(theme: &Theme, hints: &[(&str, &str)]) -> Vec<Span<'static>> {
    key_hint_segments(theme, hints, theme.bg, theme.bg_panel)
        .into_iter()
        .flatten()
        .collect()
}

/// One chip-and-label segment per hint, with `chip` filling the key and its
/// rounded edges blended into the surrounding `bg`.
pub(super) fn key_hint_segments(
    theme: &Theme,
    hints: &[(&str, &str)],
    bg: ratatui::style::Color,
    chip: ratatui::style::Color,
) -> Vec<Vec<Span<'static>>> {
    hints
        .iter()
        .map(|(keys, label)| {
            let mut spans = key_on(theme, keys, bg, chip);
            spans.push(Span::styled(format!(" {label}  "), theme.dim()));
            spans
        })
        .collect()
}

fn key(theme: &Theme, label: &str) -> Vec<Span<'static>> {
    key_on(theme, label, theme.bg, theme.bg_panel)
}

fn key_on(
    theme: &Theme,
    label: &str,
    bg: ratatui::style::Color,
    chip: ratatui::style::Color,
) -> Vec<Span<'static>> {
    let key_style = theme.muted().bg(chip).add_modifier(Modifier::BOLD);
    let separator_style = theme.dim().bg(chip);
    let edge_style = Style::new().fg(chip).bg(bg);
    let mut spans = vec![Span::styled(theme.pill_edge("").to_string(), edge_style)];
    for (index, part) in label.split('/').enumerate() {
        if index > 0 {
            spans.push(Span::styled("/".to_string(), separator_style));
        }
        spans.push(Span::styled(part.to_string(), key_style));
    }
    spans.push(Span::styled(theme.pill_edge("").to_string(), edge_style));
    spans
}

fn cmd(theme: &Theme, mode: FooterMode, label: &str) -> Span<'static> {
    let style = match mode {
        FooterMode::StatusChoice => theme.status(label),
        FooterMode::PriorityChoice => theme.priority(label),
        FooterMode::Bulk
        | FooterMode::List
        | FooterMode::Columns
        | FooterMode::Detail
        | FooterMode::DetailDeleted
        | FooterMode::DetailNested
        | FooterMode::DetailNestedDeleted
        | FooterMode::RecurrenceDetailActive
        | FooterMode::RecurrenceDetailPaused
        | FooterMode::RecurrenceDetailStopped
        | FooterMode::DetailLinks
        | FooterMode::DetailNote
        | FooterMode::DetailAttachment
        | FooterMode::DetailEpicChild
        | FooterMode::AttachmentPreview
        | FooterMode::DetailSelection => theme.dim(),
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
    fn footer_shows_marked_task_scope() {
        for mode in [
            FooterMode::List,
            FooterMode::Columns,
            FooterMode::Detail,
            FooterMode::StatusChoice,
            FooterMode::PriorityChoice,
        ] {
            for count in [0, 3] {
                let mut terminal = Terminal::new(TestBackend::new(100, 3)).unwrap();
                terminal
                    .draw(|frame| {
                        frame.render_widget(
                            footer_bar(&Theme::dark(), mode, 100, count),
                            frame.area(),
                        );
                    })
                    .unwrap();
                let rendered = buffer_text(terminal.backend());
                let bulk = count > 0 && matches!(mode, FooterMode::List | FooterMode::Columns);
                assert_eq!(
                    rendered.contains("● 3 marked"),
                    count > 0 && mode != FooterMode::Detail
                );
                for label in ["actions", "Esc/t C", "clear", "undo"] {
                    assert_eq!(rendered.contains(label), bulk, "{rendered}");
                }
                if bulk {
                    assert!(!rendered.contains("detail"));
                } else {
                    let hint = match mode {
                        FooterMode::Detail => "scroll",
                        FooterMode::Columns => "select",
                        FooterMode::StatusChoice => "todo",
                        FooterMode::PriorityChoice => "high",
                        _ => "detail",
                    };
                    assert!(rendered.contains(hint), "{rendered}");
                }
            }
        }
    }

    #[test]
    fn bulk_footer_keys_keep_standard_keycap_contrast() {
        let backend = TestBackend::new(100, 3);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                frame.render_widget(
                    footer_bar(&Theme::dark(), FooterMode::List, 100, 3),
                    frame.area(),
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let key_column = (0..100)
            .find(|&column| buffer[(column, 1)].symbol() == ":")
            .expect("expected command key");

        assert_eq!(buffer[(key_column, 1)].bg, Theme::dark().bg_panel);
        assert_eq!(buffer[(key_column - 1, 1)].bg, Theme::dark().bg);
        assert_eq!(buffer[(key_column + 1, 1)].bg, Theme::dark().bg);
    }

    #[test]
    fn bulk_footer_actions_share_rendered_hit_ranges() {
        for width in [20, 30, 45, 100] {
            let area = Rect::new(7, 4, width, 2);
            let mut terminal = Terminal::new(TestBackend::new(120, 8)).unwrap();
            terminal
                .draw(|frame| {
                    frame.render_widget(
                        footer_bar(&Theme::dark(), FooterMode::List, width, 3),
                        area,
                    );
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            let row = (area.x..area.right())
                .map(|x| buffer[(x, area.y + 1)].symbol())
                .collect::<String>();
            let mut visible_actions = Vec::new();
            for (label, action) in [
                ("actions", Action::BeginCommand),
                ("clear", Action::ClearMarks),
                ("undo", Action::Undo),
            ] {
                if let Some(byte) = row.find(label) {
                    let start = area.x + row[..byte].chars().count() as u16;
                    for column in start..start + label.len() as u16 {
                        assert_eq!(
                            bulk_footer_action_at(&Theme::dark(), area, 3, column, area.y + 1),
                            Some(action)
                        );
                    }
                    visible_actions.push(action);
                }
            }
            if width == 100 {
                assert_eq!(
                    visible_actions,
                    vec![Action::BeginCommand, Action::ClearMarks, Action::Undo]
                );
            }
            for column in area.x..area.right() {
                if let Some(action) =
                    bulk_footer_action_at(&Theme::dark(), area, 3, column, area.y + 1)
                {
                    assert!(
                        visible_actions.contains(&action),
                        "hidden action at {width}/{column}"
                    );
                }
                assert_eq!(
                    bulk_footer_action_at(&Theme::dark(), area, 0, column, area.y + 1),
                    None
                );
            }
            for (column, row) in [
                (area.x, area.y),
                (area.x, area.bottom()),
                (area.x - 1, area.y + 1),
                (area.right(), area.y + 1),
                (area.x + 2, area.y + 1),
            ] {
                assert_eq!(
                    bulk_footer_action_at(&Theme::dark(), area, 3, column, row),
                    None
                );
            }
        }
    }

    #[test]
    fn detail_links_footer_advertises_navigation_controls() {
        let hints = footer_hints(FooterMode::DetailLinks, 80);

        assert_eq!(
            hints,
            &[
                ("j/k", "select link"),
                ("Tab", "next section"),
                ("Enter", "open"),
                ("Esc", "browse"),
                ("q", "task list"),
            ]
        );
    }

    #[test]
    fn detail_attachment_footer_advertises_save_controls() {
        let hints = footer_hints(FooterMode::DetailAttachment, 80);

        assert!(hints.contains(&("Enter", "preview")));
        assert!(hints.contains(&("o", "system viewer")));
        assert!(hints.contains(&("s", "save as")));
        assert!(hints.contains(&("D", "remove")));
    }

    #[test]
    fn attachment_preview_footer_advertises_preview_controls() {
        assert_eq!(
            footer_hints(FooterMode::AttachmentPreview, 80),
            &[
                ("j/k", "switch image"),
                ("o", "system viewer"),
                ("s", "save as"),
                ("D", "remove"),
                ("Esc", "detail"),
                ("q", "task list"),
            ]
        );
    }

    fn detail_command_has_key(label: &str) -> bool {
        crate::tui::event::CommandContext::Detail
            .commands()
            .flat_map(|command| {
                command
                    .keys(crate::tui::event::CommandContext::Detail)
                    .iter()
            })
            .any(|key| key.label == label)
    }

    #[test]
    fn restore_hint_tracks_root_and_nested_deletion_state() {
        assert!(detail_command_has_key("t R"));
        assert!(detail_command_has_key("d"));
        assert!(detail_command_has_key("x"));
        for mode in [FooterMode::Detail, FooterMode::DetailNested] {
            assert!(
                !footer_hints(mode, 128)
                    .iter()
                    .any(|(_, label)| label.contains("restore"))
            );
        }
        for mode in [FooterMode::DetailDeleted, FooterMode::DetailNestedDeleted] {
            assert!(footer_hints(mode, 128).contains(&("t R", "restore deleted")));
            assert!(footer_hints(mode, 80).contains(&("t R", "restore deleted")));
            assert!(footer_hints(mode, 80).contains(&("d/x", "done/cancel")));
        }
    }

    #[test]
    fn recurrence_detail_footer_advertises_catalog_lifecycle_shortcuts() {
        let active = footer_hints(FooterMode::RecurrenceDetailActive, 128);
        assert!(active.contains(&("t r p", "pause series")));
        assert!(active.contains(&("t r s", "stop series")));
        assert!(active.contains(&("t r h", "history")));

        let paused = footer_hints(FooterMode::RecurrenceDetailPaused, 128);
        assert!(paused.contains(&("t r r", "resume series")));
        assert!(paused.contains(&("t r s", "stop series")));

        let stopped = footer_hints(FooterMode::RecurrenceDetailStopped, 128);
        assert!(
            !stopped
                .iter()
                .any(|(_, label)| label.contains("stop series"))
        );
        assert!(stopped.contains(&("t r h", "history")));

        for label in ["t r p", "t r r", "t r s", "t r h"] {
            assert!(detail_command_has_key(label), "missing catalog key {label}");
        }
    }

    #[test]
    fn detail_mutation_hints_use_catalog_bindings() {
        let mut terminal = Terminal::new(TestBackend::new(128, 3)).unwrap();
        terminal
            .draw(|frame| {
                frame.render_widget(
                    footer_bar(&Theme::dark(), FooterMode::Detail, 128, 0),
                    frame.area(),
                );
            })
            .unwrap();
        let rendered = buffer_text(terminal.backend()).replace(['', ''], "");
        for hint in [
            "j/k Pg",
            "[/] task",
            "s status",
            "e p priority",
            "d/x done/cancel",
            "? more",
        ] {
            assert!(rendered.contains(hint), "{rendered}");
        }

        let hints = footer_hints(FooterMode::Detail, 128);
        assert!(hints.contains(&("s", "status")));
        assert!(hints.contains(&("e p", "priority")));
        assert!(hints.contains(&("d/x", "done/cancel")));
        assert!(hints.contains(&("y r", "copy ref")));
        for label in ["s", "e p", "d", "x", "y r"] {
            assert!(detail_command_has_key(label), "missing catalog key {label}");
        }
    }

    #[test]
    fn detail_footer_distinguishes_root_and_nested_back_targets() {
        let root = footer_hints(FooterMode::Detail, 80);
        let nested = footer_hints(FooterMode::DetailNested, 80);

        assert!(root.contains(&("Esc", "task list")));
        assert!(nested.contains(&("Esc", "parent")));
        assert!(nested.contains(&("q", "task list")));
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
