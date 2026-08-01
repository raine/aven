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
#[cfg(test)]
use super::state::ScheduleEditorMode;
use super::state::{
    AddTaskMode, ConfirmState, HeaderMenuState, MultilineInputMode, OrderMenuState, OverlayOutcome,
    OverlayState, OverlaySubmit, PickerMode, PickerState, ScheduleEditorField, ScheduleEditorState,
    TagComboboxState, TextPanelState,
};
use super::tag_combobox::{
    handle_tag_combobox_key, normalize_tag_combobox_highlight, tag_combobox_matches,
    toggle_tag_combobox_label,
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
            if let AddTaskMode::Picker { state: picker, .. } = &mut state.mode {
                picker.filter.insert_paste(text);
                super::picker::sync_project_creation_item(picker);
                normalize_picker_selection(picker);
                normalize_picker_scroll(picker, crate::tui::overlay::GENERIC_PICKER_VIEWPORT_ROWS);
                return OverlayState::AddTask(state);
            }
            if let AddTaskMode::Schedule(editor) = &mut state.mode {
                if let Some(input) = schedule_editor_input_mut(editor) {
                    input.insert_paste(text);
                    editor.validation_requested = false;
                    editor.refresh();
                }
                return OverlayState::AddTask(state);
            }
            match state.focus {
                AddTaskStep::Schedule if state.template_schedule.is_none() => {
                    state.schedule_input.insert_paste(text);
                    state.schedule_validation_requested = false;
                    state.apply_schedule_input();
                }
                AddTaskStep::Title => state.title.insert_paste(text),
                AddTaskStep::AvailableAt => state.available_at.insert_paste(text),
                AddTaskStep::Due => state.due_on.insert_paste(text),
                AddTaskStep::RepeatRule if state.is_step_editable(state.focus) => {
                    state.repeat_rule.insert_paste(text)
                }
                AddTaskStep::RepeatAt => state.repeat_at.insert_paste(text),
                AddTaskStep::RepeatStartOn if state.is_step_editable(state.focus) => {
                    state.repeat_start_on.insert_paste(text)
                }
                AddTaskStep::Description => state.description.insert_paste(text),
                _ => {}
            }
            state.refresh_repeat_status();
            state.refresh_recurrence_preview();
            OverlayState::AddTask(state)
        }
        OverlayState::TextInput(mut state) => {
            state.input.insert_paste(text);
            OverlayState::TextInput(state)
        }
        OverlayState::RecurrenceHistory(state) => OverlayState::RecurrenceHistory(state),
        OverlayState::MultilineInput(mut state) => {
            if state.mode == MultilineInputMode::Compose {
                state.insert_paste(text);
            }
            OverlayState::MultilineInput(state)
        }
        OverlayState::Picker(mut state) => {
            state.filter.insert_paste(text);
            super::picker::sync_project_creation_item(&mut state);
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

fn schedule_editor_input_mut(editor: &mut ScheduleEditorState) -> Option<&mut super::LineEdit> {
    match editor.focus {
        ScheduleEditorField::Available => Some(&mut editor.available_at),
        ScheduleEditorField::Due => Some(&mut editor.due_on),
        ScheduleEditorField::Repeat if !editor.template_locked => Some(&mut editor.repeat_rule),
        ScheduleEditorField::Time => Some(&mut editor.repeat_at),
        ScheduleEditorField::Starts if !editor.template_locked => Some(&mut editor.repeat_start_on),
        _ => None,
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
                AddTaskMode::Schedule(mut editor) => {
                    match key.code {
                        KeyCode::Esc => {}
                        KeyCode::Tab | KeyCode::Down => {
                            editor.validate_current_field();
                            editor.focus_next(false);
                            state.mode = AddTaskMode::Schedule(editor);
                        }
                        KeyCode::BackTab | KeyCode::Up => {
                            editor.validate_current_field();
                            editor.focus_next(true);
                            state.mode = AddTaskMode::Schedule(editor);
                        }
                        KeyCode::Left | KeyCode::Right
                            if editor.focus == ScheduleEditorField::Mode =>
                        {
                            editor.cycle_mode(key.code == KeyCode::Left);
                            state.mode = AddTaskMode::Schedule(editor);
                        }
                        KeyCode::Left | KeyCode::Right
                            if editor.focus == ScheduleEditorField::DuePolicy =>
                        {
                            editor.repeat_due = if editor.repeat_due == "same-day" {
                                "none".to_string()
                            } else {
                                "same-day".to_string()
                            };
                            editor.refresh();
                            state.mode = AddTaskMode::Schedule(editor);
                        }
                        KeyCode::Enter | KeyCode::Char('s')
                            if key.code == KeyCode::Enter
                                || key.modifiers.contains(KeyModifiers::CONTROL) =>
                        {
                            editor.validate();
                            if editor.error.is_none() {
                                state.apply_schedule_editor(editor);
                            } else {
                                state.mode = AddTaskMode::Schedule(editor);
                            }
                        }
                        _ => {
                            if let Some(input) = schedule_editor_input_mut(&mut editor) {
                                input.handle_key(key);
                            }
                            if matches!(
                                key.code,
                                KeyCode::Char(_) | KeyCode::Backspace | KeyCode::Delete
                            ) {
                                editor.validation_requested = false;
                            }
                            editor.refresh();
                            state.mode = AddTaskMode::Schedule(editor);
                        }
                    }
                    return OverlayOutcome::None(OverlayState::AddTask(state));
                }
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
                            if field == AddTaskStep::Project
                                && let Some(name) =
                                    crate::tui::store::create_project_picker_name(&value)
                            {
                                return OverlayOutcome::Submitted(
                                    OverlaySubmit::CreateAddTaskProject {
                                        state,
                                        name: name.to_string(),
                                    },
                                );
                            }
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
                                AddTaskStep::Status => {
                                    state.status = value;
                                    state.status_origin =
                                        crate::tui::authoring::InitialStatusOrigin::Explicit;
                                }
                                AddTaskStep::Priority => state.priority = value,
                                AddTaskStep::Epic => state.is_epic = value == "true",
                                AddTaskStep::RepeatDue => state.repeat_due = value,
                                _ => {}
                            }
                            state.refresh_recurrence_preview();
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
                        OverlayOutcome::Submitted(OverlaySubmit::TagCombobox {
                            values, ..
                        }) => {
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
                    if state.focus == AddTaskStep::Schedule {
                        state.schedule_validation_requested = true;
                        state.canonicalize_schedule_input();
                    }
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
                KeyCode::Left if state.focus.is_metadata() && !state.focus.is_inline_text() => {
                    state.focus_metadata_next(true);
                    OverlayOutcome::None(OverlayState::AddTask(state))
                }
                KeyCode::Right if state.focus.is_metadata() && !state.focus.is_inline_text() => {
                    state.focus_metadata_next(false);
                    OverlayOutcome::None(OverlayState::AddTask(state))
                }
                KeyCode::Down if state.focus.is_metadata() => {
                    if state.focus == AddTaskStep::Schedule {
                        state.schedule_validation_requested = true;
                    }
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
                    if state.focus.is_metadata() && !state.focus.is_inline_text() =>
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
                        AddTaskStep::Schedule if state.template_schedule.is_none() => {
                            state.schedule_input.handle_key(key);
                            if matches!(
                                key.code,
                                KeyCode::Char(_) | KeyCode::Backspace | KeyCode::Delete
                            ) {
                                state.schedule_validation_requested = false;
                            }
                            state.apply_schedule_input();
                        }
                        AddTaskStep::Title => {
                            state.title.handle_key(key);
                            state.title_error = false;
                        }
                        AddTaskStep::AvailableAt => state.available_at.handle_key(key),
                        AddTaskStep::Due => state.due_on.handle_key(key),
                        AddTaskStep::RepeatRule if state.is_step_editable(state.focus) => {
                            state.repeat_rule.handle_key(key);
                            state.refresh_repeat_status();
                        }
                        AddTaskStep::RepeatAt => state.repeat_at.handle_key(key),
                        AddTaskStep::RepeatStartOn if state.is_step_editable(state.focus) => {
                            state.repeat_start_on.handle_key(key)
                        }
                        AddTaskStep::Description => {
                            edit_multiline_input(&mut state.description, key)
                        }
                        _ => {}
                    }
                    state.refresh_recurrence_preview();
                    OverlayOutcome::None(OverlayState::AddTask(state))
                }
            }
        }
        OverlayState::TextInput(mut state) => match key.code {
            KeyCode::Esc => OverlayOutcome::Cancelled,
            KeyCode::Char('d')
                if key.modifiers.contains(KeyModifiers::CONTROL) && state.intent.is_date_edit() =>
            {
                OverlayOutcome::Submitted(OverlaySubmit::ClearDate {
                    intent: state.intent,
                })
            }
            KeyCode::Enter => OverlayOutcome::Submitted(OverlaySubmit::Text {
                intent: state.intent,
                value: state.input.text.clone(),
            }),
            _ => {
                state.input.handle_key(key);
                OverlayOutcome::None(OverlayState::TextInput(state))
            }
        },
        OverlayState::MultilineInput(mut state) => {
            if state.mode == MultilineInputMode::ConfirmDiscard {
                return match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => OverlayOutcome::Cancelled,
                    KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                        state.mode = MultilineInputMode::Compose;
                        OverlayOutcome::None(OverlayState::MultilineInput(state))
                    }
                    _ => OverlayOutcome::None(OverlayState::MultilineInput(state)),
                };
            }
            if (key.code == KeyCode::Char('s') && key.modifiers.contains(KeyModifiers::CONTROL))
                || (key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::CONTROL))
            {
                let value = state.lines.join("\n");
                return OverlayOutcome::Submitted(OverlaySubmit::Multiline {
                    intent: state.intent,
                    value,
                });
            }
            match key.code {
                KeyCode::Esc if state.should_confirm_discard() => {
                    state.mode = MultilineInputMode::ConfirmDiscard;
                    OverlayOutcome::None(OverlayState::MultilineInput(state))
                }
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
                OverlayOutcome::Submitted(OverlaySubmit::Confirm {
                    intent: state.intent,
                })
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
        OverlayState::Changelog(state) => OverlayOutcome::None(OverlayState::Changelog(state)),
        OverlayState::RecurrenceHistory(state) => {
            OverlayOutcome::None(OverlayState::RecurrenceHistory(state))
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
        OverlayState::Detail => OverlayOutcome::None(OverlayState::Detail),
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
            toggle_tag_combobox_label(&mut state, index);
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
        kind: (&state.intent).into(),
        title: state.title.clone(),
        input: state.input.text.clone(),
        input_cursor: state.input.cursor,
        completion: None,
        options: state.options.clone(),
        selected: state.selected.clone(),
        partial: state.partial.clone(),
        highlighted: state.highlighted,
        visible_indices: tag_combobox_matches(state),
        visible_start: 0,
    };
    let layout = tag_combobox_layout(&view, terminal_size);
    if !contains(layout.area, column, row) {
        return TagComboboxMouseTarget::Outside;
    }
    if !contains(layout.inner, column, row) {
        return TagComboboxMouseTarget::Interior;
    }
    let inner_row = row.saturating_sub(layout.inner.y);
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
        kind: (&state.intent).into(),
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
        ConfirmMouseTarget::Yes => OverlayOutcome::Submitted(OverlaySubmit::Confirm {
            intent: state.intent,
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
        ConfirmIntent, LineEdit, MultilineInputState, MultilineIntent, PickerIntent, PickerItem,
        TagComboboxIntent, TagComboboxView, TextInputState, TextIntent,
    };
    use crate::tui::task_selection::TaskSelection;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    fn add_note_intent() -> MultilineIntent {
        MultilineIntent::AddNote {
            task_id: crate::test_support::task_id("task-1"),
            display_ref: "APP-1234".to_string(),
        }
    }

    fn description_intent() -> MultilineIntent {
        MultilineIntent::EditDescription {
            selection: task_selection(),
        }
    }

    fn manual_conflict_description_intent() -> MultilineIntent {
        MultilineIntent::ResolveConflictManually {
            target: crate::tui::store::ConflictTarget {
                task_id: crate::test_support::task_id("task-1"),
                recurrence_series_id: None,
                display_ref: "APP-1234".to_string(),
                field: "description".to_string(),
                variant_a: "a".to_string(),
                local_value: "local".to_string(),
                variant_b: "b".to_string(),
                remote_value: "remote".to_string(),
            },
        }
    }

    fn task_selection() -> TaskSelection {
        TaskSelection::resolve(
            &[crate::tui::test_support::task_list_item("Task")],
            &std::collections::BTreeSet::new(),
            Some(0),
        )
        .unwrap()
    }

    fn due_intent() -> TextIntent {
        TextIntent::EditDue {
            selection: task_selection(),
            mixed: false,
        }
    }

    fn config_intent() -> ConfirmIntent {
        ConfirmIntent::InitializeConfig {
            path: std::path::PathBuf::from("/tmp/config.toml"),
        }
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
                MultilineIntent::AddTaskDescription,
                "Add task: description",
                "",
            ),
            focus,
            project: "aven".to_string(),
            inferred_project: None,
            selected_project: Some("aven".to_string()),
            initial_project: Some("aven".to_string()),
            status: "inbox".to_string(),
            status_origin: crate::tui::authoring::InitialStatusOrigin::UntouchedDefault,
            priority: "none".to_string(),
            labels: Vec::new(),
            is_epic: false,
            available_at: LineEdit::blank(),
            due_on: LineEdit::blank(),
            schedule_input: LineEdit::blank(),
            schedule_error: None,
            schedule_validation_requested: false,
            attachments: Vec::new(),
            selected_attachment: 0,
            recurrence_series_id: None,
            template_schedule: None,
            repeat_rule: LineEdit::blank(),
            repeat_at: LineEdit::blank(),
            repeat_due: "same-day".to_string(),
            time_zone: "UTC".to_string(),
            repeat_start_on: LineEdit::new("2026-07-20".to_string()),
            schedule_expanded: false,
            recurrence_preview: Vec::new(),
            recurrence_error: None,
            mode: crate::tui::overlay::AddTaskMode::Compose,
            title_error: false,
        })
    }

    fn handle(key: KeyEvent, overlay: OverlayState) -> OverlayOutcome {
        handle_generic_overlay_key(key, overlay, 100)
    }

    fn left_click(column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn tag_combobox_state(selected: Vec<&str>, partial: Vec<&str>) -> TagComboboxState {
        TagComboboxState {
            intent: TagComboboxIntent::AddTaskLabels,
            title: "Labels".to_string(),
            input: LineEdit::blank(),
            options: vec!["bug".to_string(), "feature".to_string()],
            selected: selected.into_iter().map(str::to_string).collect(),
            partial: partial.into_iter().map(str::to_string).collect(),
            highlighted: 0,
        }
    }

    fn tag_combobox_test_layout(
        state: &TagComboboxState,
        terminal_size: Size,
    ) -> crate::tui::overlay::layout::TagComboboxLayout {
        let visible_indices = tag_combobox_matches(state);
        tag_combobox_layout(
            &TagComboboxView {
                kind: (&state.intent).into(),
                title: state.title.clone(),
                input: state.input.text.clone(),
                input_cursor: state.input.cursor,
                completion: None,
                options: state.options.clone(),
                selected: state.selected.clone(),
                partial: state.partial.clone(),
                highlighted: state.highlighted,
                visible_indices,
                visible_start: 0,
            },
            terminal_size,
        )
    }

    fn retained_tag_combobox(outcome: OverlayOutcome) -> TagComboboxState {
        let OverlayOutcome::None(OverlayState::TagCombobox(state)) = outcome else {
            panic!("expected retained tag combobox");
        };
        state
    }

    #[test]
    fn tag_combobox_mouse_cycles_partial_label_to_selected_then_absent() {
        let terminal_size = Size::new(80, 24);
        let state = tag_combobox_state(Vec::new(), vec!["bug"]);
        let layout = tag_combobox_test_layout(&state, terminal_size);
        let click = left_click(layout.inner.x, layout.inner.y + layout.list_start);

        let state = retained_tag_combobox(handle_tag_combobox_mouse(state, click, terminal_size));
        assert_eq!(state.selected, vec!["bug".to_string()]);
        assert!(state.partial.is_empty());

        let state = retained_tag_combobox(handle_tag_combobox_mouse(state, click, terminal_size));
        assert!(state.selected.is_empty());
        assert!(state.partial.is_empty());
    }

    #[test]
    fn tag_combobox_mouse_removes_selected_label() {
        let terminal_size = Size::new(80, 24);
        let state = tag_combobox_state(vec!["bug"], Vec::new());
        let layout = tag_combobox_test_layout(&state, terminal_size);

        let state = retained_tag_combobox(handle_tag_combobox_mouse(
            state,
            left_click(layout.inner.x, layout.inner.y + layout.list_start),
            terminal_size,
        ));

        assert!(state.selected.is_empty());
        assert!(state.partial.is_empty());
    }

    #[test]
    fn tag_combobox_dialog_borders_and_padding_do_not_toggle_rows() {
        let terminal_size = Size::new(80, 24);
        let state = tag_combobox_state(Vec::new(), Vec::new());
        let layout = tag_combobox_test_layout(&state, terminal_size);
        let mut points = Vec::new();
        for column in layout.area.x..layout.area.x + layout.area.width {
            points.push((column, layout.area.y));
            points.push((column, layout.area.y + layout.area.height - 1));
        }
        for row in layout.area.y..layout.area.y + layout.area.height {
            points.push((layout.area.x, row));
            points.push((layout.area.x + layout.area.width - 1, row));
        }
        for row in layout.inner.y..layout.inner.y + layout.inner.height {
            points.push((layout.inner.x - 1, row));
            points.push((layout.inner.x + layout.inner.width, row));
        }

        for (column, row) in points {
            let retained = retained_tag_combobox(handle_tag_combobox_mouse(
                state.clone(),
                left_click(column, row),
                terminal_size,
            ));
            assert!(retained.selected.is_empty(), "toggle at ({column}, {row})");
        }
    }

    #[test]
    fn tag_combobox_clipped_and_small_terminal_rows_do_not_toggle() {
        let terminal_size = Size::new(80, 24);
        let state = tag_combobox_state(Vec::new(), Vec::new());
        let layout = tag_combobox_test_layout(&state, terminal_size);
        let retained = retained_tag_combobox(handle_tag_combobox_mouse(
            state,
            left_click(
                layout.inner.x,
                layout.inner.y + layout.list_start + layout.viewport_rows as u16,
            ),
            terminal_size,
        ));
        assert!(retained.selected.is_empty());

        let terminal_size = Size::new(20, 6);
        let state = tag_combobox_state(Vec::new(), Vec::new());
        let layout = tag_combobox_test_layout(&state, terminal_size);
        let retained = retained_tag_combobox(handle_tag_combobox_mouse(
            state,
            left_click(layout.inner.x, layout.inner.y + layout.list_start),
            terminal_size,
        ));
        assert!(retained.selected.is_empty());
    }

    #[test]
    fn tag_combobox_outside_click_cancels() {
        assert_eq!(
            handle_tag_combobox_mouse(
                tag_combobox_state(Vec::new(), Vec::new()),
                left_click(0, 0),
                Size::new(80, 24),
            ),
            OverlayOutcome::Cancelled
        );
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
    fn add_task_recurrence_defaults_only_untouched_inbox_to_todo() {
        let mut untouched = add_task_state(AddTaskStep::RepeatRule);
        untouched.set_repeat_rule("daily".to_string());
        assert_eq!(untouched.status, "todo");
        assert_eq!(
            untouched.status_origin,
            crate::tui::authoring::InitialStatusOrigin::RecurrenceDefault
        );

        let mut explicit = add_task_state(AddTaskStep::RepeatRule);
        explicit.status_origin = crate::tui::authoring::InitialStatusOrigin::Explicit;
        explicit.set_repeat_rule("weekly".to_string());
        assert_eq!(explicit.status, "inbox");
    }

    #[test]
    fn add_task_partial_recurrence_keeps_composer_schedule_fields_hidden() {
        let mut state = add_task_state(AddTaskStep::Schedule);
        state.set_repeat_rule("d".to_string());
        assert_eq!(state.status, "inbox");
        assert_eq!(
            state.status_origin,
            crate::tui::authoring::InitialStatusOrigin::UntouchedDefault
        );
        assert!(!state.is_step_editable(AddTaskStep::AvailableAt));
        assert!(!state.is_step_editable(AddTaskStep::RepeatAt));
    }

    #[test]
    fn add_task_recurrence_disable_restores_only_automatic_todo() {
        let mut automatic = add_task_state(AddTaskStep::RepeatRule);
        automatic.set_repeat_rule("daily".to_string());
        automatic.set_repeat_rule("none".to_string());
        assert_eq!(automatic.status, "inbox");

        let mut explicit = add_task_state(AddTaskStep::RepeatRule);
        explicit.set_repeat_rule("daily".to_string());
        explicit.status_origin = crate::tui::authoring::InitialStatusOrigin::Explicit;
        explicit.set_repeat_rule("none".to_string());
        assert_eq!(explicit.status, "todo");
    }

    #[test]
    fn add_task_metadata_navigation_treats_schedule_as_one_field() {
        let mut state = add_task_state(AddTaskStep::Schedule);
        state.focus_metadata_next(false);
        assert_eq!(state.focus, AddTaskStep::Epic);
        state.focus_metadata_next(false);
        assert_eq!(state.focus, AddTaskStep::Project);
        state.focus_metadata_next(true);
        assert_eq!(state.focus, AddTaskStep::Epic);
        state.focus_metadata_next(true);
        assert_eq!(state.focus, AddTaskStep::Schedule);
    }

    #[test]
    fn add_task_read_only_schedule_fields_ignore_keys_and_paste() {
        let zone = add_task_state(AddTaskStep::TimeZone);
        let original_zone = zone.time_zone.clone();
        let OverlayOutcome::None(OverlayState::AddTask(zone)) =
            handle(key(KeyCode::Char('x')), OverlayState::AddTask(zone))
        else {
            panic!("expected add task state");
        };
        assert_eq!(zone.time_zone, original_zone);

        let mut template = add_task_state(AddTaskStep::RepeatStartOn);
        template.template_schedule = Some(aven_core::recurrence::RecurrenceSchedule::new(
            aven_core::recurrence::RecurrenceRule::daily(),
            "UTC".parse().unwrap(),
            chrono::NaiveDate::from_ymd_opt(2026, 7, 20).unwrap(),
            None,
            aven_core::recurrence::RecurrenceDuePolicy::SameDay,
        ));
        let original_start = template.repeat_start_on.text.clone();
        let OverlayState::AddTask(template) =
            handle_generic_overlay_paste("2030-01-01", OverlayState::AddTask(template))
        else {
            panic!("expected add task state");
        };
        assert_eq!(template.repeat_start_on.text, original_start);
    }

    #[test]
    fn add_task_tab_and_backtab_traverse_all_fields() {
        let mut state = add_task_state(AddTaskStep::Project);
        state.attachments.push(pending_attachment("draft.png"));
        let mut overlay = OverlayState::AddTask(state);
        for expected in [
            AddTaskStep::Status,
            AddTaskStep::Priority,
            AddTaskStep::Labels,
            AddTaskStep::Schedule,
            AddTaskStep::Epic,
            AddTaskStep::Images,
            AddTaskStep::Title,
            AddTaskStep::Description,
        ] {
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
    fn recurring_schedule_tab_order_matches_visual_order() {
        let mut state = add_task_state(AddTaskStep::Schedule);
        state.repeat_rule = LineEdit::new("daily".to_string());
        let editor = state.schedule_editor(ScheduleEditorField::Mode);
        state.mode = AddTaskMode::Schedule(editor);
        let mut overlay = OverlayState::AddTask(state);

        for expected in [
            ScheduleEditorField::Repeat,
            ScheduleEditorField::Time,
            ScheduleEditorField::DuePolicy,
            ScheduleEditorField::Starts,
            ScheduleEditorField::Mode,
        ] {
            let OverlayOutcome::None(next) = handle(key(KeyCode::Tab), overlay) else {
                panic!("expected composer");
            };
            let OverlayState::AddTask(state) = &next else {
                panic!("expected add task");
            };
            let AddTaskMode::Schedule(editor) = &state.mode else {
                panic!("expected schedule editor");
            };
            assert_eq!(editor.focus, expected);
            overlay = next;
        }
    }

    #[test]
    fn add_task_tab_skips_empty_image_field() {
        let state = add_task_state(AddTaskStep::RepeatStartOn);
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
        assert_eq!(state.focus, AddTaskStep::Epic);
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
    fn add_task_schedule_text_updates_structured_fields() {
        let state = add_task_state(AddTaskStep::Schedule);
        let OverlayState::AddTask(state) =
            handle_generic_overlay_paste("tomorrow", OverlayState::AddTask(state))
        else {
            panic!("expected composer");
        };
        assert_eq!(state.schedule_input.text, "tomorrow");
        assert_eq!(state.available_at.text, "tomorrow");
        assert!(state.schedule_error.is_none());
    }

    #[test]
    fn add_task_schedule_text_validates_after_leaving_the_field() {
        let state = add_task_state(AddTaskStep::Schedule);
        let OverlayState::AddTask(state) =
            handle_generic_overlay_paste("d", OverlayState::AddTask(state))
        else {
            panic!("expected composer");
        };
        assert!(state.schedule_error.is_some());
        assert!(!state.schedule_validation_requested);

        let OverlayOutcome::None(OverlayState::AddTask(state)) =
            handle(key(KeyCode::Tab), OverlayState::AddTask(state))
        else {
            panic!("expected composer");
        };
        assert!(state.schedule_validation_requested);
    }

    #[test]
    fn cancelling_schedule_editor_restores_content_sized_composer() {
        let mut state = add_task_state(AddTaskStep::Schedule);
        state.mode = AddTaskMode::Schedule(state.schedule_editor(ScheduleEditorField::Mode));

        let OverlayOutcome::None(OverlayState::AddTask(state)) =
            handle(key(KeyCode::Esc), OverlayState::AddTask(state))
        else {
            panic!("expected composer");
        };

        assert_eq!(state.mode, AddTaskMode::Compose);
        assert!(!state.mode.expands_composer());
    }

    #[test]
    fn add_task_schedule_editor_applies_natural_summary() {
        let mut state = add_task_state(AddTaskStep::Schedule);
        let mut editor = state.schedule_editor(ScheduleEditorField::Repeat);
        editor.mode = ScheduleEditorMode::Repeat;
        editor.focus = ScheduleEditorField::Repeat;
        editor.repeat_rule = LineEdit::new("daily".to_string());
        editor.refresh();
        state.mode = AddTaskMode::Schedule(editor);

        let OverlayOutcome::None(OverlayState::AddTask(state)) =
            handle(key(KeyCode::Enter), OverlayState::AddTask(state))
        else {
            panic!("expected composer");
        };
        assert_eq!(state.mode, AddTaskMode::Compose);
        assert_eq!(state.repeat_rule.text, "daily");
        assert!(state.schedule_input.text.starts_with("daily"));
    }

    #[test]
    fn schedule_editor_validates_on_tab_and_clears_while_editing() {
        let mut state = add_task_state(AddTaskStep::Schedule);
        let mut editor = state.schedule_editor(ScheduleEditorField::Repeat);
        editor.mode = ScheduleEditorMode::Repeat;
        editor.refresh();
        state.mode = AddTaskMode::Schedule(editor);
        assert!(matches!(
            &state.mode,
            AddTaskMode::Schedule(editor) if editor.error.is_none()
        ));

        let OverlayOutcome::None(OverlayState::AddTask(state)) =
            handle(key(KeyCode::Char('d')), OverlayState::AddTask(state))
        else {
            panic!("expected composer");
        };
        assert!(matches!(
            &state.mode,
            AddTaskMode::Schedule(editor) if editor.error.is_none()
        ));

        let OverlayOutcome::None(OverlayState::AddTask(state)) =
            handle(key(KeyCode::Tab), OverlayState::AddTask(state))
        else {
            panic!("expected composer");
        };
        assert!(matches!(
            &state.mode,
            AddTaskMode::Schedule(editor)
                if editor.focus == ScheduleEditorField::Time && editor.error.is_some()
        ));

        let OverlayOutcome::None(OverlayState::AddTask(state)) =
            handle(key(KeyCode::Char('0')), OverlayState::AddTask(state))
        else {
            panic!("expected composer");
        };
        assert!(matches!(
            &state.mode,
            AddTaskMode::Schedule(editor) if editor.error.is_none()
        ));
    }

    #[test]
    fn schedule_editor_up_and_down_navigate_fields() {
        let mut state = add_task_state(AddTaskStep::Schedule);
        let mut editor = state.schedule_editor(ScheduleEditorField::Repeat);
        editor.mode = ScheduleEditorMode::Repeat;
        state.mode = AddTaskMode::Schedule(editor);

        let OverlayOutcome::None(OverlayState::AddTask(state)) =
            handle(key(KeyCode::Down), OverlayState::AddTask(state))
        else {
            panic!("expected composer");
        };
        assert!(matches!(
            &state.mode,
            AddTaskMode::Schedule(editor) if editor.focus == ScheduleEditorField::Time
        ));

        let OverlayOutcome::None(OverlayState::AddTask(state)) =
            handle(key(KeyCode::Up), OverlayState::AddTask(state))
        else {
            panic!("expected composer");
        };
        assert!(matches!(
            &state.mode,
            AddTaskMode::Schedule(editor) if editor.focus == ScheduleEditorField::Repeat
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
        let state = MultilineInputState::from_value(
            MultilineIntent::AddTaskNatural,
            "Notes",
            "Body",
            "line".to_string(),
        );
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
        let state = MultilineInputState::from_value(
            add_note_intent(),
            "Add note",
            "note body:",
            "line".to_string(),
        );
        let outcome = handle(ctrl(KeyCode::Enter), OverlayState::MultilineInput(state));
        assert!(matches!(
            outcome,
            OverlayOutcome::Submitted(OverlaySubmit::Multiline { .. })
        ));
    }

    #[test]
    fn populated_add_note_requires_discard_confirmation() {
        let mut state = MultilineInputState::blank(add_note_intent(), "Add note", "note body:");
        state.insert_paste(" first\nsecond ");
        state.row = 0;
        state.column = 3;

        let OverlayOutcome::None(OverlayState::MultilineInput(state)) =
            handle(key(KeyCode::Esc), OverlayState::MultilineInput(state))
        else {
            panic!("expected discard confirmation");
        };
        assert_eq!(state.mode, MultilineInputMode::ConfirmDiscard);
        assert_eq!(state.lines, vec![" first", "second "]);
        assert_eq!((state.row, state.column), (0, 3));
    }

    #[test]
    fn clean_add_note_cancels_without_confirmation() {
        let state = MultilineInputState::blank(add_note_intent(), "Add note", "");
        assert!(matches!(
            handle(key(KeyCode::Esc), OverlayState::MultilineInput(state)),
            OverlayOutcome::Cancelled
        ));
    }

    #[test]
    fn changed_blank_add_note_requires_discard_confirmation() {
        let mut state = MultilineInputState::blank(add_note_intent(), "Add note", "");
        state.lines = vec!["   ".to_string()];
        assert!(matches!(
            handle(key(KeyCode::Esc), OverlayState::MultilineInput(state)),
            OverlayOutcome::None(OverlayState::MultilineInput(MultilineInputState {
                mode: MultilineInputMode::ConfirmDiscard,
                ..
            }))
        ));
    }

    #[test]
    fn add_note_discard_confirmation_accepts_y_and_rejects_submission() {
        for code in [KeyCode::Char('y'), KeyCode::Char('Y')] {
            let mut state = MultilineInputState::from_value(
                add_note_intent(),
                "Add note",
                "",
                "draft".to_string(),
            );
            state.mode = MultilineInputMode::ConfirmDiscard;
            assert!(matches!(
                handle(key(code), OverlayState::MultilineInput(state)),
                OverlayOutcome::Cancelled
            ));
        }

        for key in [ctrl(KeyCode::Char('s')), ctrl(KeyCode::Enter)] {
            let mut state = MultilineInputState::from_value(
                add_note_intent(),
                "Add note",
                "",
                "draft".to_string(),
            );
            state.mode = MultilineInputMode::ConfirmDiscard;
            assert!(matches!(
                handle(key, OverlayState::MultilineInput(state)),
                OverlayOutcome::None(OverlayState::MultilineInput(MultilineInputState {
                    mode: MultilineInputMode::ConfirmDiscard,
                    ..
                }))
            ));
        }
    }

    #[test]
    fn add_note_discard_confirmation_preserves_draft_when_cancelled() {
        for code in [KeyCode::Char('n'), KeyCode::Char('N'), KeyCode::Esc] {
            let mut state = MultilineInputState::from_value(
                add_note_intent(),
                "Add note",
                "",
                "first\nsecond".to_string(),
            );
            state.row = 0;
            state.column = 2;
            state.mode = MultilineInputMode::ConfirmDiscard;
            let OverlayOutcome::None(OverlayState::MultilineInput(state)) =
                handle(key(code), OverlayState::MultilineInput(state))
            else {
                panic!("expected composer");
            };
            assert_eq!(state.mode, MultilineInputMode::Compose);
            assert_eq!(state.lines, vec!["first", "second"]);
            assert_eq!((state.row, state.column), (0, 2));
        }
    }

    #[test]
    fn repeated_esc_never_discards_populated_add_note() {
        let mut state = MultilineInputState::blank(add_note_intent(), "Add note", "");
        state.insert_paste("draft");
        let OverlayOutcome::None(OverlayState::MultilineInput(state)) =
            handle(key(KeyCode::Esc), OverlayState::MultilineInput(state))
        else {
            panic!("expected discard confirmation");
        };
        assert_eq!(state.mode, MultilineInputMode::ConfirmDiscard);
        let OverlayOutcome::None(OverlayState::MultilineInput(state)) =
            handle(key(KeyCode::Esc), OverlayState::MultilineInput(state))
        else {
            panic!("expected composer");
        };
        assert_eq!(state.mode, MultilineInputMode::Compose);
        assert_eq!(state.lines, vec!["draft"]);
    }

    #[test]
    fn clean_initialized_description_edit_cancels_without_confirmation() {
        let state = MultilineInputState::from_value(
            description_intent(),
            "Edit description",
            "",
            "existing description".to_string(),
        );
        assert!(matches!(
            handle(key(KeyCode::Esc), OverlayState::MultilineInput(state)),
            OverlayOutcome::Cancelled
        ));
    }

    #[test]
    fn changed_description_edit_requires_discard_confirmation() {
        let mut state = MultilineInputState::from_value(
            description_intent(),
            "Edit description",
            "",
            "existing description".to_string(),
        );
        state.insert_paste(" updated");
        assert!(matches!(
            handle(key(KeyCode::Esc), OverlayState::MultilineInput(state)),
            OverlayOutcome::None(OverlayState::MultilineInput(MultilineInputState {
                mode: MultilineInputMode::ConfirmDiscard,
                ..
            }))
        ));
    }

    #[test]
    fn clean_initialized_manual_conflict_merge_cancels_without_confirmation() {
        let state = MultilineInputState::from_value(
            manual_conflict_description_intent(),
            "Resolve conflict: manual",
            "",
            "local description".to_string(),
        );
        assert!(matches!(
            handle(key(KeyCode::Esc), OverlayState::MultilineInput(state)),
            OverlayOutcome::Cancelled
        ));
    }

    #[test]
    fn changed_manual_conflict_merge_requires_discard_confirmation() {
        let mut state = MultilineInputState::from_value(
            manual_conflict_description_intent(),
            "Resolve conflict: manual",
            "",
            "local description".to_string(),
        );
        state.insert_paste(" updated");
        assert!(matches!(
            handle(key(KeyCode::Esc), OverlayState::MultilineInput(state)),
            OverlayOutcome::None(OverlayState::MultilineInput(MultilineInputState {
                mode: MultilineInputMode::ConfirmDiscard,
                ..
            }))
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
    fn detail_marker_does_not_own_scroll_input() {
        assert!(matches!(
            handle(key(KeyCode::Char('j')), OverlayState::Detail),
            OverlayOutcome::None(OverlayState::Detail)
        ));
    }

    #[test]
    fn esc_cancels_all_input_overlay_variants() {
        let overlays = vec![
            OverlayState::TextInput(TextInputState::new(
                TextIntent::AddProject,
                "Title",
                "Prompt",
                "value".to_string(),
            )),
            OverlayState::MultilineInput(MultilineInputState::from_value(
                MultilineIntent::AddTaskNatural,
                "Body",
                "Prompt",
                "value".to_string(),
            )),
            OverlayState::Picker(PickerState {
                intent: PickerIntent::FilterLabel,
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
                intent: config_intent(),
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
            intent: config_intent(),
            title: "Delete".to_string(),
            prompt: "Sure?".to_string(),
        };
        assert!(matches!(
            handle(
                key(KeyCode::Char('y')),
                OverlayState::Confirm(state.clone())
            ),
            OverlayOutcome::Submitted(OverlaySubmit::Confirm {
                intent: ConfirmIntent::InitializeConfig { .. },
            })
        ));
        assert!(matches!(
            handle(key(KeyCode::Char('n')), OverlayState::Confirm(state)),
            OverlayOutcome::Cancelled
        ));
    }

    #[test]
    fn ctrl_d_requests_explicit_date_clear() {
        let outcome = handle(
            ctrl(KeyCode::Char('d')),
            OverlayState::TextInput(TextInputState::new(
                due_intent(),
                "Edit due date",
                "",
                String::new(),
            )),
        );

        assert!(matches!(
            outcome,
            OverlayOutcome::Submitted(OverlaySubmit::ClearDate {
                intent: TextIntent::EditDue { .. }
            })
        ));
    }

    #[test]
    fn submit_variants_propagate_intents() {
        let text = handle(
            key(KeyCode::Enter),
            OverlayState::TextInput(TextInputState::new(
                TextIntent::AddProject,
                "Add project",
                "name:",
                "app".to_string(),
            )),
        );
        assert!(matches!(
            text,
            OverlayOutcome::Submitted(OverlaySubmit::Text {
                intent: TextIntent::AddProject,
                ..
            })
        ));

        let multiline = handle(
            ctrl(KeyCode::Char('s')),
            OverlayState::MultilineInput(MultilineInputState::from_value(
                add_note_intent(),
                "Add note",
                "body:",
                "note".to_string(),
            )),
        );
        assert!(matches!(
            multiline,
            OverlayOutcome::Submitted(OverlaySubmit::Multiline {
                intent: MultilineIntent::AddNote { .. },
                ..
            })
        ));

        let picker = handle(
            key(KeyCode::Enter),
            OverlayState::Picker(PickerState {
                intent: PickerIntent::FilterLabel,
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
                intent: PickerIntent::FilterLabel,
                ..
            })
        ));
    }
}
