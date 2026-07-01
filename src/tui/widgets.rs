use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::query::TaskListItem;
use crate::queue::unix_seconds;
use crate::tui::theme::{self, FG, FG_DIM, FG_MUTED, GREEN, ORANGE, RED};

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
        label.to_string(),
        theme::status_style(status).add_modifier(Modifier::BOLD),
    )
}

pub(crate) fn age_style(created_at: &str, now_seconds: i64) -> Style {
    let days = unix_seconds(created_at)
        .map(|created| {
            now_seconds
                .saturating_sub(created)
                .max(0)
                .saturating_div(86_400)
        })
        .unwrap_or(0);
    let color = if days < 1 {
        GREEN
    } else if days < 4 {
        FG_MUTED
    } else if days < 8 {
        FG
    } else if days < 15 {
        ORANGE
    } else {
        RED
    };
    Style::new().fg(color)
}

pub(crate) fn title_cell(item: &TaskListItem, max_width: usize) -> Line<'static> {
    let marker = if item.has_conflict { "⚡ " } else { "" };
    let content_width = max_width.saturating_sub(1);
    let title_style = if item.task.deleted {
        Style::new()
            .fg(FG_MUTED)
            .add_modifier(Modifier::CROSSED_OUT)
    } else {
        Style::new().fg(FG)
    };
    let marker_width = marker.chars().count();
    let title_width = content_width.saturating_sub(marker_width);
    let title = truncate_title(&item.task.title, title_width);
    Line::from(vec![
        Span::styled(marker.to_string(), Style::new().fg(ORANGE)),
        Span::styled(title, title_style),
    ])
}

pub(crate) fn label_cell(labels: &[String], max_width: usize) -> Line<'static> {
    if max_width == 0 {
        return Line::from("");
    }
    let text = label_summary_text(labels);
    let text_width = text.chars().count();
    if text.is_empty() || text_width > max_width {
        return Line::from("");
    }
    let trailing_gap = usize::from(text_width < max_width);
    let padding = max_width.saturating_sub(text_width + trailing_gap);
    let mut spans = vec![
        Span::raw(" ".repeat(padding)),
        Span::styled(text, Style::new().fg(FG_DIM)),
    ];
    if trailing_gap > 0 {
        spans.push(Span::raw(" "));
    }
    Line::from(spans)
}

fn label_summary_text(labels: &[String]) -> String {
    let Some(first) = labels.first() else {
        return String::new();
    };
    let more = labels.len().saturating_sub(1);
    if more == 0 {
        first.clone()
    } else {
        format!("{first} +{more}")
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::choices::{TaskPriority, TaskStatus};

    fn task_item(title: &str) -> TaskListItem {
        TaskListItem {
            task: crate::types::Task {
                id: "task-1".to_string(),
                workspace_id: "workspace-1".to_string(),
                title: title.to_string(),
                description: String::new(),
                project_id: "project-id".to_string(),
                project_key: "app".to_string(),
                project_prefix: "APP".to_string(),
                status: TaskStatus::Todo,
                priority: TaskPriority::None,
                created_at: "2026-06-20T00:00:00Z".to_string(),
                updated_at: "2026-06-20T00:00:00Z".to_string(),
                queue_activity_at: "2026-06-20T00:00:00Z".to_string(),
                deleted: false,
            },
            display_ref: "APP-1".to_string(),
            labels: Vec::new(),
            notes: Vec::new(),
            has_conflict: false,
            unresolved_blocker_count: 0,
            dependent_count: 0,
            depends_on: Vec::new(),
            blocks: Vec::new(),
            queue: Default::default(),
        }
    }

    #[test]
    fn label_cell_right_aligns_summary_when_space_allows() {
        let mut item = task_item("Short title");
        item.labels = vec!["search".to_string(), "ux".to_string()];

        let rendered = label_cell(&item.labels, 11).to_string();

        assert_eq!(rendered, " search +1 ");
        assert_eq!(rendered.chars().count(), 11);
    }

    #[test]
    fn label_cell_uses_dim_text() {
        let mut item = task_item("Short title");
        item.labels = vec!["search".to_string()];

        let line = label_cell(&item.labels, 6);
        let rendered = line.to_string();

        assert_eq!(rendered, "search");
        let label = line
            .spans
            .iter()
            .find(|span| span.content == "search")
            .unwrap();
        assert_eq!(label.style.fg, Some(FG_DIM));
        assert_eq!(label.style.bg, None);
        assert!(!label.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn label_cell_hides_labels_when_space_is_tight() {
        let mut item = task_item("Short title");
        item.labels = vec!["search".to_string()];

        let rendered = label_cell(&item.labels, 5).to_string();

        assert_eq!(rendered, "");
    }

    #[test]
    fn title_cell_truncates_title_when_labels_are_hidden() {
        let mut item = task_item("A very long task title");
        item.labels = vec!["search".to_string()];

        let rendered = title_cell(&item, 12).to_string();

        assert_eq!(rendered, "A very lon…");
    }
}
