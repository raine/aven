use std::borrow::Cow;

use ratatui::buffer::Buffer;
use ratatui::layout::Position;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};

use crate::tui::text::{
    char_boundary_at_or_before, char_cells, str_cells, take_leading_cells, take_trailing_cells,
};
use crate::tui::theme::Theme;

/// Render-only caret metadata, removed before a frame reaches the backend.
/// No visible style may use this modifier.
pub(crate) const CARET_MARKER: Modifier = Modifier::RAPID_BLINK;

pub(in crate::tui::ui) fn input_line(
    theme: &Theme,
    prefix: &'static str,
    input: &str,
    cursor: usize,
) -> Line<'static> {
    if prefix.is_empty() {
        return Line::from(input_cursor_spans(theme, input, cursor, InputWidth::Full));
    }
    prefixed_input_line(theme, Span::raw(prefix), input, cursor)
}

pub(in crate::tui::ui) fn prefixed_input_line(
    theme: &Theme,
    prefix: Span<'static>,
    input: &str,
    cursor: usize,
) -> Line<'static> {
    let mut spans = vec![prefix];
    spans.extend(input_cursor_spans(theme, input, cursor, InputWidth::Full));
    Line::from(spans)
}

pub(in crate::tui::ui) fn clipped_input_line(
    theme: &Theme,
    input: &str,
    cursor: usize,
    width: usize,
) -> Line<'static> {
    Line::from(input_cursor_spans(
        theme,
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

/// Builds the spans for a single input row, scrolled so the cursor stays
/// visible. Every budget is counted in terminal cells, not characters, so wide
/// characters (CJK, emoji) neither overflow the field nor push the cursor out of
/// view.
pub(in crate::tui::ui) fn input_cursor_spans(
    theme: &Theme,
    input: &str,
    cursor: usize,
    width: InputWidth,
) -> Vec<Span<'static>> {
    let cursor = char_boundary_at_or_before(input, cursor);
    let cursor_char = input[cursor..].chars().next();
    let cursor_cells = cursor_char.map_or(1, char_cells);
    let max_width = match width {
        InputWidth::Full => str_cells(input).saturating_add(cursor_cells),
        InputWidth::Clipped(width) => width,
    };
    let Some(cursor_char) = cursor_char else {
        let before = take_trailing_cells(input, max_width.saturating_sub(1));
        return vec![Span::raw(before.to_string()), cursor_cell(theme, " ")];
    };
    let after_budget = max_width.saturating_sub(cursor_cells);
    let after = take_leading_cells(&input[cursor + cursor_char.len_utf8()..], after_budget);
    let before = take_trailing_cells(
        &input[..cursor],
        after_budget.saturating_sub(str_cells(after)),
    );
    vec![
        Span::raw(before.to_string()),
        cursor_cell(theme, cursor_char.to_string()),
        Span::raw(after.to_string()),
    ]
}

pub(in crate::tui::ui) fn cursor_cell(
    theme: &Theme,
    content: impl Into<Cow<'static, str>>,
) -> Span<'static> {
    Span::styled(content, theme.cursor().add_modifier(CARET_MARKER))
}

/// Finds the drawn cursor cell in a rendered frame so the caller can park the
/// real terminal cursor there.
///
/// The TUI paints its own cursor instead of using the terminal cursor, which
/// leaves the terminal cursor wherever the last buffer diff happened. Input
/// method editors (Korean, Japanese, Chinese) draw their in-progress
/// composition at the terminal cursor, so without this the preedit text appears
/// somewhere unrelated to the field being typed into.
///
/// Modal dimming invalidates underlay markers even on blank caret cells.
pub(crate) fn text_cursor_position(buffer: &Buffer) -> Option<Position> {
    let index = buffer
        .content()
        .iter()
        .position(|cell| cell.modifier.contains(CARET_MARKER))?;
    let (x, y) = buffer.pos_of(index);
    Some(Position { x, y })
}

pub(super) fn finish_input_frame(buffer: &mut Buffer) -> Option<Position> {
    let position = text_cursor_position(buffer);
    for cell in &mut buffer.content {
        cell.modifier.remove(CARET_MARKER);
    }
    position
}

