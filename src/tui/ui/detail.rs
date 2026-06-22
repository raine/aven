use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Borders, Clear, Padding, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};

use super::task_display::{description_or_placeholder, labels_display};
use crate::query::TaskListItem;
use crate::tui::app::WidgetState;
use crate::tui::markdown::render_markdown;
use crate::tui::store::TuiStore;
use crate::tui::theme::{self, ACCENT, BG, BG_PANEL, BORDER, FG, FG_DIM, FG_MUTED, ORANGE, RED};
use crate::tui::widgets::{priority_short, status_chip, status_span};

fn render_detail(frame: &mut Frame, item: &TaskListItem, scroll: u16) {
    let area = frame.area();
    let [_, body, _] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(2),
    ])
    .areas(area);
    frame.render_widget(Clear, body);
    frame.render_widget(Block::new().style(Style::new().bg(BG)), body);
    if body.width == 0 || body.height == 0 {
        return;
    }

    let [content_area, metadata_area] = if body.width >= 96 {
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(34)]).areas(body)
    } else {
        [body, Rect::default()]
    };
    let content_area = content_area.inner(detail_content_margin());
    render_detail_content(frame, item, content_area, scroll);
    if metadata_area.width > 0 {
        render_detail_metadata(frame, item, metadata_area);
    }
}

fn keycap_style() -> Style {
    Style::new()
        .fg(FG)
        .bg(BG_PANEL)
        .add_modifier(Modifier::BOLD)
}

pub(crate) fn detail_scroll_cap(
    item: &TaskListItem,
    terminal_width: u16,
    terminal_height: u16,
) -> u16 {
    let area = Rect::new(0, 0, terminal_width, terminal_height);
    let [_, body, _] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(2),
    ])
    .areas(area);
    let [content_area, _] = if body.width >= 96 {
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(34)]).areas(body)
    } else {
        [body, Rect::default()]
    };
    let content_area = content_area.inner(detail_content_margin());
    let content_height = detail_content_lines(item, content_area.width as usize)
        .len()
        .max(1);
    detail_scroll_max_start(content_height, content_area.height as usize) as u16
}

fn detail_content_margin() -> Margin {
    Margin {
        horizontal: 2,
        vertical: 1,
    }
}

fn render_detail_content(frame: &mut Frame, item: &TaskListItem, area: Rect, scroll: u16) {
    let lines = detail_content_lines(item, area.width as usize);
    let visible = area.height as usize;
    let content_height = lines.len().max(1);
    let start = detail_scroll_start(scroll, content_height, visible);
    frame.render_widget(
        Paragraph::new(Text::from(
            lines.into_iter().skip(start).collect::<Vec<_>>(),
        ))
        .style(Style::new().fg(FG).bg(BG)),
        area,
    );
    if content_height > visible {
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .style(Style::new().fg(FG_DIM).bg(BG))
                .thumb_style(Style::new().fg(FG_MUTED)),
            area,
            &mut ScrollbarState::new(content_height)
                .position(detail_scrollbar_position(start, content_height, visible))
                .viewport_content_length(visible),
        );
    }
}

fn detail_scroll_start(scroll: u16, content_height: usize, visible: usize) -> usize {
    let max_start = detail_scroll_max_start(content_height, visible);
    (scroll as usize).min(max_start)
}

fn detail_scrollbar_position(start: usize, content_height: usize, visible: usize) -> usize {
    let max_start = detail_scroll_max_start(content_height, visible);
    start
        .saturating_mul(content_height.saturating_sub(1))
        .checked_div(max_start)
        .unwrap_or(0)
}

fn detail_scroll_max_start(content_height: usize, visible: usize) -> usize {
    content_height.saturating_sub(visible)
}

fn detail_content_lines(item: &TaskListItem, width: usize) -> Vec<Line<'static>> {
    let mut lines = detail_header_options(item, width);
    lines.extend(quoted_block_lines(
        &description_or_placeholder(&item.task.description),
        width,
        Style::new().fg(FG_MUTED),
    ));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(
            "NOTES",
            Style::new().fg(FG_DIM).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" (", Style::new().fg(FG_DIM)),
        Span::styled("n", keycap_style()),
        Span::styled(" add)", Style::new().fg(FG_DIM)),
    ]));
    if item.notes.is_empty() {
        lines.push(Line::from(Span::styled("none", Style::new().fg(FG_MUTED))));
    } else {
        for note in &item.notes {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled(note.created_at.clone(), Style::new().fg(FG_DIM)),
                Span::styled("  you", Style::new().fg(ACCENT)),
            ]));
            lines.extend(note_card_lines(&note.body, width));
        }
    }
    lines
}

