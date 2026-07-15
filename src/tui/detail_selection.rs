use std::ops::Range;

use unicode_width::UnicodeWidthChar;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TextCell {
    pub(crate) start: usize,
    pub(crate) end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DetailTextSelection {
    pub(crate) task_id: String,
    pub(crate) terminal_width: u16,
    pub(crate) anchor: TextCell,
    pub(crate) focus: TextCell,
}

impl DetailTextSelection {
    pub(crate) fn new(task_id: String, terminal_width: u16, cell: TextCell) -> Self {
        Self {
            task_id,
            terminal_width,
            anchor: cell,
            focus: cell,
        }
    }

    pub(crate) fn range(&self) -> Range<usize> {
        if self.focus.start < self.anchor.start {
            self.focus.start..self.anchor.end
        } else {
            self.anchor.start..self.focus.end
        }
    }
}

pub(crate) fn text_cell_at_column(text: &str, column: usize) -> Option<Range<usize>> {
    let mut display_column = 0;
    let mut cells: Vec<(usize, usize, Range<usize>)> = Vec::new();

    for (byte, character) in text.char_indices() {
        let width = character.width().unwrap_or(0);
        if width == 0 {
            if let Some((_, end, _)) = cells.last_mut() {
                *end = byte + character.len_utf8();
            }
            continue;
        }
        cells.push((
            byte,
            byte + character.len_utf8(),
            display_column..display_column + width,
        ));
        display_column += width;
    }

    cells
        .into_iter()
        .find(|(_, _, columns)| columns.contains(&column))
        .map(|(start, end, _)| start..end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_cell_maps_ascii_and_wide_characters() {
        let text = "a界b";

        assert_eq!(text_cell_at_column(text, 0), Some(0..1));
        assert_eq!(text_cell_at_column(text, 1), Some(1..4));
        assert_eq!(text_cell_at_column(text, 2), Some(1..4));
        assert_eq!(text_cell_at_column(text, 3), Some(4..5));
        assert_eq!(text_cell_at_column(text, 4), None);
    }

    #[test]
    fn text_cell_keeps_combining_marks_with_their_base() {
        let text = "e\u{301}x";

        assert_eq!(text_cell_at_column(text, 0), Some(0..3));
        assert_eq!(text_cell_at_column(text, 1), Some(3..4));
    }

    #[test]
    fn selection_range_includes_both_endpoint_cells() {
        let first = TextCell { start: 2, end: 3 };
        let last = TextCell { start: 8, end: 10 };

        assert_eq!(
            DetailTextSelection::new("task".to_string(), 80, first).range(),
            2..3
        );
        let mut reverse = DetailTextSelection::new("task".to_string(), 80, last);
        reverse.focus = first;
        assert_eq!(reverse.range(), 2..10);
    }
}
