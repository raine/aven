use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::labels::normalize_label;

use super::state::{OverlayOutcome, OverlayState, OverlaySubmit, TagComboboxState};

pub(crate) fn tag_combobox_matches(state: &TagComboboxState) -> Vec<usize> {
    ranked_matches(&state.options, state.input.as_str())
}

pub(crate) fn tag_combobox_completion(state: &TagComboboxState) -> Option<String> {
    let input = normalize_label(state.input.as_str());
    if input.is_empty() {
        return None;
    }
    state
        .options
        .get(state.highlighted)
        .filter(|label| label.starts_with(&input))
        .and_then(|label| label.get(input.len()..))
        .filter(|suffix| !suffix.is_empty())
        .map(str::to_string)
}

pub(crate) fn normalize_tag_combobox_highlight(state: &mut TagComboboxState) {
    let matches = tag_combobox_matches(state);
    state.highlighted = matches
        .iter()
        .copied()
        .find(|index| *index == state.highlighted)
        .or_else(|| matches.first().copied())
        .unwrap_or(0);
}

pub(crate) fn tag_combobox_has_create_option(state: &TagComboboxState) -> bool {
    let input = normalize_label(state.input.as_str());
    !input.is_empty() && !state.options.iter().any(|label| label == &input)
}

pub(crate) fn handle_tag_combobox_key(
    mut state: TagComboboxState,
    key: KeyEvent,
) -> OverlayOutcome {
    match key.code {
        KeyCode::Esc if !state.input.as_str().is_empty() => {
            clear_input(&mut state);
            OverlayOutcome::None(OverlayState::TagCombobox(state))
        }
        KeyCode::Esc => OverlayOutcome::Cancelled,
        KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            tag_combobox_submit(state)
        }
        KeyCode::Enter => tag_combobox_submit(state),
        KeyCode::Char(' ') if state.input.as_str().is_empty() => {
            toggle_highlighted_label(&mut state, false);
            OverlayOutcome::None(OverlayState::TagCombobox(state))
        }
        KeyCode::Backspace if state.input.as_str().is_empty() => {
            state.selected.pop();
            OverlayOutcome::None(OverlayState::TagCombobox(state))
        }
        KeyCode::Tab => {
            if tag_combobox_matches(&state).is_empty() && tag_combobox_has_create_option(&state) {
                activate_highlighted(&mut state)
            } else if !state.input.as_str().is_empty() {
                activate_highlighted(&mut state)
            } else {
                toggle_highlighted_label(&mut state, false);
                OverlayOutcome::None(OverlayState::TagCombobox(state))
            }
        }
        KeyCode::Down => {
            move_highlight(&mut state, 1);
            OverlayOutcome::None(OverlayState::TagCombobox(state))
        }
        KeyCode::Up => {
            move_highlight(&mut state, -1);
            OverlayOutcome::None(OverlayState::TagCombobox(state))
        }
        KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            move_highlight(&mut state, 1);
            OverlayOutcome::None(OverlayState::TagCombobox(state))
        }
        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            move_highlight(&mut state, -1);
            OverlayOutcome::None(OverlayState::TagCombobox(state))
        }
        _ => {
            state.input.handle_key(key);
            normalize_tag_combobox_highlight(&mut state);
            OverlayOutcome::None(OverlayState::TagCombobox(state))
        }
    }
}

fn activate_highlighted(state: &mut TagComboboxState) -> OverlayOutcome {
    if tag_combobox_matches(state).is_empty() && tag_combobox_has_create_option(state) {
        let label = normalize_label(state.input.as_str());
        add_label(state, label);
        clear_input(state);
        return OverlayOutcome::None(OverlayState::TagCombobox(state.clone()));
    }

    toggle_highlighted_label(state, true);
    OverlayOutcome::None(OverlayState::TagCombobox(state.clone()))
}

fn tag_combobox_submit(state: TagComboboxState) -> OverlayOutcome {
    OverlayOutcome::Submitted(OverlaySubmit::Picker {
        route: state.route,
        title: state.title,
        values: dedupe_labels(state.selected),
    })
}

fn toggle_highlighted_label(state: &mut TagComboboxState, clear_after_toggle: bool) {
    let Some(label) = state.options.get(state.highlighted).cloned() else {
        return;
    };
    if let Some(index) = state
        .selected
        .iter()
        .position(|selected| selected == &label)
    {
        state.selected.remove(index);
    } else {
        state.selected.push(label);
    }
    if clear_after_toggle {
        clear_input(state);
    }
}

fn clear_input(state: &mut TagComboboxState) {
    state.input.text.clear();
    state.input.cursor = 0;
    normalize_tag_combobox_highlight(state);
}

fn add_label(state: &mut TagComboboxState, label: String) {
    if !state.options.contains(&label) {
        state.options.push(label.clone());
        state.options.sort();
    }
    if !state.selected.contains(&label) {
        state.selected.push(label.clone());
    }
    if let Some(index) = state.options.iter().position(|option| option == &label) {
        state.highlighted = index;
    }
}

fn move_highlight(state: &mut TagComboboxState, delta: isize) {
    let matches = tag_combobox_matches(state);
    if matches.is_empty() {
        return;
    }
    let current = matches
        .iter()
        .position(|index| *index == state.highlighted)
        .unwrap_or(0);
    let next = current as isize + delta;
    let next = if next < 0 {
        matches.len() - 1
    } else if next >= matches.len() as isize {
        0
    } else {
        next as usize
    };
    state.highlighted = matches[next];
}

fn ranked_matches(options: &[String], input: &str) -> Vec<usize> {
    let input = normalize_label(input);
    if input.is_empty() {
        return (0..options.len()).collect();
    }
    let dashless_input = input.replace('-', "");
    let mut ranked = Vec::new();
    for rank in 0..4 {
        for (index, label) in options.iter().enumerate() {
            if ranked.iter().any(|existing| *existing == index) {
                continue;
            }
            let dashless_label = label.replace('-', "");
            let matches = match rank {
                0 => label.starts_with(&input),
                1 => dashless_label.starts_with(&dashless_input),
                2 => label.contains(&input),
                _ => dashless_label.contains(&dashless_input),
            };
            if matches {
                ranked.push(index);
            }
        }
    }
    ranked
}

fn dedupe_labels(labels: Vec<String>) -> Vec<String> {
    labels.into_iter().fold(Vec::new(), |mut deduped, label| {
        if !deduped.contains(&label) {
            deduped.push(label);
        }
        deduped
    })
}
