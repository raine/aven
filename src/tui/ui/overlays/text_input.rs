use ratatui::Frame;
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;

use super::super::dialog::{Dialog, dialog_hint_line};
use super::super::input::{InputWidth, clipped_input_line, input_cursor_spans, input_line};
use super::add_task::{
    add_task_hint_line, add_task_metadata_title, add_task_title_input_line, add_task_title_metadata,
};
use crate::tui::authoring::AddTaskStep;
use crate::tui::overlay::{OverlayRoute, TextInputView};
use crate::tui::theme::{FG, FG_DIM};

pub(in crate::tui::ui) fn render_text_input(frame: &mut Frame, state: &TextInputView) {
    if let Some(placeholder) = text_input_placeholder(state.route) {
        render_placeholder_text_input(frame, state, placeholder);
        return;
    }

    if let Some((project, priority)) = add_task_title_metadata(&state.title) {
        let dialog = Dialog::new("Add task", 74, 5);
        let width = dialog.area(frame).width;
        let dialog = dialog.right_title(add_task_metadata_title(project, "inbox", priority, width));
        let content = dialog.render_block(frame);
        let input =
            add_task_title_input_line(&state.input, Some(state.cursor), content.width as usize);
        let text = Text::from(vec![
            input,
            Line::from(""),
            add_task_hint_line(AddTaskStep::Title, false, false),
        ]);
        frame.render_widget(
            Paragraph::new(text).style(Style::new().fg(FG).bg(crate::tui::theme::BG_ALT)),
            content,
        );
        return;
    }

    if state.prompt.is_empty() {
        let dialog = Dialog::new(&state.title, 54, 5);
        let content = dialog.render_block(frame);
        let input = clipped_input_line(&state.input, state.cursor, content.width as usize);
        let text = Text::from(vec![
            input,
            Line::from(""),
            dialog_hint_line(&[("Enter", "submit"), ("Esc", "cancel")]),
        ]);
        frame.render_widget(
            Paragraph::new(text).style(Style::new().fg(FG).bg(crate::tui::theme::BG_ALT)),
            content,
        );
        return;
    }

    let text = Text::from(vec![
        Line::from(Span::styled(&state.prompt, Style::new().fg(FG_DIM))),
        input_line("", &state.input, state.cursor),
        Line::from(""),
        dialog_hint_line(&[("Enter", "submit"), ("Esc", "cancel")]),
    ]);
    Dialog::new(&state.title, 54, 6).render_text(frame, text);
}

pub(in crate::tui::ui) const ADD_PROJECT_NAME_PLACEHOLDER: &str = "Enter project name here...";
pub(in crate::tui::ui) const ADD_LABEL_NAME_PLACEHOLDER: &str = "Enter label name here...";
pub(in crate::tui::ui) const RENAME_PROJECT_NAME_PLACEHOLDER: &str = "Enter project name here...";
pub(in crate::tui::ui) const CONFLICT_MANUAL_VALUE_PLACEHOLDER: &str = "Enter manual value here...";

fn text_input_placeholder(route: OverlayRoute) -> Option<&'static str> {
    match route {
        OverlayRoute::AddProject => Some(ADD_PROJECT_NAME_PLACEHOLDER),
        OverlayRoute::AddLabel => Some(ADD_LABEL_NAME_PLACEHOLDER),
        OverlayRoute::RenameProjectName => Some(RENAME_PROJECT_NAME_PLACEHOLDER),
        OverlayRoute::ConflictManual => Some(CONFLICT_MANUAL_VALUE_PLACEHOLDER),
        _ => None,
    }
}

fn render_placeholder_text_input(
    frame: &mut Frame,
    state: &TextInputView,
    placeholder: &'static str,
) {
    let dialog = Dialog::new(&state.title, 54, 5);
    let content = dialog.render_block(frame);
    let text = Text::from(vec![
        placeholder_text_input_line(
            &state.input,
            state.cursor,
            content.width as usize,
            placeholder,
        ),
        Line::from(""),
        dialog_hint_line(&[("Enter", "submit"), ("Esc", "cancel")]),
    ]);
    frame.render_widget(
        Paragraph::new(text).style(Style::new().fg(FG).bg(crate::tui::theme::BG_ALT)),
        content,
    );
}

pub(in crate::tui::ui) fn placeholder_text_input_line(
    input: &str,
    cursor: usize,
    width: usize,
    placeholder: &'static str,
) -> Line<'static> {
    if input.is_empty() {
        return Line::from(vec![
            super::super::input::cursor_cell(&placeholder[..1]),
            Span::styled(&placeholder[1..], Style::new().fg(FG_DIM)),
        ]);
    }
    Line::from(input_cursor_spans(
        input,
        cursor,
        InputWidth::Clipped(width),
    ))
}
