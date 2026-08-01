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
use crate::tui::overlay::{TextInputKind, TextInputView};
use crate::tui::text::char_boundary_at_or_before;
use crate::tui::theme::{FG, FG_DIM};

pub(in crate::tui::ui) fn render_text_input(frame: &mut Frame, state: &TextInputView) {
    if let Some(placeholder) = text_input_placeholder(state.kind) {
        render_placeholder_text_input(frame, state, placeholder);
        return;
    }

    if state.kind == TextInputKind::ProjectPath {
        render_project_path_input(frame, state);
        return;
    }

    if let Some((project, priority)) = add_task_title_metadata(&state.title) {
        let dialog = Dialog::new("Add task", 74, 5);
        let width = dialog.area(frame).width;
        let dialog = dialog.right_title(add_task_metadata_title(
            project,
            "inbox",
            priority,
            &[],
            width,
        ));
        let content = dialog.render_block(frame);
        let input =
            add_task_title_input_line(&state.input, Some(state.cursor), content.width as usize);
        let text = Text::from(vec![
            input,
            Line::from(""),
            add_task_hint_line(AddTaskStep::Title, false, false, false),
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

    let edit_date = state.kind == TextInputKind::EditDate;
    let dialog_width = if edit_date { 64 } else { 54 };
    let input = input_line("", &state.input, state.cursor);
    let confirms_project_delete = state.kind == TextInputKind::ConfirmDeleteProject;
    let lines = if confirms_project_delete {
        vec![
            Line::from(Span::styled(&state.prompt, Style::new().fg(FG_DIM))),
            Line::from(""),
            input,
            Line::from(""),
            dialog_hint_line(&[("Enter", "submit"), ("Esc", "cancel")]),
        ]
    } else if edit_date {
        let mut lines = state
            .prompt
            .lines()
            .map(|line| Line::from(Span::styled(line.to_string(), Style::new().fg(FG_DIM))))
            .collect::<Vec<_>>();
        let hints = if state.prompt.starts_with("Current: varies") {
            &[
                ("Enter", "keep"),
                ("Ctrl+D", "clear dates"),
                ("Esc", "cancel"),
            ][..]
        } else {
            &[("Enter", "submit"), ("Ctrl+D", "clear"), ("Esc", "cancel")][..]
        };
        lines.extend([input, Line::from(""), dialog_hint_line(hints)]);
        lines
    } else {
        vec![
            Line::from(Span::styled(&state.prompt, Style::new().fg(FG_DIM))),
            input,
            Line::from(""),
            dialog_hint_line(&[("Enter", "submit"), ("Esc", "cancel")]),
        ]
    };
    let height = if confirms_project_delete {
        7
    } else if edit_date {
        state.prompt.lines().count() as u16 + 5
    } else {
        6
    };
    Dialog::new(&state.title, dialog_width, height).render_text(frame, Text::from(lines));
}

fn render_project_path_input(frame: &mut Frame, state: &TextInputView) {
    let dialog = Dialog::new(&state.title, 74, 6);
    let content = dialog.render_block(frame);
    let input = project_path_input_line(&state.input, state.cursor, content.width as usize);
    let text = Text::from(vec![
        Line::from(Span::styled(&state.prompt, Style::new().fg(FG_DIM))),
        input,
        Line::from(""),
        dialog_hint_line(&[("Enter", "submit"), ("Esc", "cancel")]),
    ]);
    frame.render_widget(
        Paragraph::new(text).style(Style::new().fg(FG).bg(crate::tui::theme::BG_ALT)),
        content,
    );
}

pub(in crate::tui::ui) fn project_path_input_line(
    input: &str,
    cursor: usize,
    width: usize,
) -> Line<'static> {
    if input.chars().count().saturating_add(1) <= width || width < 2 {
        return clipped_input_line(input, cursor, width);
    }
    let cursor = char_boundary_at_or_before(input, cursor);
    let cursor_chars = input[..cursor].chars().count();
    let mut line = clipped_input_line(input, cursor, width.saturating_sub(1));
    let marker = Span::styled("…", Style::new().fg(FG_DIM));
    if cursor_chars >= width.saturating_sub(1) {
        line.spans.insert(0, marker);
    } else {
        line.spans.push(marker);
    }
    line
}

pub(in crate::tui::ui) const ADD_PROJECT_NAME_PLACEHOLDER: &str = "Enter project name here...";
pub(in crate::tui::ui) const ADD_LABEL_NAME_PLACEHOLDER: &str = "Enter label name here...";
pub(in crate::tui::ui) const RENAME_PROJECT_NAME_PLACEHOLDER: &str = "Enter project name here...";
pub(in crate::tui::ui) const CONFLICT_MANUAL_VALUE_PLACEHOLDER: &str = "Enter manual value here...";

fn text_input_placeholder(kind: TextInputKind) -> Option<&'static str> {
    match kind {
        TextInputKind::AddProject => Some(ADD_PROJECT_NAME_PLACEHOLDER),
        TextInputKind::AddLabel => Some(ADD_LABEL_NAME_PLACEHOLDER),
        TextInputKind::RenameProject => Some(RENAME_PROJECT_NAME_PLACEHOLDER),
        TextInputKind::ConflictManual => Some(CONFLICT_MANUAL_VALUE_PLACEHOLDER),
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
