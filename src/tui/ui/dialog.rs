use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Wrap};

use crate::tui::overlay::dialog_area;
use crate::tui::text::truncate_width;
use crate::tui::theme::Theme;
use unicode_width::UnicodeWidthStr;

pub(super) struct Dialog<'a> {
    theme: &'a Theme,
    title: &'a str,
    width: u16,
    height: u16,
    wrap: bool,
    right_title: Option<Line<'a>>,
}

impl<'a> Dialog<'a> {
    pub(super) fn new(theme: &'a Theme, title: &'a str, width: u16, height: u16) -> Self {
        Self {
            theme,
            title,
            width,
            height,
            wrap: false,
            right_title: None,
        }
    }

    pub(super) fn wrap(mut self) -> Self {
        self.wrap = true;
        self
    }

    pub(super) fn right_title(mut self, title: Line<'a>) -> Self {
        self.right_title = Some(title);
        self
    }

    pub(super) fn area(&self, frame: &Frame) -> Rect {
        dialog_area(frame.area(), self.width, self.height)
    }

    pub(super) fn render_block(self, frame: &mut Frame) -> Rect {
        let area = self.area(frame);
        self.render_block_at(frame, area)
    }

    pub(super) fn content_area(area: Rect) -> Rect {
        overlay_frame().inner(area)
    }

    pub(super) fn render_block_at(self, frame: &mut Frame, area: Rect) -> Rect {
        frame.render_widget(Clear, area);
        let block = overlay_block(self.theme, self.title, area.width)
            .title_top(right_edge_title(self.theme, self.right_title).right_aligned());
        let inner = block.inner(area);
        frame.render_widget(block, area);
        inner
    }

    pub(super) fn render_text<'text>(self, frame: &mut Frame, text: impl Into<Text<'text>>) {
        let area = self.area(frame);
        self.render_text_at(frame, area, text);
    }

    pub(super) fn render_text_at<'text>(
        self,
        frame: &mut Frame,
        area: Rect,
        text: impl Into<Text<'text>>,
    ) {
        let wrap = self.wrap;
        let theme = self.theme;
        let inner = self.render_block_at(frame, area);
        let mut paragraph =
            Paragraph::new(text).style(Style::new().fg(theme.fg).bg(theme.bg_raised));
        if wrap {
            paragraph = paragraph.wrap(Wrap { trim: false });
        }
        frame.render_widget(paragraph, inner);
    }
}

/// Borders and padding of an overlay dialog, which decide its content area.
fn overlay_frame() -> Block<'static> {
    Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .padding(Padding::horizontal(1))
}

fn overlay_block<'a>(theme: &Theme, title: &'a str, width: u16) -> Block<'a> {
    overlay_frame()
        .title(edge_title(theme, title, width))
        .border_style(Style::new().fg(theme.accent))
        .style(Style::new().bg(theme.bg_raised))
}

fn edge_title<'a>(theme: &Theme, title: &'a str, width: u16) -> Line<'a> {
    let title = truncate_width(title, width.saturating_sub(5) as usize);
    Line::from(vec![
        Span::styled("─ ", Style::new().fg(theme.accent)),
        Span::raw(title),
        Span::styled(" ", Style::new().fg(theme.accent)),
    ])
}

fn right_edge_title<'a>(theme: &Theme, title: Option<Line<'a>>) -> Line<'a> {
    let Some(mut title) = title else {
        return Line::from(Span::styled("─", Style::new().fg(theme.accent)));
    };
    title
        .spans
        .push(Span::styled(" ─", Style::new().fg(theme.accent)));
    title
}

pub(super) fn dim_rendered_background(frame: &mut Frame, theme: &Theme) {
    let area = frame.area();
    dim_rendered_area(frame, theme, area);
}

