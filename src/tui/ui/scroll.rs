//! Helpers for scroll clamping and scrollbar rendering.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState};

use crate::tui::theme::{BG_ALT, FG_DIM, FG_MUTED};

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
    area: Rect,
    content_height: usize,
    scroll: u16,
) {
    let visible_rows = area.height as usize;
    if content_height > visible_rows {
        let start = clamp_scroll_start(scroll, content_height, visible_rows);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .style(Style::new().fg(FG_DIM).bg(BG_ALT))
                .thumb_style(Style::new().fg(FG_MUTED)),
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
