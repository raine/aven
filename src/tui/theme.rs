use ratatui::style::{Color, Modifier, Style};

/// Colors the TUI draws with, named by what each color is for.
///
/// `App` owns the active theme and render code receives it as `&Theme`.
/// Composite styles are methods over the base roles, so changing a base role
/// also changes every composite built from it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Theme {
    terminal_palette: bool,
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
    Terminal,
}

impl Appearance {
    pub(crate) const fn theme(self) -> Theme {
        match self {
            Self::Dark => Theme::dark(),
            Self::Light => Theme::light(),
            Self::Terminal => Theme::terminal(),
        }
    }
}

impl Theme {
    pub(crate) const fn dark() -> Self {
        let bg_raised = Color::Rgb(34, 35, 33);
        let border = Color::Rgb(88, 88, 83);
        let blue = Color::Rgb(70, 128, 203);
        Self {
            terminal_palette: false,
            bg: Color::Reset,
            bg_raised,
            bg_panel: Color::Rgb(39, 40, 38),
            bg_selected: Color::Rgb(50, 45, 78),
            bg_row_marked: bg_raised,
            bg_lane: Color::Rgb(24, 25, 24),
            bg_lane_active: Color::Rgb(27, 26, 32),
            lane_divider: Color::Rgb(49, 50, 47),

            fg: Color::Rgb(239, 238, 232),
            fg_muted: Color::Rgb(191, 188, 180),
            fg_dim: Color::Rgb(147, 145, 138),
            inverse_fg: Color::Rgb(18, 19, 18),

            border,
            scrollbar_track: border,
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
            terminal_palette: false,
            bg: Color::Reset,
            bg_raised,
            bg_panel: Color::Rgb(229, 229, 223),
            bg_selected: Color::Rgb(219, 211, 248),
            bg_row_marked: bg_raised,
            bg_lane: Color::Rgb(244, 244, 240),
            bg_lane_active: Color::Rgb(238, 236, 247),
            lane_divider: Color::Rgb(213, 213, 207),

            fg: Color::Rgb(28, 29, 27),
            fg_muted: Color::Rgb(84, 86, 82),
            fg_dim: Color::Rgb(126, 128, 122),
            inverse_fg: paper,

            border,
            scrollbar_track: border,
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

    pub(crate) const fn terminal() -> Self {
        Self {
            terminal_palette: true,
            bg: Color::Reset,
            bg_raised: Color::Reset,
            bg_panel: Color::Reset,
            bg_selected: Color::Reset,
            bg_row_marked: Color::Reset,
            bg_lane: Color::Reset,
            bg_lane_active: Color::Reset,
            lane_divider: Color::Reset,
            fg: Color::Reset,
            fg_muted: Color::Reset,
            fg_dim: Color::Reset,
            inverse_fg: Color::Reset,
            border: Color::Reset,
            scrollbar_track: Color::Reset,
            badge_bg: Color::Reset,
            badge_installed_bg: Color::Reset,
            accent: Color::Magenta,
            info: Color::Blue,
            success: Color::Green,
            warning: Color::Yellow,
            danger: Color::Red,
            attention: Color::Yellow,
            conflict: Color::Magenta,
            code: Color::Blue,
            link: Color::Blue,
            status_todo: Color::Blue,
            priority_medium: Color::Magenta,
            priority_none: Color::Reset,
            custom_command_name: Color::Yellow,
            custom_command_tag: Color::Yellow,
            field_label_active: Color::Magenta,
            field_error: Color::Red,
            task_label: Color::Cyan,
            splash_gradient: [Color::Magenta; 3],
            splash_check: Color::Reset,
            splash_check_dimmed: Color::Reset,
            splash_afterglow: Color::Magenta,
            project_palette: [
                Color::Magenta,
                Color::Cyan,
                Color::Yellow,
                Color::Red,
                Color::Magenta,
                Color::Green,
                Color::Blue,
                Color::Yellow,
                Color::Cyan,
                Color::Magenta,
                Color::Red,
                Color::Green,
                Color::Blue,
                Color::Magenta,
            ],
            quote: Color::Green,
            dim_target: Color::Reset,
        }
    }

    pub(crate) const fn is_terminal(&self) -> bool {
        self.terminal_palette
    }

    pub(crate) const fn selected(&self) -> Style {
        let style = Style::new()
            .fg(self.fg)
            .bg(self.bg_selected)
            .add_modifier(Modifier::BOLD);
        if self.terminal_palette {
            style
                .add_modifier(Modifier::REVERSED)
                .remove_modifier(Modifier::DIM)
        } else {
            style
        }
    }

    /// Highlighted menu rows and the focused sidebar entry.
    pub(crate) const fn highlight(&self) -> Style {
        if self.terminal_palette {
            self.selected()
        } else {
            Style::new().bg(self.bg_selected)
        }
    }

    pub(crate) const fn selected_inactive(&self) -> Style {
        let style = Style::new().fg(self.fg_muted).bg(self.bg_panel);
        if self.terminal_palette {
            style.add_modifier(Modifier::UNDERLINED)
        } else {
            style
        }
    }

    /// Rows related to the selected task.
    pub(crate) const fn related(&self) -> Style {
        let style = Style::new().fg(self.fg).bg(self.bg_lane_active);
        if self.terminal_palette {
            style.add_modifier(Modifier::UNDERLINED)
        } else {
            style
        }
    }

    /// Inline code and commands inside plain text.
    pub(crate) const fn code(&self) -> Style {
        Style::new().fg(self.code).bg(self.bg_panel)
    }

    /// Visible caret; input rendering tracks its location separately.
    pub(crate) const fn cursor(&self) -> Style {
        let style = Style::new().fg(self.bg_raised).bg(self.fg);
        if self.terminal_palette {
            style.add_modifier(Modifier::REVERSED)
        } else {
            style
        }
    }

    pub(crate) const fn dim(&self) -> Style {
        let style = Style::new().fg(self.fg_dim);
        if self.terminal_palette {
            style.add_modifier(Modifier::DIM)
        } else {
            style
        }
    }

    pub(crate) const fn muted(&self) -> Style {
        let style = Style::new().fg(self.fg_muted);
        if self.terminal_palette {
            style.add_modifier(Modifier::DIM)
        } else {
            style
        }
    }

    pub(crate) const fn badge(&self, fill: Color) -> Style {
        if self.terminal_palette {
            Style::new()
                .fg(fill)
                .bg(Color::Reset)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(self.inverse_fg).bg(fill)
        }
    }

    /// Section titles such as sidebar headings and the task table header.
    pub(crate) const fn section_title(&self) -> Style {
        self.dim().bg(self.bg_raised).add_modifier(Modifier::BOLD)
    }

    /// The task table header, styled like other section titles. Terminal
    /// palettes have no raised background, so the header is underlined to
    /// stay distinct from rows.
    pub(crate) const fn table_header(&self) -> Style {
        let style = self.section_title();
        if self.terminal_palette {
            style.add_modifier(Modifier::UNDERLINED)
        } else {
            style
        }
    }

    pub(crate) const fn pill_edge(&self, glyph: &'static str) -> &'static str {
        if self.terminal_palette { " " } else { glyph }
    }

    /// Apply selection after semantic spans, so reverse video has a uniform fill.
    pub(crate) fn paint_selection(
        &self,
        buffer: &mut ratatui::buffer::Buffer,
        area: ratatui::layout::Rect,
        focused: bool,
    ) {
        if !self.terminal_palette {
            return;
        }
        let area = area.intersection(buffer.area);
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                let cell = &mut buffer[(x, y)];
                if focused {
                    cell.set_style(self.selected());
                    if cell.modifier.contains(crate::tui::ui::CARET_MARKER) {
                        cell.modifier.remove(Modifier::REVERSED);
                    }
                } else {
                    cell.modifier.insert(Modifier::UNDERLINED);
                }
            }
        }
    }

    /// Reversed spans inherit semantic colors; default colors keep their fill uniform.
    pub(crate) fn finish_selection(&self, buffer: &mut ratatui::buffer::Buffer) {
        if !self.terminal_palette {
            return;
        }
        for cell in &mut buffer.content {
            if cell.modifier.contains(Modifier::REVERSED)
                && !cell.modifier.contains(crate::tui::ui::CARET_MARKER)
            {
                cell.fg = Color::Reset;
                cell.bg = Color::Reset;
            }
        }
    }

    pub(crate) fn priority(&self, priority: &str) -> Style {
        let color = match priority {
            "urgent" => self.danger,
            "high" => self.warning,
            "medium" => self.priority_medium,
            "low" => self.fg_dim,
            _ => self.priority_none,
        };
        if self.terminal_palette && matches!(priority, "low" | "none" | "") {
            self.dim()
        } else {
            Style::new().fg(color)
        }
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
        if self.terminal_palette && !matches!(status, "active" | "todo" | "done" | "canceled") {
            self.dim()
        } else {
            Style::new().fg(color)
        }
    }

    pub(crate) fn project(&self, key: &str) -> Color {
        let hash = key
            .bytes()
            .fold(5381usize, |acc, byte| acc.wrapping_mul(33) ^ byte as usize);
        self.project_palette[hash % self.project_palette.len()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{buffer::Buffer, layout::Rect};

    #[test]
    fn curated_composite_styles_remain_unchanged() {
        for theme in [Theme::dark(), Theme::light()] {
            assert_eq!(theme.dim(), Style::new().fg(theme.fg_dim));
            assert_eq!(theme.muted(), Style::new().fg(theme.fg_muted));
            assert_eq!(
                theme.badge(theme.accent),
                Style::new().fg(theme.inverse_fg).bg(theme.accent)
            );
            assert_eq!(
                theme.cursor(),
                Style::new().fg(theme.bg_raised).bg(theme.fg)
            );
            assert_eq!(
                theme.selected(),
                Style::new()
                    .fg(theme.fg)
                    .bg(theme.bg_selected)
                    .add_modifier(Modifier::BOLD)
            );
            assert_eq!(
                theme.selected_inactive(),
                Style::new().fg(theme.fg_muted).bg(theme.bg_panel)
            );
            assert_eq!(
                theme.related(),
                Style::new().fg(theme.fg).bg(theme.bg_lane_active)
            );
            let mut buffer = Buffer::empty(Rect::new(0, 0, 4, 1));
            buffer[(0, 0)].set_style(theme.cursor());
            let old = buffer.clone();
            theme.paint_selection(&mut buffer, Rect::new(0, 0, 4, 1), true);
            theme.finish_selection(&mut buffer);
            assert_eq!(buffer, old);
        }
    }

    #[test]
    fn terminal_styles_use_defaults_and_portable_accents() {
        let theme = Theme::terminal();
        assert!(!format!("{theme:?}").contains("Rgb("));
        assert!(!format!("{theme:?}").contains("DarkGray"));
        assert_eq!(theme.bg, Color::Reset);
        assert_eq!(
            theme.dim(),
            Style::new().fg(Color::Reset).add_modifier(Modifier::DIM)
        );
        assert_eq!(theme.badge(theme.accent).bg, Some(Color::Reset));
        assert!(theme.selected().add_modifier.contains(Modifier::REVERSED));
        assert!(
            theme
                .selected_inactive()
                .add_modifier
                .contains(Modifier::UNDERLINED)
        );
        assert_eq!(theme.status("inbox"), theme.dim());
        assert_eq!(theme.priority("low"), theme.dim());
    }

    #[test]
    fn terminal_selection_normalizes_spans_and_inverts_the_caret() {
        let theme = Theme::terminal();
        let area = Rect::new(0, 0, 4, 1);
        let mut buffer = Buffer::empty(area);
        buffer[(0, 0)].set_style(Style::new().fg(Color::Red).add_modifier(Modifier::DIM));
        buffer[(1, 0)].set_style(theme.cursor().add_modifier(crate::tui::ui::CARET_MARKER));
        theme.paint_selection(&mut buffer, area, true);
        assert_eq!(buffer[(0, 0)].fg, Color::Reset);
        assert_eq!(buffer[(0, 0)].bg, Color::Reset);
        assert!(buffer[(0, 0)].modifier.contains(Modifier::REVERSED));
        assert!(!buffer[(0, 0)].modifier.contains(Modifier::DIM));
        assert!(!buffer[(1, 0)].modifier.contains(Modifier::REVERSED));
        assert!(
            buffer[(1, 0)]
                .modifier
                .contains(crate::tui::ui::CARET_MARKER)
        );
        buffer[(2, 0)].fg = Color::Blue;
        theme.finish_selection(&mut buffer);
        assert_eq!(buffer[(2, 0)].fg, Color::Reset);
    }
}
