use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::query::TaskListItem;
use crate::tui::theme::{BLUE, CHIP_BG, FG, GREEN, ORANGE, PURPLE, RED};

pub(crate) fn priority_short(priority: &str) -> &'static str {
    match priority {
        "urgent" => "▲ urgent",
        "high" => "↑ high",
        "medium" => "◆ med",
        "low" => "─ low",
        _ => "· none",
    }
}

pub(crate) fn title_cell(item: &TaskListItem) -> Line<'static> {
    let marker = if item.has_conflict { "⚡ " } else { "" };
    let deleted = if item.task.deleted { "deleted " } else { "" };
    let spans = vec![
        Span::styled(marker.to_string(), Style::new().fg(ORANGE)),
        Span::styled(deleted.to_string(), Style::new().fg(RED)),
        Span::styled(
            item.task.title.clone(),
            Style::new().fg(FG).add_modifier(Modifier::BOLD),
        ),
    ];
    Line::from(spans)
}

pub(crate) fn label_pill(label: &str) -> Span<'static> {
    let color = match label {
        "bug" => RED,
        "cleanup" => FG,
        "agent" => PURPLE,
        "docs" => GREEN,
        _ => BLUE,
    };
    Span::styled(
        format!(" {label} "),
        Style::new()
            .fg(color)
            .bg(CHIP_BG)
            .add_modifier(Modifier::BOLD),
    )
}
