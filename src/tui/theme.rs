use ratatui::style::{Color, Modifier, Style};

/// Colors the TUI draws with, named by what each color is for.
///
/// `App` owns the active theme and render code receives it as `&Theme`.
/// Composite styles are methods over the base roles, so changing a base role
/// also changes every composite built from it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Theme {
    /// Main surface. `Color::Reset` inherits the terminal background.
    pub(crate) bg: Color,
    /// Raised surfaces: dialogs, overlays, kanban cards, sidebar headers.
    pub(crate) bg_raised: Color,
    /// Secondary panels and unfocused selection.
    pub(crate) bg_panel: Color,
    /// Focused selection.
    pub(crate) bg_selected: Color,
    /// Rows marked for a bulk action.
    pub(crate) bg_row_marked: Color,
    /// Group heading rows in task lists.
    pub(crate) bg_row_group: Color,
    /// Kanban lane.
    pub(crate) bg_lane: Color,
    /// Emphasized kanban lane and rows related to the selection.
    pub(crate) bg_lane_active: Color,
    pub(crate) lane_divider: Color,

    pub(crate) fg: Color,
    pub(crate) fg_muted: Color,
    pub(crate) fg_dim: Color,
    /// Text drawn on a colored fill, such as badges and inverse labels.
    pub(crate) inverse_fg: Color,

    /// Borders, separators, rules, and tree rails.
    pub(crate) border: Color,
    pub(crate) scrollbar_track: Color,
    pub(crate) table_header_bg: Color,
    /// Fill behind neutral badges and unselected chips.
    pub(crate) badge_bg: Color,
    /// Fill behind the changelog's installed-version badge.
    pub(crate) badge_installed_bg: Color,

    pub(crate) accent: Color,
    pub(crate) info: Color,
    pub(crate) success: Color,
    pub(crate) warning: Color,
    pub(crate) danger: Color,
    /// Due today, epics, marks, and other things that want a second look.
    pub(crate) attention: Color,
    pub(crate) conflict: Color,

    /// Foreground of inline code and commands inside plain text.
    pub(crate) code: Color,
    pub(crate) link: Color,
    pub(crate) status_todo: Color,
    pub(crate) priority_medium: Color,
    pub(crate) priority_none: Color,

    pub(crate) custom_command_name: Color,
    pub(crate) custom_command_tag: Color,

    /// Label of the focused field in the add-task form.
    pub(crate) field_label_active: Color,
    /// Validation errors inside form fields.
    pub(crate) field_error: Color,
    /// Task labels in the add-task summary.
    pub(crate) task_label: Color,

    /// Vertical gradient stops of the splash logo, top to bottom.
    pub(crate) splash_gradient: [Color; 3],
    /// Check mark in the splash logo.
    pub(crate) splash_check: Color,
    /// Check mark color when the splash is fully dimmed.
    pub(crate) splash_check_dimmed: Color,
    /// Highlight of the ring that sweeps over the splash logo.
    pub(crate) splash_afterglow: Color,

    /// Project colors, picked by a stable hash of the project key.
    pub(crate) project_palette: [Color; 14],

    /// Text inside markdown block quotes.
    pub(crate) quote: Color,

    /// Color that dimmed surfaces recede toward, such as the underlay of a
    /// modal dialog and the dimmed splash logo.
    pub(crate) dim_target: Color,
}

/// Built-in themes, matched to the terminal background they stay readable on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Appearance {
    #[default]
    Dark,
    Light,
}

impl Appearance {
    pub(crate) const fn theme(self) -> Theme {
        match self {
            Self::Dark => Theme::dark(),
            Self::Light => Theme::light(),
        }
    }

    pub(crate) const fn toggled(self) -> Self {
        match self {
            Self::Dark => Self::Light,
            Self::Light => Self::Dark,
        }
    }
}

