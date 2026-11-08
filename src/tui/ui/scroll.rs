//! Helpers for scroll clamping and scrollbar rendering.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState};

use crate::tui::theme::Theme;

/// Thumb glyph for list scrollbars: a right-aligned half block, wide enough to
/// read at a glance while staying clear of row text.
pub(in crate::tui::ui) const LIST_SCROLLBAR_THUMB: &str = "▐";
/// Track glyph for list scrollbars: a right-aligned eighth block that lines up
/// with the thumb's right edge.
pub(in crate::tui::ui) const LIST_SCROLLBAR_TRACK: &str = "▕";

/// Builds the right-edge scrollbar shared by selectable lists.
///
/// The scrollbar draws over the rightmost cell of each row, so it clears the
/// dim modifier that blocked or inactive rows apply to their full width.
pub(in crate::tui::ui) fn list_scrollbar(
    thumb: Color,
    track: Color,
    background: Color,
) -> Scrollbar<'static> {
    Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None)
        .thumb_symbol(LIST_SCROLLBAR_THUMB)
        .track_symbol(Some(LIST_SCROLLBAR_TRACK))
        .thumb_style(
            Style::new()
                .fg(thumb)
                .bg(background)
                .remove_modifier(Modifier::DIM),
        )
        .track_style(
            Style::new()
                .fg(track)
                .bg(background)
                .remove_modifier(Modifier::DIM),
        )
}

/// Clamp `scroll` (a raw u16 offset) so it does not exceed the last valid
/// start position, i.e. `content_height - visible_rows`.
pub(crate) fn clamp_scroll_start(scroll: u16, content_height: usize, visible_rows: usize) -> usize {
    let max_start = content_height.saturating_sub(visible_rows);
    (scroll as usize).min(max_start)
}

/// Compute the scrollbar thumb position given the current start offset.
///
/// Returns 0 when the content fits entirely within the viewport (so there
/// is nothing to scroll), matching `checked_div(max_start).unwrap_or(0)`
/// semantics.
pub(crate) fn scrollbar_thumb_position(
    start: usize,
    content_height: usize,
    visible_rows: usize,
) -> usize {
    let max_start = content_height.saturating_sub(visible_rows);
    start
        .saturating_mul(content_height.saturating_sub(1))
        .checked_div(max_start)
        .unwrap_or(0)
}

pub(in crate::tui::ui) fn render_vertical_scrollbar(
    frame: &mut Frame,
    theme: &Theme,
    area: Rect,
    content_height: usize,
    scroll: u16,
) {
    let visible_rows = area.height as usize;
    if content_height > visible_rows {
        let start = clamp_scroll_start(scroll, content_height, visible_rows);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .style(theme.dim().bg(theme.bg_raised))
                .thumb_style(theme.muted()),
            area,
            &mut ScrollbarState::new(content_height)
                .position(scrollbar_thumb_position(
                    start,
                    content_height,
                    visible_rows.max(1),
                ))
                .viewport_content_length(visible_rows),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrollbar_position_reaches_end_at_last_visible_row() {
        assert_eq!(scrollbar_thumb_position(0, 20, 5), 0);
        assert_eq!(scrollbar_thumb_position(15, 20, 5), 19);
        assert_eq!(scrollbar_thumb_position(0, 3, 5), 0);
    }

    #[test]
    fn clamp_scroll_start_stops_at_max_start() {
        assert_eq!(clamp_scroll_start(0, 20, 5), 0);
        assert_eq!(clamp_scroll_start(8, 20, 5), 8);
        assert_eq!(clamp_scroll_start(30, 20, 5), 15);
        assert_eq!(clamp_scroll_start(50, 3, 5), 0);
    }
}
