use ratatui::Frame;
use ratatui::text::{Line, Text};

use super::super::dialog::{Dialog, dialog_hint_line};
use crate::tui::overlay::{ConfirmView, confirm_width};
use crate::tui::text::char_count_ranges;

pub(in crate::tui::ui) fn render_confirm(frame: &mut Frame, state: &ConfirmView) {
    let width = confirm_width(frame.area().width, &state.prompt);
    let prompt_rows = char_count_ranges(&state.prompt, width.saturating_sub(4) as usize).len();
    let height = prompt_rows.saturating_add(4) as u16;
    let text = Text::from(vec![
        Line::from(state.prompt.as_str()),
        Line::from(""),
        confirm_hint_line(),
    ]);
    Dialog::new(&state.title, width, height)
        .wrap()
        .render_text(frame, text);
}

pub(in crate::tui::ui) fn confirm_hint_line() -> ratatui::text::Line<'static> {
    dialog_hint_line(&[("y", "yes"), ("n", "no"), ("Esc", "cancel")])
}
