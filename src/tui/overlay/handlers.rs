use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Rect, Size};

use super::scroll::{ScrollKeyOutcome, ScrollState, handle_scroll_key};
use crate::tui::authoring::AddTaskStep;
use crate::tui::navigation::scroll_with_delta;
use crate::tui::ui::text_panel_scroll_cap;

use super::multiline::edit_multiline_input;
use super::picker::{
    handle_picker_key, normalize_picker_scroll, normalize_picker_selection, picker_submit_outcome,
    visible_picker_indices,
};
use super::state::{
    AddTaskMode, ConfirmState, HeaderMenuState, OrderMenuState, OverlayOutcome, OverlayState,
    OverlaySubmit, PickerMode, PickerState, TagComboboxState, TextPanelState,
};
use super::tag_combobox::{
    handle_tag_combobox_key, normalize_tag_combobox_highlight, tag_combobox_matches,
};
use crate::tui::overlay::{confirm_layout, picker_layout, tag_combobox_layout, text_panel_layout};
use crate::tui::store::TaskOrder;

pub(crate) fn handle_generic_overlay_paste(text: &str, overlay: OverlayState) -> OverlayState {
    match overlay {
        OverlayState::Search(mut state) => {
            state.input.insert_paste(text);
            state.results.clear();
            state.selected = 0;
            OverlayState::Search(state)
        }
        OverlayState::Command { mut state } => {
            state.input.insert_paste(text);
            state.reset_cycle();
            OverlayState::Command { state }
        }
        OverlayState::AddTask(mut state) => {
            match state.focus {
                AddTaskStep::Title => state.title.insert_paste(text),
                AddTaskStep::AvailableAt => state.available_at.insert_paste(text),
                AddTaskStep::Due => state.due_on.insert_paste(text),
                AddTaskStep::Description => state.description.insert_paste(text),
                _ => {}
            }
            OverlayState::AddTask(state)
        }
        OverlayState::TextInput(mut state) => {
            state.input.insert_paste(text);
            OverlayState::TextInput(state)
        }
        OverlayState::MultilineInput(mut state) => {
            state.insert_paste(text);
            OverlayState::MultilineInput(state)
        }
        OverlayState::Picker(mut state) => {
            state.filter.insert_paste(text);
            normalize_picker_selection(&mut state);
            normalize_picker_scroll(
                &mut state,
                crate::tui::overlay::GENERIC_PICKER_VIEWPORT_ROWS,
            );
            OverlayState::Picker(state)
        }
        OverlayState::TagCombobox(mut state) => {
            state.input.insert_paste(text);
            normalize_tag_combobox_highlight(&mut state);
            OverlayState::TagCombobox(state)
        }
        other => other,
    }
}

pub(crate) fn handle_generic_overlay_mouse(
    overlay: OverlayState,
    mouse: MouseEvent,
    terminal_size: Size,
) -> OverlayOutcome {
    match overlay {
        OverlayState::Picker(state) => handle_picker_mouse(state, mouse, terminal_size),
        OverlayState::TagCombobox(state) => handle_tag_combobox_mouse(state, mouse, terminal_size),
        OverlayState::Confirm(state) => handle_confirm_mouse(state, mouse, terminal_size),
        OverlayState::TextPanel(state) => handle_text_panel_mouse(state, mouse, terminal_size),
        other => OverlayOutcome::None(other),
    }
}