impl Theme {
    pub(crate) const fn dark() -> Self {
        let bg_raised = Color::Rgb(34, 35, 33);
        let border = Color::Rgb(88, 88, 83);
        let blue = Color::Rgb(70, 128, 203);
        Self {
            bg: Color::Reset,
            bg_raised,
            bg_panel: Color::Rgb(39, 40, 38),
            bg_selected: Color::Rgb(50, 45, 78),
            bg_row_marked: bg_raised,
            bg_row_group: bg_raised,
            bg_lane: Color::Rgb(24, 25, 24),
            bg_lane_active: Color::Rgb(27, 26, 32),
            lane_divider: Color::Rgb(49, 50, 47),

            fg: Color::Rgb(239, 238, 232),
            fg_muted: Color::Rgb(191, 188, 180),
            fg_dim: Color::Rgb(147, 145, 138),
            inverse_fg: Color::Rgb(18, 19, 18),

            border,
            scrollbar_track: border,
            table_header_bg: border,
            badge_bg: border,
            badge_installed_bg: Color::Rgb(55, 56, 52),

            accent: Color::Rgb(166, 139, 255),
            info: blue,
            success: Color::Rgb(137, 199, 82),
            warning: Color::Rgb(244, 166, 54),
            danger: Color::Rgb(239, 82, 86),
            attention: Color::Rgb(255, 207, 87),
            conflict: Color::Rgb(225, 91, 139),

            code: blue,
            link: blue,
            status_todo: blue,
            priority_medium: Color::Rgb(137, 124, 232),
            priority_none: border,

            custom_command_name: Color::Rgb(224, 151, 92),
            custom_command_tag: Color::Rgb(190, 126, 82),

            field_label_active: Color::Rgb(194, 174, 255),
            field_error: Color::Red,
            task_label: Color::Rgb(133, 222, 255),

            splash_gradient: [
                Color::Rgb(135, 40, 250),
                Color::Rgb(150, 63, 255),
                Color::Rgb(176, 108, 255),
            ],
            splash_check: Color::White,
            splash_check_dimmed: Color::Rgb(85, 85, 85),
            splash_afterglow: Color::Rgb(232, 213, 255),

            project_palette: [
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

            quote: Color::Rgb(137, 199, 82),

            dim_target: Color::Rgb(0, 0, 0),
        }
    }

    pub(crate) const fn light() -> Self {
        let paper = Color::Rgb(250, 250, 247);
        let bg_raised = Color::Rgb(236, 236, 231);
        let border = Color::Rgb(174, 174, 166);
        let blue = Color::Rgb(36, 96, 176);
        let green = Color::Rgb(52, 128, 24);
        let red = Color::Rgb(200, 44, 50);
        Self {
            bg: Color::Reset,
            bg_raised,
            bg_panel: Color::Rgb(229, 229, 223),
            bg_selected: Color::Rgb(219, 211, 248),
            bg_row_marked: bg_raised,
            bg_row_group: bg_raised,
            bg_lane: Color::Rgb(244, 244, 240),
            bg_lane_active: Color::Rgb(238, 236, 247),
            lane_divider: Color::Rgb(213, 213, 207),

            fg: Color::Rgb(28, 29, 27),
            fg_muted: Color::Rgb(84, 86, 82),
            fg_dim: Color::Rgb(126, 128, 122),
            inverse_fg: paper,

            border,
            scrollbar_track: border,
            table_header_bg: border,
            badge_bg: border,
            badge_installed_bg: Color::Rgb(218, 218, 211),

            accent: Color::Rgb(104, 72, 220),
            info: blue,
            success: green,
            warning: Color::Rgb(196, 112, 8),
            danger: red,
            attention: Color::Rgb(168, 124, 0),
            conflict: Color::Rgb(190, 48, 104),

            code: blue,
            link: blue,
            status_todo: blue,
            priority_medium: Color::Rgb(96, 82, 200),
            priority_none: border,

            custom_command_name: Color::Rgb(178, 96, 34),
            custom_command_tag: Color::Rgb(150, 86, 44),

            field_label_active: Color::Rgb(78, 44, 188),
            field_error: red,
            task_label: Color::Rgb(0, 118, 148),

            splash_gradient: [
                Color::Rgb(135, 40, 250),
                Color::Rgb(150, 63, 255),
                Color::Rgb(176, 108, 255),
            ],
            splash_check: Color::Rgb(40, 16, 72),
            // A third of the way from `dim_target` to `splash_check`, matching
            // how far the dark theme's check fades.
            splash_check_dimmed: Color::Rgb(180, 172, 189),
            splash_afterglow: Color::Rgb(88, 28, 196),

            project_palette: [
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

            quote: green,

            dim_target: paper,
        }
    }

    pub(crate) const fn selected(&self) -> Style {
        Style::new()
            .fg(self.fg)
            .bg(self.bg_selected)
            .add_modifier(Modifier::BOLD)
    }

    pub(crate) const fn selected_inactive(&self) -> Style {
        Style::new().fg(self.fg_muted).bg(self.bg_panel)
    }

    /// Rows related to the selected task.
    pub(crate) const fn related(&self) -> Style {
        Style::new().fg(self.fg).bg(self.bg_lane_active)
    }

    /// Inline code and commands inside plain text.
    pub(crate) const fn code(&self) -> Style {
        Style::new().fg(self.code).bg(self.bg_panel)
    }

    /// Drawn text cursor. `text_cursor_position` finds the caret by this exact
    /// foreground/background pair, so nothing else may use it.
    pub(crate) const fn cursor(&self) -> Style {
        Style::new().fg(self.bg_raised).bg(self.fg)
    }

    pub(crate) fn priority(&self, priority: &str) -> Style {
        let color = match priority {
            "urgent" => self.danger,
            "high" => self.warning,
            "medium" => self.priority_medium,
            "low" => self.fg_dim,
            _ => self.priority_none,
        };
        Style::new().fg(color)
    }

    pub(crate) fn status(&self, status: &str) -> Style {
        let color = match status {
            "active" => self.accent,
            "todo" => self.status_todo,
            "inbox" => self.fg_dim,
            "backlog" => self.fg_muted,
            "done" => self.success,
            "canceled" => self.danger,
            _ => self.fg_dim,
        };
        Style::new().fg(color)
    }

    pub(crate) fn project(&self, key: &str) -> Color {
        let hash = key
            .bytes()
            .fold(5381usize, |acc, byte| acc.wrapping_mul(33) ^ byte as usize);
        self.project_palette[hash % self.project_palette.len()]
    }
}
