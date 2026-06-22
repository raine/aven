use ratatui::Frame;
use ratatui::text::Text;

use super::super::dialog::Dialog;
use super::super::dialog::dialog_hint_line;
use crate::tui::overlay::TextPanelView;

pub(in crate::tui::ui) fn render_text_panel(frame: &mut Frame, state: &TextPanelView) {
    let visible_rows = 12usize;
    let content_rows = state.lines.len().min(visible_rows).max(1);
    let height = (content_rows as u16).saturating_add(4).min(16);
    let start = (state.scroll as usize).min(state.lines.len().saturating_sub(1));
    let mut lines = state
        .lines
        .iter()
        .skip(start)
        .take(visible_rows)
        .map(|line| ratatui::text::Line::from(line.as_str()))
        .collect::<Vec<_>>();
    lines.push(dialog_hint_line(&[
        ("j/k", "scroll"),
        ("Enter/Esc", "close"),
    ]));
    Dialog::new(&state.title, 60, height)
        .wrap()
        .render_text(frame, Text::from(lines));
}
