use std::cell::Cell;

use ratatui::style::{Color, Modifier, Style};

/// The terminal background the palette must stay readable on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Background {
    #[default]
    Dark,
    Light,
}

impl Background {
    pub(crate) const fn opposite(self) -> Self {
        match self {
            Self::Dark => Self::Light,
            Self::Light => Self::Dark,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Palette {
    pub(crate) default_bg: Color,
    pub(crate) bg_alt: Color,
    pub(crate) bg_panel: Color,
    pub(crate) bg_lane: Color,
    pub(crate) bg_lane_active: Color,
    pub(crate) lane_divider: Color,
    pub(crate) fg: Color,
    pub(crate) fg_muted: Color,
    pub(crate) fg_dim: Color,
    pub(crate) border: Color,
    pub(crate) selected_bg: Color,
    pub(crate) accent: Color,
    pub(crate) accent_strong: Color,
    pub(crate) blue: Color,
    pub(crate) cyan: Color,
    pub(crate) orange: Color,
    pub(crate) custom_command_name: Color,
    pub(crate) custom_command_tag: Color,
    pub(crate) red: Color,
    pub(crate) pink: Color,
    pub(crate) yellow: Color,
    pub(crate) purple: Color,
    pub(crate) green: Color,
    pub(crate) badge_bg: Color,
    pub(crate) project_colors: [Color; 14],
    pub(crate) splash: SplashColors,
}

/// Colors for the onboarding splash logo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SplashColors {
    /// The logo gradient runs from top to middle to bottom.
    pub(crate) gradient: [Color; 3],
    pub(crate) check: Color,
    pub(crate) afterglow: Color,
    /// Dimmed logo colors move toward this color.
    pub(crate) dim_toward: Color,
}

/// A theme holds one palette for each terminal background.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Theme {
    pub(crate) dark: Palette,
    pub(crate) light: Palette,
}

impl Theme {
    pub(crate) const DEFAULT: Self = Self {
        dark: DARK,
        light: LIGHT,
    };

