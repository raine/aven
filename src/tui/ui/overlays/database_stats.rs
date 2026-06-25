use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};

use super::super::dialog::{Dialog, dialog_hint_line};
use crate::tui::config_overlay::DATABASE_STATS_TITLE;
use crate::tui::store::TuiDatabaseStats;
use crate::tui::theme::{ACCENT, BG_ALT, FG, FG_DIM, FG_MUTED, GREEN, ORANGE};

pub(in crate::tui::ui) fn render_database_stats(
    frame: &mut Frame,
    stats: &TuiDatabaseStats,
    scroll: u16,
) {
    let width = frame.area().width.saturating_sub(8).clamp(72, 86);
    let lines = database_stats_lines(stats);
    let height = frame.area().height.saturating_sub(1).clamp(12, 30);
    let content_rows = height.saturating_sub(2);
    let visible_rows = content_rows.saturating_sub(2) as usize;
    let start = scroll_start(scroll, lines.len(), visible_rows);
    let visible = lines
        .iter()
        .skip(start)
        .take(visible_rows)
        .cloned()
        .collect::<Vec<_>>();
    let dialog = if let Some(title) = scroll_title(scroll, lines.len(), visible_rows) {
        Dialog::new(DATABASE_STATS_TITLE, width, height)
            .right_title(Line::from(Span::styled(title, Style::new().fg(FG_MUTED))))
    } else {
        Dialog::new(DATABASE_STATS_TITLE, width, height)
    };
    let content = dialog.render_block(frame);
    let stats_area = Rect {
        height: content.height.saturating_sub(2),
        ..content
    };
    let hint_area = Rect {
        y: content.y + content.height.saturating_sub(1),
        height: 1,
        ..content
    };

    frame.render_widget(
        Paragraph::new(Text::from(visible)).style(Style::new().fg(FG).bg(BG_ALT)),
        stats_area,
    );
    frame.render_widget(
        Paragraph::new(dialog_hint_line(&[
            ("j/k", "scroll"),
            ("Enter/Esc", "close"),
        ]))
        .style(Style::new().fg(FG).bg(BG_ALT)),
        hint_area,
    );
    render_scrollbar(frame, stats_area, lines.len(), scroll);
}

fn database_stats_lines(stats: &TuiDatabaseStats) -> Vec<Line<'static>> {
    let sections = [
        section(
            "workspace",
            vec![
                value_row("name", stats.workspace_name.clone(), Style::new().fg(FG)),
                value_row(
                    "key",
                    stats.workspace_key.clone(),
                    Style::new().fg(FG_MUTED),
                ),
            ],
        ),
        section(
            "tasks",
            vec![
                value_row(
                    "total",
                    stats.total_tasks.to_string(),
                    Style::new().fg(FG_MUTED),
                ),
                value_row(
                    "open",
                    stats.open_tasks.to_string(),
                    Style::new().fg(FG_MUTED),
                ),
                value_row(
                    "deleted",
                    stats.deleted_tasks.to_string(),
                    Style::new().fg(FG_MUTED),
                ),
            ],
        ),
        section(
            "statuses",
            vec![
                value_row(
                    "inbox",
                    stats.statuses.inbox.to_string(),
                    Style::new().fg(FG_MUTED),
                ),
                value_row(
                    "backlog",
                    stats.statuses.backlog.to_string(),
                    Style::new().fg(FG_MUTED),
                ),
                value_row(
                    "todo",
                    stats.statuses.todo.to_string(),
                    Style::new().fg(FG_MUTED),
                ),
                value_row(
                    "active",
                    stats.statuses.active.to_string(),
                    Style::new().fg(FG_MUTED),
                ),
                value_row(
                    "done",
                    stats.statuses.done.to_string(),
                    Style::new().fg(FG_MUTED),
                ),
                value_row(
                    "canceled",
                    stats.statuses.canceled.to_string(),
                    Style::new().fg(FG_MUTED),
                ),
            ],
        ),
        section(
            "priorities",
            vec![
                value_row(
                    "none",
                    stats.priorities.none.to_string(),
                    Style::new().fg(FG_MUTED),
                ),
                value_row(
                    "low",
                    stats.priorities.low.to_string(),
                    Style::new().fg(FG_MUTED),
                ),
                value_row(
                    "medium",
                    stats.priorities.medium.to_string(),
                    Style::new().fg(FG_MUTED),
                ),
                value_row(
                    "high",
                    stats.priorities.high.to_string(),
                    Style::new().fg(FG_MUTED),
                ),
                value_row(
                    "urgent",
                    stats.priorities.urgent.to_string(),
                    Style::new().fg(FG_MUTED),
                ),
            ],
        ),
        section(
            "related rows",
            vec![
                value_row(
                    "projects",
                    stats.projects.to_string(),
                    Style::new().fg(FG_MUTED),
                ),
                value_row(
                    "labels",
                    stats.labels.to_string(),
                    Style::new().fg(FG_MUTED),
                ),
                value_row("notes", stats.notes.to_string(), Style::new().fg(FG_MUTED)),
                value_row(
                    "task labels",
                    stats.task_labels.to_string(),
                    Style::new().fg(FG_MUTED),
                ),
            ],
        ),
        section(
            "sync rows",
            vec![
                count_row("pending all", stats.pending_changes),
                count_row("conflicts", stats.conflicts),
            ],
        ),
        section(
            "sqlite",
            vec![
                value_row(
                    "page size",
                    format!("{} bytes", stats.sqlite_page_size),
                    Style::new().fg(FG_MUTED),
                ),
                value_row(
                    "pages",
                    stats.sqlite_page_count.to_string(),
                    Style::new().fg(FG_MUTED),
                ),
                value_row(
                    "free pages",
                    stats.sqlite_freelist_count.to_string(),
                    Style::new().fg(FG_MUTED),
                ),
                value_row(
                    "main db size",
                    format_bytes(stats.sqlite_page_size * stats.sqlite_page_count),
                    Style::new().fg(FG_MUTED),
                ),
                value_row(
                    "free",
                    format_bytes(stats.sqlite_page_size * stats.sqlite_freelist_count),
                    Style::new().fg(FG_MUTED),
                ),
            ],
        ),
        section(
            "latest task timestamps",
            vec![
                value_row(
                    "created",
                    stats.latest_created_at.as_deref().unwrap_or("none"),
                    Style::new().fg(FG_MUTED),
                ),
                value_row(
                    "updated",
                    stats.latest_updated_at.as_deref().unwrap_or("none"),
                    Style::new().fg(FG_MUTED),
                ),
            ],
        ),
    ];

    let mut lines = Vec::new();
    for (pair_index, pair) in sections.chunks(2).enumerate() {
        let left = &pair[0];
        let right = pair.get(1);
        let rows = left
            .lines
            .len()
            .max(right.map_or(0, |section| section.lines.len()));
        for index in 0..rows {
            lines.push(two_column_line(
                left.lines.get(index),
                right.and_then(|section| section.lines.get(index)),
            ));
        }
        if pair_index + 1 < sections.len().div_ceil(2) {
            lines.push(Line::from(""));
        }
    }
    lines
}

