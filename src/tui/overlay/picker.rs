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

pub(crate) fn normalize_picker_scroll(state: &mut PickerState, viewport_rows: usize) {
    let visible = visible_picker_indices(state);
    let selected_position = visible
        .iter()
        .position(|index| *index == state.selected)
        .unwrap_or(0);
    state.scroll = picker_viewport_start(
        state.scroll,
        selected_position,
        visible.len(),
        viewport_rows,
    );
}

pub(crate) fn picker_viewport_start(
    current_start: usize,
    selected_position: usize,
    row_count: usize,
    viewport_rows: usize,
) -> usize {
    if viewport_rows == 0 || row_count <= viewport_rows {
        return 0;
    }
    let max_start = row_count - viewport_rows;
    let start = current_start.min(max_start);
    if selected_position <= start {
        selected_position
    } else if selected_position >= start + viewport_rows {
        selected_position.saturating_sub(viewport_rows.saturating_sub(1))
    } else {
        start
    }
}

pub(crate) fn handle_picker_key(state: PickerState, key: KeyEvent) -> OverlayOutcome {
    match state.mode {
        PickerMode::Navigate => handle_picker_navigation_key(state, key),
        PickerMode::Filter => handle_picker_filter_key(state, key),
    }
}

fn handle_picker_navigation_key(mut state: PickerState, key: KeyEvent) -> OverlayOutcome {
    if apply_shared_picker_action(&mut state, key) {
        return continue_picker(state);
    }

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
        _ => OverlayOutcome::None(OverlayState::Picker(state)),
    }
}

fn handle_picker_filter_key(mut state: PickerState, key: KeyEvent) -> OverlayOutcome {
    if apply_shared_picker_action(&mut state, key) {
        return continue_picker(state);
    }

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
        KeyCode::Up => {
            move_picker_selection(&mut state, -1);
            OverlayOutcome::None(OverlayState::Picker(state))
        }
        _ => {
            state.filter.handle_key(key);
            normalize_picker_selection(&mut state);
            OverlayOutcome::None(OverlayState::Picker(state))
        }
    }
}

fn apply_shared_picker_action(state: &mut PickerState, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Char('n') if key.modifiers == KeyModifiers::CONTROL => {
            move_picker_selection(state, 1);
            true
        }
        KeyCode::Char('p') if key.modifiers == KeyModifiers::CONTROL => {
            move_picker_selection(state, -1);
            true
        }
        KeyCode::Char(' ') if state.multi => {
            toggle_picker_item(state);
            true
        }
        _ => false,
    }
}

fn continue_picker(state: PickerState) -> OverlayOutcome {
    OverlayOutcome::None(OverlayState::Picker(state))
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
    if let Some(next) =
        super::wrap_index_by_value(&visible_picker_indices(state), state.selected, delta)
    {
        state.selected = next;
        normalize_picker_scroll(state, crate::tui::overlay::GENERIC_PICKER_VIEWPORT_ROWS);
    }
}
