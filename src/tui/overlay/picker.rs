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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::overlay::OverlayRoute;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    fn picker_navigation_state() -> PickerState {
        PickerState {
            route: OverlayRoute::MessageOnly,
            title: "Pick".to_string(),
            filter: crate::tui::overlay::LineEdit::blank(),
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
            selected: 0,
            scroll: 0,
            multi: false,
            mode: PickerMode::Navigate,
        }
    }

    fn picker_state_with_items(count: usize) -> PickerState {
        PickerState {
            route: OverlayRoute::EditLabels,
            title: "Pick".to_string(),
            filter: crate::tui::overlay::LineEdit::blank(),
            items: (0..count)
                .map(|index| PickerItem {
                    label: format!("Label {index}"),
                    value: format!("label-{index}"),
                    selected: false,
                })
                .collect(),
            selected: 0,
            scroll: 0,
            multi: true,
            mode: PickerMode::Navigate,
        }
    }

    #[test]
    fn picker_filter_and_selection_normalize() {
        let mut state = PickerState {
            route: OverlayRoute::MessageOnly,
            title: "Pick".to_string(),
            filter: crate::tui::overlay::LineEdit::new("alp".to_string()),
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
            scroll: 0,
            multi: false,
            mode: PickerMode::Navigate,
        };
        normalize_picker_selection(&mut state);
        assert_eq!(state.selected, 0);
        assert_eq!(visible_picker_indices(&state), vec![0]);
    }

    #[test]
    fn picker_filter_ignores_dashes_in_labels() {
        let state = PickerState {
            route: OverlayRoute::MessageOnly,
            title: "Go: project".to_string(),
            filter: crate::tui::overlay::LineEdit::new("gitsur".to_string()),
            items: vec![PickerItem {
                label: "GS git-surgeon".to_string(),
                value: "git-surgeon".to_string(),
                selected: false,
            }],
            selected: 0,
            scroll: 0,
            multi: false,
            mode: PickerMode::Navigate,
        };

        assert_eq!(visible_picker_indices(&state), vec![0]);
    }

    #[test]
    fn picker_filter_preserves_dash_matching() {
        let state = PickerState {
            route: OverlayRoute::MessageOnly,
            title: "Pick".to_string(),
            filter: crate::tui::overlay::LineEdit::new("git-sur".to_string()),
            items: vec![PickerItem {
                label: "GS git-surgeon".to_string(),
                value: "git-surgeon".to_string(),
                selected: false,
            }],
            selected: 0,
            scroll: 0,
            multi: false,
            mode: PickerMode::Navigate,
        };

        assert_eq!(visible_picker_indices(&state), vec![0]);
    }

    #[test]
    fn picker_moves_with_j_and_k_in_navigation_mode() {
        let state = picker_navigation_state();
        let OverlayOutcome::None(OverlayState::Picker(state)) =
            handle_picker_key(state, key(KeyCode::Char('j')))
        else {
            panic!("expected picker state");
        };
        assert_eq!(state.selected, 1);
        assert_eq!(state.filter.as_str(), "");
        let OverlayOutcome::None(OverlayState::Picker(state)) =
            handle_picker_key(state, key(KeyCode::Char('k')))
        else {
            panic!("expected picker state");
        };
        assert_eq!(state.selected, 0);
        assert_eq!(state.filter.as_str(), "");
    }

    #[test]
    fn picker_types_j_and_k_in_filter_mode() {
        let mut state = picker_navigation_state();
        state.mode = PickerMode::Filter;
        let OverlayOutcome::None(OverlayState::Picker(state)) =
            handle_picker_key(state, key(KeyCode::Char('j')))
        else {
            panic!("expected picker state");
        };
        let OverlayOutcome::None(OverlayState::Picker(state)) =
            handle_picker_key(state, key(KeyCode::Char('k')))
        else {
            panic!("expected picker state");
        };
        assert_eq!(state.filter.as_str(), "jk");
    }

    #[test]
    fn picker_slash_enters_filter_mode_and_esc_returns_to_navigation() {
        let state = picker_navigation_state();
        let OverlayOutcome::None(OverlayState::Picker(state)) =
            handle_picker_key(state, key(KeyCode::Char('/')))
        else {
            panic!("expected picker state");
        };
        assert_eq!(state.mode, PickerMode::Filter);
        let OverlayOutcome::None(OverlayState::Picker(state)) =
            handle_picker_key(state, key(KeyCode::Esc))
        else {
            panic!("expected picker state");
        };
        assert_eq!(state.mode, PickerMode::Navigate);
    }

    #[test]
    fn picker_moves_with_arrow_keys() {
        let state = picker_navigation_state();
        let OverlayOutcome::None(OverlayState::Picker(state)) =
            handle_picker_key(state, key(KeyCode::Down))
        else {
            panic!("expected picker state");
        };
        assert_eq!(state.selected, 1);
        let OverlayOutcome::None(OverlayState::Picker(state)) =
            handle_picker_key(state, key(KeyCode::Up))
        else {
            panic!("expected picker state");
        };
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn picker_moves_with_ctrl_n_and_ctrl_p() {
        let state = picker_navigation_state();
        let OverlayOutcome::None(OverlayState::Picker(state)) =
            handle_picker_key(state, ctrl(KeyCode::Char('n')))
        else {
            panic!("expected picker state");
        };
        assert_eq!(state.selected, 1);
        let OverlayOutcome::None(OverlayState::Picker(state)) =
            handle_picker_key(state, ctrl(KeyCode::Char('p')))
        else {
            panic!("expected picker state");
        };
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn picker_scroll_stays_at_bottom_until_selection_reaches_top_edge() {
        let mut state = picker_state_with_items(10);
        state.selected = 9;
        state.scroll = 2;

        for expected_selected in (2..9).rev() {
            let OverlayOutcome::None(OverlayState::Picker(next)) =
                handle_picker_key(state, key(KeyCode::Up))
            else {
                panic!("expected picker state");
            };
            assert_eq!(next.selected, expected_selected);
            assert_eq!(next.scroll, 2);
            state = next;
        }

        let OverlayOutcome::None(OverlayState::Picker(next)) =
            handle_picker_key(state, key(KeyCode::Up))
        else {
            panic!("expected picker state");
        };
        assert_eq!(next.selected, 1);
        assert_eq!(next.scroll, 1);
    }

    #[test]
    fn picker_scroll_moves_down_after_bottom_edge() {
        let state = picker_state_with_items(10);
        let mut next = state;
        for expected_selected in 1..=8 {
            let OverlayOutcome::None(OverlayState::Picker(state)) =
                handle_picker_key(next, key(KeyCode::Down))
            else {
                panic!("expected picker state");
            };
            next = state;
            assert_eq!(next.selected, expected_selected);
        }
        assert_eq!(next.scroll, 1);
    }
}
