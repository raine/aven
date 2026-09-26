//! Plain TUI text with `backtick` code spans, rendered without the backticks
//! in the shared code style.

use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::tui::text::char_cells;
use crate::tui::theme::CODE;

/// Each character with whether it sits inside a code span. A backtick without
/// a closing partner stays literal text.
fn classify(text: &str) -> Vec<(char, bool)> {
    let mut chars = Vec::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('`') {
        let Some(close) = rest[open + 1..].find('`') else {
            break;
        };
        chars.extend(rest[..open].chars().map(|ch| (ch, false)));
        chars.extend(
            rest[open + 1..open + 1 + close]
                .chars()
                .map(|ch| (ch, true)),
        );
        rest = &rest[open + 1 + close + 1..];
    }
    chars.extend(rest.chars().map(|ch| (ch, false)));
    chars
}

fn spans(chars: &[(char, bool)], base: Style) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut run = String::new();
    let mut run_code = false;
    for &(ch, code) in chars {
        if code != run_code && !run.is_empty() {
            spans.push(styled(std::mem::take(&mut run), run_code, base));
        }
        run_code = code;
        run.push(ch);
    }
    if !run.is_empty() || spans.is_empty() {
        spans.push(styled(run, run_code, base));
    }
    spans
}

fn styled(text: String, code: bool, base: Style) -> Span<'static> {
    Span::styled(text, if code { base.patch(CODE) } else { base })
}

/// One line of `text` with code spans styled.
pub(crate) fn code_spans(text: &str, base: Style) -> Vec<Span<'static>> {
    spans(&classify(text), base)
}

/// Wraps `text` at spaces to `width` cells, splitting only words wider than
/// the line. Code spans lose their backticks, so they count only their
/// visible cells.
pub(crate) fn wrap_with_code(text: &str, base: Style, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let chars = classify(text);
    let mut lines = Vec::new();
    let mut current: Vec<(char, bool)> = Vec::new();
    let mut current_width = 0;
    let mut gap: Option<(char, bool)> = None;
    let mut index = 0;
    while index < chars.len() {
        if chars[index].0.is_whitespace() {
            if !current.is_empty() && gap.is_none() {
                gap = Some((' ', chars[index].1));
            }
            index += 1;
            continue;
        }
        let end = chars[index..]
            .iter()
            .position(|(ch, _)| ch.is_whitespace())
            .map_or(chars.len(), |offset| index + offset);
        let word = &chars[index..end];
        let word_width = word.iter().map(|(ch, _)| char_cells(*ch)).sum::<usize>();
        index = end;
        if !current.is_empty() && current_width + 1 + word_width > width {
            lines.push(Line::from(spans(&std::mem::take(&mut current), base)));
            current_width = 0;
        }
        if let Some(space) = gap.take()
            && !current.is_empty()
        {
            current.push(space);
            current_width += 1;
        }
        for &(ch, code) in word {
            let cells = char_cells(ch);
            if current_width + cells > width && !current.is_empty() {
                lines.push(Line::from(spans(&std::mem::take(&mut current), base)));
                current_width = 0;
            }
            current.push((ch, code));
            current_width += cells;
        }
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(Line::from(spans(&current, base)));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::theme::{BLUE, FG};

    fn texts(lines: &[Line<'_>]) -> Vec<String> {
        lines.iter().map(|line| line.to_string()).collect()
    }

    #[test]
    fn code_spans_drop_backticks_and_use_code_style() {
        let base = Style::new().fg(FG);
        let spans = code_spans("run `aven sync` now", base);
        let contents = spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<Vec<_>>();
        assert_eq!(contents, ["run ", "aven sync", " now"]);
        assert_eq!(spans[0].style, base);
        assert_eq!(spans[1].style.fg, Some(BLUE));
        assert_eq!(spans[1].style.bg, CODE.bg);
    }

    #[test]
    fn unmatched_backtick_stays_literal() {
        assert_eq!(code_spans("a ` b", Style::new())[0].content, "a ` b");
    }

    #[test]
    fn wrapping_counts_visible_cells_and_breaks_inside_code_at_spaces() {
        let lines = wrap_with_code("run `aven sync invite` on it", Style::new(), 12);
        assert_eq!(texts(&lines), ["run aven", "sync invite", "on it"]);
        assert_eq!(lines[1].spans[0].style, CODE);
    }

    #[test]
    fn wrapping_splits_only_words_wider_than_the_line() {
        let lines = wrap_with_code("backup restore or import", Style::new(), 10);
        assert_eq!(texts(&lines), ["backup", "restore or", "import"]);
        let lines = wrap_with_code("abcdefghij", Style::new(), 4);
        assert_eq!(texts(&lines), ["abcd", "efgh", "ij"]);
        assert_eq!(texts(&wrap_with_code("", Style::new(), 4)), [""]);
    }
}
