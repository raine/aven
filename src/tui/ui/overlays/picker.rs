use ratatui::Frame;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use unicode_width::UnicodeWidthStr;

use super::super::dialog::{Dialog, dialog_hint_line};
use super::super::input::prefixed_input_line;
use super::super::task_display::linked_task_ref_spans;
use crate::tui::overlay::{PickerItem, PickerKind, PickerMode, PickerView, picker_layout};
use crate::tui::text::truncate_width;
use crate::tui::theme;
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

    let layout = picker_layout(state, frame.area().as_size());
    let mut lines = Vec::new();
    if matches!(state.mode, PickerMode::Filter) {
        lines.push(picker_filter_line(
            Span::raw("/"),
            &state.filter,
            state.filter_cursor,
        ));
        lines.push(Line::from(""));
    }
    let blocker_ref_width = if state.kind == PickerKind::GoToBlocker {
        state
            .items
            .iter()
            .filter_map(blocker_picker_columns)
            .map(|(_, display_ref, _)| display_ref.width())
            .max()
            .unwrap_or(0)
    } else {
        0
    };
    for index in state
        .visible_indices
        .iter()
        .skip(layout.visible_start)
        .take(layout.visible_end - layout.visible_start)
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
        } else if state.kind == PickerKind::GoToBlocker {
            lines.push(blocker_picker_line(
                item,
                *index == state.selected,
                blocker_ref_width,
            ));
        } else {
            lines.push(Line::from(format!("{marker}{}{check}", item.label)));
        }
    }
    if state.visible_indices.is_empty()
        && let Some(label) = picker_empty_label(state.kind)
    {
        lines.push(Line::from(Span::styled(
            label,
            Style::new().fg(theme::fg_dim()),
        )));
    }
    lines.push(Line::from(""));
    lines.push(picker_hint_line_with_escape(
        state.mode,
        state.multi,
        priority_picker_submit_label(state.kind)
            .or_else(|| generic_picker_submit_label(state.kind))
            .unwrap_or("submit"),
        matches!(state.kind, PickerKind::SwitchWorkspace),
    ));
    Dialog::new(&state.title, 0, 0).render_text_at(frame, layout.area, Text::from(lines));
}

fn picker_empty_label(kind: PickerKind) -> Option<&'static str> {
    matches!(kind, PickerKind::SwitchWorkspace).then_some("  no matching workspaces")
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

fn blocker_picker_columns(item: &PickerItem) -> Option<(&str, &str, &str)> {
    let mut columns = item.label.splitn(3, "  ");
    Some((columns.next()?, columns.next()?, columns.next()?))
}

pub(in crate::tui::ui) fn blocker_picker_line(
    item: &PickerItem,
    selected: bool,
    reference_width: usize,
) -> Line<'static> {
    let marker = if selected { "▸ " } else { "  " };
    let Some((project_key, display_ref, title)) = blocker_picker_columns(item) else {
        return Line::raw(format!("{marker}{}", item.label));
    };
    let mut spans = vec![Span::raw(marker)];
    spans.extend(linked_task_ref_spans(display_ref, project_key));
    spans.push(Span::raw(format!(
        "{}  ",
        " ".repeat(reference_width.saturating_sub(display_ref.width()))
    )));
    spans.push(Span::styled(
        title.to_string(),
        Style::new().fg(if selected {
            theme::fg()
        } else {
            theme::fg_muted()
        }),
    ));
    Line::from(spans)
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
    picker_hint_line_with_escape(mode, multi, submit_label, false)
}