#[cfg(test)]
mod tests {
    use super::*;

    const THEME: &Theme = &Theme::dark();

    #[test]
    fn input_line_draws_cursor_as_cell() {
        let line = input_line(THEME, "", "abc", 1);
        assert_eq!(line.spans[0].content.as_ref(), "a");
        assert_eq!(line.spans[1].content.as_ref(), "b");
        assert_eq!(line.spans[1].style.fg, THEME.cursor().fg);
        assert_eq!(line.spans[1].style.bg, THEME.cursor().bg);
        assert_eq!(line.spans[2].content.as_ref(), "c");
    }

    #[test]
    fn input_cursor_spans_draws_end_cursor_as_blank_cell() {
        let spans = input_cursor_spans(THEME, "abc", 3, InputWidth::Full);
        assert_eq!(spans[0].content.as_ref(), "abc");
        assert_eq!(spans[1].content.as_ref(), " ");
        assert_eq!(spans[1].style.bg, THEME.cursor().bg);
    }

    #[test]
    fn clipped_input_line_scrolls_to_cursor_cell() {
        let line = clipped_input_line(THEME, "abcdef", 5, 4);
        assert_eq!(line.spans[0].content.as_ref(), "cde");
        assert_eq!(line.spans[1].content.as_ref(), "f");
    }

    #[test]
    fn input_cursor_handles_byte_indexed_unicode_cursor() {
        let line = input_line(THEME, "", "aéz", 3);
        assert_eq!(line.spans[0].content.as_ref(), "aé");
        assert_eq!(line.spans[1].content.as_ref(), "z");
        assert_eq!(line.spans[1].style.bg, THEME.cursor().bg);
    }

    #[test]
    fn clipped_input_line_budgets_wide_characters_by_cells() {
        let line = clipped_input_line(THEME, "한글입력", "한글입력".len(), 6);
        assert_eq!(line.spans[0].content.as_ref(), "입력");
        assert_eq!(line.spans[1].content.as_ref(), " ");
        assert!(line.width() <= 6);
    }

    #[test]
    fn clipped_input_line_keeps_wide_cursor_visible() {
        let line = clipped_input_line(THEME, "한글입력", 3, 4);
        assert_eq!(line.spans[0].content.as_ref(), "");
        assert_eq!(line.spans[1].content.as_ref(), "글");
        assert_eq!(line.spans[2].content.as_ref(), "입");
        assert!(line.width() <= 4);
    }

    #[test]
    fn input_line_keeps_full_width_value_unclipped() {
        let line = input_line(THEME, "", "한글", 3);
        assert_eq!(line.spans[0].content.as_ref(), "한");
        assert_eq!(line.spans[1].content.as_ref(), "글");
        assert_eq!(line.spans[2].content.as_ref(), "");
    }

    #[test]
    fn prefixed_input_line_preserves_prefix() {
        let line = prefixed_input_line(THEME, Span::raw("/"), "abc", 1);
        assert_eq!(line.spans[0].content.as_ref(), "/");
        assert_eq!(line.spans[1].content.as_ref(), "a");
        assert_eq!(line.spans[2].content.as_ref(), "b");
    }
    #[test]
    fn caret_metadata_survives_layout_but_never_reaches_the_backend() {
        use ratatui::layout::Rect;
        use ratatui::widgets::{Paragraph, Widget};
        for theme in [Theme::dark(), Theme::light(), Theme::terminal()] {
            let area = Rect::new(4, 8, 6, 1);
            let mut buffer = Buffer::empty(area);
            assert_eq!(text_cursor_position(&buffer), None);
            Paragraph::new(clipped_input_line(&theme, "한글입력", 3, 6)).render(area, &mut buffer);
            let caret = text_cursor_position(&buffer).unwrap();
            assert_eq!(caret, Position::new(4, 8));
            assert_eq!(buffer[(caret.x, caret.y)].symbol(), "글");
            assert_eq!(finish_input_frame(&mut buffer), Some(caret));
            assert!(
                buffer
                    .content
                    .iter()
                    .all(|cell| !cell.modifier.contains(CARET_MARKER))
            );
            assert_eq!(text_cursor_position(&buffer), None);
        }
    }
}