fn detail_header_options(item: &TaskListItem, width: usize) -> Vec<Line<'static>> {
    vec![
        Line::from(vec![Span::styled(
            item.task.title.clone(),
            Style::new().fg(FG).add_modifier(Modifier::BOLD),
        )]),
        Line::from(Span::styled("─".repeat(width), Style::new().fg(BORDER))),
        Line::from(vec![
            Span::styled(item.display_ref.clone(), Style::new().fg(FG_DIM)),
            Span::styled("   ", Style::new().fg(FG_DIM)),
            status_span(&item.task.status),
            Span::styled("   ", Style::new().fg(FG_DIM)),
            Span::styled(
                priority_short(&item.task.priority),
                theme::priority_style(&item.task.priority).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
    ]
}

fn quoted_block_lines(body: &str, width: usize, style: Style) -> Vec<Line<'static>> {
    let content_width = width.saturating_sub(3).max(1);
    render_markdown(body, content_width)
        .into_iter()
        .map(|line| {
            let mut spans = line_with_base_style(line, style).spans;
            spans.insert(0, Span::styled("│ ", Style::new().fg(BORDER)));
            Line::from(spans)
        })
        .collect()
}

fn note_card_lines(body: &str, width: usize) -> Vec<Line<'static>> {
    let content_width = width.saturating_sub(4).max(1);
    render_markdown(body, content_width)
        .into_iter()
        .map(|line| {
            let mut spans = line_with_base_style(line, Style::new().fg(FG).bg(BG_PANEL)).spans;
            spans.insert(0, Span::styled("  ", Style::new().bg(BG_PANEL)));
            spans.push(Span::styled("  ", Style::new().bg(BG_PANEL)));
            Line::from(spans)
        })
        .collect()
}

fn line_with_base_style(mut line: Line<'static>, base: Style) -> Line<'static> {
    for span in &mut line.spans {
        span.style = base.patch(span.style);
    }
    line
}

fn render_detail_metadata(frame: &mut Frame, item: &TaskListItem, area: Rect) {
    let block = Block::new()
        .borders(Borders::LEFT)
        .border_style(Style::new().fg(BORDER))
        .padding(Padding::horizontal(1))
        .style(Style::new().bg(BG));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(Text::from(detail_metadata_lines(item))).style(Style::new().fg(FG).bg(BG)),
        inner,
    );
}

