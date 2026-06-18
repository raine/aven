use ratatui::style::Style;
use ratatui::style::palette::tailwind;
use ratatui::text::{Line, Span};

use crate::query::TaskListItem;

pub(crate) fn priority_short(priority: &str) -> &'static str {
    match priority {
        "urgent" => "U",
        "high" => "H",
        "medium" => "M",
        "low" => "L",
        _ => " ",
    }
}

pub(crate) fn title_cell(item: &TaskListItem) -> Line<'static> {
    let marker = if item.has_conflict { "! " } else { "" };
    let deleted = if item.task.deleted { "deleted " } else { "" };
    Line::from(vec![
        Span::styled(marker.to_string(), Style::new().fg(tailwind::YELLOW.c400)),
        Span::styled(deleted.to_string(), Style::new().fg(tailwind::RED.c400)),
        Span::raw(item.task.title.clone()),
    ])
}