pub(crate) fn handle_generic_overlay_key(
    key: KeyEvent,
    overlay: OverlayState,
    help_scroll_cap: u16,
) -> OverlayOutcome {
    match overlay {
        OverlayState::AddTask(mut state) => {
            match std::mem::replace(&mut state.mode, AddTaskMode::Compose) {
                AddTaskMode::Picker {
                    field,
                    state: picker,
                } => {
                    match handle_picker_key(picker, key) {
                        OverlayOutcome::None(OverlayState::Picker(picker)) => {
                            state.mode = AddTaskMode::Picker {
                                field,
                                state: picker,
                            };
                        }
                        OverlayOutcome::Cancelled => {}
                        OverlayOutcome::Submitted(OverlaySubmit::Picker { values, .. }) => {
                            let value = values.first().cloned().unwrap_or_default();
                            match field {
                                AddTaskStep::Project => {
                                    state.selected_project =
                                        (!value.is_empty()).then_some(value.clone());
                                    state.project = if value.is_empty() {
                                        state
                                            .inferred_project
                                            .clone()
                                            .unwrap_or_else(|| "no project".to_string())
                                    } else {
                                        value
                                    };
                                }
                                AddTaskStep::Status => state.status = value,
                                AddTaskStep::Priority => state.priority = value,
                                _ => {}
                            }
                        }
                        _ => {}
                    }
                    return OverlayOutcome::None(OverlayState::AddTask(state));
                }
                AddTaskMode::Labels(labels) => {
                    match handle_tag_combobox_key(labels, key) {
                        OverlayOutcome::None(OverlayState::TagCombobox(labels)) => {
                            state.mode = AddTaskMode::Labels(labels);
                        }
                        OverlayOutcome::Cancelled => {}
                        OverlayOutcome::Submitted(OverlaySubmit::Picker { values, .. }) => {
                            state.labels = values;
                        }
                        _ => {}
                    }
                    return OverlayOutcome::None(OverlayState::AddTask(state));
                }
                AddTaskMode::Help { mut scroll } => {
                    scroll = scroll.min(help_scroll_cap);
                    match key.code {
                        KeyCode::Esc | KeyCode::Enter | KeyCode::Char('?') => {}
                        KeyCode::Down | KeyCode::Char('j') => {
                            scroll = scroll.saturating_add(1).min(help_scroll_cap);
                            state.mode = AddTaskMode::Help { scroll };
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            scroll = scroll.saturating_sub(1);
                            state.mode = AddTaskMode::Help { scroll };
                        }
                        _ => state.mode = AddTaskMode::Help { scroll },
                    }
                    return OverlayOutcome::None(OverlayState::AddTask(state));
                }
                AddTaskMode::ConfirmDiscard => {
                    return match key.code {
                        KeyCode::Char('y') | KeyCode::Char('Y') => OverlayOutcome::Cancelled,
                        KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                            OverlayOutcome::None(OverlayState::AddTask(state))
                        }
                        _ => {
                            state.mode = AddTaskMode::ConfirmDiscard;
                            OverlayOutcome::None(OverlayState::AddTask(state))
                        }
                    };
                }
                AddTaskMode::Compose => {}
            }

            match key.code {
                KeyCode::Esc if state.is_populated() => {
                    state.mode = AddTaskMode::ConfirmDiscard;
                    OverlayOutcome::None(OverlayState::AddTask(state))
                }
                KeyCode::Esc => OverlayOutcome::Cancelled,
                KeyCode::Tab | KeyCode::BackTab => {
                    state.focus_next(key.code == KeyCode::BackTab);
                    OverlayOutcome::None(OverlayState::AddTask(state))
                }
                KeyCode::Left if state.focus == AddTaskStep::Images => {
                    let count = state.attachments.len();
                    if count > 0 {
                        state.selected_attachment = state
                            .selected_attachment
                            .checked_sub(1)
                            .unwrap_or(count - 1);
                    }
                    OverlayOutcome::None(OverlayState::AddTask(state))
                }
                KeyCode::Right if state.focus == AddTaskStep::Images => {
                    let count = state.attachments.len();
                    if count > 0 {
                        state.selected_attachment = (state.selected_attachment + 1) % count;
                    }
                    OverlayOutcome::None(OverlayState::AddTask(state))
                }
                KeyCode::Left
                    if state.focus.is_metadata()
                        && !matches!(state.focus, AddTaskStep::AvailableAt | AddTaskStep::Due) =>
                {
                    state.focus = match state.focus {
                        AddTaskStep::Project => AddTaskStep::Due,
                        AddTaskStep::Status => AddTaskStep::Project,
                        AddTaskStep::Priority => AddTaskStep::Status,
                        AddTaskStep::Labels => AddTaskStep::Priority,
                        _ => state.focus,
                    };
                    OverlayOutcome::None(OverlayState::AddTask(state))
                }
                KeyCode::Right
                    if state.focus.is_metadata()
                        && !matches!(state.focus, AddTaskStep::AvailableAt | AddTaskStep::Due) =>
                {
                    state.focus = match state.focus {
                        AddTaskStep::Project => AddTaskStep::Status,
                        AddTaskStep::Status => AddTaskStep::Priority,
                        AddTaskStep::Priority => AddTaskStep::Labels,
                        AddTaskStep::Labels => AddTaskStep::AvailableAt,
                        _ => state.focus,
                    };
                    OverlayOutcome::None(OverlayState::AddTask(state))
                }
                KeyCode::Down if state.focus.is_metadata() => {
                    state.focus = if state.attachments.is_empty() {
                        AddTaskStep::Title
                    } else {
                        AddTaskStep::Images
                    };
                    OverlayOutcome::None(OverlayState::AddTask(state))
                }
                KeyCode::Up if state.focus == AddTaskStep::Images => {
                    state.focus = AddTaskStep::Project;
                    OverlayOutcome::None(OverlayState::AddTask(state))
                }
                KeyCode::Down if state.focus == AddTaskStep::Images => {
                    state.focus = AddTaskStep::Title;
                    OverlayOutcome::None(OverlayState::AddTask(state))
                }
                KeyCode::Up if state.focus == AddTaskStep::Title => {
                    state.focus = if state.attachments.is_empty() {
                        AddTaskStep::Project
                    } else {
                        AddTaskStep::Images
                    };
                    OverlayOutcome::None(OverlayState::AddTask(state))
                }
                KeyCode::Down if state.focus == AddTaskStep::Title => {
                    state.focus = AddTaskStep::Description;
                    OverlayOutcome::None(OverlayState::AddTask(state))
                }
                KeyCode::Up
                    if state.focus == AddTaskStep::Description && state.description.row == 0 =>
                {
                    state.focus = AddTaskStep::Title;
                    OverlayOutcome::None(OverlayState::AddTask(state))
                }
                KeyCode::F(1) => {
                    state.mode = AddTaskMode::Help { scroll: 0 };
                    OverlayOutcome::None(OverlayState::AddTask(state))
                }
                KeyCode::Char('?')
                    if state.focus.is_metadata()
                        && !matches!(state.focus, AddTaskStep::AvailableAt | AddTaskStep::Due) =>
                {
                    state.mode = AddTaskMode::Help { scroll: 0 };
                    OverlayOutcome::None(OverlayState::AddTask(state))
                }
                KeyCode::Enter if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    OverlayOutcome::Submitted(OverlaySubmit::AddTask(state))
                }
                KeyCode::Enter if state.focus == AddTaskStep::Title => {
                    OverlayOutcome::Submitted(OverlaySubmit::AddTask(state))
                }
                KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    OverlayOutcome::Submitted(OverlaySubmit::AddTask(state))
                }
                _ => {
                    match state.focus {
                        AddTaskStep::Title => {
                            state.title.handle_key(key);
                            state.title_error = false;
                        }
                        AddTaskStep::AvailableAt => state.available_at.handle_key(key),
                        AddTaskStep::Due => state.due_on.handle_key(key),
                        AddTaskStep::Description => {
                            edit_multiline_input(&mut state.description, key)
                        }
                        _ => {}
                    }
                    OverlayOutcome::None(OverlayState::AddTask(state))
                }
            }
        }
        OverlayState::TextInput(mut state) => match key.code {
            KeyCode::Esc => OverlayOutcome::Cancelled,
            KeyCode::Enter => OverlayOutcome::Submitted(OverlaySubmit::Text {
                route: state.route,
                value: state.input.text.clone(),
            }),
            _ => {
                state.input.handle_key(key);
                OverlayOutcome::None(OverlayState::TextInput(state))
            }
        },
        OverlayState::MultilineInput(mut state) => {
            if (key.code == KeyCode::Char('s') && key.modifiers.contains(KeyModifiers::CONTROL))
                || (key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::CONTROL))
            {
                let value = state.lines.join("\n");
                return OverlayOutcome::Submitted(OverlaySubmit::Multiline {
                    route: state.route,
                    value,
                });
            }
            match key.code {
                KeyCode::Esc => OverlayOutcome::Cancelled,
                _ => {
                    edit_multiline_input(&mut state, key);
                    OverlayOutcome::None(OverlayState::MultilineInput(state))
                }
            }
        }
        OverlayState::Picker(state) => handle_picker_key(state, key),
        OverlayState::TagCombobox(state) => handle_tag_combobox_key(state, key),
        OverlayState::HeaderMenu(state) => handle_header_menu_key(state, key),
        OverlayState::OrderMenu(state) => handle_order_menu_key(state, key),
        OverlayState::Confirm(state) => match key.code {
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => OverlayOutcome::Cancelled,
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                OverlayOutcome::Submitted(OverlaySubmit::Confirm { route: state.route })
            }
            _ => OverlayOutcome::None(OverlayState::Confirm(state)),
        },
        OverlayState::TextPanel(mut state) => {
            let cap = text_panel_scroll_cap(&state.lines);
            match handle_scroll_key(
                key,
                ScrollState {
                    scroll: state.scroll,
                    cap,
                },
                &[KeyCode::Esc, KeyCode::Enter],
                0,
            ) {
                ScrollKeyOutcome::Cancelled => OverlayOutcome::Cancelled,
                ScrollKeyOutcome::Continue(s) => {
                    state.scroll = s.scroll;
                    OverlayOutcome::None(OverlayState::TextPanel(state))
                }
                ScrollKeyOutcome::Ignored => OverlayOutcome::None(OverlayState::TextPanel(state)),
            }
        }
        OverlayState::SyncStatus(state) => match key.code {
            KeyCode::Esc | KeyCode::Enter => OverlayOutcome::Cancelled,
            _ => OverlayOutcome::None(OverlayState::SyncStatus(state)),
        },
        OverlayState::DatabaseStats { stats, scroll } => {
            match handle_scroll_key(
                key,
                ScrollState {
                    scroll,
                    cap: help_scroll_cap,
                },
                &[KeyCode::Esc, KeyCode::Enter],
                0,
            ) {
                ScrollKeyOutcome::Cancelled => OverlayOutcome::Cancelled,
                ScrollKeyOutcome::Continue(s) => {
                    OverlayOutcome::None(OverlayState::DatabaseStats {
                        stats,
                        scroll: s.scroll,
                    })
                }
                ScrollKeyOutcome::Ignored => {
                    OverlayOutcome::None(OverlayState::DatabaseStats { stats, scroll })
                }
            }
        }
        OverlayState::Help { scroll } => {
            match handle_scroll_key(
                key,
                ScrollState {
                    scroll,
                    cap: help_scroll_cap,
                },
                &[KeyCode::Esc, KeyCode::Enter],
                0,
            ) {
                ScrollKeyOutcome::Cancelled => OverlayOutcome::Cancelled,
                ScrollKeyOutcome::Continue(s) => {
                    OverlayOutcome::None(OverlayState::Help { scroll: s.scroll })
                }
                ScrollKeyOutcome::Ignored => OverlayOutcome::None(OverlayState::Help { scroll }),
            }
        }
        OverlayState::DetailHelp { scroll } => {
            match handle_scroll_key(
                key,
                ScrollState {
                    scroll,
                    cap: help_scroll_cap,
                },
                &[KeyCode::Esc, KeyCode::Enter, KeyCode::Char('?')],
                0,
            ) {
                ScrollKeyOutcome::Cancelled => OverlayOutcome::Cancelled,
                ScrollKeyOutcome::Continue(s) => {
                    OverlayOutcome::None(OverlayState::DetailHelp { scroll: s.scroll })
                }
                ScrollKeyOutcome::Ignored => {
                    OverlayOutcome::None(OverlayState::DetailHelp { scroll })
                }
            }
        }
        OverlayState::Detail { scroll } => {
            match handle_scroll_key(
                key,
                ScrollState {
                    scroll,
                    cap: u16::MAX,
                },
                &[KeyCode::Esc, KeyCode::Enter],
                0,
            ) {
                ScrollKeyOutcome::Cancelled => OverlayOutcome::Cancelled,
                ScrollKeyOutcome::Continue(s) => {
                    OverlayOutcome::None(OverlayState::Detail { scroll: s.scroll })
                }
                ScrollKeyOutcome::Ignored => OverlayOutcome::None(OverlayState::Detail { scroll }),
            }
        }
        other => OverlayOutcome::None(other),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PickerMouseTarget {
    Outside,
    Filter,
    Row(usize),
    Interior,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfirmMouseTarget {
    Yes,
    No,
    Cancel,
    Interior,
    Outside,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TagComboboxMouseTarget {
    Outside,
    Input,
    Row(usize),
    Interior,
}

fn handle_tag_combobox_mouse(
    mut state: TagComboboxState,
    mouse: MouseEvent,
    terminal_size: Size,
) -> OverlayOutcome {
    match mouse.kind {
        MouseEventKind::ScrollDown => {
            move_tag_combobox_highlight(&mut state, 1);
            return OverlayOutcome::None(OverlayState::TagCombobox(state));
        }
        MouseEventKind::ScrollUp => {
            move_tag_combobox_highlight(&mut state, -1);
            return OverlayOutcome::None(OverlayState::TagCombobox(state));
        }
        MouseEventKind::Down(MouseButton::Left) => {}
        _ => return OverlayOutcome::None(OverlayState::TagCombobox(state)),
    }

    match tag_combobox_mouse_target(&state, mouse.column, mouse.row, terminal_size) {
        TagComboboxMouseTarget::Outside => OverlayOutcome::Cancelled,
        TagComboboxMouseTarget::Input => OverlayOutcome::None(OverlayState::TagCombobox(state)),
        TagComboboxMouseTarget::Row(index) => {
            state.highlighted = index;
            let Some(label) = state.options.get(index).cloned() else {
                return OverlayOutcome::None(OverlayState::TagCombobox(state));
            };
            if let Some(selected) = state.selected.iter().position(|item| item == &label) {
                state.selected.remove(selected);
            } else {
                state.selected.push(label);
            }
            OverlayOutcome::None(OverlayState::TagCombobox(state))
        }
        TagComboboxMouseTarget::Interior => OverlayOutcome::None(OverlayState::TagCombobox(state)),
    }
}

fn tag_combobox_mouse_target(
    state: &TagComboboxState,
    column: u16,
    row: u16,
    terminal_size: Size,
) -> TagComboboxMouseTarget {
    let view = crate::tui::overlay::TagComboboxView {
        route: state.route,
        title: state.title.clone(),
        input: state.input.text.clone(),
        input_cursor: state.input.cursor,
        completion: None,
        options: state.options.clone(),
        selected: state.selected.clone(),
        highlighted: state.highlighted,
        visible_indices: tag_combobox_matches(state),
        visible_start: 0,
    };
    let layout = tag_combobox_layout(&view, terminal_size);
    if !layout
        .area
        .contains(ratatui::layout::Position { x: column, y: row })
    {
        return TagComboboxMouseTarget::Outside;
    }
    let inner_column = column.saturating_sub(layout.inner.x);
    let inner_row = row.saturating_sub(layout.inner.y);
    if inner_column >= layout.inner.width {
        return TagComboboxMouseTarget::Interior;
    }
    if inner_row == layout.input_row {
        return TagComboboxMouseTarget::Input;
    }
    if inner_row >= layout.list_start
        && inner_row
            < layout
                .list_start
                .saturating_add(layout.viewport_rows as u16)
    {
        let offset = (inner_row - layout.list_start) as usize;
        let visible = tag_combobox_matches(state);
        return visible
            .get(layout.visible_start + offset)
            .copied()
            .map(TagComboboxMouseTarget::Row)
            .unwrap_or(TagComboboxMouseTarget::Interior);
    }
    TagComboboxMouseTarget::Interior
}

pub(crate) fn wrap_index_by_value(
    indices: &[usize],
    current_value: usize,
    delta: isize,
) -> Option<usize> {
    if indices.is_empty() {
        return None;
    }
    let current = indices
        .iter()
        .position(|index| *index == current_value)
        .unwrap_or(0) as isize;
    let next = (current + delta).rem_euclid(indices.len() as isize) as usize;
    indices.get(next).copied()
}

fn move_tag_combobox_highlight(state: &mut TagComboboxState, delta: isize) {
    if let Some(next) = wrap_index_by_value(&tag_combobox_matches(state), state.highlighted, delta)
    {
        state.highlighted = next;
    }
}

fn handle_picker_mouse(
    mut state: PickerState,
    mouse: MouseEvent,
    terminal_size: Size,
) -> OverlayOutcome {
    if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
        return OverlayOutcome::None(OverlayState::Picker(state));
    }
    match picker_mouse_target(&state, mouse.column, mouse.row, terminal_size) {
        PickerMouseTarget::Outside => OverlayOutcome::Cancelled,
        PickerMouseTarget::Filter => {
            state.mode = PickerMode::Filter;
            OverlayOutcome::None(OverlayState::Picker(state))
        }
        PickerMouseTarget::Row(index) => {
            state.selected = index;
            if state.multi {
                state.items[index].selected = !state.items[index].selected;
                OverlayOutcome::None(OverlayState::Picker(state))
            } else {
                picker_submit_outcome(state)
            }
        }
        PickerMouseTarget::Interior => OverlayOutcome::None(OverlayState::Picker(state)),
    }
}

fn picker_mouse_target(
    state: &PickerState,
    column: u16,
    row: u16,
    terminal_size: Size,
) -> PickerMouseTarget {
    let view = crate::tui::overlay::PickerView {
        route: state.route,
        title: state.title.clone(),
        filter: state.filter.text.clone(),
        filter_cursor: state.filter.cursor,
        items: state.items.clone(),
        selected: state.selected,
        multi: state.multi,
        mode: state.mode,
        visible_indices: visible_picker_indices(state),
        scroll: state.scroll,
    };
    let layout = picker_layout(&view, terminal_size);
    if !contains(layout.area, column, row) {
        return PickerMouseTarget::Outside;
    }
    if !contains(layout.inner, column, row) {
        return PickerMouseTarget::Interior;
    }
    let inner_row = row.saturating_sub(layout.inner.y);
    if inner_row == 0 {
        return PickerMouseTarget::Filter;
    }
    let Some(row_offset) = inner_row.checked_sub(layout.list_start) else {
        return PickerMouseTarget::Interior;
    };
    if row_offset >= layout.viewport_rows as u16 {
        return PickerMouseTarget::Interior;
    }
    let visible_position = layout.visible_start.saturating_add(row_offset as usize);
    match view.visible_indices.get(visible_position) {
        Some(index) => PickerMouseTarget::Row(*index),
        None => PickerMouseTarget::Interior,
    }
}

fn handle_confirm_mouse(
    state: ConfirmState,
    mouse: MouseEvent,
    terminal_size: Size,
) -> OverlayOutcome {
    if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
        return OverlayOutcome::None(OverlayState::Confirm(state));
    }
    match confirm_mouse_target(&state.prompt, mouse.column, mouse.row, terminal_size) {
        ConfirmMouseTarget::Yes => {
            OverlayOutcome::Submitted(OverlaySubmit::Confirm { route: state.route })
        }
        ConfirmMouseTarget::No | ConfirmMouseTarget::Cancel | ConfirmMouseTarget::Outside => {
            OverlayOutcome::Cancelled
        }
        ConfirmMouseTarget::Interior => OverlayOutcome::None(OverlayState::Confirm(state)),
    }
}

