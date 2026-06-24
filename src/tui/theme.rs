use ratatui::style::{Color, Modifier, Style};

const DEFAULT_BG: Color = Color::Rgb(18, 19, 18);
pub(crate) const INVERSE_FG: Color = DEFAULT_BG;
pub(crate) const BG_ALT: Color = Color::Rgb(34, 35, 33);
pub(crate) const BG_PANEL: Color = Color::Rgb(39, 40, 38);
pub(crate) const FG: Color = Color::Rgb(239, 238, 232);
pub(crate) const FG_MUTED: Color = Color::Rgb(191, 188, 180);
pub(crate) const FG_DIM: Color = Color::Rgb(147, 145, 138);
pub(crate) const BORDER: Color = Color::Rgb(88, 88, 83);
pub(crate) const SELECTED_BG: Color = Color::Rgb(50, 45, 78);
pub(crate) const ACCENT: Color = Color::Rgb(166, 139, 255);
pub(crate) const BLUE: Color = Color::Rgb(70, 128, 203);
pub(crate) const ORANGE: Color = Color::Rgb(244, 166, 54);
pub(crate) const RED: Color = Color::Rgb(239, 82, 86);
pub(crate) const PINK: Color = Color::Rgb(225, 91, 139);
pub(crate) const PURPLE: Color = Color::Rgb(137, 124, 232);
pub(crate) const GREEN: Color = Color::Rgb(137, 199, 82);
const PROJECT_COLORS: [Color; 14] = [
    Color::Rgb(174, 127, 255),
    Color::Rgb(60, 203, 162),
    Color::Rgb(255, 177, 74),
    Color::Rgb(255, 116, 92),
    Color::Rgb(242, 112, 166),
    Color::Rgb(149, 213, 85),
    Color::Rgb(92, 181, 255),
    Color::Rgb(255, 207, 87),
    Color::Rgb(120, 223, 225),
    Color::Rgb(199, 143, 255),
    Color::Rgb(255, 139, 104),
    Color::Rgb(126, 220, 135),
    Color::Rgb(123, 156, 255),
    Color::Rgb(232, 128, 214),
];
pub(crate) const SELECTED: Style = Style::new()
    .fg(FG)
    .bg(SELECTED_BG)
    .add_modifier(Modifier::BOLD);
pub(crate) const SELECTED_INACTIVE: Style = Style::new().fg(FG_MUTED).bg(BG_PANEL);

pub(crate) const BG: Color = Color::Reset;

pub(crate) fn priority_style(priority: &str) -> Style {
    let color = match priority {
        "urgent" => RED,
        "high" => ORANGE,
        "medium" => PURPLE,
        "low" => FG_DIM,
        _ => BORDER,
    };
    Style::new().fg(color)
}

pub(crate) fn status_style(status: &str) -> Style {
    let color = match status {
        "active" => ACCENT,
        "todo" => BLUE,
        "inbox" => FG_DIM,
        "backlog" => FG_MUTED,
        "done" => GREEN,
        "canceled" => RED,
        _ => FG_DIM,
    };
    Style::new().fg(color)
}

pub(crate) fn project_color(key: &str) -> Color {
    let hash = key
        .bytes()
        .fold(5381usize, |acc, byte| acc.wrapping_mul(33) ^ byte as usize);
    PROJECT_COLORS[hash % PROJECT_COLORS.len()]
}
