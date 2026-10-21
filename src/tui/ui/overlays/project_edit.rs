use ratatui::{
    Frame,
    style::Style,
    text::{Line, Span, Text},
    widgets::Paragraph,
};

use super::super::dialog::{Dialog, dialog_hint_line};
use super::super::input::{InputWidth, input_cursor_spans};
use crate::tui::overlay::{ProjectEditField, ProjectEditState};
use crate::tui::theme::Theme;

pub(in crate::tui::ui) fn render_project_edit(
    frame: &mut Frame,
    theme: &Theme,
    state: &ProjectEditState,
) {
    let content = Dialog::new(theme, "Rename project", 62, 6).render_block(frame);
    let width = (content.width as usize).saturating_sub(8);
    let mut lines = Vec::new();
    for (label, input, field) in [
        ("Name    ", &state.name, ProjectEditField::Name),
        ("Prefix  ", &state.prefix, ProjectEditField::Prefix),
    ] {
        let mut spans = vec![Span::styled(label, theme.dim())];
        if state.focus == field {
            spans.extend(input_cursor_spans(
                theme,
                &input.text,
                input.cursor,
                InputWidth::Clipped(width),
            ));
        } else {
            spans.push(Span::raw(
                crate::tui::text::take_leading_cells(&input.text, width).to_string(),
            ));
        }
        lines.push(Line::from(spans));
    }
    lines.push(Line::from(""));
    lines.push(dialog_hint_line(
        theme,
        &[
            ("Enter", "save"),
            ("Tab", "field"),
            ("Ctrl+U", "clear"),
            ("Esc", "cancel"),
        ],
    ));
    frame.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::new().fg(theme.fg).bg(theme.bg_raised)),
        content,
    );
}