fn picker_hint_line_with_escape(
    mode: PickerMode,
    multi: bool,
    submit_label: &str,
    filter_escape_cancels: bool,
) -> Line<'static> {
    let mut items = match mode {
        PickerMode::Navigate => vec![("j/k", "move"), ("/", "filter")],
        PickerMode::Filter if filter_escape_cancels => {
            vec![("type", "filter"), ("Up/Down", "move"), ("Esc", "cancel")]
        }
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
    let layout = picker_layout(state, frame.area().as_size());
    let mut lines = Vec::new();
    if matches!(state.mode, PickerMode::Filter) {
        lines.push(picker_filter_line(
            Span::styled(
                "/",
                Style::new()
                    .fg(theme::accent())
                    .add_modifier(Modifier::BOLD),
            ),
            &state.filter,
            state.filter_cursor,
        ));
    }
    lines.push(Line::from(vec![
        Span::styled(
            "  LABEL                         ",
            Style::new().fg(theme::fg_dim()).bg(theme::bg_panel()),
        ),
        Span::styled(
            "   TASKS",
            Style::new().fg(theme::fg_dim()).bg(theme::bg_panel()),
        ),
        Span::styled(
            "  RECURRING SERIES",
            Style::new().fg(theme::fg_dim()).bg(theme::bg_panel()),
        ),
    ]));
    for index in state
        .visible_indices
        .iter()
        .skip(layout.visible_start)
        .take(layout.visible_end - layout.visible_start)
    {
        lines.push(label_picker_line(
            &state.items[*index],
            *index == state.selected,
        ));
    }
    if state.visible_indices.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no matching labels",
            Style::new().fg(theme::fg_dim()),
        )));
    }
    lines.push(Line::from(""));
    lines.push(picker_hint_line(state.mode, false, "choose"));
    Dialog::new(&state.title, 0, 0).render_text_at(frame, layout.area, Text::from(lines));
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
        theme::selected()
    } else {
        Style::new().bg(theme::bg_alt())
    };
    let label_style = if selected {
        row_style.add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(theme::fg()).bg(theme::bg_alt())
    };
    let label = truncate_width(label, 30);
    let label_padding = 30usize.saturating_sub(label.width());
    Line::from(vec![
        Span::styled(format!("{marker} "), row_style),
        Span::styled(format!("{label}{}", " ".repeat(label_padding)), label_style),
        Span::styled(format!("{task_count:>8}"), row_style),
        Span::styled(format!("{series_count:>18}"), row_style),
    ])
}

fn render_project_picker(frame: &mut Frame, state: &PickerView, submit_label: &'static str) {
    let layout = picker_layout(state, frame.area().as_size());
    let mut lines = Vec::new();
    if matches!(state.mode, PickerMode::Filter) {
        lines.push(picker_filter_line(
            Span::styled(
                "/",
                Style::new()
                    .fg(theme::accent())
                    .add_modifier(Modifier::BOLD),
            ),
            &state.filter,
            state.filter_cursor,
        ));
    }
    lines.push(Line::from(vec![
        Span::styled(
            "  PREFIX ",
            Style::new().fg(theme::fg_dim()).bg(theme::bg_panel()),
        ),
        Span::styled(
            "PROJECT",
            Style::new().fg(theme::fg_dim()).bg(theme::bg_panel()),
        ),
    ]));
    let list_start = lines.len();
    for index in state
        .visible_indices
        .iter()
        .skip(layout.visible_start)
        .take(layout.visible_end - layout.visible_start)
    {
        lines.push(project_picker_line(
            &state.items[*index],
            *index == state.selected,
        ));
    }
    if state.visible_indices.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no matching projects",
            Style::new().fg(theme::fg_dim()),
        )));
    }
    while lines.len().saturating_sub(list_start) < layout.list_rows {
        lines.push(Line::from(""));
    }
    lines.push(Line::from(""));
    lines.push(project_picker_hint_line(
        state.mode,
        submit_label,
        state.kind == PickerKind::ScopeProject,
    ));
    Dialog::new(&state.title, 0, 0).render_text_at(frame, layout.area, Text::from(lines));
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

fn generic_picker_submit_label(kind: PickerKind) -> Option<&'static str> {
    matches!(kind, PickerKind::SwitchWorkspace).then_some("switch")
}

fn priority_picker_submit_label(kind: PickerKind) -> Option<&'static str> {
    matches!(kind, PickerKind::AddTaskPriority).then_some("submit")
}

pub(in crate::tui::ui) fn project_picker_line(item: &PickerItem, selected: bool) -> Line<'static> {
    let (prefix, name) = item
        .label
        .split_once(' ')
        .unwrap_or((item.label.as_str(), item.value.as_str()));
    let marker = if selected { "▸" } else { " " };
    let row_style = if selected {
        theme::selected()
    } else {
        Style::new().bg(theme::bg_alt())
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
        .bg(row_style.bg.unwrap_or(theme::bg_alt()));
    let name_style = Style::new()
        .fg(if selected {
            theme::fg()
        } else {
            theme::fg_dim()
        })
        .bg(row_style.bg.unwrap_or(theme::bg_alt()));
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
    filter_escape_cancels: bool,
) -> Line<'static> {
    picker_hint_line_with_escape(mode, false, submit_label, filter_escape_cancels)
}
