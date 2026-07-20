/// SQL fragment for tasks visible through ordinary list-like reports.
///
/// Paused projections and archived projections remain addressable through direct
/// task detail and recurrence history reports.
pub fn ordinary_task_clause(alias: &'static str) -> String {
    format!(
        "NOT EXISTS (
            SELECT 1 FROM recurrence_occurrences ro
            JOIN recurrence_series rs
              ON rs.workspace_id = ro.workspace_id AND rs.id = ro.series_id
            WHERE ro.workspace_id = {alias}.workspace_id AND ro.task_id = {alias}.id
              AND (ro.projection_state = 'archived'
                   OR (ro.projection_state = 'projected' AND rs.state = 'paused'))
        )"
    )
}

/// SQL fragment for visible open task constraints.
///
/// `alias` must be a static identifier such as `t`, `blocker`, or `dependent`.
pub fn open_task_clause(alias: &'static str) -> String {
    format!(
        "{alias}.deleted = 0 AND {alias}.status NOT IN ('done', 'canceled') AND {}",
        ordinary_task_clause(alias)
    )
}

/// SQL fragment for terminal status constraint: done or canceled.
///
/// `alias` must be a static identifier (e.g., `"t"`, `"blocker"`, `"dependent"`).
pub fn terminal_status_clause(alias: &'static str) -> String {
    format!("{alias}.status IN ('done', 'canceled')")
}