fn detail_metadata_lines(item: &TaskListItem) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(
            " TASK ",
            Style::new().fg(BG).bg(BORDER).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        metadata_label("PROJECT"),
        Line::from(vec![
            Span::styled(
                "● ",
                Style::new().fg(theme::project_color(&item.task.project_key)),
            ),
            Span::styled(
                item.task.project_key.clone(),
                Style::new().fg(FG).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        metadata_label("STATUS"),
        status_chip(&item.task.status),
        Line::from(""),
        metadata_label("PRIORITY"),
        Line::from(Span::styled(
            priority_short(&item.task.priority),
            theme::priority_style(&item.task.priority).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        metadata_label("LABELS"),
        Line::from(labels_display(&item.labels, ", ")),
        Line::from(""),
        metadata_label("REF"),
        Line::from(Span::styled(
            item.display_ref.clone(),
            Style::new().fg(FG).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        metadata_label("CREATED"),
        Line::from(Span::styled(
            item.task.created_at.clone(),
            Style::new().fg(FG_MUTED),
        )),
        Line::from(""),
        metadata_label("UPDATED"),
        Line::from(Span::styled(
            item.task.updated_at.clone(),
            Style::new().fg(FG_MUTED),
        )),
    ];
    if item.has_conflict {
        lines.extend([
            Line::from(""),
            metadata_label("CONFLICTS"),
            Line::from(Span::styled(
                "yes",
                Style::new().fg(ORANGE).add_modifier(Modifier::BOLD),
            )),
        ]);
    }
    if item.task.deleted {
        lines.extend([
            Line::from(""),
            metadata_label("DELETED"),
            Line::from(Span::styled(
                "yes",
                Style::new().fg(RED).add_modifier(Modifier::BOLD),
            )),
        ]);
    }
    lines
}

fn metadata_label(label: &'static str) -> Line<'static> {
    Line::from(Span::styled(
        label,
        Style::new().fg(FG_DIM).add_modifier(Modifier::BOLD),
    ))
}

pub(super) fn render_detail_underlay(
    frame: &mut Frame,
    store: &TuiStore,
    widgets: &mut WidgetState,
    scroll: u16,
) {
    if let Some(task) = store.selected_task(widgets.table.selected()) {
        render_detail(frame, task, scroll);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detail_content_includes_notes() {
        let item = detail_test_item();
        let rendered = detail_content_lines(&item, 60)
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("Fix token refresh race"));
        assert!(rendered.contains("Confirmed race in useTokenRefresh.ts"));
        assert!(rendered.contains("2026-06-20T12:00:00Z"));
    }

    #[test]
    fn detail_content_renders_markdown_description_and_notes() {
        let mut item = detail_test_item();
        item.task.description = "## Context\n- **One** item".to_string();
        item.notes[0].body = "Use `aven show` after edits".to_string();

        let rendered = detail_content_lines(&item, 60)
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("Context"));
        assert!(rendered.contains("- One item"));
        assert!(rendered.contains("aven show"));
    }

    #[test]
    fn detail_description_lines_keep_quote_rail() {
        let mut item = detail_test_item();
        item.task.description = "## Context\nsecond line".to_string();
        let lines = detail_content_lines(&item, 60);
        let description_lines: Vec<_> = lines
            .into_iter()
            .filter(|line| {
                let text = line.to_string();
                text.contains("Context") || text.contains("second")
            })
            .collect();
        assert!(!description_lines.is_empty());
        for line in description_lines {
            assert!(
                line.spans
                    .first()
                    .is_some_and(|span| span.content.as_ref() == "│ "),
                "missing quote rail: {line:?}"
            );
        }
    }

    #[test]
    fn detail_note_lines_keep_card_background() {
        let mut item = detail_test_item();
        item.notes[0].body = "Use `aven` here".to_string();
        let lines = detail_content_lines(&item, 60);
        let note_lines: Vec<_> = lines
            .into_iter()
            .filter(|line| line.to_string().contains("aven"))
            .collect();
        assert_eq!(note_lines.len(), 1);
        let line = &note_lines[0];
        assert!(
            line.spans
                .first()
                .is_some_and(|span| span.style.bg == Some(BG_PANEL)),
            "missing left card padding background"
        );
        assert!(
            line.spans
                .last()
                .is_some_and(|span| span.style.bg == Some(BG_PANEL)),
            "missing right card padding background"
        );
        assert!(
            line.spans
                .iter()
                .any(|span| span.content.as_ref() == "aven" && span.style.bg == Some(BG_PANEL)),
            "note body span missing card background"
        );
    }

    #[test]
    fn detail_metadata_includes_operational_fields() {
        let item = detail_test_item();
        let rendered = detail_metadata_lines(&item)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("PROJECT\n● app"));
        assert!(rendered.contains("STATUS\n● active"));
        assert!(rendered.contains("PRIORITY\n▲ urgent"));
        assert!(rendered.contains("LABELS\nbug, mobile"));
        assert!(rendered.contains("CONFLICTS\nyes"));
    }

    #[test]
    fn detail_scroll_start_is_capped_by_visible_rows() {
        assert_eq!(detail_scroll_start(0, 20, 5), 0);
        assert_eq!(detail_scroll_start(8, 20, 5), 8);
        assert_eq!(detail_scroll_start(30, 20, 5), 15);
        assert_eq!(detail_scroll_start(4, 3, 5), 0);
    }

    #[test]
    fn detail_scrollbar_position_reaches_end_at_last_visible_row() {
        assert_eq!(detail_scrollbar_position(0, 20, 5), 0);
        assert_eq!(detail_scrollbar_position(15, 20, 5), 19);
        assert_eq!(detail_scrollbar_position(0, 3, 5), 0);
    }

    fn detail_test_item() -> TaskListItem {
        TaskListItem {
            task: crate::types::Task {
                id: "7KQ9A1X".to_string(),
                workspace_id: "workspace-1".to_string(),
                title: "Fix token refresh race".to_string(),
                description: "Two token refresh requests fire together.".to_string(),
                project_key: "app".to_string(),
                project_prefix: "APP".to_string(),
                status: "active".to_string(),
                priority: "urgent".to_string(),
                created_at: "2026-06-19T12:00:00Z".to_string(),
                updated_at: "2026-06-20T12:00:00Z".to_string(),
                queue_activity_at: "2026-06-20T12:00:00Z".to_string(),
                deleted: false,
            },
            display_ref: "APP-7KQ9A1X".to_string(),
            labels: vec!["bug".to_string(), "mobile".to_string()],
            notes: vec![crate::query::TaskNote {
                body: "Confirmed race in useTokenRefresh.ts".to_string(),
                created_at: "2026-06-20T12:00:00Z".to_string(),
            }],
            has_conflict: true,
            queue: Default::default(),
        }
    }
}
