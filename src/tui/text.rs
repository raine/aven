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

pub(crate) fn cell_width_ranges(line: &str, width: usize) -> Vec<(usize, usize)> {
    if line.is_empty() {
        return vec![(0, 0)];
    }
    let width = width.max(1);
    let mut ranges = Vec::new();
    let mut start = 0;
    let mut count = 0;
    for (index, ch) in line.char_indices() {
        let char_width = ch.width().unwrap_or(0).max(1);
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

pub(crate) fn char_count_ranges(line: &str, width: usize) -> Vec<(usize, usize)> {
    let width = width.max(1);
    let mut boundaries = line
        .char_indices()
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    boundaries.push(line.len());
    let char_count = boundaries.len().saturating_sub(1);
    if char_count == 0 {
        return vec![(0, 0)];
    }
    (0..char_count)
        .step_by(width)
        .map(|start| {
            let end = start.saturating_add(width).min(char_count);
            (boundaries[start], boundaries[end])
        })
        .collect()
}

pub(crate) fn char_count_segment_index(line: &str, cursor: usize, width: usize) -> usize {
    let width = width.max(1);
    let cursor = char_boundary_at_or_before(line, cursor);
    let cursor_chars = line[..cursor].chars().count();
    let line_chars = line.chars().count();
    if line_chars == 0 {
        return 0;
    }
    if cursor_chars == line_chars {
        line_chars.saturating_sub(1) / width
    } else {
        cursor_chars / width
    }
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
    fn char_count_ranges_keep_valid_boundaries() {
        let line = "aé中b";
        assert_eq!(char_count_ranges(line, 2), vec![(0, 3), (3, 7)]);
        for (start, end) in char_count_ranges(line, 2) {
            assert!(line.is_char_boundary(start));
            assert!(line.is_char_boundary(end));
        }
    }

    #[test]
    fn char_count_segment_index_matches_end_cursor_behavior() {
        assert_eq!(char_count_segment_index("abcd", 4, 2), 1);
        assert_eq!(char_count_segment_index("abcd", 2, 2), 1);
    }
}