    pub(crate) const fn palette(&self, background: Background) -> Palette {
        match background {
            Background::Dark => self.dark,
            Background::Light => self.light,
        }
    }
}

const DARK: Palette = Palette {
    default_bg: Color::Rgb(18, 19, 18),
    bg_alt: Color::Rgb(34, 35, 33),
    bg_panel: Color::Rgb(39, 40, 38),
    bg_lane: Color::Rgb(24, 25, 24),
    bg_lane_active: Color::Rgb(27, 26, 32),
    lane_divider: Color::Rgb(49, 50, 47),
    fg: Color::Rgb(239, 238, 232),
    fg_muted: Color::Rgb(191, 188, 180),
    fg_dim: Color::Rgb(147, 145, 138),
    border: Color::Rgb(88, 88, 83),
    selected_bg: Color::Rgb(50, 45, 78),
    accent: Color::Rgb(166, 139, 255),
    accent_strong: Color::Rgb(194, 174, 255),
    blue: Color::Rgb(70, 128, 203),
    cyan: Color::Rgb(133, 222, 255),
    orange: Color::Rgb(244, 166, 54),
    custom_command_name: Color::Rgb(224, 151, 92),
    custom_command_tag: Color::Rgb(190, 126, 82),
    red: Color::Rgb(239, 82, 86),
    pink: Color::Rgb(225, 91, 139),
    yellow: Color::Rgb(255, 207, 87),
    purple: Color::Rgb(137, 124, 232),
    green: Color::Rgb(137, 199, 82),
    badge_bg: Color::Rgb(55, 56, 52),
    project_colors: [
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
    ],
    splash: SplashColors {
        gradient: [
            Color::Rgb(135, 40, 250),
            Color::Rgb(150, 63, 255),
            Color::Rgb(176, 108, 255),
        ],
        check: Color::Rgb(255, 255, 255),
        afterglow: Color::Rgb(232, 213, 255),
        dim_toward: Color::Rgb(0, 0, 0),
    },
};

const LIGHT: Palette = Palette {
    default_bg: Color::Rgb(250, 250, 247),
    bg_alt: Color::Rgb(236, 236, 231),
    bg_panel: Color::Rgb(229, 229, 223),
    bg_lane: Color::Rgb(244, 244, 240),
    bg_lane_active: Color::Rgb(238, 236, 247),
    lane_divider: Color::Rgb(213, 213, 207),
    fg: Color::Rgb(28, 29, 27),
    fg_muted: Color::Rgb(84, 86, 82),
    fg_dim: Color::Rgb(126, 128, 122),
    border: Color::Rgb(174, 174, 166),
    selected_bg: Color::Rgb(219, 211, 248),
    accent: Color::Rgb(104, 72, 220),
    accent_strong: Color::Rgb(78, 44, 188),
    blue: Color::Rgb(36, 96, 176),
    cyan: Color::Rgb(0, 118, 148),
    orange: Color::Rgb(196, 112, 8),
    custom_command_name: Color::Rgb(178, 96, 34),
    custom_command_tag: Color::Rgb(150, 86, 44),
    red: Color::Rgb(200, 44, 50),
    pink: Color::Rgb(190, 48, 104),
    yellow: Color::Rgb(168, 124, 0),
    purple: Color::Rgb(96, 82, 200),
    green: Color::Rgb(52, 128, 24),
    badge_bg: Color::Rgb(218, 218, 211),
    project_colors: [
        Color::Rgb(118, 64, 212),
        Color::Rgb(16, 138, 104),
        Color::Rgb(184, 112, 8),
        Color::Rgb(200, 62, 40),
        Color::Rgb(184, 48, 112),
        Color::Rgb(84, 140, 24),
        Color::Rgb(24, 112, 192),
        Color::Rgb(168, 124, 0),
        Color::Rgb(16, 136, 140),
        Color::Rgb(136, 80, 212),
        Color::Rgb(196, 76, 40),
        Color::Rgb(40, 136, 60),
        Color::Rgb(56, 92, 208),
        Color::Rgb(176, 56, 156),
    ],
    splash: SplashColors {
        gradient: [
            Color::Rgb(135, 40, 250),
            Color::Rgb(150, 63, 255),
            Color::Rgb(176, 108, 255),
        ],
        check: Color::Rgb(40, 16, 72),
        afterglow: Color::Rgb(88, 28, 196),
        dim_toward: Color::Rgb(250, 250, 247),
    },
};

thread_local! {
    static ACTIVE: Cell<Palette> = const { Cell::new(DARK) };
}

/// Makes `palette` the one the color accessors return on this thread.
/// The UI calls this once per frame, before it draws anything.
pub(crate) fn activate(palette: Palette) {
    ACTIVE.with(|active| active.set(palette));
}

pub(crate) fn palette() -> Palette {
    ACTIVE.with(Cell::get)
}

macro_rules! palette_colors {
    ($($name:ident),* $(,)?) => {
        $(
            pub(crate) fn $name() -> Color {
                ACTIVE.with(|active| active.get().$name)
            }
        )*
    };
}

palette_colors!(
    default_bg,
    bg_alt,
    bg_panel,
    bg_lane,
    bg_lane_active,
    lane_divider,
    fg,
    fg_muted,
    fg_dim,
    border,
    selected_bg,
    accent,
    accent_strong,
    blue,
    cyan,
    orange,
    custom_command_name,
    custom_command_tag,
    red,
    pink,
    yellow,
    purple,
    green,
    badge_bg,
);

pub(crate) fn splash() -> SplashColors {
    ACTIVE.with(|active| active.get().splash)
}

/// The main surface uses the terminal's own background in both palettes.
pub(crate) const BG: Color = Color::Reset;

pub(crate) fn inverse_fg() -> Color {
    default_bg()
}

pub(crate) fn selected() -> Style {
    Style::new()
        .fg(fg())
        .bg(selected_bg())
        .add_modifier(Modifier::BOLD)
}

pub(crate) fn related() -> Style {
    Style::new().fg(fg()).bg(bg_lane_active())
}

pub(crate) fn selected_inactive() -> Style {
    Style::new().fg(fg_muted()).bg(bg_panel())
}

pub(crate) fn priority_style(priority: &str) -> Style {
    let color = match priority {
        "urgent" => red(),
        "high" => orange(),
        "medium" => purple(),
        "low" => fg_dim(),
        _ => border(),
    };
    Style::new().fg(color)
}

pub(crate) fn status_style(status: &str) -> Style {
    let color = match status {
        "active" => accent(),
        "todo" => blue(),
        "inbox" => fg_dim(),
        "backlog" => fg_muted(),
        "done" => green(),
        "canceled" => red(),
        _ => fg_dim(),
    };
    Style::new().fg(color)
}

pub(crate) fn project_color(key: &str) -> Color {
    let hash = key
        .bytes()
        .fold(5381usize, |acc, byte| acc.wrapping_mul(33) ^ byte as usize);
    let colors = palette().project_colors;
    colors[hash % colors.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activate_switches_the_colors_the_accessors_return() {
        activate(Theme::DEFAULT.palette(Background::Light));
        assert_eq!(fg(), LIGHT.fg);
        let light_project = project_color("app");
        assert!(LIGHT.project_colors.contains(&light_project));

        activate(Theme::DEFAULT.palette(Background::Dark));
        assert_eq!(fg(), DARK.fg);
        assert!(DARK.project_colors.contains(&project_color("app")));
        assert_ne!(project_color("app"), light_project);
        assert_eq!(selected().bg, Some(DARK.selected_bg));
    }

    #[test]
    fn dark_is_the_default_background() {
        assert_eq!(Background::default(), Background::Dark);
        assert_eq!(palette(), DARK);
    }
}
