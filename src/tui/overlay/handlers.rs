use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::tui::authoring::AddTaskStep;

use super::multiline::edit_multiline_input;
use super::picker::{handle_picker_key, normalize_picker_selection};
use super::state::{OverlayOutcome, OverlayState, OverlaySubmit};

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
                state.scroll = state.scroll.saturating_add(1);
                OverlayOutcome::None(OverlayState::TextPanel(state))
            }
            KeyCode::Char('k') | KeyCode::Up => {
                state.scroll = state.scroll.saturating_sub(1);
                OverlayOutcome::None(OverlayState::TextPanel(state))
            }
            _ => OverlayOutcome::None(OverlayState::TextPanel(state)),
        },
        OverlayState::SyncStatus(state) => match key.code {
            KeyCode::Esc | KeyCode::Enter => OverlayOutcome::Cancelled,
            _ => OverlayOutcome::None(OverlayState::SyncStatus(state)),
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
