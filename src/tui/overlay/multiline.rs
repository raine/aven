use crossterm::event::KeyEvent;

use super::state::MultilineInputState;

pub(crate) fn edit_multiline_input(state: &mut MultilineInputState, key: KeyEvent) {
    super::text_buffer::edit_text_buffer(&mut state.buffer, key);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::overlay::MultilineIntent;
    use crossterm::event::{KeyCode, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    fn state_with_lines(lines: Vec<String>, row: usize, column: usize) -> MultilineInputState {
        let mut state = MultilineInputState::from_value(
            MultilineIntent::AddTaskNatural,
            "Title",
            "Prompt",
            lines.join("\n"),
        );
        state.buffer.row = row;
        state.buffer.column = column;
        state
    }

    #[test]
    fn shared_keys_match_single_line_for_unicode_and_invalid_byte_cursors() {
        use crate::tui::overlay::text_input::LineEdit;

        let mut keys = vec![
            key(KeyCode::Left),
            key(KeyCode::Right),
            key(KeyCode::Home),
            key(KeyCode::End),
            key(KeyCode::Backspace),
            key(KeyCode::Delete),
            key(KeyCode::Char('界')),
            key(KeyCode::Char('\u{301}')),
            KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT),
            key(KeyCode::Tab),
        ];
        keys.extend("bfaehdkuwx".chars().map(|ch| ctrl(KeyCode::Char(ch))));
        for text in ["", "ascii words", "中界 e\u{301} 文", "  中  界  "] {
            for cursor in (0..=text.len() + 1).chain([usize::MAX]) {
                for &key in &keys {
                    let mut single = LineEdit::new(text.to_string());
                    single.cursor = cursor;
                    let mut multi = state_with_lines(vec![text.to_string()], 0, cursor);
                    single.handle_key(key);
                    edit_multiline_input(&mut multi, key);
                    assert_eq!(multi.buffer.lines, vec![single.text.clone()]);
                    assert_eq!(multi.buffer.column, single.cursor);
                    assert!(single.text.is_char_boundary(single.cursor));
                    assert_eq!(multi.buffer.is_dirty(), single.text != text);
                }
            }
        }
    }

    #[test]
    fn unicode_line_boundaries_keep_control_deletion_local() {
        for (code, row, column) in [('h', 1, 0), ('d', 0, "中".len())] {
            let mut state = state_with_lines(vec!["中".into(), "e\u{301}".into()], row, column);
            edit_multiline_input(&mut state, ctrl(KeyCode::Char(code)));
            assert_eq!(state.buffer.lines, vec!["中", "e\u{301}"]);
            assert_eq!((state.buffer.row, state.buffer.column), (row, column));
            assert!(!state.buffer.is_dirty());
            let code = if code == 'h' {
                KeyCode::Backspace
            } else {
                KeyCode::Delete
            };
            edit_multiline_input(&mut state, key(code));
            assert_eq!(state.buffer.lines, vec!["中e\u{301}"]);
            assert_eq!((state.buffer.row, state.buffer.column), (0, "中".len()));
        }
    }

    #[test]
    fn word_deletion_crosses_empty_lines_and_preserves_unicode_suffix() {
        let mut state = state_with_lines(vec!["中 界 ".into(), "".into(), "e\u{301}".into()], 2, 0);
        edit_multiline_input(&mut state, ctrl(KeyCode::Char('w')));
        assert_eq!(state.buffer.lines, vec!["中 e\u{301}"]);
        assert_eq!((state.buffer.row, state.buffer.column), (0, "中 ".len()));
    }

    #[test]
    fn newline_split_normalizes_unicode_cursor_and_vertical_navigation() {
        let mut state = state_with_lines(vec!["中e\u{301}".into()], 0, 2);
        edit_multiline_input(&mut state, key(KeyCode::Enter));
        assert_eq!(state.buffer.lines, vec!["", "中e\u{301}"]);
        assert_eq!((state.buffer.row, state.buffer.column), (1, 0));
        edit_multiline_input(&mut state, key(KeyCode::End));
        edit_multiline_input(&mut state, key(KeyCode::Up));
        assert_eq!((state.buffer.row, state.buffer.column), (0, 0));
        edit_multiline_input(&mut state, key(KeyCode::Left));
        assert_eq!((state.buffer.row, state.buffer.column), (0, 0));
    }

    #[test]
    fn multiline_input_splits_and_merges_lines() {
        let mut state = state_with_lines(vec!["ab".to_string()], 0, 1);
        edit_multiline_input(&mut state, key(KeyCode::Enter));
        assert_eq!(state.buffer.lines, vec!["a".to_string(), "b".to_string()]);
        state.buffer.row = 1;
        state.buffer.column = 0;
        edit_multiline_input(&mut state, key(KeyCode::Backspace));
        assert_eq!(state.buffer.lines, vec!["ab".to_string()]);
    }

    #[test]
    fn multiline_input_supports_emacs_navigation() {
        let mut state = state_with_lines(vec!["abc".to_string(), "déf".to_string()], 0, 1);
        edit_multiline_input(&mut state, ctrl(KeyCode::Char('e')));
        assert_eq!(state.buffer.column, 3);
        edit_multiline_input(&mut state, ctrl(KeyCode::Char('b')));
        assert_eq!(state.buffer.column, 2);
        edit_multiline_input(&mut state, ctrl(KeyCode::Char('f')));
        assert_eq!(state.buffer.column, 3);
        edit_multiline_input(&mut state, ctrl(KeyCode::Char('n')));
        assert_eq!(state.buffer.row, 1);
        assert_eq!(state.buffer.column, 3);
        edit_multiline_input(&mut state, ctrl(KeyCode::Char('a')));
        assert_eq!(state.buffer.column, 0);
        edit_multiline_input(&mut state, ctrl(KeyCode::Char('p')));
        assert_eq!(state.buffer.row, 0);
        assert_eq!(state.buffer.column, 0);
    }

    #[test]
    fn multiline_input_supports_emacs_deletion() {
        let mut state = state_with_lines(vec!["one two three".to_string()], 0, 7);
        edit_multiline_input(&mut state, ctrl(KeyCode::Char('w')));
        assert_eq!(state.buffer.lines, vec!["one three".to_string()]);
        assert_eq!(state.buffer.column, 3);
        edit_multiline_input(&mut state, ctrl(KeyCode::Char('k')));
        assert_eq!(state.buffer.lines, vec!["one".to_string()]);
        assert_eq!(state.buffer.column, 3);
        edit_multiline_input(&mut state, ctrl(KeyCode::Char('u')));
        assert_eq!(state.buffer.lines, vec![String::new()]);
        assert_eq!(state.buffer.column, 0);
    }

    #[test]
    fn multiline_ctrl_w_merges_previous_line_at_line_start() {
        let mut state = state_with_lines(vec!["one ".to_string(), "two three".to_string()], 1, 0);
        edit_multiline_input(&mut state, ctrl(KeyCode::Char('w')));
        assert_eq!(state.buffer.lines, vec!["two three".to_string()]);
        assert_eq!(state.buffer.row, 0);
        assert_eq!(state.buffer.column, 0);
    }

    #[test]
    fn multiline_delete_at_line_end_merges_next_line() {
        let mut state = state_with_lines(vec!["one".to_string(), "two".to_string()], 0, 3);
        edit_multiline_input(&mut state, key(KeyCode::Delete));
        assert_eq!(state.buffer.lines, vec!["onetwo".to_string()]);
        assert_eq!(state.buffer.row, 0);
        assert_eq!(state.buffer.column, 3);
    }

    #[test]
    fn multiline_ignores_control_chars_that_are_not_editing_keys() {
        let mut state = state_with_lines(vec!["ab".to_string()], 0, 1);
        edit_multiline_input(&mut state, ctrl(KeyCode::Char('x')));
        assert_eq!(state.buffer.lines, vec!["ab".to_string()]);
        assert_eq!(state.buffer.column, 1);
    }

    #[test]
    fn multiline_long_line_navigation_keeps_byte_cursor_valid() {
        let mut state = state_with_lines(vec!["a".repeat(140), "é".to_string()], 0, 139);
        edit_multiline_input(&mut state, key(KeyCode::Right));
        assert_eq!(state.buffer.column, 140);
        edit_multiline_input(&mut state, key(KeyCode::Down));
        assert_eq!(state.buffer.row, 1);
        assert_eq!(state.buffer.column, "é".len());
        edit_multiline_input(&mut state, key(KeyCode::Left));
        assert_eq!(state.buffer.column, 0);
        assert!(state.buffer.lines[state.buffer.row].is_char_boundary(state.buffer.column));
    }
}
