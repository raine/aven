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
        let cursor = char_boundary_at_or_before(&self.text, self.cursor);
        match key.code {
            KeyCode::Left => self.cursor = previous_char_boundary(&self.text, cursor),
            KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cursor = previous_char_boundary(&self.text, cursor);
            }
            KeyCode::Right => self.cursor = next_char_boundary(&self.text, cursor),
            KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cursor = next_char_boundary(&self.text, cursor);
            }
            KeyCode::Home => self.cursor = 0,
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cursor = 0;
            }
            KeyCode::End => self.cursor = self.text.len(),
            KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cursor = self.text.len();
            }
            KeyCode::Backspace if cursor > 0 => {
                let previous = previous_char_boundary(&self.text, cursor);
                self.text.drain(previous..cursor);
                self.cursor = previous;
            }
            KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) && cursor > 0 => {
                let previous = previous_char_boundary(&self.text, cursor);
                self.text.drain(previous..cursor);
                self.cursor = previous;
            }
            KeyCode::Delete if cursor < self.text.len() => {
                let next = next_char_boundary(&self.text, cursor);
                self.text.drain(cursor..next);
                self.cursor = cursor;
            }
            KeyCode::Char('d')
                if key.modifiers.contains(KeyModifiers::CONTROL) && cursor < self.text.len() =>
            {
                let next = next_char_boundary(&self.text, cursor);
                self.text.drain(cursor..next);
                self.cursor = cursor;
            }
            KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.text.truncate(cursor);
                self.cursor = cursor;
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.text.drain(..cursor);
                self.cursor = 0;
            }
            KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let previous = previous_word_start(&self.text, cursor);
                self.text.drain(previous..cursor);
                if previous > 0 && next_char_is_whitespace(&self.text, previous) {
                    let before = previous_char_boundary(&self.text, previous);
                    if self.text[before..previous].chars().all(char::is_whitespace) {
                        self.text.drain(before..previous);
                        self.cursor = before;
                    } else {
                        self.cursor = previous;
                    }
                } else {
                    self.cursor = previous;
                }
            }
            KeyCode::Char(ch)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                self.text.insert(cursor, ch);
                self.cursor = cursor + ch.len_utf8();
            }
            _ => self.cursor = cursor,
        }
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