fn confirm_mouse_target(
    prompt: &str,
    column: u16,
    row: u16,
    terminal_size: Size,
) -> ConfirmMouseTarget {
    let layout = confirm_layout(terminal_size, prompt);
    if !contains(layout.area, column, row) {
        return ConfirmMouseTarget::Outside;
    }
    if !contains(layout.inner, column, row) {
        return ConfirmMouseTarget::Interior;
    }
    if row.saturating_sub(layout.inner.y) != layout.hint_row {
        return ConfirmMouseTarget::Interior;
    }
    match column.saturating_sub(layout.inner.x) {
        0..=4 => ConfirmMouseTarget::Yes,
        7..=10 => ConfirmMouseTarget::No,
        13..=22 => ConfirmMouseTarget::Cancel,
        _ => ConfirmMouseTarget::Interior,
    }
}

fn handle_text_panel_mouse(
    mut state: TextPanelState,
    mouse: MouseEvent,
    terminal_size: Size,
) -> OverlayOutcome {
    let cap = text_panel_scroll_cap(&state.lines);
    match mouse.kind {
        MouseEventKind::ScrollDown => {
            state.scroll = scroll_with_delta(state.scroll, 1, cap);
            OverlayOutcome::None(OverlayState::TextPanel(state))
        }
        MouseEventKind::ScrollUp => {
            state.scroll = scroll_with_delta(state.scroll, -1, cap);
            OverlayOutcome::None(OverlayState::TextPanel(state))
        }
        MouseEventKind::Down(MouseButton::Left) => {
            let layout = text_panel_layout(terminal_size, state.lines.len());
            if contains(layout.area, mouse.column, mouse.row) {
                OverlayOutcome::None(OverlayState::TextPanel(state))
            } else {
                OverlayOutcome::Cancelled
            }
        }
        _ => OverlayOutcome::None(OverlayState::TextPanel(state)),
    }
}