pub(super) fn dim_rendered_area(frame: &mut Frame, theme: &Theme, area: Rect) {
    let buffer = frame.buffer_mut();
    for row in area.top()..area.bottom() {
        for column in area.left()..area.right() {
            let cell = &mut buffer[(column, row)];
            if cell
                .symbol()
                .chars()
                .any(|character| !character.is_whitespace())
            {
                cell.modifier.insert(Modifier::DIM);
            }
            cell.bg = dim_background_color(cell.bg, theme.dim_target);
        }
    }
}

/// Moves an RGB background two fifths of the way toward `target`. Other
/// colors, including the terminal default, stay as they are.
fn dim_background_color(color: Color, target: Color) -> Color {
    let (Color::Rgb(red, green, blue), Color::Rgb(to_red, to_green, to_blue)) = (color, target)
    else {
        return color;
    };

    Color::Rgb(
        dim_color_channel(red, to_red),
        dim_color_channel(green, to_green),
        dim_color_channel(blue, to_blue),
    )
}

fn dim_color_channel(value: u8, target: u8) -> u8 {
    let target = i32::from(target);
    (target + (i32::from(value) - target) * 3 / 5) as u8
}

/// Width of `dialog_hint_line`, for sizing without styling.
pub(super) fn dialog_hint_width(items: &[(&str, &str)]) -> usize {
    let gaps = items.len().saturating_sub(1) * 2;
    items
        .iter()
        .map(|(key, label)| key.width() + 1 + label.width())
        .sum::<usize>()
        + gaps
}

pub(super) fn dialog_hint_line(theme: &Theme, items: &[(&str, &str)]) -> Line<'static> {
    let mut spans = Vec::new();
    for (index, (key, label)) in items.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled("  ", Style::new().fg(theme.fg_muted)));
        }
        spans.push(Span::styled(
            key.to_string(),
            Style::new().fg(theme.fg).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!(" {label}"),
            Style::new().fg(theme.fg_muted),
        ));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    const THEME: &Theme = &Theme::dark();

    #[test]
    fn dimming_darkens_rgb_and_preserves_default_backgrounds() {
        let target = Theme::dark().dim_target;
        assert_eq!(
            dim_background_color(Color::Rgb(150, 100, 50), target),
            Color::Rgb(90, 60, 30)
        );
        assert_eq!(dim_background_color(Color::Reset, target), Color::Reset);
    }

    #[test]
    fn light_dimming_fades_backgrounds_toward_the_paper_color() {
        let light = Theme::light();
        assert_eq!(
            dim_background_color(light.bg_raised, light.dim_target),
            Color::Rgb(242, 242, 238)
        );
    }

    #[test]
    fn dialog_hint_line_styles_keys() {
        let keys = styled_key_contents(dialog_hint_line(
            THEME,
            &[("Enter", "submit"), ("Esc", "cancel")],
        ));
        assert_eq!(keys, vec!["Enter", "Esc"]);
    }

    #[test]
    fn dialog_hint_width_matches_hint_line() {
        let items = [
            ("Ctrl-Enter / ^S", "submit"),
            ("←→", "select"),
            ("Esc", "cancel"),
        ];
        assert_eq!(
            dialog_hint_width(&items),
            dialog_hint_line(THEME, &items).width()
        );
        assert_eq!(dialog_hint_width(&[]), 0);
    }

    #[test]
    fn dialog_truncates_title_to_border_width() {
        let backend = ratatui::backend::TestBackend::new(20, 5);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| Dialog::new(THEME, "abcdef", 7, 3).render_text(frame, ""))
            .unwrap();
        let rendered = (0..terminal.backend().buffer().area.width)
            .map(|column| terminal.backend().buffer()[(column, 1)].symbol())
            .collect::<String>();

        assert!(rendered.contains("─ a"));
        assert!(!rendered.contains("abcdef"));
    }

    fn styled_key_contents(line: Line<'static>) -> Vec<String> {
        line.spans
            .iter()
            .filter(|span| span.style.fg == Some(THEME.fg))
            .map(|span| span.content.to_string())
            .collect()
    }
}
