use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::state::MultilineInputState;
use crate::tui::text::{
    char_boundary_at_or_before, next_char_boundary, next_char_is_whitespace,
    previous_char_boundary, previous_word_start,
};

pub(crate) fn edit_multiline_input(state: &mut MultilineInputState, key: KeyEvent) {
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
        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) && row > 0 => {
            state.row = row - 1;
            state.column = char_boundary_at_or_before(&state.lines[state.row], state.column);
        }
        KeyCode::Down if row + 1 < state.lines.len() => {
            state.row = row + 1;
            state.column = char_boundary_at_or_before(&state.lines[state.row], state.column);
        }
        KeyCode::Char('n')
            if key.modifiers.contains(KeyModifiers::CONTROL) && row + 1 < state.lines.len() =>
        {
            state.row = row + 1;
            state.column = char_boundary_at_or_before(&state.lines[state.row], state.column);
        }
        KeyCode::Left if column > 0 => {
            state.column = previous_char_boundary(&state.lines[row], column);
        }
        KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::CONTROL) && column > 0 => {
            state.column = previous_char_boundary(&state.lines[row], column);
        }
        KeyCode::Right if column < state.lines[row].len() => {
            state.column = next_char_boundary(&state.lines[row], column);
        }
        KeyCode::Char('f')
            if key.modifiers.contains(KeyModifiers::CONTROL) && column < state.lines[row].len() =>
        {
            state.column = next_char_boundary(&state.lines[row], column);
        }
        KeyCode::Home => state.column = 0,
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => state.column = 0,
        KeyCode::End => state.column = state.lines[row].len(),
        KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.column = state.lines[row].len()
        }
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
        KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) && column > 0 => {
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
        KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.lines[row].truncate(column);
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.lines[row].drain(..column);
            state.column = 0;
        }
        KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            kill_multiline_word_before_cursor(state);
        }
        KeyCode::Delete if column < state.lines[row].len() => {
            let next = next_char_boundary(&state.lines[row], column);
            state.lines[row].drain(column..next);
        }
        KeyCode::Char('d')
            if key.modifiers.contains(KeyModifiers::CONTROL) && column < state.lines[row].len() =>
        {
            let next = next_char_boundary(&state.lines[row], column);
            state.lines[row].drain(column..next);
        }
        KeyCode::Delete if row + 1 < state.lines.len() => {
            let line = state.lines.remove(row + 1);
            state.lines[row].push_str(&line);
        }
        KeyCode::Char(ch) if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT => {
            state.lines[row].insert(column, ch);
            state.column = column + ch.len_utf8();
        }
        _ => state.column = column,
    }
}

