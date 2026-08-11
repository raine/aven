use unicode_width::UnicodeWidthChar;

pub(crate) fn normalize_pasted_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

pub(crate) fn char_boundary_at_or_before(input: &str, index: usize) -> usize {
    let mut index = index.min(input.len());
    while !input.is_char_boundary(index) {
        index -= 1;
    }
    index
}

pub(crate) fn previous_char_boundary(input: &str, index: usize) -> usize {
    let mut index = char_boundary_at_or_before(input, index).saturating_sub(1);
    while !input.is_char_boundary(index) {
        index -= 1;
    }
    index
}

pub(crate) fn next_char_boundary(input: &str, index: usize) -> usize {
    let mut index = char_boundary_at_or_before(input, index)
        .saturating_add(1)
        .min(input.len());
    while !input.is_char_boundary(index) {
        index += 1;
    }
    index
}

pub(crate) fn previous_word_start(input: &str, index: usize) -> usize {
    let mut index = char_boundary_at_or_before(input, index);
    while index > 0 {
        let previous = previous_char_boundary(input, index);
        if !input[previous..index].chars().all(char::is_whitespace) {
            break;
        }
        index = previous;
    }
    while index > 0 {
        let previous = previous_char_boundary(input, index);
        if input[previous..index].chars().all(char::is_whitespace) {
            break;
        }
        index = previous;
    }
    index
}

pub(crate) fn next_char_is_whitespace(input: &str, index: usize) -> bool {
    input[index..]
        .chars()
        .next()
        .is_some_and(char::is_whitespace)
}

/// Terminal cells one character occupies. Zero-width characters still claim one
/// cell so every character stays addressable by the cursor.
pub(crate) fn char_cells(ch: char) -> usize {
    ch.width().unwrap_or(0).max(1)
}

pub(crate) fn str_cells(text: &str) -> usize {
    text.chars().map(char_cells).sum()
}

pub(crate) fn cell_width_ranges(line: &str, width: usize) -> Vec<(usize, usize)> {
    if line.is_empty() {
        return vec![(0, 0)];
    }
    let width = width.max(1);
    let mut ranges = Vec::new();
    let mut start = 0;
    let mut count = 0;
    for (index, ch) in line.char_indices() {
        let char_width = char_cells(ch);
        if count > 0 && count + char_width > width {
            ranges.push((start, index));
            start = index;
            count = 0;
        }
        count += char_width;
    }
    ranges.push((start, line.len()));
    ranges
}

pub(crate) fn segment_index_at(ranges: &[(usize, usize)], cursor: usize) -> usize {
    ranges
        .iter()
        .position(|(start, end)| cursor < *end || (*start == *end && cursor == *start))
        .unwrap_or_else(|| ranges.len().saturating_sub(1))
}

pub(crate) fn cell_width_segment_index(line: &str, cursor: usize, width: usize) -> usize {
    let cursor = char_boundary_at_or_before(line, cursor);
    segment_index_at(&cell_width_ranges(line, width), cursor)
}

/// Longest prefix of `text` that fits in `width` cells. Characters that would
/// straddle the budget are dropped whole so a wide character never renders as
/// half a cell.
pub(crate) fn take_leading_cells(text: &str, width: usize) -> &str {
    let mut used = 0;
    for (index, ch) in text.char_indices() {
        let char_width = char_cells(ch);
        if used + char_width > width {
            return &text[..index];
        }
        used += char_width;
    }
    text
}

/// Longest suffix of `text` that fits in `width` cells.
pub(crate) fn take_trailing_cells(text: &str, width: usize) -> &str {
    let mut used = 0;
    let mut start = text.len();
    for (index, ch) in text.char_indices().rev() {
        let char_width = char_cells(ch);
        if used + char_width > width {
            break;
        }
        used += char_width;
        start = index;
    }
    &text[start..]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundaries_snap_to_valid_utf8_indices() {
        let input = "aé中";
        assert_eq!(char_boundary_at_or_before(input, 2), 1);
        assert_eq!(previous_char_boundary(input, input.len()), 3);
        assert_eq!(next_char_boundary(input, 1), 3);
    }

    #[test]
    fn previous_word_start_skips_trailing_whitespace() {
        assert_eq!(previous_word_start("one two  ", 9), 4);
    }

    #[test]
    fn normalizes_crlf_and_cr_newlines() {
        assert_eq!(normalize_pasted_newlines("a\r\nb\rc"), "a\nb\nc");
    }

    #[test]
    fn cell_width_ranges_keep_valid_boundaries() {
        let line = "a中b";
        assert_eq!(cell_width_ranges(line, 2), vec![(0, 1), (1, 4), (4, 5)]);
        for (start, end) in cell_width_ranges(line, 2) {
            assert!(line.is_char_boundary(start));
            assert!(line.is_char_boundary(end));
        }
    }

    #[test]
    fn cell_width_ranges_wrap_wide_characters_by_cells() {
        let line = "한글";
        assert_eq!(cell_width_ranges(line, 2), vec![(0, 3), (3, 6)]);
        assert_eq!(cell_width_ranges(line, 4), vec![(0, 6)]);
    }

    #[test]
    fn cell_width_segment_index_matches_end_cursor_behavior() {
        assert_eq!(cell_width_segment_index("abcd", 4, 2), 1);
        assert_eq!(cell_width_segment_index("abcd", 2, 2), 1);
        assert_eq!(cell_width_segment_index("한글", "한글".len(), 2), 1);
        assert_eq!(cell_width_segment_index("한글", 3, 2), 1);
        assert_eq!(cell_width_segment_index("한글", 0, 2), 0);
    }

    #[test]
    fn cell_takes_never_split_wide_characters() {
        assert_eq!(take_leading_cells("한글", 3), "한");
        assert_eq!(take_leading_cells("한글", 4), "한글");
        assert_eq!(take_trailing_cells("한글", 3), "글");
        assert_eq!(take_trailing_cells("한글", 1), "");
        assert_eq!(take_trailing_cells("abc", 2), "bc");
    }
}
