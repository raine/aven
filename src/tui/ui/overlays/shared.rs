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

pub(super) fn selected_viewport_start(
    visible_indices: &[usize],
    selected: usize,
    viewport_rows: usize,
) -> usize {
    visible_indices
        .iter()
        .position(|index| *index == selected)
        .unwrap_or(0)
        .saturating_sub(viewport_rows.saturating_sub(1))
}

pub(super) fn tail_viewport_start(cursor_row: usize, visible_rows: usize) -> usize {
    cursor_row.saturating_sub(visible_rows.saturating_sub(1))
}
