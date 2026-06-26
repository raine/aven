use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::state::{
    OverlayOutcome, OverlayState, OverlaySubmit, PickerItem, PickerMode, PickerState,
};

pub(crate) fn visible_picker_indices(state: &PickerState) -> Vec<usize> {
    let filter = state.filter.as_str().trim().to_ascii_lowercase();
    let dashless_filter = filter.replace('-', "");
    state
        .items
        .iter()
        .enumerate()
        .filter(|(_, item)| picker_item_matches(item, &filter, &dashless_filter))
        .map(|(index, _)| index)
        .collect()
}

fn picker_item_matches(item: &PickerItem, filter: &str, dashless_filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    let label = item.label.to_ascii_lowercase();
    label.contains(filter) || label.replace('-', "").contains(dashless_filter)
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

pub(crate) fn handle_picker_key(state: PickerState, key: KeyEvent) -> OverlayOutcome {
    match state.mode {
        PickerMode::Navigate => handle_picker_navigation_key(state, key),
        PickerMode::Filter => handle_picker_filter_key(state, key),
    }
}

fn handle_picker_navigation_key(mut state: PickerState, key: KeyEvent) -> OverlayOutcome {
    match key.code {
        KeyCode::Esc => OverlayOutcome::Cancelled,
        KeyCode::Enter => picker_submit_outcome(state),
        KeyCode::Char('/') | KeyCode::Char('i') => {
            state.mode = PickerMode::Filter;
            OverlayOutcome::None(OverlayState::Picker(state))
        }
        KeyCode::Char('j') | KeyCode::Down => {
            move_picker_selection(&mut state, 1);
            OverlayOutcome::None(OverlayState::Picker(state))
        }
        KeyCode::Char('k') | KeyCode::Up => {
            move_picker_selection(&mut state, -1);
            OverlayOutcome::None(OverlayState::Picker(state))
        }
        KeyCode::Char('n') if key.modifiers == KeyModifiers::CONTROL => {
            move_picker_selection(&mut state, 1);
            OverlayOutcome::None(OverlayState::Picker(state))
        }
        KeyCode::Char('p') if key.modifiers == KeyModifiers::CONTROL => {
            move_picker_selection(&mut state, -1);
            OverlayOutcome::None(OverlayState::Picker(state))
        }
        KeyCode::Char(' ') if state.multi => {
            toggle_picker_item(&mut state);
            OverlayOutcome::None(OverlayState::Picker(state))
        }
        _ => OverlayOutcome::None(OverlayState::Picker(state)),
    }
}

fn handle_picker_filter_key(mut state: PickerState, key: KeyEvent) -> OverlayOutcome {
    match key.code {
        KeyCode::Esc => {
            state.mode = PickerMode::Navigate;
            OverlayOutcome::None(OverlayState::Picker(state))
        }
        KeyCode::Enter => picker_submit_outcome(state),
        KeyCode::Down => {
            move_picker_selection(&mut state, 1);
            OverlayOutcome::None(OverlayState::Picker(state))
        }
        KeyCode::Char('n') if key.modifiers == KeyModifiers::CONTROL => {
            move_picker_selection(&mut state, 1);
            OverlayOutcome::None(OverlayState::Picker(state))
        }
        KeyCode::Up => {
            move_picker_selection(&mut state, -1);
            OverlayOutcome::None(OverlayState::Picker(state))
        }
        KeyCode::Char('p') if key.modifiers == KeyModifiers::CONTROL => {
            move_picker_selection(&mut state, -1);
            OverlayOutcome::None(OverlayState::Picker(state))
        }
        KeyCode::Char(' ') if state.multi => {
            toggle_picker_item(&mut state);
            OverlayOutcome::None(OverlayState::Picker(state))
        }
        _ => {
            state.filter.handle_key(key);
            normalize_picker_selection(&mut state);
            OverlayOutcome::None(OverlayState::Picker(state))
        }
    }
}

pub(super) fn picker_submit_outcome(state: PickerState) -> OverlayOutcome {
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
        route: state.route,
        title: state.title,
        values,
    })
}

fn toggle_picker_item(state: &mut PickerState) {
    if let Some(index) = visible_picker_indices(state)
        .iter()
        .find(|item| **item == state.selected)
        .copied()
    {
        state.items[index].selected = !state.items[index].selected;
    }
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
