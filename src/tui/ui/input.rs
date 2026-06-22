use std::borrow::Cow;

use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::tui::text::char_boundary_at_or_before;
use crate::tui::theme::{BG_ALT, FG};

pub(in crate::tui::ui) fn input_line(
    prefix: &'static str,
    input: &str,
    cursor: usize,
) -> Line<'static> {
    if prefix.is_empty() {
        return Line::from(input_cursor_spans(input, cursor, InputWidth::Full));
    }
    prefixed_input_line(Span::raw(prefix), input, cursor)
}

pub(in crate::tui::ui) fn prefixed_input_line(
    prefix: Span<'static>,
    input: &str,
    cursor: usize,
) -> Line<'static> {
    let mut spans = vec![prefix];
    spans.extend(input_cursor_spans(input, cursor, InputWidth::Full));
    Line::from(spans)
}

pub(in crate::tui::ui) fn clipped_input_line(
    input: &str,
    cursor: usize,
    width: usize,
) -> Line<'static> {
    Line::from(input_cursor_spans(
        input,
        cursor,
        InputWidth::Clipped(width),
    ))
}

#[derive(Clone, Copy)]
pub(in crate::tui::ui) enum InputWidth {
    Full,
    Clipped(usize),
}

pub(in crate::tui::ui) fn input_cursor_spans(
    input: &str,
    cursor: usize,
    width: InputWidth,
) -> Vec<Span<'static>> {
    let cursor = char_boundary_at_or_before(input, cursor);
    let input_chars = input.chars().count();
    let max_width = match width {
        InputWidth::Full => input_chars.saturating_add(1),
        InputWidth::Clipped(width) => width,
    };
    let Some(cursor_char) = input[cursor..].chars().next() else {
        let before = input
            .chars()
            .skip(input_chars.saturating_sub(max_width.saturating_sub(1)))
            .collect::<String>();
        return vec![Span::raw(before), cursor_cell(" ")];
    };
    let cursor_end = cursor + cursor_char.len_utf8();
    let before = &input[..cursor];
    let after = &input[cursor_end..];
    let after_chars = after.chars().count();
    let value_width = input_chars.saturating_add(1).min(max_width);
    let before_visible = value_width.saturating_sub(1 + after_chars);
    let before = before
        .chars()
        .skip(before.chars().count().saturating_sub(before_visible))
        .collect::<String>();
    vec![
        Span::raw(before),
        cursor_cell(cursor_char.to_string()),
        Span::raw(
            after
                .chars()
                .take(max_width.saturating_sub(1))
                .collect::<String>(),
        ),
    ]
}

pub(in crate::tui::ui) fn cursor_cell(content: impl Into<Cow<'static, str>>) -> Span<'static> {
    Span::styled(content, Style::new().fg(BG_ALT).bg(FG))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_cell_has_correct_style() {
        let span = cursor_cell("a");
        assert_eq!(span.content.as_ref(), "a");
        assert_eq!(span.style.fg, Some(BG_ALT));
        assert_eq!(span.style.bg, Some(FG));
    }

    #[test]
    fn input_line_draws_cursor_as_cell() {
        let line = input_line("", "abc", 1);
        assert_eq!(line.spans[0].content.as_ref(), "a");
        assert_eq!(line.spans[1].content.as_ref(), "b");
        assert_eq!(line.spans[1].style.fg, Some(BG_ALT));
        assert_eq!(line.spans[1].style.bg, Some(FG));
        assert_eq!(line.spans[2].content.as_ref(), "c");
    }

    #[test]
    fn input_cursor_spans_draws_end_cursor_as_blank_cell() {
        let spans = input_cursor_spans("abc", 3, InputWidth::Full);
        assert_eq!(spans[0].content.as_ref(), "abc");
        assert_eq!(spans[1].content.as_ref(), " ");
        assert_eq!(spans[1].style.bg, Some(FG));
    }

    #[test]
    fn clipped_input_line_scrolls_to_cursor_cell() {
        let line = clipped_input_line("abcdef", 5, 4);
        assert_eq!(line.spans[0].content.as_ref(), "cde");
        assert_eq!(line.spans[1].content.as_ref(), "f");
    }

    #[test]
    fn input_cursor_handles_byte_indexed_unicode_cursor() {
        let line = input_line("", "aéz", 3);
        assert_eq!(line.spans[0].content.as_ref(), "aé");
        assert_eq!(line.spans[1].content.as_ref(), "z");
        assert_eq!(line.spans[1].style.bg, Some(FG));
    }

    #[test]
    fn prefixed_input_line_preserves_prefix() {
        let line = prefixed_input_line(Span::raw("/"), "abc", 1);
        assert_eq!(line.spans[0].content.as_ref(), "/");
        assert_eq!(line.spans[1].content.as_ref(), "a");
        assert_eq!(line.spans[2].content.as_ref(), "b");
    }
}
