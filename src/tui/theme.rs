use ratatui::style::palette::tailwind;
use ratatui::style::{Color, Modifier, Style};

pub(crate) const BG: Color = tailwind::SLATE.c950;
pub(crate) const BG_ALT: Color = tailwind::SLATE.c900;
pub(crate) const FG: Color = tailwind::SLATE.c200;
pub(crate) const FG_DIM: Color = tailwind::SLATE.c500;
pub(crate) const ACCENT: Color = tailwind::BLUE.c500;
pub(crate) const BORDER: Color = tailwind::SLATE.c700;
pub(crate) const SELECTED: Style = Style::new()
    .bg(tailwind::SLATE.c800)
    .add_modifier(Modifier::BOLD);

pub(crate) fn priority_style(priority: &str) -> Style {
    let color = match priority {
        "urgent" => tailwind::RED.c500,
        "high" => tailwind::ORANGE.c500,
        "medium" => tailwind::YELLOW.c500,
        "low" => tailwind::SLATE.c400,
        _ => tailwind::SLATE.c600,
    };
    Style::new().fg(color)
}

pub(crate) fn status_style(status: &str) -> Style {
    let color = match status {
        "active" => tailwind::GREEN.c500,
        "todo" => tailwind::BLUE.c500,
        "inbox" => tailwind::SLATE.c400,
        "backlog" => tailwind::GRAY.c500,
        "done" => tailwind::EMERALD.c600,
        "canceled" => tailwind::RED.c400,
        _ => FG_DIM,
    };
    Style::new().fg(color)
}