fn kill_multiline_word_before_cursor(state: &mut MultilineInputState) {
    while state.row > 0 && state.column == 0 {
        let line = state.lines.remove(state.row);
        state.row -= 1;
        state.column = state.lines[state.row].len();
        state.lines[state.row].push_str(&line);
    }

    if state.lines.is_empty() || state.column == 0 {
        return;
    }

    let row = state.row.min(state.lines.len() - 1);
    let column = char_boundary_at_or_before(&state.lines[row], state.column);
    let previous = previous_word_start(&state.lines[row], column);
    state.lines[row].drain(previous..column);
    if previous > 0 && next_char_is_whitespace(&state.lines[row], previous) {
        let before = previous_char_boundary(&state.lines[row], previous);
        if state.lines[row][before..previous]
            .chars()
            .all(char::is_whitespace)
        {
            state.lines[row].drain(before..previous);
            state.column = before;
        } else {
            state.column = previous;
        }
    } else {
        state.column = previous;
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

    fn state_with_lines(lines: Vec<String>, row: usize, column: usize) -> MultilineInputState {
        MultilineInputState {
            route: OverlayRoute::MessageOnly,
            title: "Title".to_string(),
            prompt: "Prompt".to_string(),
            lines,
            row,
            column,
        }
    }

    #[test]
    fn multiline_input_splits_and_merges_lines() {
        let mut state = state_with_lines(vec!["ab".to_string()], 0, 1);
        edit_multiline_input(&mut state, key(KeyCode::Enter));
        assert_eq!(state.lines, vec!["a".to_string(), "b".to_string()]);
        state.row = 1;
        state.column = 0;
        edit_multiline_input(&mut state, key(KeyCode::Backspace));
        assert_eq!(state.lines, vec!["ab".to_string()]);
    }

    #[test]
    fn multiline_input_supports_emacs_navigation() {
        let mut state = state_with_lines(vec!["abc".to_string(), "déf".to_string()], 0, 1);
        edit_multiline_input(&mut state, ctrl(KeyCode::Char('e')));
        assert_eq!(state.column, 3);
        edit_multiline_input(&mut state, ctrl(KeyCode::Char('b')));
        assert_eq!(state.column, 2);
        edit_multiline_input(&mut state, ctrl(KeyCode::Char('f')));
        assert_eq!(state.column, 3);
        edit_multiline_input(&mut state, ctrl(KeyCode::Char('n')));
        assert_eq!(state.row, 1);
        assert_eq!(state.column, 3);
        edit_multiline_input(&mut state, ctrl(KeyCode::Char('a')));
        assert_eq!(state.column, 0);
        edit_multiline_input(&mut state, ctrl(KeyCode::Char('p')));
        assert_eq!(state.row, 0);
        assert_eq!(state.column, 0);
    }

    #[test]
    fn multiline_input_supports_emacs_deletion() {
        let mut state = state_with_lines(vec!["one two three".to_string()], 0, 7);
        edit_multiline_input(&mut state, ctrl(KeyCode::Char('w')));
        assert_eq!(state.lines, vec!["one three".to_string()]);
        assert_eq!(state.column, 3);
        edit_multiline_input(&mut state, ctrl(KeyCode::Char('k')));
        assert_eq!(state.lines, vec!["one".to_string()]);
        assert_eq!(state.column, 3);
        edit_multiline_input(&mut state, ctrl(KeyCode::Char('u')));
        assert_eq!(state.lines, vec![String::new()]);
        assert_eq!(state.column, 0);
    }

    #[test]
    fn multiline_ctrl_w_merges_previous_line_at_line_start() {
        let mut state = state_with_lines(vec!["one ".to_string(), "two three".to_string()], 1, 0);
        edit_multiline_input(&mut state, ctrl(KeyCode::Char('w')));
        assert_eq!(state.lines, vec!["two three".to_string()]);
        assert_eq!(state.row, 0);
        assert_eq!(state.column, 0);
    }

    #[test]
    fn multiline_delete_at_line_end_merges_next_line() {
        let mut state = state_with_lines(vec!["one".to_string(), "two".to_string()], 0, 3);
        edit_multiline_input(&mut state, key(KeyCode::Delete));
        assert_eq!(state.lines, vec!["onetwo".to_string()]);
        assert_eq!(state.row, 0);
        assert_eq!(state.column, 3);
    }

    #[test]
    fn multiline_ignores_control_chars_that_are_not_editing_keys() {
        let mut state = state_with_lines(vec!["ab".to_string()], 0, 1);
        edit_multiline_input(&mut state, ctrl(KeyCode::Char('x')));
        assert_eq!(state.lines, vec!["ab".to_string()]);
        assert_eq!(state.column, 1);
    }

    #[test]
    fn multiline_long_line_navigation_keeps_byte_cursor_valid() {
        let mut state = state_with_lines(vec!["a".repeat(140), "é".to_string()], 0, 139);
        edit_multiline_input(&mut state, key(KeyCode::Right));
        assert_eq!(state.column, 140);
        edit_multiline_input(&mut state, key(KeyCode::Down));
        assert_eq!(state.row, 1);
        assert_eq!(state.column, "é".len());
        edit_multiline_input(&mut state, key(KeyCode::Left));
        assert_eq!(state.column, 0);
        assert!(state.lines[state.row].is_char_boundary(state.column));
    }
}
