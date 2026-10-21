use ratatui::Frame;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};

use super::super::dialog::{Dialog, dialog_hint_line};
use super::super::input::{InputWidth, cursor_cell, input_cursor_spans};
use crate::labels::normalize_label;
use crate::tui::overlay::{
    TAG_COMBOBOX_CREATE, TAG_COMBOBOX_VIEWPORT_ROWS, TAG_COMBOBOX_WIDTH, TagComboboxView,
    tag_combobox_layout,
};
use crate::tui::theme::Theme;

pub(in crate::tui::ui) fn render_tag_combobox(
    frame: &mut Frame,
    theme: &Theme,
    state: &TagComboboxView,
) {
    let layout = tag_combobox_layout(state, frame.area().as_size());
    let content = Dialog::new(theme, &state.title, TAG_COMBOBOX_WIDTH, layout.area.height)
        .render_block(frame);
    let lines = tag_combobox_lines(theme, state);
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .style(Style::new().fg(theme.fg).bg(theme.bg_panel))
            .wrap(Wrap { trim: false }),
        content,
    );
}

pub(in crate::tui::ui) fn tag_combobox_lines(
    theme: &Theme,
    state: &TagComboboxView,
) -> Vec<Line<'static>> {
    tag_combobox_lines_with_viewport(theme, state, TAG_COMBOBOX_VIEWPORT_ROWS)
}

pub(in crate::tui::ui) fn tag_combobox_lines_with_viewport(
    theme: &Theme,
    state: &TagComboboxView,
    viewport_rows: usize,
) -> Vec<Line<'static>> {
    let mut lines = vec![tag_combobox_input_line(theme, state)];
    lines.push(Line::from(""));
    lines.extend(option_lines(theme, state, viewport_rows));
    lines.push(Line::from(""));
    let hints = if state.partial.is_empty() {
        vec![
            ("Tab/Space", "add"),
            ("Enter", "add/save"),
            ("^S", "save"),
            ("BS", "remove"),
            ("Esc", "clear"),
        ]
    } else {
        vec![
            ("Tab/Space", "toggle"),
            ("Enter", "save"),
            ("^S", "save"),
            ("~", "on some"),
        ]
    };
    lines.push(dialog_hint_line(theme, &hints));
    lines
}

fn tag_chip(theme: &Theme, label: &str) -> Vec<Span<'static>> {
    let fill = theme.accent;
    let edge_style = Style::new().fg(fill).bg(theme.bg_panel);
    let label_style = theme.badge(fill).add_modifier(Modifier::BOLD);
    vec![
        Span::styled(theme.pill_edge(""), edge_style),
        Span::styled(label.to_string(), label_style),
        Span::styled(theme.pill_edge(""), edge_style),
    ]
}

fn tag_combobox_input_line(theme: &Theme, state: &TagComboboxView) -> Line<'static> {
    let mut spans = Vec::new();
    for label in state.selected {
        spans.extend(tag_chip(theme, label));
        spans.push(Span::raw(" "));
    }

    let normalized = normalize_label(&state.input);
    if state.selected.is_empty() && normalized.is_empty() {
        spans.push(cursor_cell(theme, "E"));
        spans.push(Span::styled("nter labels here...", theme.dim()));
    } else if (normalized.len() == state.input_cursor || state.input_cursor == state.input.len())
        && state.completion.is_some()
    {
        spans.push(Span::raw(normalized));
        let completion = state.completion.as_deref().unwrap_or_default();
        let mut chars = completion.chars();
        if let Some(cursor) = chars.next() {
            spans.push(cursor_cell(theme, cursor.to_string()));
            let rest = chars.collect::<String>();
            if !rest.is_empty() {
                spans.push(Span::styled(rest, theme.dim()));
            }
        }
    } else {
        spans.extend(input_cursor_spans(
            theme,
            &normalized,
            normalized.len().min(state.input_cursor),
            InputWidth::Full,
        ));
    }
    spans.push(Span::styled(" ▾", theme.dim()));
    Line::from(spans)
}

fn option_lines(
    theme: &Theme,
    state: &TagComboboxView,
    viewport_rows: usize,
) -> Vec<Line<'static>> {
    let highlighted_position = state
        .visible_indices
        .iter()
        .position(|index| *index == state.highlighted);
    let visible_start = highlighted_position
        .map(|position| {
            state
                .visible_start
                .max(position.saturating_add(1).saturating_sub(viewport_rows))
        })
        .unwrap_or(state.visible_start)
        .min(state.visible_indices.len().saturating_sub(viewport_rows));
    let mut lines = state
        .visible_indices
        .iter()
        .skip(visible_start)
        .take(viewport_rows)
        .map(|index| option_line(theme, state, *index))
        .collect::<Vec<_>>();

    if lines.is_empty() {
        lines.push(Line::from(Span::styled("  no labels", theme.dim())));
    }
    while lines.len() < viewport_rows {
        lines.push(Line::from(""));
    }
    lines
}

fn option_line(theme: &Theme, state: &TagComboboxView, index: usize) -> Line<'static> {
    if index == TAG_COMBOBOX_CREATE {
        return create_option_line(theme, state);
    }
    let label = &state.options[index];
    let highlighted = index == state.highlighted;
    let selected = state.selected.contains(label);
    let partial = state.partial.contains(label);
    let marker = if highlighted { "▸" } else { " " };
    let check = if selected {
        "✓"
    } else if partial {
        "~"
    } else {
        " "
    };
    let style = if highlighted {
        theme.selected()
    } else {
        Style::new().bg(theme.bg_panel)
    };
    Line::from(vec![
        Span::styled(format!("{marker} {check} "), style),
        Span::styled(label.clone(), style),
    ])
}

fn create_option_line(theme: &Theme, state: &TagComboboxView) -> Line<'static> {
    let value = normalize_label(&state.input);
    if value.is_empty() {
        return Line::from(Span::styled("  no labels", theme.dim()));
    }
    let highlighted = state.highlighted == TAG_COMBOBOX_CREATE;
    let style = if highlighted {
        theme.selected()
    } else {
        Style::new().bg(theme.bg_panel)
    };
    let marker = if highlighted { "▸" } else { " " };
    Line::from(vec![
        Span::styled(format!("{marker} + "), style),
        Span::styled(format!("create {value}"), style),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::overlay::{OverlayState, TagComboboxIntent};

    #[test]
    fn create_row_renders_below_matches_and_scrolls_into_view() {
        let OverlayState::TagCombobox(mut state) = OverlayState::tag_combobox(
            TagComboboxIntent::AddTaskLabels,
            "Labels",
            vec!["abcd".into()],
            Vec::new(),
        ) else {
            panic!("expected combobox")
        };
        state.input = crate::tui::overlay::LineEdit::new("abc".into());
        let theme = Theme::dark();
        let view = TagComboboxView::from_state(&state);
        let lines = option_lines(&theme, &view, 2);
        assert_eq!(lines[0].to_string(), "▸   abcd");
        assert_eq!(lines[1].to_string(), "  + create abc");
        state.highlighted = TAG_COMBOBOX_CREATE;
        let view = TagComboboxView::from_state(&state);
        let lines = option_lines(&theme, &view, 1);
        assert_eq!(lines[0].to_string(), "▸ + create abc");
    }
}