fn contains(area: Rect, column: u16, row: u16) -> bool {
    column >= area.x
        && column < area.x.saturating_add(area.width)
        && row >= area.y
        && row < area.y.saturating_add(area.height)
}

fn handle_header_menu_key(mut state: HeaderMenuState, key: KeyEvent) -> OverlayOutcome {
    match key.code {
        KeyCode::Esc => OverlayOutcome::Cancelled,
        KeyCode::Enter => match state.selected_action() {
            Some(action) => OverlayOutcome::Submitted(OverlaySubmit::HeaderMenu { action }),
            None => OverlayOutcome::Cancelled,
        },
        KeyCode::Char('j') | KeyCode::Down => {
            if !state.items.is_empty() {
                state.selected = (state.selected + 1) % state.items.len();
            }
            OverlayOutcome::None(OverlayState::HeaderMenu(state))
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if !state.items.is_empty() {
                state.selected = state
                    .selected
                    .checked_sub(1)
                    .unwrap_or(state.items.len().saturating_sub(1));
            }
            OverlayOutcome::None(OverlayState::HeaderMenu(state))
        }
        KeyCode::Char(ch) => match state.items.iter().find(|item| item.key == ch.to_string()) {
            Some(item) => OverlayOutcome::Submitted(OverlaySubmit::HeaderMenu {
                action: item.action.clone(),
            }),
            None => OverlayOutcome::None(OverlayState::HeaderMenu(state)),
        },
        _ => OverlayOutcome::None(OverlayState::HeaderMenu(state)),
    }
}

