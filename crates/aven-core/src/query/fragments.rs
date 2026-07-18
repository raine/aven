/// SQL fragment for open task constraints: not deleted and not terminal status.
///
/// `alias` must be a static identifier (e.g., `"t"`, `"blocker"`, `"dependent"`).
pub fn open_task_clause(alias: &'static str) -> String {
    format!("{alias}.deleted = 0 AND {alias}.status NOT IN ('done', 'canceled')")
}

/// SQL fragment for terminal status constraint: done or canceled.
///
/// `alias` must be a static identifier (e.g., `"t"`, `"blocker"`, `"dependent"`).
pub fn terminal_status_clause(alias: &'static str) -> String {
    format!("{alias}.status IN ('done', 'canceled')")
}
