use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::tui::text::char_boundary_at_or_before;

use super::text_input::edit_line;

/// Editable UTF-8 text with a byte cursor and an initial discard baseline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextBuffer {
    pub(crate) lines: Vec<String>,
    pub(crate) row: usize,
    pub(crate) column: usize,
    baseline: Vec<String>,
}

impl TextBuffer {
    pub(crate) fn from_value(value: String) -> Self {
        Self::from_value_with_baseline(value.clone(), value)
    }

    pub(crate) fn from_value_with_baseline(value: String, baseline: String) -> Self {
        let lines = value.split('\n').map(str::to_string).collect::<Vec<_>>();
        let baseline = baseline.split('\n').map(str::to_string).collect();
        let row = lines.len() - 1;
        let column = lines[row].len();
        Self {
            lines,
            row,
            column,
            baseline,
        }
    }

    pub(crate) fn is_dirty(&self) -> bool {
        self.lines != self.baseline
    }

    pub(crate) fn baseline_value(&self) -> String {
        self.baseline.join("\n")
    }

    pub(crate) fn insert_exact(&mut self, text: &str) {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        let row = self.row.min(self.lines.len() - 1);
        let column = char_boundary_at_or_before(&self.lines[row], self.column);
        self.row = row;
        self.column = column;

        let mut pasted_lines = text.split('\n');
        let first = pasted_lines.next().unwrap_or_default();
        let rest = self.lines[row].split_off(column);
        self.lines[row].push_str(first);

        let mut insert_at = row;
        for line in pasted_lines {
            insert_at += 1;
            self.lines.insert(insert_at, line.to_string());
        }
        self.lines[insert_at].push_str(&rest);
        self.row = insert_at;
        self.column = self.lines[insert_at].len().saturating_sub(rest.len());
    }
}

pub(crate) fn edit_text_buffer(state: &mut TextBuffer, key: KeyEvent) {
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
        KeyCode::Enter => {
            let rest = state.lines[row].split_off(column);
            state.lines.insert(row + 1, rest);
            state.row = row + 1;
            state.column = 0;
        }
        KeyCode::Backspace if column == 0 && row > 0 => {
            let line = state.lines.remove(row);
            state.row = row - 1;
            state.column = state.lines[state.row].len();
            state.lines[state.row].push_str(&line);
        }
        KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            kill_multiline_word_before_cursor(state);
        }
        KeyCode::Delete if column == state.lines[row].len() && row + 1 < state.lines.len() => {
            let line = state.lines.remove(row + 1);
            state.lines[row].push_str(&line);
        }
        _ => edit_line(&mut state.lines[row], &mut state.column, key),
    }
}

fn kill_multiline_word_before_cursor(state: &mut TextBuffer) {
    while state.row > 0 && state.column == 0 {
        let line = state.lines.remove(state.row);
        state.row -= 1;
        state.column = state.lines[state.row].len();
        state.lines[state.row].push_str(&line);
    }

    if state.lines.is_empty() || state.column == 0 {
        return;
    }

    edit_line(
        &mut state.lines[state.row],
        &mut state.column,
        KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_values_and_paste_preserve_opaque_text() {
        for value in ["", " \t ", "\r", "\r\n", "é\r中\r\n\n ", "last\n\n"] {
            let original = TextBuffer::from_value(value.to_string());
            assert_eq!(original.lines.join("\n"), value);
            assert_eq!(original.baseline_value(), value);
            assert!(!original.is_dirty());
            let mut pasted = TextBuffer::from_value(String::new());
            pasted.insert_exact(value);
            assert_eq!(pasted.lines, original.lines);
            assert_eq!((pasted.row, pasted.column), (original.row, original.column));
            assert_eq!(pasted.baseline_value(), "");
            assert_eq!(pasted.is_dirty(), !value.is_empty());
        }
    }

    #[test]
    fn returned_text_keeps_initial_baseline_across_retries() {
        let initial = " \ré\r\n\n";
        let mut buffer = TextBuffer::from_value(initial.to_string());
        buffer.insert_exact("draft");
        let returned =
            TextBuffer::from_value_with_baseline("editor\r\n".to_string(), buffer.baseline_value());
        let retry = TextBuffer::from_value_with_baseline(
            "retry\n\n".to_string(),
            returned.baseline_value(),
        );
        assert!(returned.is_dirty());
        assert!(retry.is_dirty());
        assert_eq!(retry.baseline_value(), initial);
        let reverted =
            TextBuffer::from_value_with_baseline(initial.to_string(), retry.baseline_value());
        assert!(!reverted.is_dirty());
    }

    #[test]
    fn exact_insertion_and_editing_clamp_to_unicode_byte_boundaries() {
        let mut buffer = TextBuffer::from_value("é中z".to_string());
        buffer.column = 1;
        buffer.insert_exact("🙂\r\n界");
        assert_eq!(buffer.lines.join("\n"), "🙂\r\n界é中z");
        assert_eq!((buffer.row, buffer.column), (1, "界".len()));
        edit_text_buffer(
            &mut buffer,
            KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE),
        );
        assert_eq!(buffer.lines[1], "界中z");
        buffer.column = 2;
        edit_text_buffer(
            &mut buffer,
            KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
        );
        assert_eq!(buffer.column, "界".len());
        assert!(buffer.lines[buffer.row].is_char_boundary(buffer.column));
        buffer.row = usize::MAX;
        buffer.column = usize::MAX;
        buffer.insert_exact("\n");
        assert_eq!((buffer.row, buffer.column), (2, 0));
    }
}