struct StatsSection {
    lines: Vec<Line<'static>>,
}

fn section(title: &str, rows: Vec<Line<'static>>) -> StatsSection {
    let mut lines = vec![section_line(title)];
    lines.extend(rows);
    StatsSection { lines }
}

fn two_column_line(left: Option<&Line<'static>>, right: Option<&Line<'static>>) -> Line<'static> {
    let mut spans = Vec::new();
    if let Some(line) = left {
        spans.extend(line.spans.clone());
    }
    spans.push(Span::raw(
        " ".repeat(34usize.saturating_sub(line_width(left))),
    ));
    if let Some(line) = right {
        spans.extend(line.spans.clone());
    }
    Line::from(spans)
}

fn line_width(line: Option<&Line<'static>>) -> usize {
    line.map(|line| {
        line.spans
            .iter()
            .map(|span| span.content.chars().count())
            .sum()
    })
    .unwrap_or(0)
}

fn section_line(label: &str) -> Line<'static> {
    Line::from(Span::styled(
        label.to_ascii_uppercase(),
        Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
    ))
}

fn count_row(label: &str, value: i64) -> Line<'static> {
    let style = if value > 0 {
        Style::new().fg(ORANGE)
    } else {
        Style::new().fg(GREEN)
    };
    value_row(label, value.to_string(), style)
}

fn value_row(label: &str, value: impl Into<String>, value_style: Style) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<14}"), Style::new().fg(FG_DIM)),
        Span::styled(value.into(), value_style),
    ])
}

fn render_scrollbar(frame: &mut Frame, area: Rect, content_height: usize, scroll: u16) {
    let visible_rows = area.height as usize;
    if content_height > visible_rows {
        let start = scroll_start(scroll, content_height, visible_rows);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .style(Style::new().fg(FG_DIM).bg(BG_ALT))
                .thumb_style(Style::new().fg(FG_MUTED)),
            area,
            &mut ScrollbarState::new(content_height)
                .position(scrollbar_position(
                    start,
                    content_height,
                    visible_rows.max(1),
                ))
                .viewport_content_length(visible_rows),
        );
    }
}

fn scroll_start(scroll: u16, content_height: usize, visible_rows: usize) -> usize {
    let max_scroll = content_height.saturating_sub(visible_rows);
    (scroll as usize).min(max_scroll)
}

fn scrollbar_position(start: usize, content_height: usize, visible_rows: usize) -> usize {
    let max_start = content_height.saturating_sub(visible_rows);
    start
        .saturating_mul(content_height.saturating_sub(1))
        .checked_div(max_start)
        .unwrap_or(0)
}

fn scroll_title(scroll: u16, content_height: usize, visible_rows: usize) -> Option<String> {
    if content_height <= visible_rows {
        return None;
    }
    let total = content_height
        .saturating_sub(visible_rows)
        .saturating_add(1);
    let current = scroll_start(scroll, content_height, visible_rows)
        .saturating_add(1)
        .min(total);
    Some(format!(" {current}/{total} "))
}

pub(crate) fn database_stats_scroll_cap(frame_height: u16) -> u16 {
    let content_rows = frame_height
        .saturating_sub(1)
        .clamp(12, 30)
        .saturating_sub(2);
    let visible_rows = content_rows.saturating_sub(2) as usize;
    database_stats_lines(&TuiDatabaseStats::default())
        .len()
        .saturating_sub(visible_rows) as u16
}

fn format_bytes(bytes: i64) -> String {
    const KIB: i64 = 1024;
    const MIB: i64 = KIB * 1024;
    const GIB: i64 = MIB * 1024;
    const TIB: i64 = GIB * 1024;
    if bytes >= TIB {
        format!("{:.1} TiB", bytes as f64 / TIB as f64)
    } else if bytes >= GIB {
        format!("{:.1} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::format_bytes;

    #[test]
    fn formats_byte_counts() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.0 KiB");
        assert_eq!(format_bytes(1024 * 1024), "1.0 MiB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.0 GiB");
    }
}