fn handle_order_menu_key(mut state: OrderMenuState, key: KeyEvent) -> OverlayOutcome {
    match key.code {
        KeyCode::Esc => OverlayOutcome::Cancelled,
        KeyCode::Enter => OverlayOutcome::Submitted(OverlaySubmit::Order {
            order: state.selected,
        }),
        KeyCode::Char('j') | KeyCode::Down => {
            state.selected = next_order(state.selected);
            OverlayOutcome::None(OverlayState::OrderMenu(state))
        }
        KeyCode::Char('k') | KeyCode::Up => {
            state.selected = previous_order(state.selected);
            OverlayOutcome::None(OverlayState::OrderMenu(state))
        }
        KeyCode::Char('d') => OverlayOutcome::Submitted(OverlaySubmit::Order {
            order: TaskOrder::DueOn,
        }),
        KeyCode::Char('c') => OverlayOutcome::Submitted(OverlaySubmit::Order {
            order: TaskOrder::Created,
        }),
        KeyCode::Char('u') => OverlayOutcome::Submitted(OverlaySubmit::Order {
            order: TaskOrder::Updated,
        }),
        KeyCode::Char('p') => OverlayOutcome::Submitted(OverlaySubmit::Order {
            order: TaskOrder::Priority,
        }),
        KeyCode::Char('g') => OverlayOutcome::Submitted(OverlaySubmit::Order {
            order: TaskOrder::Project,
        }),
        KeyCode::Char('t') => OverlayOutcome::Submitted(OverlaySubmit::Order {
            order: TaskOrder::Title,
        }),
        _ => OverlayOutcome::None(OverlayState::OrderMenu(state)),
    }
}

fn next_order(order: TaskOrder) -> TaskOrder {
    match order {
        TaskOrder::DueOn => TaskOrder::Created,
        TaskOrder::Created => TaskOrder::Updated,
        TaskOrder::Updated => TaskOrder::Priority,
        TaskOrder::Priority => TaskOrder::Project,
        TaskOrder::Project => TaskOrder::Title,
        TaskOrder::Title => TaskOrder::DueOn,
    }
}

