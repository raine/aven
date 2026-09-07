use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::tui::text::{
    char_boundary_at_or_before, next_char_boundary, next_char_is_whitespace,
    normalize_pasted_newlines, previous_char_boundary, previous_word_start,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LineEdit {
    pub(crate) text: String,
    pub(crate) cursor: usize,
}

impl LineEdit {
    pub(crate) fn new(text: String) -> Self {
        let cursor = text.len();
        Self { text, cursor }
    }

    pub(crate) fn blank() -> Self {
        Self::new(String::new())
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.text
    }

    pub(crate) fn insert_paste(&mut self, text: &str) {
        let text = normalize_pasted_newlines(text).replace('\n', " ");
        let cursor = char_boundary_at_or_before(&self.text, self.cursor);
        self.text.insert_str(cursor, &text);
        self.cursor = cursor + text.len();
    }

    pub(crate) fn handle_key(&mut self, key: KeyEvent) {
        edit_line(&mut self.text, &mut self.cursor, key);
    }
}

/// Apply line-local keys using a UTF-8 byte cursor normalized before editing.
pub(super) fn edit_line(text: &mut String, byte_cursor: &mut usize, key: KeyEvent) {
    let cursor = char_boundary_at_or_before(text, *byte_cursor);
    match key.code {
        KeyCode::Left => *byte_cursor = previous_char_boundary(text, cursor),
        KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            *byte_cursor = previous_char_boundary(text, cursor);
        }
        KeyCode::Right => *byte_cursor = next_char_boundary(text, cursor),
        KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            *byte_cursor = next_char_boundary(text, cursor);
        }
        KeyCode::Home => *byte_cursor = 0,
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            *byte_cursor = 0;
        }
        KeyCode::End => *byte_cursor = text.len(),
        KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            *byte_cursor = text.len();
        }
        KeyCode::Backspace if cursor > 0 => {
            let previous = previous_char_boundary(text, cursor);
            text.drain(previous..cursor);
            *byte_cursor = previous;
        }
        KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) && cursor > 0 => {
            let previous = previous_char_boundary(text, cursor);
            text.drain(previous..cursor);
            *byte_cursor = previous;
        }
        KeyCode::Delete if cursor < text.len() => {
            let next = next_char_boundary(text, cursor);
            text.drain(cursor..next);
            *byte_cursor = cursor;
        }
        KeyCode::Char('d')
            if key.modifiers.contains(KeyModifiers::CONTROL) && cursor < text.len() =>
        {
            let next = next_char_boundary(text, cursor);
            text.drain(cursor..next);
            *byte_cursor = cursor;
        }
        KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            text.truncate(cursor);
            *byte_cursor = cursor;
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            text.drain(..cursor);
            *byte_cursor = 0;
        }
        KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            let previous = previous_word_start(text, cursor);
            text.drain(previous..cursor);
            if previous > 0 && next_char_is_whitespace(text, previous) {
                let before = previous_char_boundary(text, previous);
                if text[before..previous].chars().all(char::is_whitespace) {
                    text.drain(before..previous);
                    *byte_cursor = before;
                } else {
                    *byte_cursor = previous;
                }
            } else {
                *byte_cursor = previous;
            }
        }
        KeyCode::Char(ch) if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT => {
            text.insert(cursor, ch);
            *byte_cursor = cursor + ch.len_utf8();
        }
        _ => *byte_cursor = cursor,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    fn line_edit(input: &str, cursor: usize) -> LineEdit {
        LineEdit {
            text: input.to_string(),
            cursor,
        }
    }

    #[test]
    fn unicode_edits_use_scalar_boundaries_including_combining_marks() {
        let mut state = line_edit("中e\u{301}界", "中e\u{301}".len());
        state.handle_key(key(KeyCode::Backspace));
        assert_eq!((state.as_str(), state.cursor), ("中e界", "中e".len()));
        state.handle_key(key(KeyCode::Delete));
        assert_eq!((state.as_str(), state.cursor), ("中e", "中e".len()));
        state.cursor = 2;
        state.handle_key(key(KeyCode::Char('界')));
        assert_eq!((state.as_str(), state.cursor), ("界中e", "界".len()));
    }

    #[test]
    fn text_input_edits_at_cursor() {
        let mut state = line_edit("ab", 1);
        state.handle_key(key(KeyCode::Char('x')));
        assert_eq!(state.text, "axb");
        assert_eq!(state.cursor, 2);
        state.handle_key(key(KeyCode::Backspace));
        assert_eq!(state.text, "ab");
        assert_eq!(state.cursor, 1);
    }

    #[test]
    fn text_input_supports_emacs_navigation() {
        let mut state = line_edit("abc", 1);
        state.handle_key(ctrl(KeyCode::Char('a')));
        assert_eq!(state.cursor, 0);
        state.handle_key(ctrl(KeyCode::Char('e')));
        assert_eq!(state.cursor, 3);
        state.handle_key(ctrl(KeyCode::Char('b')));
        assert_eq!(state.cursor, 2);
        state.handle_key(ctrl(KeyCode::Char('f')));
        assert_eq!(state.cursor, 3);
    }

    #[test]
    fn text_input_supports_emacs_deletion() {
        let mut state = line_edit("one two three", 7);
        state.handle_key(ctrl(KeyCode::Char('w')));
        assert_eq!(state.text, "one three");
        assert_eq!(state.cursor, 3);
        state.handle_key(ctrl(KeyCode::Char('k')));
        assert_eq!(state.text, "one");
        assert_eq!(state.cursor, 3);
        state.handle_key(ctrl(KeyCode::Char('u')));
        assert_eq!(state.text, "");
        assert_eq!(state.cursor, 0);
    }

    #[test]
    fn text_input_ignores_control_chars_that_are_not_editing_keys() {
        let mut state = line_edit("ab", 1);
        state.handle_key(ctrl(KeyCode::Char('x')));
        assert_eq!(state.text, "ab");
        assert_eq!(state.cursor, 1);
    }
}
