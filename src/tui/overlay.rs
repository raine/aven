use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OverlayState {
    Help,
    Detail,
    Search { input: String },
    Command { input: String },
    TextInput(TextInputState),
    MultilineInput(MultilineInputState),
    Picker(PickerState),
    Confirm(ConfirmState),
    TextPanel(TextPanelState),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextPanelState {
    pub(crate) title: String,
    pub(crate) lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextInputState {
    pub(crate) title: String,
    pub(crate) prompt: String,
    pub(crate) input: String,
    pub(crate) cursor: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MultilineInputState {
    pub(crate) title: String,
    pub(crate) prompt: String,
    pub(crate) lines: Vec<String>,
    pub(crate) row: usize,
    pub(crate) column: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PickerState {
    pub(crate) title: String,
    pub(crate) filter: String,
    pub(crate) items: Vec<PickerItem>,
    pub(crate) selected: usize,
    pub(crate) multi: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PickerItem {
    pub(crate) label: String,
    pub(crate) value: String,
    pub(crate) selected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConfirmState {
    pub(crate) title: String,
    pub(crate) prompt: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OverlayView {
    Help,
    Detail,
    Search { input: String },
    Command { input: String },
    TextInput(TextInputView),
    MultilineInput(MultilineInputView),
    Picker(PickerView),
    Confirm(ConfirmView),
    TextPanel(TextPanelView),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextPanelView {
    pub(crate) title: String,
    pub(crate) lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextInputView {
    pub(crate) title: String,
    pub(crate) prompt: String,
    pub(crate) input: String,
    pub(crate) cursor: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MultilineInputView {
    pub(crate) title: String,
    pub(crate) prompt: String,
    pub(crate) lines: Vec<String>,
    pub(crate) row: usize,
    pub(crate) column: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PickerView {
    pub(crate) title: String,
    pub(crate) filter: String,
    pub(crate) items: Vec<PickerItem>,
    pub(crate) selected: usize,
    pub(crate) multi: bool,
    pub(crate) visible_indices: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConfirmView {
    pub(crate) title: String,
    pub(crate) prompt: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OverlaySubmit {
    Text { title: String, value: String },
    Multiline { title: String, value: String },
    Picker { title: String, values: Vec<String> },
    Confirm { title: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OverlayOutcome {
    None(OverlayState),
    Cancelled,
    Submitted(OverlaySubmit),
}

impl OverlaySubmit {
    pub(crate) fn message(&self) -> String {
        match self {
            Self::Text { title, .. } => format!("submitted {title}"),
            Self::Multiline { title, .. } => format!("submitted {title}"),
            Self::Picker { title, .. } => format!("selected {title}"),
            Self::Confirm { title } => format!("confirmed {title}"),
        }
    }
}

impl OverlayState {
    pub(crate) fn captures_input(&self) -> bool {
        matches!(
            self,
            Self::Search { .. }
                | Self::Command { .. }
                | Self::TextInput(_)
                | Self::MultilineInput(_)
                | Self::Picker(_)
                | Self::Confirm(_)
                | Self::TextPanel(_)
        )
    }
}

impl OverlayView {
    pub(crate) fn captures_input(&self) -> bool {
        matches!(
            self,
            Self::Search { .. }
                | Self::Command { .. }
                | Self::TextInput(_)
                | Self::MultilineInput(_)
                | Self::Picker(_)
                | Self::Confirm(_)
                | Self::TextPanel(_)
        )
    }
}

impl From<&OverlayState> for OverlayView {
    fn from(state: &OverlayState) -> Self {
        match state {
            OverlayState::Help => Self::Help,
            OverlayState::Detail => Self::Detail,
            OverlayState::Search { input } => Self::Search {
                input: input.clone(),
            },
            OverlayState::Command { input } => Self::Command {
                input: input.clone(),
            },
            OverlayState::TextInput(state) => Self::TextInput(TextInputView {
                title: state.title.clone(),
                prompt: state.prompt.clone(),
                input: state.input.clone(),
                cursor: state.cursor,
            }),
            OverlayState::MultilineInput(state) => Self::MultilineInput(MultilineInputView {
                title: state.title.clone(),
                prompt: state.prompt.clone(),
                lines: state.lines.clone(),
                row: state.row,
                column: state.column,
            }),
            OverlayState::Picker(state) => Self::Picker(PickerView {
                title: state.title.clone(),
                filter: state.filter.clone(),
                items: state.items.clone(),
                selected: state.selected,
                multi: state.multi,
                visible_indices: visible_picker_indices(state),
            }),
            OverlayState::Confirm(state) => Self::Confirm(ConfirmView {
                title: state.title.clone(),
                prompt: state.prompt.clone(),
            }),
            OverlayState::TextPanel(state) => Self::TextPanel(TextPanelView {
                title: state.title.clone(),
                lines: state.lines.clone(),
            }),
        }
    }
}

pub(crate) fn visible_picker_indices(state: &PickerState) -> Vec<usize> {
    let filter = state.filter.trim().to_ascii_lowercase();
    state
        .items
        .iter()
        .enumerate()
        .filter(|(_, item)| filter.is_empty() || item.label.to_ascii_lowercase().contains(&filter))
        .map(|(index, _)| index)
        .collect()
}

pub(crate) fn normalize_picker_selection(state: &mut PickerState) {
    let visible = visible_picker_indices(state);
    state.selected = visible
        .iter()
        .copied()
        .find(|index| *index == state.selected)
        .or_else(|| visible.first().copied())
        .unwrap_or(0);
}

pub(crate) fn handle_generic_overlay_key(key: KeyEvent, overlay: OverlayState) -> OverlayOutcome {
    match overlay {
        OverlayState::TextInput(mut state) => match key.code {
            KeyCode::Esc => OverlayOutcome::Cancelled,
            KeyCode::Enter => OverlayOutcome::Submitted(OverlaySubmit::Text {
                title: state.title.clone(),
                value: state.input.clone(),
            }),
            _ => {
                edit_text_input(&mut state, key);
                OverlayOutcome::None(OverlayState::TextInput(state))
            }
        },
        OverlayState::MultilineInput(mut state) => {
            if key.code == KeyCode::Char('s') && key.modifiers.contains(KeyModifiers::CONTROL) {
                let value = state.lines.join("\n");
                return OverlayOutcome::Submitted(OverlaySubmit::Multiline {
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
        OverlayState::Picker(mut state) => match key.code {
            KeyCode::Esc => OverlayOutcome::Cancelled,
            KeyCode::Enter => {
                let values = if state.multi {
                    state
                        .items
                        .iter()
                        .filter(|item| item.selected)
                        .map(|item| item.value.clone())
                        .collect()
                } else {
                    visible_picker_indices(&state)
                        .iter()
                        .find(|index| **index == state.selected)
                        .map(|index| vec![state.items[*index].value.clone()])
                        .unwrap_or_default()
                };
                OverlayOutcome::Submitted(OverlaySubmit::Picker {
                    title: state.title.clone(),
                    values,
                })
            }
            KeyCode::Char('j') | KeyCode::Down => {
                move_picker_selection(&mut state, 1);
                OverlayOutcome::None(OverlayState::Picker(state))
            }
            KeyCode::Char('k') | KeyCode::Up => {
                move_picker_selection(&mut state, -1);
                OverlayOutcome::None(OverlayState::Picker(state))
            }
            KeyCode::Char(' ') if state.multi => {
                if let Some(index) = visible_picker_indices(&state)
                    .iter()
                    .find(|item| **item == state.selected)
                    .copied()
                {
                    state.items[index].selected = !state.items[index].selected;
                }
                OverlayOutcome::None(OverlayState::Picker(state))
            }
            KeyCode::Backspace => {
                state.filter.pop();
                normalize_picker_selection(&mut state);
                OverlayOutcome::None(OverlayState::Picker(state))
            }
            KeyCode::Char(ch) => {
                state.filter.push(ch);
                normalize_picker_selection(&mut state);
                OverlayOutcome::None(OverlayState::Picker(state))
            }
            _ => OverlayOutcome::None(OverlayState::Picker(state)),
        },
        OverlayState::Confirm(state) => match key.code {
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => OverlayOutcome::Cancelled,
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                OverlayOutcome::Submitted(OverlaySubmit::Confirm {
                    title: state.title.clone(),
                })
            }
            _ => OverlayOutcome::None(OverlayState::Confirm(state)),
        },
        OverlayState::TextPanel(state) => match key.code {
            KeyCode::Esc | KeyCode::Enter => OverlayOutcome::Cancelled,
            _ => OverlayOutcome::None(OverlayState::TextPanel(state)),
        },
        other => OverlayOutcome::None(other),
    }
}

fn edit_text_input(state: &mut TextInputState, key: KeyEvent) {
    let cursor = char_boundary_at_or_before(&state.input, state.cursor);
    match key.code {
        KeyCode::Left => state.cursor = previous_char_boundary(&state.input, cursor),
        KeyCode::Right => state.cursor = next_char_boundary(&state.input, cursor),
        KeyCode::Home => state.cursor = 0,
        KeyCode::End => state.cursor = state.input.len(),
        KeyCode::Backspace if cursor > 0 => {
            let previous = previous_char_boundary(&state.input, cursor);
            state.input.drain(previous..cursor);
            state.cursor = previous;
        }
        KeyCode::Delete if cursor < state.input.len() => {
            let next = next_char_boundary(&state.input, cursor);
            state.input.drain(cursor..next);
            state.cursor = cursor;
        }
        KeyCode::Char(ch) => {
            state.input.insert(cursor, ch);
            state.cursor = cursor + ch.len_utf8();
        }
        _ => state.cursor = cursor,
    }
}

fn edit_multiline_input(state: &mut MultilineInputState, key: KeyEvent) {
    if state.lines.is_empty() {
        state.lines.push(String::new());
    }
    let row = state.row.min(state.lines.len() - 1);
    let column = char_boundary_at_or_before(&state.lines[row], state.column);
    state.row = row;
    state.column = column;

    match key.code {
        KeyCode::Up if row > 0 => {
            state.row = row - 1;
            state.column = char_boundary_at_or_before(&state.lines[state.row], state.column);
        }
        KeyCode::Down if row + 1 < state.lines.len() => {
            state.row = row + 1;
            state.column = char_boundary_at_or_before(&state.lines[state.row], state.column);
        }
        KeyCode::Left if column > 0 => {
            state.column = previous_char_boundary(&state.lines[row], column);
        }
        KeyCode::Right if column < state.lines[row].len() => {
            state.column = next_char_boundary(&state.lines[row], column);
        }
        KeyCode::Home => state.column = 0,
        KeyCode::End => state.column = state.lines[row].len(),
        KeyCode::Enter => {
            let rest = state.lines[row].split_off(column);
            state.lines.insert(row + 1, rest);
            state.row = row + 1;
            state.column = 0;
        }
        KeyCode::Backspace if column > 0 => {
            let previous = previous_char_boundary(&state.lines[row], column);
            state.lines[row].drain(previous..column);
            state.column = previous;
        }
        KeyCode::Backspace if row > 0 => {
            let line = state.lines.remove(row);
            state.row = row - 1;
            state.column = state.lines[state.row].len();
            state.lines[state.row].push_str(&line);
        }
        KeyCode::Delete if column < state.lines[row].len() => {
            let next = next_char_boundary(&state.lines[row], column);
            state.lines[row].drain(column..next);
        }
        KeyCode::Char(ch) => {
            state.lines[row].insert(column, ch);
            state.column = column + ch.len_utf8();
        }
        _ => state.column = column,
    }
}

fn char_boundary_at_or_before(input: &str, index: usize) -> usize {
    let mut index = index.min(input.len());
    while !input.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn previous_char_boundary(input: &str, index: usize) -> usize {
    let mut index = char_boundary_at_or_before(input, index).saturating_sub(1);
    while !input.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn next_char_boundary(input: &str, index: usize) -> usize {
    let mut index = char_boundary_at_or_before(input, index)
        .saturating_add(1)
        .min(input.len());
    while !input.is_char_boundary(index) {
        index += 1;
    }
    index
}

fn move_picker_selection(state: &mut PickerState, delta: isize) {
    let visible = visible_picker_indices(state);
    if visible.is_empty() {
        return;
    }
    let current = visible
        .iter()
        .position(|index| *index == state.selected)
        .unwrap_or(0);
    let next = current as isize + delta;
    let next = if next < 0 {
        visible.len() - 1
    } else if next >= visible.len() as isize {
        0
    } else {
        next as usize
    };
    state.selected = visible[next];
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    #[test]
    fn text_input_edits_at_cursor() {
        let mut state = TextInputState {
            title: "Title".to_string(),
            prompt: "Prompt".to_string(),
            input: "ab".to_string(),
            cursor: 1,
        };
        edit_text_input(&mut state, key(KeyCode::Char('x')));
        assert_eq!(state.input, "axb");
        assert_eq!(state.cursor, 2);
        edit_text_input(&mut state, key(KeyCode::Backspace));
        assert_eq!(state.input, "ab");
        assert_eq!(state.cursor, 1);
    }

    #[test]
    fn multiline_input_splits_and_merges_lines() {
        let mut state = MultilineInputState {
            title: "Title".to_string(),
            prompt: "Prompt".to_string(),
            lines: vec!["ab".to_string()],
            row: 0,
            column: 1,
        };
        edit_multiline_input(&mut state, key(KeyCode::Enter));
        assert_eq!(state.lines, vec!["a".to_string(), "b".to_string()]);
        state.row = 1;
        state.column = 0;
        edit_multiline_input(&mut state, key(KeyCode::Backspace));
        assert_eq!(state.lines, vec!["ab".to_string()]);
    }

    #[test]
    fn multiline_ctrl_s_submits() {
        let state = MultilineInputState {
            title: "Notes".to_string(),
            prompt: "Body".to_string(),
            lines: vec!["line".to_string()],
            row: 0,
            column: 4,
        };
        let outcome = handle_generic_overlay_key(
            ctrl(KeyCode::Char('s')),
            OverlayState::MultilineInput(state),
        );
        assert!(matches!(
            outcome,
            OverlayOutcome::Submitted(OverlaySubmit::Multiline { .. })
        ));
    }

    #[test]
    fn picker_filter_and_selection_normalize() {
        let mut state = PickerState {
            title: "Pick".to_string(),
            filter: String::new(),
            items: vec![
                PickerItem {
                    label: "Alpha".to_string(),
                    value: "a".to_string(),
                    selected: false,
                },
                PickerItem {
                    label: "Beta".to_string(),
                    value: "b".to_string(),
                    selected: false,
                },
            ],
            selected: 1,
            multi: false,
        };
        state.filter = "alp".to_string();
        normalize_picker_selection(&mut state);
        assert_eq!(state.selected, 0);
        assert_eq!(visible_picker_indices(&state), vec![0]);
    }

    #[test]
    fn text_panel_closes_on_enter_and_esc() {
        let state = TextPanelState {
            title: "Conflicts".to_string(),
            lines: vec!["field=title".to_string()],
        };
        assert!(matches!(
            handle_generic_overlay_key(key(KeyCode::Enter), OverlayState::TextPanel(state.clone())),
            OverlayOutcome::Cancelled
        ));
        assert!(matches!(
            handle_generic_overlay_key(key(KeyCode::Esc), OverlayState::TextPanel(state)),
            OverlayOutcome::Cancelled
        ));
    }

    #[test]
    fn esc_cancels_all_generic_overlay_variants() {
        let overlays = vec![
            OverlayState::TextInput(TextInputState {
                title: "Title".to_string(),
                prompt: "Prompt".to_string(),
                input: "value".to_string(),
                cursor: 5,
            }),
            OverlayState::MultilineInput(MultilineInputState {
                title: "Body".to_string(),
                prompt: "Prompt".to_string(),
                lines: vec!["value".to_string()],
                row: 0,
                column: 5,
            }),
            OverlayState::Picker(PickerState {
                title: "Pick".to_string(),
                filter: String::new(),
                items: vec![PickerItem {
                    label: "One".to_string(),
                    value: "one".to_string(),
                    selected: false,
                }],
                selected: 0,
                multi: false,
            }),
            OverlayState::Confirm(ConfirmState {
                title: "Confirm".to_string(),
                prompt: "Continue?".to_string(),
            }),
            OverlayState::TextPanel(TextPanelState {
                title: "Panel".to_string(),
                lines: vec!["line".to_string()],
            }),
        ];

        for overlay in overlays {
            assert!(matches!(
                handle_generic_overlay_key(key(KeyCode::Esc), overlay),
                OverlayOutcome::Cancelled
            ));
        }
    }

    #[test]
    fn confirm_yes_and_no() {
        let state = ConfirmState {
            title: "Delete".to_string(),
            prompt: "Sure?".to_string(),
        };
        assert!(matches!(
            handle_generic_overlay_key(
                key(KeyCode::Char('y')),
                OverlayState::Confirm(state.clone())
            ),
            OverlayOutcome::Submitted(_)
        ));
        assert!(matches!(
            handle_generic_overlay_key(key(KeyCode::Char('n')), OverlayState::Confirm(state)),
            OverlayOutcome::Cancelled
        ));
    }
}
