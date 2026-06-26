use ratatui::Frame;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};

use super::super::dialog::{Dialog, dialog_hint_line};
use super::super::input::{InputWidth, cursor_cell, input_cursor_spans};
use crate::labels::normalize_label;
use crate::tui::overlay::{
    TAG_COMBOBOX_VIEWPORT_ROWS, TAG_COMBOBOX_WIDTH, TagComboboxView, tag_combobox_layout,
};
use crate::tui::theme::{ACCENT, BG_PANEL, FG, FG_DIM, INVERSE_FG, SELECTED};

pub(in crate::tui::ui) fn render_tag_combobox(frame: &mut Frame, state: &TagComboboxView) {
    let layout = tag_combobox_layout(state, frame.area().as_size());
    let content =
        Dialog::new(&state.title, TAG_COMBOBOX_WIDTH, layout.area.height).render_block(frame);
    let lines = tag_combobox_lines(state);
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .style(Style::new().fg(FG).bg(BG_PANEL))
            .wrap(Wrap { trim: false }),
        content,
    );
}

fn tag_combobox_lines(state: &TagComboboxView) -> Vec<Line<'static>> {
    let mut lines = vec![tag_combobox_input_line(state)];
    lines.push(Line::from(""));
    lines.extend(option_lines(state));
    lines.push(Line::from(""));
    lines.push(dialog_hint_line(&[
        ("type", "search"),
        ("Tab", "add"),
        ("BS", "remove"),
        ("Enter", "save"),
        ("Esc", "clear"),
    ]));
    lines
}

fn tag_chip(label: &str) -> Vec<Span<'static>> {
    let fill = ACCENT;
    let edge_style = Style::new().fg(fill).bg(BG_PANEL);
    let label_style = Style::new()
        .fg(INVERSE_FG)
        .bg(fill)
        .add_modifier(Modifier::BOLD);
    vec![
        Span::styled("", edge_style),
        Span::styled(label.to_string(), label_style),
        Span::styled("", edge_style),
    ]
}

fn tag_combobox_input_line(state: &TagComboboxView) -> Line<'static> {
    let mut spans = Vec::new();
    for label in &state.selected {
        spans.extend(tag_chip(label));
        spans.push(Span::raw(" "));
    }

    let normalized = normalize_label(&state.input);
    if state.selected.is_empty() && normalized.is_empty() {
        spans.push(cursor_cell("E"));
        spans.push(Span::styled("nter labels here...", Style::new().fg(FG_DIM)));
    } else if (normalized.len() == state.input_cursor || state.input_cursor == state.input.len())
        && state.completion.is_some()
    {
        spans.push(Span::raw(normalized));
        let completion = state.completion.as_deref().unwrap_or_default();
        let mut chars = completion.chars();
        if let Some(cursor) = chars.next() {
            spans.push(cursor_cell(cursor.to_string()));
            let rest = chars.collect::<String>();
            if !rest.is_empty() {
                spans.push(Span::styled(rest, Style::new().fg(FG_DIM)));
            }
        }
    } else {
        spans.extend(input_cursor_spans(
            &normalized,
            normalized.len().min(state.input_cursor),
            InputWidth::Full,
        ));
    }
    spans.push(Span::styled(" ▾", Style::new().fg(FG_DIM)));
    Line::from(spans)
}

fn option_lines(state: &TagComboboxView) -> Vec<Line<'static>> {
    let mut lines = state
        .visible_indices
        .iter()
        .skip(state.visible_start)
        .take(TAG_COMBOBOX_VIEWPORT_ROWS)
        .map(|index| option_line(state, *index))
        .collect::<Vec<_>>();

    if lines.is_empty() {
        lines.push(create_option_line(state));
    }
    while lines.len() < TAG_COMBOBOX_VIEWPORT_ROWS {
        lines.push(Line::from(""));
    }
    lines
}

fn option_line(state: &TagComboboxView, index: usize) -> Line<'static> {
    let label = &state.options[index];
    let highlighted = index == state.highlighted;
    let selected = state.selected.contains(label);
    let marker = if highlighted { "▸" } else { " " };
    let check = if selected { "✓" } else { " " };
    let style = if highlighted {
        SELECTED
    } else {
        Style::new().bg(BG_PANEL)
    };
    Line::from(vec![
        Span::styled(format!("{marker} {check} "), style),
        Span::styled(label.clone(), style),
    ])
}

fn create_option_line(state: &TagComboboxView) -> Line<'static> {
    let value = normalize_label(&state.input);
    if value.is_empty() {
        return Line::from(Span::styled("  no labels", Style::new().fg(FG_DIM)));
    }
    Line::from(vec![
        Span::styled("▸ + ", SELECTED),
        Span::styled(format!("create {value}"), SELECTED),
    ])
}
