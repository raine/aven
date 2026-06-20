use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::query::TaskListItem;
use crate::tui::theme::{self, FG, ORANGE, RED};

pub(crate) fn priority_icon(priority: &str) -> &'static str {
    match priority {
        "urgent" => "▲",
        "high" => "●",
        "medium" => "◐",
        "low" => "◌",
        _ => "─",
    }
}

pub(crate) fn priority_short(priority: &str) -> &'static str {
    match priority {
        "urgent" => "▲ urgent",
        "high" => "● high",
        "medium" => "◐ med",
        "low" => "◌ low",
        _ => "─ none",
    }
}

pub(crate) fn status_chip(status: &str) -> Line<'static> {
    Line::from(status_span(status))
}

pub(crate) fn status_span(status: &str) -> Span<'static> {
    let label = match status {
        "active" => "● active",
        "todo" => "□ todo",
        "inbox" => "▣ inbox",
        "backlog" => "◌ back",
        "done" => "✓ done",
        "canceled" => "× cancel",
        _ => status,
    };
    Span::styled(
        format!(" {label} "),
        theme::status_style(status).add_modifier(Modifier::BOLD),
    )
}

pub(crate) fn title_cell(item: &TaskListItem, max_width: usize) -> Line<'static> {
    let marker = if item.has_conflict { "⚡ " } else { "" };
    let deleted = if item.task.deleted { "deleted " } else { "" };
    let content_width = max_width.saturating_sub(1);
    let prefix_width = marker.chars().count() + deleted.chars().count();
    let title_width = content_width.saturating_sub(prefix_width);
    let spans = vec![
        Span::styled(marker.to_string(), Style::new().fg(ORANGE)),
        Span::styled(deleted.to_string(), Style::new().fg(RED)),
        Span::styled(
            truncate_title(&item.task.title, title_width),
            Style::new().fg(FG).add_modifier(Modifier::BOLD),
        ),
    ];
    Line::from(spans)
}

fn truncate_title(title: &str, max_width: usize) -> String {
    let title_len = title.chars().count();
    if title_len <= max_width {
        return title.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    if max_width == 1 {
        return "…".to_string();
    }
    let mut truncated = title.chars().take(max_width - 1).collect::<String>();
    truncated.push('…');
    truncated
}
