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
