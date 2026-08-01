use ratatui::Frame;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};

use super::super::dialog::{Dialog, dialog_hint_line};
use super::super::input::prefixed_input_line;
use crate::tui::overlay::{
    GENERIC_PICKER_VIEWPORT_ROWS, GENERIC_PICKER_WIDTH, LABEL_PICKER_WIDTH,
    PROJECT_PICKER_VIEWPORT_ROWS, PROJECT_PICKER_WIDTH, PickerItem, PickerKind, PickerMode,
    PickerView, picker_viewport_start,
};
use crate::tui::theme::{self, ACCENT, BG_ALT, BG_PANEL, FG, FG_DIM, SELECTED};
use crate::tui::widgets::priority_icon;

pub(in crate::tui::ui) fn render_picker(frame: &mut Frame, state: &PickerView) {
    if state.kind == PickerKind::LabelAdministration {
        render_label_picker(frame, state);
        return;
    }
    if let Some(submit_label) = project_picker_submit_label(state.kind) {
        render_project_picker(frame, state, submit_label);
        return;
    }

    let viewport_rows = GENERIC_PICKER_VIEWPORT_ROWS;
    let selected_position = picker_visible_start(state, viewport_rows);
    let mut lines = Vec::new();
    if matches!(state.mode, PickerMode::Filter) {
        lines.push(picker_filter_line(
            Span::raw("/"),
            &state.filter,
            state.filter_cursor,
        ));
        lines.push(Line::from(""));
    }
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
        if priority_picker_submit_label(state.kind).is_some() {
            lines.push(priority_picker_line(item, *index == state.selected));
        } else {
            lines.push(Line::from(format!("{marker}{}{check}", item.label)));
        }
    }
    lines.push(Line::from(""));
    lines.push(picker_hint_line(
        state.mode,
        state.multi,
        priority_picker_submit_label(state.kind).unwrap_or("submit"),
    ));
    let height = (lines.len() as u16).saturating_add(2);
    Dialog::new(&state.title, GENERIC_PICKER_WIDTH, height).render_text(frame, Text::from(lines));
}

fn picker_visible_start(state: &PickerView, viewport_rows: usize) -> usize {
    let selected_position = state
        .visible_indices
        .iter()
        .position(|index| *index == state.selected)
        .unwrap_or(0);
    picker_viewport_start(
        state.scroll,
        selected_position,
        state.visible_indices.len(),
        viewport_rows,
    )
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

pub(in crate::tui::ui) fn picker_filter_line(
    prefix: Span<'static>,
    filter: &str,
    cursor: usize,
) -> Line<'static> {
    prefixed_input_line(prefix, filter, cursor)
}

pub(in crate::tui::ui) fn picker_hint_line(
    mode: PickerMode,
    multi: bool,
    submit_label: &str,
) -> Line<'static> {
    let mut items = match mode {
        PickerMode::Navigate => vec![("j/k", "move"), ("/", "filter")],
        PickerMode::Filter => vec![("type", "filter"), ("Up/Down", "move"), ("Esc", "normal")],
    };
    if multi {
        items.push(("Space", "toggle"));
    }
    if matches!(mode, PickerMode::Navigate) {
        items.push(("Esc", "cancel"));
    }
    items.push(("Enter", submit_label));
    dialog_hint_line(&items)
}

