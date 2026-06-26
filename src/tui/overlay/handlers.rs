use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Rect, Size};

use crate::tui::authoring::AddTaskStep;
use crate::tui::navigation::scroll_with_delta;
use crate::tui::ui::text_panel_scroll_cap;

use super::multiline::edit_multiline_input;
use super::picker::{
    handle_picker_key, normalize_picker_selection, picker_submit_outcome, visible_picker_indices,
};
use super::state::{
    ConfirmState, HeaderMenuState, OrderMenuState, OverlayOutcome, OverlayState, OverlaySubmit,
    PickerMode, PickerState, TextPanelState,
};
use crate::tui::overlay::{confirm_layout, picker_layout, text_panel_layout};
use crate::tui::store::TaskOrder;

pub(crate) fn handle_generic_overlay_paste(text: &str, overlay: OverlayState) -> OverlayState {
    match overlay {
        OverlayState::Search { mut input } => {
            input.insert_paste(text);
            OverlayState::Search { input }
        }
        OverlayState::Command { mut state } => {
            state.input.insert_paste(text);
            state.reset_cycle();
            OverlayState::Command { state }
        }
        OverlayState::AddTask(mut state) => {
            match state.focus {
                AddTaskStep::Title => state.title.insert_paste(text),
                AddTaskStep::Description => state.description.insert_paste(text),
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
            OverlayState::Picker(state)
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
        OverlayState::AddTask(mut state) => match key.code {
            KeyCode::Esc => OverlayOutcome::Cancelled,
            KeyCode::Tab => {
                state.focus = match state.focus {
                    AddTaskStep::Title => AddTaskStep::Description,
                    AddTaskStep::Description => AddTaskStep::Title,
                };
                OverlayOutcome::None(OverlayState::AddTask(state))
            }
            KeyCode::Enter if state.focus == AddTaskStep::Title => {
                OverlayOutcome::Submitted(OverlaySubmit::AddTask {
                    title: state.title.text.clone(),
                    description: state.description.lines.join("\n"),
                })
            }
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                OverlayOutcome::Submitted(OverlaySubmit::AddTask {
                    title: state.title.text.clone(),
                    description: state.description.lines.join("\n"),
                })
            }
            _ => {
                match state.focus {
                    AddTaskStep::Title => state.title.handle_key(key),
                    AddTaskStep::Description => edit_multiline_input(&mut state.description, key),
                }
                OverlayOutcome::None(OverlayState::AddTask(state))
            }
        },
        OverlayState::TextInput(mut state) => match key.code {
            KeyCode::Esc => OverlayOutcome::Cancelled,
            KeyCode::Enter => OverlayOutcome::Submitted(OverlaySubmit::Text {
                route: state.route,
                title: state.title.clone(),
                value: state.input.text.clone(),
            }),
            _ => {
                state.input.handle_key(key);
                OverlayOutcome::None(OverlayState::TextInput(state))
            }
        },
        OverlayState::MultilineInput(mut state) => {
            if key.code == KeyCode::Char('s') && key.modifiers.contains(KeyModifiers::CONTROL) {
                let value = state.lines.join("\n");
                return OverlayOutcome::Submitted(OverlaySubmit::Multiline {
                    route: state.route,
                    title: state.title.clone(),
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
        OverlayState::HeaderMenu(state) => handle_header_menu_key(state, key),
        OverlayState::OrderMenu(state) => handle_order_menu_key(state, key),
        OverlayState::Confirm(state) => match key.code {
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => OverlayOutcome::Cancelled,
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                OverlayOutcome::Submitted(OverlaySubmit::Confirm {
                    route: state.route,
                    title: state.title.clone(),
                })
            }
            _ => OverlayOutcome::None(OverlayState::Confirm(state)),
        },
        OverlayState::TextPanel(mut state) => match key.code {
            KeyCode::Esc | KeyCode::Enter => OverlayOutcome::Cancelled,
            KeyCode::Char('j') | KeyCode::Down => {
                let cap = text_panel_scroll_cap(&state.lines);
                state.scroll = scroll_with_delta(state.scroll, 1, cap);
                OverlayOutcome::None(OverlayState::TextPanel(state))
            }
            KeyCode::Char('k') | KeyCode::Up => {
                let cap = text_panel_scroll_cap(&state.lines);
                state.scroll = scroll_with_delta(state.scroll, -1, cap);
                OverlayOutcome::None(OverlayState::TextPanel(state))
            }
            _ => OverlayOutcome::None(OverlayState::TextPanel(state)),
        },
        OverlayState::SyncStatus(state) => match key.code {
            KeyCode::Esc | KeyCode::Enter => OverlayOutcome::Cancelled,
            _ => OverlayOutcome::None(OverlayState::SyncStatus(state)),
        },
        OverlayState::DatabaseStats { stats, mut scroll } => match key.code {
            KeyCode::Esc | KeyCode::Enter => OverlayOutcome::Cancelled,
            KeyCode::Char('j') | KeyCode::Down => {
                scroll = scroll.saturating_add(1).min(help_scroll_cap);
                OverlayOutcome::None(OverlayState::DatabaseStats { stats, scroll })
            }
            KeyCode::Char('k') | KeyCode::Up => {
                scroll = scroll.saturating_sub(1);
                OverlayOutcome::None(OverlayState::DatabaseStats { stats, scroll })
            }
            _ => OverlayOutcome::None(OverlayState::DatabaseStats { stats, scroll }),
        },
        OverlayState::Help { mut scroll } => match key.code {
            KeyCode::Esc | KeyCode::Enter => OverlayOutcome::Cancelled,
            KeyCode::Char('j') | KeyCode::Down => {
                scroll = scroll.saturating_add(1).min(help_scroll_cap);
                OverlayOutcome::None(OverlayState::Help { scroll })
            }
            KeyCode::Char('k') | KeyCode::Up => {
                scroll = scroll.saturating_sub(1);
                OverlayOutcome::None(OverlayState::Help { scroll })
            }
            _ => OverlayOutcome::None(OverlayState::Help { scroll }),
        },
        OverlayState::DetailHelp { mut scroll } => match key.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('?') => OverlayOutcome::Cancelled,
            KeyCode::Char('j') | KeyCode::Down => {
                scroll = scroll.saturating_add(1).min(help_scroll_cap);
                OverlayOutcome::None(OverlayState::DetailHelp { scroll })
            }
            KeyCode::Char('k') | KeyCode::Up => {
                scroll = scroll.saturating_sub(1);
                OverlayOutcome::None(OverlayState::DetailHelp { scroll })
            }
            _ => OverlayOutcome::None(OverlayState::DetailHelp { scroll }),
        },
        OverlayState::Detail { mut scroll } => match key.code {
            KeyCode::Esc | KeyCode::Enter => OverlayOutcome::Cancelled,
            KeyCode::Char('j') | KeyCode::Down => {
                scroll = scroll.saturating_add(1);
                OverlayOutcome::None(OverlayState::Detail { scroll })
            }
            KeyCode::Char('k') | KeyCode::Up => {
                scroll = scroll.saturating_sub(1);
                OverlayOutcome::None(OverlayState::Detail { scroll })
            }
            _ => OverlayOutcome::None(OverlayState::Detail { scroll }),
        },
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
        ConfirmMouseTarget::Yes => OverlayOutcome::Submitted(OverlaySubmit::Confirm {
            route: state.route,
            title: state.title,
        }),
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
        TaskOrder::Created => TaskOrder::Updated,
        TaskOrder::Updated => TaskOrder::Priority,
        TaskOrder::Priority => TaskOrder::Project,
        TaskOrder::Project => TaskOrder::Title,
        TaskOrder::Title => TaskOrder::Created,
    }
}

fn previous_order(order: TaskOrder) -> TaskOrder {
    match order {
        TaskOrder::Created => TaskOrder::Title,
        TaskOrder::Updated => TaskOrder::Created,
        TaskOrder::Priority => TaskOrder::Updated,
        TaskOrder::Project => TaskOrder::Priority,
        TaskOrder::Title => TaskOrder::Project,
    }
}
