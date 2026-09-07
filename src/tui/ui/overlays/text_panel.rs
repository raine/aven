use ratatui::Frame;
use ratatui::text::Text;

use super::super::dialog::Dialog;
use super::super::dialog::dialog_hint_line;
use crate::tui::overlay::{TextPanelView, text_panel_layout};

pub(crate) fn text_panel_scroll_cap(lines: &[String]) -> u16 {
    crate::tui::overlay::text_panel_scroll_cap(lines.len())
}

pub(in crate::tui::ui) fn render_text_panel(frame: &mut Frame, state: &TextPanelView) {
    let layout = text_panel_layout(frame.area().as_size(), state.lines.len());
    let start = (state.scroll as usize).min(text_panel_scroll_cap(state.lines) as usize);
    let mut lines = state
        .lines
        .iter()
        .skip(start)
        .take(layout.visible_rows)
        .map(|line| ratatui::text::Line::from(line.as_str()))
        .collect::<Vec<_>>();
    lines.push(dialog_hint_line(&[
        ("j/k", "scroll"),
        ("Enter/Esc", "close"),
    ]));
    Dialog::new(&state.title, 0, 0)
        .wrap()
        .render_text_at(frame, layout.area, Text::from(lines));
}
