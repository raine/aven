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

/// SQL fragment for available task constraints through the time comparison.
///
/// `alias` must be a static identifier such as `t`. The caller supplies the
/// trusted time expression or bound value after this fragment.
pub fn available_task_prefix(alias: &'static str) -> String {
    format!("({alias}.available_at = '' OR {alias}.available_at <= ")
}

/// SQL fragment for available task constraints.
///
/// `alias` must be a static identifier such as `t`. `now_expression` must be a
/// trusted SQL expression supplied by the query implementation.
pub fn available_task_clause(alias: &'static str, now_expression: &str) -> String {
    format!("{}{now_expression})", available_task_prefix(alias))
}

/// SQL fragment for an unresolved task dependency.
///
/// `alias` must be a static identifier such as `t`.
pub fn unresolved_blocker_clause(alias: &'static str) -> String {
    format!(
        "EXISTS (SELECT 1 FROM task_dependencies d
         JOIN tasks blocker ON blocker.workspace_id = d.workspace_id AND blocker.id = d.depends_on_task_id
         WHERE d.workspace_id = {alias}.workspace_id AND d.task_id = {alias}.id
           AND {})",
        open_task_clause("blocker"),
    )
}

/// SQL fragment for a task without an unresolved dependency.
///
/// `alias` must be a static identifier such as `t`.
pub fn ready_dependency_clause(alias: &'static str) -> String {
    format!("NOT {}", unresolved_blocker_clause(alias))
}

/// SQL fragment for overdue task constraints through the date comparison.
///
/// `alias` must be a static identifier such as `t`. The caller supplies the
/// trusted date expression or bound value after this fragment.
pub fn overdue_task_prefix(alias: &'static str) -> String {
    format!(
        "{} AND {alias}.due_on != '' AND {alias}.due_on < ",
        open_task_clause(alias),
    )
}

/// SQL fragment for terminal status constraint: done or canceled.
///
/// `alias` must be a static identifier (e.g., `"t"`, `"blocker"`, `"dependent"`).
pub fn terminal_status_clause(alias: &'static str) -> String {
    format!("{alias}.status IN ('done', 'canceled')")
}
