use ratatui::Frame;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;

use super::super::dialog::Dialog;
use super::super::input::input_line;
use crate::tui::theme::{FG, FG_DIM};

pub(in crate::tui::ui) fn render_search(frame: &mut Frame, input: &str, cursor: usize) {
    let content = vec![
        input_line("/", input, cursor),
        Line::from(vec![
            Span::styled("Enter", Style::new().fg(FG).add_modifier(Modifier::BOLD)),
            Span::styled(" search all tasks", Style::new().fg(FG_DIM)),
            Span::styled("  Esc", Style::new().fg(FG).add_modifier(Modifier::BOLD)),
            Span::styled(" close", Style::new().fg(FG_DIM)),
        ]),
    ];
    let area = Dialog::new("Search", 72, 4).render_block(frame);
    frame.render_widget(Paragraph::new(Text::from(content)), area);
}
