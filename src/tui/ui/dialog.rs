use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Wrap};

use super::truncate::truncate_chars;
use crate::tui::overlay::dialog_area;
use crate::tui::theme::{ACCENT, BG_ALT, FG, FG_MUTED};

pub(super) struct Dialog<'a> {
    title: &'a str,
    width: u16,
    height: u16,
    wrap: bool,
    right_title: Option<Line<'a>>,
}

impl<'a> Dialog<'a> {
    pub(super) fn new(title: &'a str, width: u16, height: u16) -> Self {
        Self {
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

    pub(super) fn render_block_at(self, frame: &mut Frame, area: Rect) -> Rect {
        frame.render_widget(Clear, area);
        let block = overlay_block(self.title, area.width)
            .title_top(right_edge_title(self.right_title).right_aligned());
        let inner = block.inner(area);
        frame.render_widget(block, area);
        inner
    }

    pub(super) fn render_text<'text>(self, frame: &mut Frame, text: impl Into<Text<'text>>) {
        let wrap = self.wrap;
        let inner = self.render_block(frame);
        let mut paragraph = Paragraph::new(text).style(Style::new().fg(FG).bg(BG_ALT));
        if wrap {
            paragraph = paragraph.wrap(Wrap { trim: false });
        }
        frame.render_widget(paragraph, inner);
    }
}

fn overlay_block(title: &str, width: u16) -> Block<'_> {
    Block::new()
        .title(edge_title(title, width))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .padding(Padding::horizontal(1))
        .style(Style::new().bg(BG_ALT))
}

fn edge_title(title: &str, width: u16) -> Line<'_> {
    let title = truncate_chars(title, width.saturating_sub(5) as usize);
    Line::from(vec![
        Span::styled("─ ", Style::new().fg(ACCENT)),
        Span::raw(title),
        Span::styled(" ", Style::new().fg(ACCENT)),
    ])
}

fn right_edge_title(title: Option<Line<'_>>) -> Line<'_> {
    let Some(mut title) = title else {
        return Line::from(Span::styled("─", Style::new().fg(ACCENT)));
    };
    title
        .spans
        .push(Span::styled(" ─", Style::new().fg(ACCENT)));
    title
}

pub(super) fn dialog_hint_line(items: &[(&str, &str)]) -> Line<'static> {
    let mut spans = Vec::new();
    for (index, (key, label)) in items.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled("  ", Style::new().fg(FG_MUTED)));
        }
        spans.push(Span::styled(
            key.to_string(),
            Style::new().fg(FG).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(format!(" {label}"), Style::new().fg(FG_MUTED)));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::theme::FG;

    #[test]
    fn dialog_hint_line_styles_keys() {
        let keys = styled_key_contents(dialog_hint_line(&[("Enter", "submit"), ("Esc", "cancel")]));
        assert_eq!(keys, vec!["Enter", "Esc"]);
    }

    #[test]
    fn dialog_truncates_title_to_border_width() {
        let backend = ratatui::backend::TestBackend::new(20, 5);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| Dialog::new("abcdef", 7, 3).render_text(frame, ""))
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
            .filter(|span| span.style.fg == Some(FG))
            .map(|span| span.content.to_string())
            .collect()
    }
}
