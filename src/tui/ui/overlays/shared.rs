pub(super) fn viewport_start_for_cursor(
    cursor_row: usize,
    visible_rows: usize,
    row_count: usize,
    focused: bool,
) -> usize {
    if row_count <= visible_rows {
        return 0;
    }
    if !focused {
        return 0;
    }
    cursor_row
        .saturating_sub(visible_rows / 2)
        .min(row_count.saturating_sub(visible_rows))
}

pub(in crate::tui::ui) fn tail_viewport_start(cursor_row: usize, visible_rows: usize) -> usize {
    cursor_row.saturating_sub(visible_rows.saturating_sub(1))
}

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::tui::theme::{ACCENT, FG_DIM, GREEN, ORANGE};

pub(super) fn section_line(label: &str) -> Line<'static> {
    Line::from(Span::styled(
        label.to_ascii_uppercase(),
        Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
    ))
}

pub(super) fn value_row(
    label_width: usize,
    label: &str,
    value: impl Into<String>,
    value_style: Style,
) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<label_width$}"), Style::new().fg(FG_DIM)),
        Span::styled(value.into(), value_style),
    ])
}

pub(super) fn count_row(label_width: usize, label: &str, value: i64) -> Line<'static> {
    let style = if value > 0 {
        Style::new().fg(ORANGE)
    } else {
        Style::new().fg(GREEN)
    };
    value_row(label_width, label, value.to_string(), style)
}
