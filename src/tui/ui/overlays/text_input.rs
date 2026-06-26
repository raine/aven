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
    if state.route == OverlayRoute::AddProject {
        render_add_project_input(frame, state);
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

fn render_add_project_input(frame: &mut Frame, state: &TextInputView) {
    let dialog = Dialog::new(&state.title, 54, 5);
    let content = dialog.render_block(frame);
    let text = Text::from(vec![
        add_project_name_input_line(&state.input, state.cursor, content.width as usize),
        Line::from(""),
        dialog_hint_line(&[("Enter", "submit"), ("Esc", "cancel")]),
    ]);
    frame.render_widget(
        Paragraph::new(text).style(Style::new().fg(FG).bg(crate::tui::theme::BG_ALT)),
        content,
    );
}

pub(in crate::tui::ui) fn add_project_name_input_line(
    input: &str,
    cursor: usize,
    width: usize,
) -> Line<'static> {
    if input.is_empty() {
        return Line::from(vec![
            super::super::input::cursor_cell(&ADD_PROJECT_NAME_PLACEHOLDER[..1]),
            Span::styled(&ADD_PROJECT_NAME_PLACEHOLDER[1..], Style::new().fg(FG_DIM)),
        ]);
    }
    Line::from(input_cursor_spans(
        input,
        cursor,
        InputWidth::Clipped(width),
    ))
}
