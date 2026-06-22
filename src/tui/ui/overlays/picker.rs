use ratatui::Frame;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};

use super::super::dialog::{Dialog, dialog_hint_line};
use super::super::input::{input_line, prefixed_input_line};
use super::shared::selected_viewport_start;
use crate::tui::overlay::{OverlayRoute, PickerItem, PickerMode, PickerView};
use crate::tui::theme::{self, ACCENT, BG_ALT, BG_PANEL, FG, FG_DIM, SELECTED};
use crate::tui::widgets::priority_icon;

pub(in crate::tui::ui) fn render_picker(frame: &mut Frame, state: &PickerView) {
    if let Some(submit_label) = project_picker_submit_label(state.route) {
        render_project_picker(frame, state, submit_label);
        return;
    }

    let visible_count = state.visible_indices.len().max(1);
    let viewport_rows = 8usize;
    let height = (visible_count.min(viewport_rows) as u16).saturating_add(5);
    let selected_position =
        selected_viewport_start(&state.visible_indices, state.selected, viewport_rows);
    let mut lines = vec![
        input_line("/", &state.filter, state.filter_cursor),
        Line::from(""),
    ];
    for index in state
        .visible_indices
        .iter()
        .skip(selected_position)
        .take(viewport_rows)
    {
        let item = &state.items[*index];
        let marker = if *index == state.selected {
            "▸ "
        } else {
            "  "
        };
        let check = if state.multi && item.selected {
            " ✓"
        } else {
            ""
        };
        if priority_picker_submit_label(state.route).is_some() {
            lines.push(priority_picker_line(item, *index == state.selected));
        } else {
            lines.push(Line::from(format!("{marker}{}{check}", item.label)));
        }
    }
    lines.push(Line::from(""));
    lines.push(picker_hint_line(
        state.mode,
        state.multi,
        priority_picker_submit_label(state.route).unwrap_or("submit"),
    ));
    Dialog::new(&state.title, 60, height.saturating_add(1)).render_text(frame, Text::from(lines));
}

pub(in crate::tui::ui) fn priority_picker_line(item: &PickerItem, selected: bool) -> Line<'static> {
    let marker = if selected { "▸ " } else { "  " };
    Line::from(vec![
        Span::raw(marker),
        Span::styled(
            format!("{} ", priority_icon(&item.value)),
            theme::priority_style(&item.value).add_modifier(Modifier::BOLD),
        ),
        Span::styled(item.label.clone(), theme::priority_style(&item.value)),
    ])
}

pub(in crate::tui::ui) fn picker_hint_line(
    mode: PickerMode,
    multi: bool,
    submit_label: &str,
) -> Line<'static> {
    let mut items = match mode {
        PickerMode::Navigate => vec![("j/k", "move"), ("/", "filter")],
        PickerMode::Filter => vec![("type", "filter"), ("Up/Down", "move")],
    };
    if multi {
        items.push(("Space", "toggle"));
    }
    let esc_label = match mode {
        PickerMode::Navigate => "cancel",
        PickerMode::Filter => "normal",
    };
    items.extend([("Enter", submit_label), ("Esc", esc_label)]);
    dialog_hint_line(&items)
}

fn render_project_picker(frame: &mut Frame, state: &PickerView, submit_label: &'static str) {
    let viewport_rows = 10usize;
    let height = (viewport_rows as u16).saturating_add(6);
    let selected_position =
        selected_viewport_start(&state.visible_indices, state.selected, viewport_rows);
    let mut lines = vec![
        prefixed_input_line(
            Span::styled("/", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
            &state.filter,
            state.filter_cursor,
        ),
        Line::from(vec![
            Span::styled("  PREFIX ", Style::new().fg(FG_DIM).bg(BG_PANEL)),
            Span::styled("PROJECT", Style::new().fg(FG_DIM).bg(BG_PANEL)),
        ]),
    ];
    let list_start = lines.len();
    for index in state
        .visible_indices
        .iter()
        .skip(selected_position)
        .take(viewport_rows)
    {
        lines.push(project_picker_line(
            &state.items[*index],
            *index == state.selected,
        ));
    }
    if state.visible_indices.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no matching projects",
            Style::new().fg(FG_DIM),
        )));
    }
    while lines.len().saturating_sub(list_start) < viewport_rows {
        lines.push(Line::from(""));
    }
    lines.push(Line::from(""));
    lines.push(project_picker_hint_line(state.mode, submit_label));
    Dialog::new(&state.title, 70, height).render_text(frame, Text::from(lines));
}

pub(in crate::tui::ui) fn project_picker_submit_label(route: OverlayRoute) -> Option<&'static str> {
    match route {
        OverlayRoute::ViewProject => Some("open"),
        OverlayRoute::EditProject | OverlayRoute::AddTaskTitleProject => Some("submit"),
        OverlayRoute::DeleteProjectPicker => Some("delete"),
        _ => None,
    }
}

fn priority_picker_submit_label(route: OverlayRoute) -> Option<&'static str> {
    match route {
        OverlayRoute::EditPriority | OverlayRoute::AddTaskTitlePriority => Some("submit"),
        _ => None,
    }
}

pub(in crate::tui::ui) fn project_picker_line(item: &PickerItem, selected: bool) -> Line<'static> {
    let (prefix, name) = item
        .label
        .split_once(' ')
        .unwrap_or((item.label.as_str(), item.value.as_str()));
    let marker = if selected { "▸" } else { " " };
    let row_style = if selected {
        SELECTED
    } else {
        Style::new().bg(BG_ALT)
    };
    let project_style = Style::new()
        .fg(theme::project_color(&item.value))
        .add_modifier(Modifier::BOLD)
        .bg(row_style.bg.unwrap_or(BG_ALT));
    let name_style = Style::new()
        .fg(if selected { FG } else { FG_DIM })
        .bg(row_style.bg.unwrap_or(BG_ALT));
    Line::from(vec![
        Span::styled(format!("{marker} "), row_style),
        Span::styled(format!("{prefix:<7}"), project_style),
        Span::styled(" ", row_style),
        Span::styled(name.to_string(), name_style),
    ])
}

pub(in crate::tui::ui) fn project_picker_hint_line(
    mode: PickerMode,
    submit_label: &'static str,
) -> Line<'static> {
    picker_hint_line(mode, false, submit_label)
}
