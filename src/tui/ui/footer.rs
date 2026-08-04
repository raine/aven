use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::tui::event::{Action, CommandContext};

use crate::tui::theme::{self, BG, BG_PANEL, BORDER, FG, FG_DIM, FG_MUTED, YELLOW};

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
        visible_bulk_footer_segments(width, marked_task_count)
            .into_iter()
            .flat_map(|segment| segment.spans)
            .collect()
    } else {
        let mut spans = if matches!(mode, FooterMode::StatusChoice | FooterMode::PriorityChoice) {
            marked_task_indicator(marked_task_count)
        } else {
            Vec::new()
        };
        let indicator_width = spans_width(&spans);
        for (keys, label) in footer_hints(mode, width.saturating_sub(indicator_width)) {
            spans.extend(key(keys));
            spans.push(cmd(mode, label));
        }
        spans
    };
    let background = if mode == FooterMode::Bulk {
        BG_PANEL
    } else {
        BG
    };
    Paragraph::new(Line::from(spans))
        .block(
            Block::new()
                .borders(Borders::TOP)
                .border_style(Style::new().fg(BORDER)),
        )
        .style(Style::new().fg(FG).bg(background))
}

struct BulkFooterSegment {
    spans: Vec<Span<'static>>,
    action: Option<Action>,
}

fn bulk_footer_segments(count: usize) -> Vec<BulkFooterSegment> {
    vec![
        BulkFooterSegment {
            spans: vec![Span::styled(
                format!(" ● {count} marked  "),
                Style::new().fg(YELLOW).add_modifier(Modifier::BOLD),
            )],
            action: None,
        },
        bulk_action_segment("actions", Action::BeginCommand),
        bulk_action_segment("clear", Action::ClearMarks),
        bulk_action_segment("undo", Action::Undo),
    ]
}

fn bulk_action_segment(label: &str, action: Action) -> BulkFooterSegment {
    let keys = CommandContext::Normal
        .commands()
        .find(|command| command.action == action)
        .and_then(|command| command.keys(CommandContext::Normal).first())
        .map_or("", |keys| keys.label);
    let mut spans = key(keys);
    spans.push(Span::styled(format!(" {label}  "), Style::new().fg(FG_DIM)));
    BulkFooterSegment {
        spans,
        action: Some(action),
    }
}

fn visible_bulk_footer_segments(width: u16, count: usize) -> Vec<BulkFooterSegment> {
    let mut used = 0_u16;
    let mut visible = Vec::new();
    for segment in bulk_footer_segments(count) {
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
    for segment in visible_bulk_footer_segments(area.width, marked_task_count) {
        let end = start.saturating_add(spans_width(&segment.spans));
        if start <= column && column < end {
            return segment.action;
        }
        start = end;
    }
    None
}

fn marked_task_indicator(count: usize) -> Vec<Span<'static>> {
    if count == 0 {
        return Vec::new();
    }
    vec![Span::styled(
        format!(" ● {count} marked  "),
        Style::new().fg(YELLOW).add_modifier(Modifier::BOLD),
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
        assert!(rendered.contains("d/x"));
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
        assert!(rendered.contains("actions"));
        assert!(rendered.contains("t C"));
        assert!(rendered.contains("clear"));
        assert!(rendered.contains("undo"));
        assert!(!rendered.contains("detail"));
    }

    #[test]
    fn marked_scope_stays_out_of_detail_footer() {
        let backend = TestBackend::new(100, 3);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| frame.render_widget(footer_bar(FooterMode::Detail, 100, 3), frame.area()))
            .unwrap();
        let rendered = buffer_text(terminal.backend());

        assert!(!rendered.contains("marked"));
        assert!(rendered.contains("scroll"));
    }

    #[test]
    fn bulk_footer_actions_share_rendered_hit_ranges() {
        let area = Rect::new(0, 0, 100, 2);
        let actions = (0..area.width)
            .filter_map(|column| bulk_footer_action_at(area, 3, column, 1))
            .collect::<Vec<_>>();

        assert!(actions.contains(&Action::BeginCommand));
        assert!(actions.contains(&Action::ClearMarks));
        assert!(actions.contains(&Action::Undo));
        assert_eq!(bulk_footer_action_at(area, 3, 5, 0), None);
    }

    #[test]
    fn marked_status_choice_keeps_scope_without_conflicting_clear_hint() {
        let backend = TestBackend::new(100, 3);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                frame.render_widget(footer_bar(FooterMode::StatusChoice, 100, 3), frame.area())
            })
            .unwrap();
        let rendered = buffer_text(terminal.backend());

        assert!(rendered.contains("● 3 marked"));
        assert!(rendered.contains("todo"));
        assert!(!rendered.contains("clear"));
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
    fn deleted_detail_footer_advertises_restore() {
        let hints = footer_hints(FooterMode::DetailDeleted, 80);

        assert!(hints.contains(&("t R", "restore deleted")));
        assert!(hints.contains(&("d/x", "done/cancel")));
        assert!(detail_command_has_key("t R"));
        assert!(detail_command_has_key("d"));
        assert!(detail_command_has_key("x"));
    }

    #[test]
    fn restore_hint_tracks_root_and_nested_deletion_state() {
        for mode in [FooterMode::Detail, FooterMode::DetailNested] {
            assert!(
                !footer_hints(mode, 128)
                    .iter()
                    .any(|(_, label)| label.contains("restore"))
            );
        }
        for mode in [FooterMode::DetailDeleted, FooterMode::DetailNestedDeleted] {
            assert!(footer_hints(mode, 128).contains(&("t R", "restore deleted")));
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