fn previous_order(order: TaskOrder) -> TaskOrder {
    match order {
        TaskOrder::DueOn => TaskOrder::Title,
        TaskOrder::Created => TaskOrder::DueOn,
        TaskOrder::Updated => TaskOrder::Created,
        TaskOrder::Priority => TaskOrder::Updated,
        TaskOrder::Project => TaskOrder::Priority,
        TaskOrder::Title => TaskOrder::Project,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::overlay::{
        LineEdit, MultilineInputState, OverlayRoute, PickerItem, TextInputState,
    };

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    fn pending_attachment(filename: &str) -> crate::tui::authoring::PendingTaskAttachmentSummary {
        crate::tui::authoring::PendingTaskAttachmentSummary {
            filename: filename.to_string(),
            byte_size: 4,
            dimensions: Some((1, 1)),
        }
    }

    fn add_task_state(focus: AddTaskStep) -> Box<crate::tui::overlay::AddTaskState> {
        Box::new(crate::tui::overlay::AddTaskState {
            title: LineEdit::blank(),
            description: MultilineInputState::blank(
                OverlayRoute::AddTaskDescription,
                "Add task: description",
                "",
            ),
            focus,
            project: "aven".to_string(),
            inferred_project: None,
            selected_project: Some("aven".to_string()),
            initial_project: Some("aven".to_string()),
            status: "inbox".to_string(),
            priority: "none".to_string(),
            labels: Vec::new(),
            available_at: LineEdit::blank(),
            due_on: LineEdit::blank(),
            attachments: Vec::new(),
            selected_attachment: 0,
            mode: crate::tui::overlay::AddTaskMode::Compose,
            title_error: false,
        })
    }

    fn handle(key: KeyEvent, overlay: OverlayState) -> OverlayOutcome {
        handle_generic_overlay_key(key, overlay, 100)
    }

    #[test]
    fn composer_help_scroll_stops_at_viewport_cap() {
        let mut state = add_task_state(AddTaskStep::Title);
        state.mode = AddTaskMode::Help { scroll: 10 };

        let OverlayOutcome::None(OverlayState::AddTask(state)) =
            handle_generic_overlay_key(key(KeyCode::Char('k')), OverlayState::AddTask(state), 2)
        else {
            panic!("expected add task state");
        };
        assert_eq!(state.mode, AddTaskMode::Help { scroll: 1 });

        let OverlayOutcome::None(OverlayState::AddTask(state)) =
            handle_generic_overlay_key(key(KeyCode::Char('j')), OverlayState::AddTask(state), 0)
        else {
            panic!("expected add task state");
        };
        assert_eq!(state.mode, AddTaskMode::Help { scroll: 0 });
    }

    #[test]
    fn add_task_description_paste_preserves_newlines() {
        let outcome = handle_generic_overlay_paste(
            "one\ntwo",
            OverlayState::AddTask(add_task_state(AddTaskStep::Description)),
        );
        let OverlayState::AddTask(state) = outcome else {
            panic!("expected add task state");
        };
        assert_eq!(
            state.description.lines,
            vec!["one".to_string(), "two".to_string()]
        );
        assert_eq!(state.description.row, 1);
        assert_eq!(state.description.column, 3);
    }

    #[test]
    fn add_task_title_paste_flattens_newlines() {
        let outcome = handle_generic_overlay_paste(
            "one\ntwo",
            OverlayState::AddTask(add_task_state(AddTaskStep::Title)),
        );
        let OverlayState::AddTask(state) = outcome else {
            panic!("expected add task state");
        };
        assert_eq!(state.title.text, "one two");
        assert_eq!(state.title.cursor, 7);
    }

    #[test]
    fn add_task_tab_and_backtab_traverse_all_fields() {
        let mut state = add_task_state(AddTaskStep::Project);
        state.attachments.push(pending_attachment("draft.png"));
        let mut overlay = OverlayState::AddTask(state);
        for expected in AddTaskStep::ALL.into_iter().skip(1) {
            let OverlayOutcome::None(next) = handle(key(KeyCode::Tab), overlay) else {
                panic!("expected composer");
            };
            let OverlayState::AddTask(state) = &next else {
                panic!("expected add task");
            };
            assert_eq!(state.focus, expected);
            overlay = next;
        }
        let OverlayOutcome::None(OverlayState::AddTask(state)) = handle(key(KeyCode::Tab), overlay)
        else {
            panic!("expected wrapped composer");
        };
        assert_eq!(state.focus, AddTaskStep::Project);
        let OverlayOutcome::None(OverlayState::AddTask(state)) =
            handle(key(KeyCode::BackTab), OverlayState::AddTask(state))
        else {
            panic!("expected reverse composer");
        };
        assert_eq!(state.focus, AddTaskStep::Description);
    }

    #[test]
    fn add_task_tab_skips_empty_image_field() {
        let state = add_task_state(AddTaskStep::Due);
        let OverlayOutcome::None(OverlayState::AddTask(state)) =
            handle(key(KeyCode::Tab), OverlayState::AddTask(state))
        else {
            panic!("expected add task state");
        };
        assert_eq!(state.focus, AddTaskStep::Title);
        let OverlayOutcome::None(OverlayState::AddTask(state)) =
            handle(key(KeyCode::BackTab), OverlayState::AddTask(state))
        else {
            panic!("expected add task state");
        };
        assert_eq!(state.focus, AddTaskStep::Due);
    }

    #[test]
    fn add_task_image_field_cycles_selected_attachment() {
        let mut state = add_task_state(AddTaskStep::Images);
        state.attachments = vec![
            pending_attachment("first.png"),
            pending_attachment("second.png"),
            pending_attachment("third.png"),
        ];
        state.selected_attachment = 1;

        let OverlayOutcome::None(OverlayState::AddTask(state)) =
            handle(key(KeyCode::Right), OverlayState::AddTask(state))
        else {
            panic!("expected add task state");
        };
        assert_eq!(state.selected_attachment, 2);
        let OverlayOutcome::None(OverlayState::AddTask(state)) =
            handle(key(KeyCode::Right), OverlayState::AddTask(state))
        else {
            panic!("expected add task state");
        };
        assert_eq!(state.selected_attachment, 0);
        let OverlayOutcome::None(OverlayState::AddTask(state)) =
            handle(key(KeyCode::Left), OverlayState::AddTask(state))
        else {
            panic!("expected add task state");
        };
        assert_eq!(state.selected_attachment, 2);
    }

    #[test]
    fn add_task_modified_enter_submits_from_every_field() {
        for focus in AddTaskStep::ALL {
            assert!(matches!(
                handle(
                    ctrl(KeyCode::Enter),
                    OverlayState::AddTask(add_task_state(focus))
                ),
                OverlayOutcome::Submitted(OverlaySubmit::AddTask(_))
            ));
        }
    }

    #[test]
    fn add_task_plain_enter_preserves_field_behavior() {
        assert!(matches!(
            handle(
                key(KeyCode::Enter),
                OverlayState::AddTask(add_task_state(AddTaskStep::Title))
            ),
            OverlayOutcome::Submitted(OverlaySubmit::AddTask(_))
        ));

        let OverlayOutcome::None(OverlayState::AddTask(state)) = handle(
            key(KeyCode::Enter),
            OverlayState::AddTask(add_task_state(AddTaskStep::Description)),
        ) else {
            panic!("expected composer");
        };
        assert_eq!(
            state.description.lines,
            vec!["".to_string(), "".to_string()]
        );
    }

    #[test]
    fn add_task_ctrl_s_submits_from_every_field() {
        for focus in AddTaskStep::ALL {
            assert!(matches!(
                handle(
                    ctrl(KeyCode::Char('s')),
                    OverlayState::AddTask(add_task_state(focus))
                ),
                OverlayOutcome::Submitted(OverlaySubmit::AddTask(_))
            ));
        }
    }

    #[test]
    fn populated_add_task_requires_discard_confirmation() {
        let mut state = add_task_state(AddTaskStep::Title);
        state.title = LineEdit::new("Keep this".to_string());
        let OverlayOutcome::None(OverlayState::AddTask(state)) =
            handle(key(KeyCode::Esc), OverlayState::AddTask(state))
        else {
            panic!("expected discard confirmation");
        };
        assert_eq!(state.mode, AddTaskMode::ConfirmDiscard);
        let OverlayOutcome::None(OverlayState::AddTask(state)) =
            handle(key(KeyCode::Char('n')), OverlayState::AddTask(state))
        else {
            panic!("expected composer");
        };
        assert_eq!(state.mode, AddTaskMode::Compose);
    }

    #[test]
    fn attachment_only_add_task_requires_discard_confirmation() {
        let mut state = add_task_state(AddTaskStep::Title);
        state.attachments = vec![pending_attachment("diagram.png")];
        let OverlayOutcome::None(OverlayState::AddTask(state)) =
            handle(key(KeyCode::Esc), OverlayState::AddTask(state))
        else {
            panic!("expected discard confirmation");
        };
        assert_eq!(state.mode, AddTaskMode::ConfirmDiscard);
    }

    #[test]
    fn empty_add_task_escape_cancels_immediately() {
        assert!(matches!(
            handle(
                key(KeyCode::Esc),
                OverlayState::AddTask(add_task_state(AddTaskStep::Title))
            ),
            OverlayOutcome::Cancelled
        ));
    }

    #[test]
    fn add_task_help_returns_without_changing_text_cursors() {
        let mut state = add_task_state(AddTaskStep::Description);
        state.title = LineEdit::new("Draft".to_string());
        state.description.lines = vec!["first".to_string(), "second".to_string()];
        state.description.row = 1;
        state.description.column = 3;
        let title_cursor = state.title.cursor;
        let description_cursor = (state.description.row, state.description.column);
        let OverlayOutcome::None(OverlayState::AddTask(help)) =
            handle(key(KeyCode::F(1)), OverlayState::AddTask(state))
        else {
            panic!("expected help");
        };
        let OverlayOutcome::None(OverlayState::AddTask(state)) =
            handle(key(KeyCode::Esc), OverlayState::AddTask(help))
        else {
            panic!("expected composer");
        };
        assert_eq!(state.title.cursor, title_cursor);
        assert_eq!(
            (state.description.row, state.description.column),
            description_cursor
        );
    }

    #[test]
    fn multiline_ctrl_s_submits() {
        let state = MultilineInputState {
            route: OverlayRoute::MessageOnly,
            title: "Notes".to_string(),
            prompt: "Body".to_string(),
            lines: vec!["line".to_string()],
            row: 0,
            column: 4,
        };
        let outcome = handle(
            ctrl(KeyCode::Char('s')),
            OverlayState::MultilineInput(state),
        );
        assert!(matches!(
            outcome,
            OverlayOutcome::Submitted(OverlaySubmit::Multiline { .. })
        ));
    }

    #[test]
    fn multiline_ctrl_enter_submits() {
        let state = MultilineInputState {
            route: OverlayRoute::AddNote,
            title: "Add note".to_string(),
            prompt: "note body:".to_string(),
            lines: vec!["line".to_string()],
            row: 0,
            column: 4,
        };
        let outcome = handle(ctrl(KeyCode::Enter), OverlayState::MultilineInput(state));
        assert!(matches!(
            outcome,
            OverlayOutcome::Submitted(OverlaySubmit::Multiline { .. })
        ));
    }

    #[test]
    fn text_panel_closes_on_enter_and_esc() {
        let state = TextPanelState {
            title: "Conflicts".to_string(),
            lines: vec!["field=title".to_string()],
            scroll: 0,
        };
        assert!(matches!(
            handle(key(KeyCode::Enter), OverlayState::TextPanel(state.clone())),
            OverlayOutcome::Cancelled
        ));
        assert!(matches!(
            handle(key(KeyCode::Esc), OverlayState::TextPanel(state)),
            OverlayOutcome::Cancelled
        ));
    }

    #[test]
    fn text_panel_scrolls_with_navigation_keys() {
        let state = TextPanelState {
            title: "Conflicts".to_string(),
            lines: (0..20)
                .map(|index| format!("line {index}"))
                .collect::<Vec<_>>(),
            scroll: 0,
        };
        let OverlayOutcome::None(OverlayState::TextPanel(state)) =
            handle(key(KeyCode::Down), OverlayState::TextPanel(state))
        else {
            panic!("expected scrolled text panel");
        };
        assert_eq!(state.scroll, 1);
        let OverlayOutcome::None(OverlayState::TextPanel(state)) =
            handle(key(KeyCode::Up), OverlayState::TextPanel(state))
        else {
            panic!("expected scrolled text panel");
        };
        assert_eq!(state.scroll, 0);
    }

    #[test]
    fn text_panel_navigation_scroll_is_capped() {
        let mut state = TextPanelState {
            title: "Conflicts".to_string(),
            lines: (0..20)
                .map(|index| format!("line {index}"))
                .collect::<Vec<_>>(),
            scroll: 0,
        };
        let expected = crate::tui::ui::text_panel_scroll_cap(&state.lines);
        for _ in 0..30 {
            let OverlayOutcome::None(OverlayState::TextPanel(next)) =
                handle(key(KeyCode::Down), OverlayState::TextPanel(state))
            else {
                panic!("expected scrolled text panel");
            };
            state = next;
        }
        assert_eq!(state.scroll, expected);

        let OverlayOutcome::None(OverlayState::TextPanel(next)) =
            handle(key(KeyCode::Up), OverlayState::TextPanel(state))
        else {
            panic!("expected scrolled text panel");
        };
        assert_eq!(next.scroll, expected.saturating_sub(1));
    }

    #[test]
    fn detail_scrolls_with_line_navigation_keys() {
        let OverlayOutcome::None(OverlayState::Detail { scroll }) =
            handle(key(KeyCode::Char('j')), OverlayState::Detail { scroll: 0 })
        else {
            panic!("expected scrolled detail");
        };
        assert_eq!(scroll, 1);
        let OverlayOutcome::None(OverlayState::Detail { scroll }) =
            handle(key(KeyCode::Char('k')), OverlayState::Detail { scroll })
        else {
            panic!("expected scrolled detail");
        };
        assert_eq!(scroll, 0);
    }

    #[test]
    fn esc_cancels_all_generic_overlay_variants() {
        let overlays = vec![
            OverlayState::TextInput(TextInputState::new(
                OverlayRoute::MessageOnly,
                "Title",
                "Prompt",
                "value".to_string(),
            )),
            OverlayState::MultilineInput(MultilineInputState {
                route: OverlayRoute::MessageOnly,
                title: "Body".to_string(),
                prompt: "Prompt".to_string(),
                lines: vec!["value".to_string()],
                row: 0,
                column: 5,
            }),
            OverlayState::Picker(PickerState {
                route: OverlayRoute::MessageOnly,
                title: "Pick".to_string(),
                filter: LineEdit::blank(),
                items: vec![PickerItem {
                    label: "One".to_string(),
                    value: "one".to_string(),
                    selected: false,
                }],
                selected: 0,
                scroll: 0,
                multi: false,
                mode: PickerMode::Navigate,
            }),
            OverlayState::Confirm(ConfirmState {
                route: OverlayRoute::MessageOnly,
                title: "Confirm".to_string(),
                prompt: "Continue?".to_string(),
            }),
            OverlayState::TextPanel(TextPanelState {
                title: "Panel".to_string(),
                lines: vec!["line".to_string()],
                scroll: 0,
            }),
        ];

        for overlay in overlays {
            assert!(matches!(
                handle(key(KeyCode::Esc), overlay),
                OverlayOutcome::Cancelled
            ));
        }
    }

    #[test]
    fn help_scroll_stops_at_cap() {
        let OverlayOutcome::None(OverlayState::Help { scroll }) =
            handle_generic_overlay_key(key(KeyCode::Down), OverlayState::Help { scroll: 2 }, 2)
        else {
            panic!("expected help overlay state");
        };
        assert_eq!(scroll, 2);
    }

    #[test]
    fn confirm_yes_and_no() {
        let state = ConfirmState {
            route: OverlayRoute::MessageOnly,
            title: "Delete".to_string(),
            prompt: "Sure?".to_string(),
        };
        assert!(matches!(
            handle(
                key(KeyCode::Char('y')),
                OverlayState::Confirm(state.clone())
            ),
            OverlayOutcome::Submitted(OverlaySubmit::Confirm {
                route: OverlayRoute::MessageOnly,
                ..
            })
        ));
        assert!(matches!(
            handle(key(KeyCode::Char('n')), OverlayState::Confirm(state)),
            OverlayOutcome::Cancelled
        ));
    }

    #[test]
    fn generic_submit_variants_propagate_route() {
        let text = handle(
            key(KeyCode::Enter),
            OverlayState::TextInput(TextInputState::new(
                OverlayRoute::AddProject,
                "Add project",
                "name:",
                "app".to_string(),
            )),
        );
        assert!(matches!(
            text,
            OverlayOutcome::Submitted(OverlaySubmit::Text {
                route: OverlayRoute::AddProject,
                ..
            })
        ));

        let multiline = handle(
            ctrl(KeyCode::Char('s')),
            OverlayState::MultilineInput(MultilineInputState {
                route: OverlayRoute::AddNote,
                title: "Add note".to_string(),
                prompt: "body:".to_string(),
                lines: vec!["note".to_string()],
                row: 0,
                column: 4,
            }),
        );
        assert!(matches!(
            multiline,
            OverlayOutcome::Submitted(OverlaySubmit::Multiline {
                route: OverlayRoute::AddNote,
                ..
            })
        ));

        let picker = handle(
            key(KeyCode::Enter),
            OverlayState::Picker(PickerState {
                route: OverlayRoute::EditStatus,
                title: "Edit task: status".to_string(),
                filter: LineEdit::blank(),
                items: vec![PickerItem {
                    label: "Todo".to_string(),
                    value: "todo".to_string(),
                    selected: false,
                }],
                selected: 0,
                scroll: 0,
                multi: false,
                mode: PickerMode::Navigate,
            }),
        );
        assert!(matches!(
            picker,
            OverlayOutcome::Submitted(OverlaySubmit::Picker {
                route: OverlayRoute::EditStatus,
                ..
            })
        ));
    }
}
