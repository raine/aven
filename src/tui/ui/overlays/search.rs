use ratatui::Frame;

use super::super::dialog::Dialog;
use super::super::input::input_line;

pub(in crate::tui::ui) fn render_search(frame: &mut Frame, input: &str, cursor: usize) {
    Dialog::new("Search", 54, 3).render_text(frame, input_line("/", input, cursor));
}
