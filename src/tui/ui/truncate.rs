use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::tui::text::take_leading_display_cells;

pub(super) fn truncate_line_width(
    line: Line<'static>,
    max_width: usize,
    ellipsis_style: Style,
) -> Line<'static> {
    Line::from(truncate_spans_width(line.spans, max_width, ellipsis_style))
}

pub(super) fn truncate_spans_width(
    spans: Vec<Span<'static>>,
    max_width: usize,
    mut ellipsis_style: Style,
) -> Vec<Span<'static>> {
    if spans.iter().map(Span::width).sum::<usize>() <= max_width {
        return spans;
    }
    if max_width == 0 {
        return Vec::new();
    }

    let target_width = max_width - 1;
    let mut used_width = 0;
    let mut truncated = Vec::new();
    for span in spans {
        ellipsis_style = span.style;
        let remaining = target_width.saturating_sub(used_width);
        let content = take_leading_display_cells(&span.content, remaining);
        used_width += content.width();
        if !content.is_empty() {
            truncated.push(Span::styled(content.to_string(), span.style));
        }
        if content.len() < span.content.len() || used_width == target_width {
            break;
        }
    }
    truncated.push(Span::styled("…", ellipsis_style));
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    #[test]
    fn truncates_styled_wide_spans_to_cell_width() {
        let first_style = Style::new().fg(Color::Blue);
        let second_style = Style::new().fg(Color::Green);
        let spans = vec![
            Span::styled("ab", first_style),
            Span::styled("한글", second_style),
        ];

        let truncated = truncate_spans_width(spans, 4, Style::default());

        assert_eq!(Line::from(truncated.clone()).width(), 3);
        assert_eq!(truncated[0].content.as_ref(), "ab");
        assert_eq!(truncated[1].content.as_ref(), "…");
        assert_eq!(truncated[1].style, second_style);
    }
}