fn render_label_picker(frame: &mut Frame, state: &PickerView) {
    let viewport_rows = GENERIC_PICKER_VIEWPORT_ROWS;
    let selected_position = picker_visible_start(state, viewport_rows);
    let mut lines = Vec::new();
    if matches!(state.mode, PickerMode::Filter) {
        lines.push(picker_filter_line(
            Span::styled("/", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
            &state.filter,
            state.filter_cursor,
        ));
    }
    lines.push(Line::from(vec![
        Span::styled(
            "  LABEL                         ",
            Style::new().fg(FG_DIM).bg(BG_PANEL),
        ),
        Span::styled("   TASKS", Style::new().fg(FG_DIM).bg(BG_PANEL)),
        Span::styled("  RECURRING SERIES", Style::new().fg(FG_DIM).bg(BG_PANEL)),
    ]));
    for index in state
        .visible_indices
        .iter()
        .skip(selected_position)
        .take(viewport_rows)
    {
        lines.push(label_picker_line(
            &state.items[*index],
            *index == state.selected,
        ));
    }
    if state.visible_indices.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no matching labels",
            Style::new().fg(FG_DIM),
        )));
    }
    lines.push(Line::from(""));
    lines.push(picker_hint_line(state.mode, false, "choose"));
    let height = (lines.len() as u16).saturating_add(2);
    Dialog::new(&state.title, LABEL_PICKER_WIDTH, height).render_text(frame, Text::from(lines));
}

pub(in crate::tui::ui) fn label_picker_line(item: &PickerItem, selected: bool) -> Line<'static> {
    let mut columns = item.label.splitn(3, "  ");
    let label = columns.next().unwrap_or(item.value.as_str());
    let task_count = columns
        .next()
        .and_then(|column| column.split_whitespace().next())
        .unwrap_or("0");
    let series_count = columns
        .next()
        .and_then(|column| column.split_whitespace().next())
        .unwrap_or("0");
    let marker = if selected { "▸" } else { " " };
    let row_style = if selected {
        SELECTED
    } else {
        Style::new().bg(BG_ALT)
    };
    let label_style = if selected {
        row_style.add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(FG).bg(BG_ALT)
    };
    Line::from(vec![
        Span::styled(format!("{marker} "), row_style),
        Span::styled(format!("{label:<30}"), label_style),
        Span::styled(format!("{task_count:>8}"), row_style),
        Span::styled(format!("{series_count:>18}"), row_style),
    ])
}

fn render_project_picker(frame: &mut Frame, state: &PickerView, submit_label: &'static str) {
    let viewport_rows = PROJECT_PICKER_VIEWPORT_ROWS;
    let height =
        (viewport_rows as u16).saturating_add(if matches!(state.mode, PickerMode::Filter) {
            6
        } else {
            5
        });
    let selected_position = picker_visible_start(state, viewport_rows);
    let mut lines = Vec::new();
    if matches!(state.mode, PickerMode::Filter) {
        lines.push(picker_filter_line(
            Span::styled("/", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
            &state.filter,
            state.filter_cursor,
        ));
    }
    lines.push(Line::from(vec![
        Span::styled("  PREFIX ", Style::new().fg(FG_DIM).bg(BG_PANEL)),
        Span::styled("PROJECT", Style::new().fg(FG_DIM).bg(BG_PANEL)),
    ]));
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
    Dialog::new(&state.title, PROJECT_PICKER_WIDTH, height).render_text(frame, Text::from(lines));
}

pub(in crate::tui::ui) fn project_picker_submit_label(kind: PickerKind) -> Option<&'static str> {
    match kind {
        PickerKind::ScopeProject => Some("scope"),
        PickerKind::ProjectPathProject => Some("select"),
        PickerKind::EditProject | PickerKind::AddTaskProject => Some("submit"),
        PickerKind::RenameProject => Some("rename"),
        PickerKind::DeleteProject => Some("delete"),
        _ => None,
    }
}

fn priority_picker_submit_label(kind: PickerKind) -> Option<&'static str> {
    matches!(kind, PickerKind::EditPriority | PickerKind::AddTaskPriority).then_some("submit")
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
    let project_color_key = if item
        .value
        .starts_with(crate::tui::store::CREATE_PROJECT_PICKER_VALUE_PREFIX)
    {
        crate::tui::store::CREATE_PROJECT_PICKER_VALUE_PREFIX
    } else {
        item.value.as_str()
    };
    let project_style = Style::new()
        .fg(theme::project_color(project_color_key))
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
