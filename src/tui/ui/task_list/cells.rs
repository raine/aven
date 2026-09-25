use super::super::input::clipped_input_line;
use super::EPIC_MARKER;
use crate::config::TableColumn;
use crate::query::TaskListItem;
use crate::queue::{now_seconds, unix_seconds};
use crate::tui::overlay::TextInputView;
use crate::tui::store::TaskListRenderMode;
use crate::tui::text::truncate_width;
use crate::tui::theme::{self, ACCENT, FG, FG_DIM, FG_MUTED, RED, SELECTED_INACTIVE, YELLOW};
use crate::tui::widgets::{
    age_style, label_cell, priority_icon, status_chip, status_icon_cell, title_cell,
};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

pub(super) const EPIC_CHILD_MARKER: &str = "↳";
pub(super) const DEFERRED_MARKER: &str = "\u{f017}";
pub(super) const TASK_CURSOR_GLYPH: &str = "›";

#[derive(Debug, Clone, Copy)]
pub(super) struct TaskTimeContext {
    pub(super) now_seconds: i64,
    pub(super) render_mode: TaskListRenderMode,
    pub(super) due_order: bool,
    pub(super) show_due: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct TaskRowState {
    pub(super) selected: bool,
    pub(super) focused: bool,
    pub(super) marked: bool,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct TaskListCellLayout<'a> {
    pub(super) widths: &'a [usize; 9],
    pub(super) state_column: Option<TableColumn>,
    pub(super) compact_status: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct EpicSelectionContext<'a> {
    pub(super) selected_epic_id: Option<&'a str>,
    pub(super) selected_parent_id: Option<&'a str>,
}

impl<'a> EpicSelectionContext<'a> {
    pub(super) fn from_selected(item: Option<&'a TaskListItem>) -> Self {
        Self {
            selected_epic_id: item
                .filter(|item| item.task.is_epic)
                .map(|item| item.task.id.as_str()),
            selected_parent_id: item
                .and_then(|item| item.epic_parent.as_ref())
                .map(|parent| parent.task_id.as_str()),
        }
    }

    pub(super) fn highlights_parent(self, item: &TaskListItem) -> bool {
        self.selected_parent_id == Some(item.task.id.as_str())
    }
}

#[cfg(test)]
pub(super) fn build_task_row_cells(
    item: &TaskListItem,
    time_context: TaskTimeContext,
    inline_title_editor: Option<&TextInputView>,
    column_widths: &[usize; 9],
    state: TaskRowState,
    epic_selection: EpicSelectionContext<'_>,
) -> Vec<Line<'static>> {
    build_task_row_cells_for_columns(
        item,
        time_context,
        inline_title_editor,
        TaskListCellLayout {
            widths: column_widths,
            state_column: Some(TableColumn::Ref),
            compact_status: false,
        },
        state,
        epic_selection,
    )
}

pub(super) fn status_cell(status: &str, compact_status: bool) -> Line<'static> {
    if compact_status {
        status_icon_cell(status)
    } else {
        status_chip(status)
    }
}

pub(super) fn build_task_row_cells_for_columns(
    item: &TaskListItem,
    time_context: TaskTimeContext,
    inline_title_editor: Option<&TextInputView>,
    cell_layout: TaskListCellLayout<'_>,
    state: TaskRowState,
    epic_selection: EpicSelectionContext<'_>,
) -> Vec<Line<'static>> {
    let column_widths = cell_layout.widths;
    let state_column = cell_layout.state_column;
    let time = task_time_cell(
        item,
        time_context.now_seconds,
        time_context.render_mode,
        time_context.due_order,
        time_context.show_due,
    );
    let state_prefix_width = if state_column == Some(TableColumn::Title) {
        spans_width(&task_state_prefix(
            state.selected,
            state.focused,
            state.marked,
        ))
    } else {
        0
    };
    let title_width = column_widths[TableColumn::Title as usize].saturating_sub(state_prefix_width);
    let title = inline_title_editor
        .map(|editor| inline_title_edit_cell(editor, title_width))
        .unwrap_or_else(|| title_cell(item, title_width));
    let labels = label_cell(&item.labels, column_widths[TableColumn::Labels as usize]);
    TableColumn::ALL
        .into_iter()
        .map(|column| {
            let cell = match column {
                TableColumn::Ref if state_column == Some(TableColumn::Ref) => {
                    task_ref_cell(item, column_widths[TableColumn::Ref as usize], state)
                }
                TableColumn::Ref => {
                    task_ref_content_cell(item, column_widths[TableColumn::Ref as usize])
                }
                TableColumn::Title => title.clone(),
                TableColumn::Labels => labels.clone(),
                TableColumn::Metadata => metadata_cell(
                    item,
                    epic_selection,
                    time_context.render_mode == TaskListRenderMode::Flat
                        && is_deferred(item, time_context.now_seconds),
                ),
                TableColumn::Project => {
                    project_cell(item, column_widths[TableColumn::Project as usize])
                }
                TableColumn::Status => {
                    status_cell(item.task.status.as_str(), cell_layout.compact_status)
                }
                TableColumn::Priority => Line::from(Span::styled(
                    priority_icon(item.task.priority.as_str()),
                    theme::priority_style(item.task.priority.as_str()).add_modifier(Modifier::BOLD),
                )),
                TableColumn::Time => time.clone(),
                TableColumn::Due => due_cell(item, time_context.now_seconds),
            };
            state_prefixed_cell(column, state_column, cell, state)
        })
        .collect()
}

/// The task's own deadline, independent of view, ordering, and the contextual time column.
fn due_cell(item: &TaskListItem, now_seconds: i64) -> Line<'static> {
    let due_on = item.task.due_on.as_deref().unwrap_or("");
    let Some(label) = crate::tui::time::compact_due_label(due_on, now_seconds) else {
        return Line::default();
    };
    let due_state = crate::tui::time::due_state_at(due_on, now_seconds);
    Line::from(Span::styled(label, due_label_style(item, due_state)))
}

fn due_label_style(item: &TaskListItem, due_state: crate::due::DueState) -> Style {
    let color = if !item.task.status.is_open() {
        FG_DIM
    } else {
        match due_state {
            crate::due::DueState::Overdue(_) => RED,
            crate::due::DueState::Today => YELLOW,
            crate::due::DueState::Future(_) => ACCENT,
            crate::due::DueState::None => FG_DIM,
        }
    };
    Style::new().fg(color).add_modifier(Modifier::BOLD)
}

pub(super) fn is_deferred(item: &TaskListItem, now_seconds: i64) -> bool {
    item.task
        .available_at
        .as_deref()
        .and_then(unix_seconds)
        .is_some_and(|available_at| available_at > now_seconds)
}

pub(super) fn task_time_cell(
    item: &TaskListItem,
    now_seconds: i64,
    render_mode: TaskListRenderMode,
    due_order: bool,
    show_due: bool,
) -> Line<'static> {
    let due_state =
        crate::tui::time::due_state_at(item.task.due_on.as_deref().unwrap_or(""), now_seconds);
    if show_due
        && render_mode != TaskListRenderMode::Upcoming
        && (due_order || item.task.status.is_open() && due_state.needs_action())
        && let Some(label) = crate::tui::time::compact_due_label(
            item.task.due_on.as_deref().unwrap_or(""),
            now_seconds,
        )
    {
        return Line::from(Span::styled(label, due_label_style(item, due_state)));
    }
    match render_mode {
        TaskListRenderMode::Upcoming => Line::from(Span::styled(
            crate::tui::time::available_in_label(
                item.task.available_at.as_deref().unwrap_or(""),
                now_seconds,
            )
            .unwrap_or_default(),
            Style::new().fg(ACCENT),
        )),
        TaskListRenderMode::Queue => {
            let style_input = if item.queue.band == crate::queue::QueueBand::Available {
                item.task.available_at.as_deref().unwrap_or("")
            } else {
                &item.task.queue_activity_at
            };
            Line::from(Span::styled(
                item.queue
                    .idle_seconds
                    .map(crate::tui::time::compact_duration)
                    .unwrap_or_default(),
                age_style(style_input, now_seconds),
            ))
        }
        TaskListRenderMode::Flat if is_deferred(item, now_seconds) => Line::from(Span::styled(
            crate::tui::time::available_in_label(
                item.task.available_at.as_deref().unwrap_or(""),
                now_seconds,
            )
            .unwrap_or_default(),
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        )),
        _ => Line::from(Span::styled(
            task_seconds_since(&item.task.created_at, now_seconds)
                .map(compact_age)
                .unwrap_or_default(),
            age_style(&item.task.created_at, now_seconds),
        )),
    }
}

pub(super) fn epic_summary_candidate(
    rollup: &crate::query::EpicRollup,
    compact: bool,
    show_done_label: bool,
    show_canceled: bool,
) -> Line<'static> {
    let mut spans = Vec::new();
    if rollup.open == 0 {
        spans.push(Span::styled(
            if compact { "res" } else { "resolved" },
            Style::new().fg(ACCENT),
        ));
    } else if rollup.overdue > 0 || rollup.blocked > 0 {
        if rollup.overdue > 0 {
            spans.push(Span::styled(
                format!("!{}", rollup.overdue),
                Style::new().fg(RED),
            ));
        }
        if rollup.blocked > 0 {
            if !spans.is_empty() {
                spans.push(Span::raw(" "));
            }
            spans.push(Span::styled(
                format!("←{}", rollup.blocked),
                Style::new().fg(YELLOW),
            ));
        }
    } else {
        spans.push(Span::styled(
            if compact {
                format!("r{}", rollup.ready)
            } else {
                format!("{} ready", rollup.ready)
            },
            Style::new().fg(if rollup.ready == 0 { YELLOW } else { ACCENT }),
        ));
    }

    spans.push(Span::styled(" · ", Style::new().fg(FG_DIM)));
    spans.push(Span::styled(
        if compact {
            format!("✓{}/{}", rollup.done, rollup.total)
        } else if show_done_label {
            format!("{}/{} done", rollup.done, rollup.total)
        } else {
            format!("{}/{}", rollup.done, rollup.total)
        },
        Style::new().fg(FG),
    ));
    if show_canceled && rollup.canceled > 0 {
        spans.push(Span::styled(
            format!(" ×{}", rollup.canceled),
            Style::new().fg(RED),
        ));
    }
    Line::from(spans)
}

pub(super) fn epic_summary_cell(
    rollup: &crate::query::EpicRollup,
    max_width: usize,
) -> Line<'static> {
    if rollup.total == 0 {
        return Line::from(Span::styled("-", Style::new().fg(FG_MUTED)));
    }

    let candidates = [
        epic_summary_candidate(rollup, false, true, true),
        epic_summary_candidate(rollup, true, false, true),
        epic_summary_candidate(rollup, false, true, false),
        epic_summary_candidate(rollup, true, false, false),
    ];
    candidates
        .into_iter()
        .find(|line| line.width() <= max_width)
        .unwrap_or_else(|| epic_summary_candidate(rollup, true, false, false))
}

pub(super) fn epic_activity_cell(
    item: &TaskListItem,
    now_seconds: i64,
    due_order: bool,
    show_due: bool,
) -> Line<'static> {
    if due_order && show_due {
        return task_time_cell(item, now_seconds, TaskListRenderMode::Epics, true, true);
    }
    let activity_at = item
        .epic_rollup
        .as_ref()
        .map(|rollup| rollup.latest_activity_at.as_str())
        .unwrap_or(&item.task.updated_at);
    Line::from(Span::styled(
        task_seconds_since(activity_at, now_seconds)
            .map(compact_age)
            .unwrap_or_default(),
        age_style(activity_at, now_seconds),
    ))
}

pub(super) fn build_epic_parent_row_cells_for_columns(
    item: &TaskListItem,
    time: TaskTimeContext,
    expanded_epic_ids: &std::collections::BTreeSet<crate::ids::TaskId>,
    inline_title_editor: Option<&TextInputView>,
    cell_layout: TaskListCellLayout<'_>,
    state: TaskRowState,
    epic_selection: EpicSelectionContext<'_>,
) -> Vec<Line<'static>> {
    let column_widths = cell_layout.widths;
    let state_column = cell_layout.state_column;
    let state_prefix_width = if state_column == Some(TableColumn::Title) {
        spans_width(&task_state_prefix(
            state.selected,
            state.focused,
            state.marked,
        ))
    } else {
        0
    };
    let title_width = column_widths[TableColumn::Title as usize].saturating_sub(state_prefix_width);
    let title = inline_title_editor
        .map(|editor| inline_title_edit_cell(editor, title_width))
        .unwrap_or_else(|| title_cell(item, title_width));
    let expanded = expanded_epic_ids.contains(&item.task.id);
    let mut ref_spans = if state_column == Some(TableColumn::Ref) {
        task_state_prefix(state.selected, state.focused, state.marked)
    } else {
        Vec::new()
    };
    ref_spans.extend([
        Span::styled(if expanded { "▾" } else { "▸" }, Style::new().fg(ACCENT)),
        Span::raw(" "),
    ]);
    let prefix_width = spans_width(&ref_spans);
    let display_ref = truncate_width(
        &item.display_ref,
        column_widths[TableColumn::Ref as usize].saturating_sub(prefix_width),
    );
    ref_spans.extend(task_ref_spans(item, display_ref));
    let summary = item
        .epic_rollup
        .as_ref()
        .map(|rollup| epic_summary_cell(rollup, column_widths[TableColumn::Labels as usize]))
        .unwrap_or_default();
    TableColumn::ALL
        .into_iter()
        .map(|column| {
            let cell = match column {
                TableColumn::Ref => Line::from(ref_spans.clone()),
                TableColumn::Title => title.clone(),
                TableColumn::Labels => summary.clone(),
                TableColumn::Metadata => metadata_cell(item, epic_selection, false),
                TableColumn::Project => {
                    project_cell(item, column_widths[TableColumn::Project as usize])
                }
                TableColumn::Status => {
                    status_cell(item.task.status.as_str(), cell_layout.compact_status)
                }
                TableColumn::Priority => Line::from(Span::styled(
                    priority_icon(item.task.priority.as_str()),
                    theme::priority_style(item.task.priority.as_str()).add_modifier(Modifier::BOLD),
                )),
                TableColumn::Time => {
                    epic_activity_cell(item, time.now_seconds, time.due_order, time.show_due)
                }
                TableColumn::Due => due_cell(item, time.now_seconds),
            };
            state_prefixed_cell(column, state_column, cell, state)
        })
        .collect()
}

#[cfg(test)]
pub(super) fn build_epic_child_row_cells(
    item: &TaskListItem,
    last: bool,
    inline_title_editor: Option<&TextInputView>,
    time: TaskTimeContext,
    column_widths: &[usize; 9],
    state: TaskRowState,
    epic_selection: EpicSelectionContext<'_>,
) -> Vec<Line<'static>> {
    build_epic_child_row_cells_for_columns(
        item,
        last,
        inline_title_editor,
        time,
        TaskListCellLayout {
            widths: column_widths,
            state_column: Some(TableColumn::Ref),
            compact_status: false,
        },
        state,
        epic_selection,
    )
}

pub(super) fn build_epic_child_row_cells_for_columns(
    item: &TaskListItem,
    last: bool,
    inline_title_editor: Option<&TextInputView>,
    time: TaskTimeContext,
    cell_layout: TaskListCellLayout<'_>,
    state: TaskRowState,
    epic_selection: EpicSelectionContext<'_>,
) -> Vec<Line<'static>> {
    let column_widths = cell_layout.widths;
    let state_column = cell_layout.state_column;
    let state_prefix_width = if state_column == Some(TableColumn::Title) {
        spans_width(&task_state_prefix(
            state.selected,
            state.focused,
            state.marked,
        ))
    } else {
        0
    };
    let title_width = column_widths[TableColumn::Title as usize].saturating_sub(state_prefix_width);
    let branch = if last { "└─" } else { "├─" };
    let mut ref_spans = if state_column == Some(TableColumn::Ref) {
        task_state_prefix(state.selected, state.focused, state.marked)
    } else {
        Vec::new()
    };
    ref_spans.extend([
        Span::styled(branch, Style::new().fg(FG_DIM)),
        Span::raw(" "),
    ]);
    let prefix_width = spans_width(&ref_spans);
    let display_ref = truncate_width(
        &item.display_ref,
        column_widths[TableColumn::Ref as usize].saturating_sub(prefix_width + 1),
    );
    ref_spans.extend([
        Span::styled(display_ref, Style::new().fg(FG_MUTED)),
        Span::raw(" "),
    ]);
    let ref_line = Line::from(ref_spans);
    TableColumn::ALL
        .into_iter()
        .map(|column| {
            let cell = match column {
                TableColumn::Ref => ref_line.clone(),
                TableColumn::Title => inline_title_editor
                    .map(|editor| inline_title_edit_cell(editor, title_width))
                    .unwrap_or_else(|| title_cell(item, title_width)),
                TableColumn::Labels => Line::default(),
                TableColumn::Metadata => metadata_cell(item, epic_selection, false),
                TableColumn::Project => {
                    project_cell(item, column_widths[TableColumn::Project as usize])
                }
                TableColumn::Status => {
                    status_cell(item.task.status.as_str(), cell_layout.compact_status)
                }
                TableColumn::Priority => Line::from(Span::styled(
                    priority_icon(item.task.priority.as_str()),
                    theme::priority_style(item.task.priority.as_str()).add_modifier(Modifier::BOLD),
                )),
                TableColumn::Time => {
                    if time.due_order && time.show_due {
                        task_time_cell(item, time.now_seconds, time.render_mode, true, true)
                    } else {
                        Line::from(Span::styled(
                            task_seconds_since(&item.task.updated_at, time.now_seconds)
                                .map(compact_age)
                                .unwrap_or_default(),
                            age_style(&item.task.updated_at, time.now_seconds),
                        ))
                    }
                }
                TableColumn::Due => due_cell(item, time.now_seconds),
            };
            state_prefixed_cell(column, state_column, cell, state)
        })
        .collect()
}

pub(super) fn blank_task_row_cells() -> Vec<Line<'static>> {
    TableColumn::ALL.map(|_| Line::from("")).to_vec()
}

pub(super) fn metadata_cell(
    item: &TaskListItem,
    epic_selection: EpicSelectionContext<'_>,
    show_deferred: bool,
) -> Line<'static> {
    let mut spans = Vec::new();
    if show_deferred {
        spans.push(Span::styled(
            DEFERRED_MARKER,
            Style::new().fg(ACCENT).remove_modifier(Modifier::BOLD),
        ));
    }
    if item.task.status.is_open()
        && crate::tui::time::due_state_at(item.task.due_on.as_deref().unwrap_or(""), now_seconds())
            .needs_action()
    {
        if !spans.is_empty() {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(
            "!",
            Style::new().fg(RED).add_modifier(Modifier::BOLD),
        ));
    }
    let is_selected_epic_child = item
        .epic_parent
        .as_ref()
        .is_some_and(|parent| Some(parent.task_id.as_str()) == epic_selection.selected_epic_id);
    if item.task.is_epic {
        if !spans.is_empty() {
            spans.push(Span::raw(" "));
        }
        let highlighted = epic_selection.highlights_parent(item);
        let style = Style::new()
            .fg(if highlighted { ACCENT } else { YELLOW })
            .remove_modifier(Modifier::BOLD);
        spans.push(Span::styled(EPIC_MARKER, style));
    } else if is_selected_epic_child {
        if !spans.is_empty() {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(
            EPIC_CHILD_MARKER,
            Style::new().fg(ACCENT).remove_modifier(Modifier::BOLD),
        ));
    }
    if item.task.deleted {
        if !spans.is_empty() {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(
            "×",
            Style::new().fg(RED).add_modifier(Modifier::BOLD),
        ));
    }
    if item.unresolved_blocker_count > 0 {
        if !spans.is_empty() {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(
            format!("←{}", item.unresolved_blocker_count),
            Style::new().fg(FG_MUTED).remove_modifier(Modifier::BOLD),
        ));
    }
    if item.dependent_count > 0 {
        if !spans.is_empty() {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(
            format!("→{}", item.dependent_count),
            Style::new().fg(FG_MUTED).remove_modifier(Modifier::BOLD),
        ));
    }
    if item.has_notes {
        if !spans.is_empty() {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(
            "✎",
            Style::new().fg(FG_MUTED).remove_modifier(Modifier::BOLD),
        ));
    }
    Line::from(spans)
}

pub(super) fn inline_title_edit_cell(editor: &TextInputView, max_width: usize) -> Line<'static> {
    clipped_input_line(&editor.input, editor.cursor, max_width.saturating_sub(1))
}

pub(super) fn task_state_prefix(selected: bool, focused: bool, marked: bool) -> Vec<Span<'static>> {
    let cursor_style = if !selected {
        Style::new()
    } else if focused {
        Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
        SELECTED_INACTIVE
    };
    vec![
        Span::styled(if selected { TASK_CURSOR_GLYPH } else { " " }, cursor_style),
        Span::styled(if marked { "●" } else { " " }, Style::new().fg(YELLOW)),
        Span::raw(" "),
    ]
}

pub(super) fn spans_width(spans: &[Span<'_>]) -> usize {
    spans
        .iter()
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
        .sum()
}

pub(super) fn task_ref_spans(item: &TaskListItem, display_ref: String) -> Vec<Span<'static>> {
    if let Some((project, suffix)) = display_ref.split_once('-') {
        vec![
            Span::styled(
                project.to_string(),
                Style::new().fg(theme::project_color(&item.task.project_key)),
            ),
            Span::styled("-", Style::new().fg(FG_DIM)),
            Span::styled(suffix.to_string(), Style::new().fg(FG_MUTED)),
        ]
    } else {
        vec![Span::styled(display_ref, Style::new().fg(FG_MUTED))]
    }
}

pub(super) fn task_ref_content_cell(item: &TaskListItem, max_width: usize) -> Line<'static> {
    let display_ref = truncate_width(&item.display_ref, max_width);
    Line::from(task_ref_spans(item, display_ref))
}

pub(super) fn task_ref_cell(
    item: &TaskListItem,
    max_width: usize,
    state: TaskRowState,
) -> Line<'static> {
    let mut spans = task_state_prefix(state.selected, state.focused, state.marked);
    let display_ref = truncate_width(
        &item.display_ref,
        max_width.saturating_sub(spans_width(&spans)),
    );
    spans.extend(task_ref_spans(item, display_ref));
    Line::from(spans)
}

pub(super) fn state_prefixed_cell(
    column: TableColumn,
    state_column: Option<TableColumn>,
    cell: Line<'static>,
    state: TaskRowState,
) -> Line<'static> {
    if state_column != Some(column) || column == TableColumn::Ref {
        return cell;
    }
    let mut spans = task_state_prefix(state.selected, state.focused, state.marked);
    spans.extend(cell.spans);
    Line::from(spans)
}

pub(super) fn task_seconds_since(value: &str, now_seconds: i64) -> Option<i64> {
    unix_seconds(value).map(|seconds| now_seconds.saturating_sub(seconds).max(0))
}

pub(super) fn compact_age(age_seconds: i64) -> String {
    let minutes = age_seconds / 60;
    if minutes < 60 {
        return format!("{}m", minutes.max(0));
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours}h");
    }
    let days = hours / 24;
    if days < 14 {
        return format!("{days}d");
    }
    let weeks = days / 7;
    if weeks < 13 {
        return format!("{weeks}w");
    }
    format!("{}mo", days / 30)
}

pub(super) fn project_cell(item: &TaskListItem, max_width: usize) -> Line<'static> {
    let project = truncate_width(&item.task.project_key, max_width.saturating_sub(1));
    Line::from(vec![
        Span::styled(
            project,
            Style::new().fg(theme::project_color(&item.task.project_key)),
        ),
        Span::raw(" "),
    ])
}

#[cfg(test)]
mod tests {
    use super::super::sizing::metadata_column_width_from_task_refs;
    use super::super::table::row_style;
    use super::super::tests::*;
    use super::*;
    use crate::tui::theme::{RELATED, SELECTED, SELECTED_INACTIVE};
    use chrono::TimeZone;
    use ratatui::text::Line;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn queue_row_time_uses_queue_idle_duration() {
        let mut item = task_list_item("queued");
        item.task.created_at = "0".to_string();
        item.task.queue_activity_at = (9 * 86_400).to_string();
        item.queue.idle_seconds = Some(86_400);

        let cells = build_task_row_cells(
            &item,
            TaskTimeContext {
                now_seconds: 10 * 86_400,
                render_mode: TaskListRenderMode::Queue,
                due_order: false,
                show_due: true,
            },
            None,
            &[12, 40, 12, 6, 9, 10, 3, 5, 6],
            TaskRowState {
                selected: false,
                focused: false,
                marked: false,
            },
            EpicSelectionContext::default(),
        );

        assert_eq!(cells[7].to_string(), "1d");
    }

    #[test]
    fn flat_row_marks_deferred_task_and_shows_availability_time() {
        let mut item = task_list_item("deferred");
        item.task.available_at = Some("200".to_string());

        let cells = build_task_row_cells(
            &item,
            TaskTimeContext {
                now_seconds: 100,
                render_mode: TaskListRenderMode::Flat,
                due_order: false,
                show_due: true,
            },
            None,
            &[12, 40, 12, 6, 9, 10, 3, 5, 6],
            TaskRowState {
                selected: false,
                focused: false,
                marked: false,
            },
            EpicSelectionContext::default(),
        );

        assert_eq!(cells[3].to_string(), DEFERRED_MARKER);
        assert_eq!(cells[7].to_string(), "in1m");
        assert_eq!(cells[7].spans[0].style.fg, Some(ACCENT));
    }

    #[test]
    fn project_cell_truncates_with_status_spacing() {
        let mut item = task_list_item("Title");
        item.task.project_key = "very-long-project-name".to_string();

        let rendered = project_cell(&item, 10).to_string();

        assert_eq!(rendered, "very-lon… ");
    }

    #[test]
    fn compact_status_column_applies_to_epic_child_rows() {
        let item = task_list_item("child");
        let widths = [14, 40, 1, 6, 9, 2, 3, 5, 6];

        let cells = build_epic_child_row_cells_for_columns(
            &item,
            false,
            None,
            TaskTimeContext {
                now_seconds: 0,
                render_mode: TaskListRenderMode::Epics,
                due_order: false,
                show_due: true,
            },
            TaskListCellLayout {
                widths: &widths,
                state_column: Some(TableColumn::Ref),
                compact_status: true,
            },
            TaskRowState {
                selected: false,
                focused: false,
                marked: false,
            },
            EpicSelectionContext::default(),
        );

        assert_eq!(cells[TableColumn::Status as usize].to_string(), "□");
    }

    #[test]
    fn due_cell_colors_deadlines_by_urgency_and_dims_closed_tasks() {
        let now = chrono::Utc
            .with_ymd_and_hms(2026, 7, 16, 12, 0, 0)
            .single()
            .unwrap()
            .timestamp();
        let mut item = task_list_item("deadline");
        for (due_on, label, color) in [
            ("2026-07-13", "3d!", RED),
            ("2026-07-16", "today", YELLOW),
            ("2026-07-24", "Jul24", ACCENT),
        ] {
            item.task.due_on = Some(due_on.to_string());
            let cell = due_cell(&item, now);
            assert_eq!(cell.to_string(), label);
            assert_eq!(cell.spans[0].style.fg, Some(color), "{due_on}");
        }

        item.task.status = crate::choices::TaskStatus::Done;
        let cell = due_cell(&item, now);
        assert_eq!(cell.to_string(), "Jul24");
        assert_eq!(cell.spans[0].style.fg, Some(FG_DIM));

        item.task.due_on = None;
        assert_eq!(due_cell(&item, now).to_string(), "");
    }

    #[test]
    fn task_state_prefix_distinguishes_cursor_and_mark_states() {
        let ordinary = Line::from(task_state_prefix(false, true, false));
        let selected = Line::from(task_state_prefix(true, true, false));
        let marked = Line::from(task_state_prefix(false, true, true));
        let combined = Line::from(task_state_prefix(true, true, true));
        let inactive = task_state_prefix(true, false, false);

        assert_eq!(ordinary.to_string(), "   ");
        assert_eq!(selected.to_string(), "›  ");
        assert_eq!(marked.to_string(), " ● ");
        assert_eq!(combined.to_string(), "›● ");
        assert_eq!(UnicodeWidthStr::width(TASK_CURSOR_GLYPH), 1);
        assert_eq!(selected.spans[0].style.fg, Some(ACCENT));
        assert!(
            selected.spans[0]
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert_eq!(inactive[0].style, SELECTED_INACTIVE);
        assert_eq!(combined.spans[1].style.fg, Some(YELLOW));
        assert_eq!(row_style(true, true, true, false, false), SELECTED);
    }

    #[test]
    fn selected_and_unselected_refs_start_at_the_same_cell() {
        let item = task_list_item("aligned");
        let selected = task_ref_cell(
            &item,
            11,
            TaskRowState {
                selected: true,
                focused: true,
                marked: false,
            },
        );
        let ordinary = task_ref_cell(
            &item,
            11,
            TaskRowState {
                selected: false,
                focused: true,
                marked: false,
            },
        );

        assert_eq!(selected.to_string(), "›  APP-1");
        assert_eq!(ordinary.to_string(), "   APP-1");
        assert_eq!(selected.spans[3].content, ordinary.spans[3].content);
        assert_eq!(
            spans_width(&selected.spans[..3]),
            spans_width(&ordinary.spans[..3])
        );
    }

    #[test]
    fn inline_title_editor_clips_to_cursor_cell() {
        let editor = TextInputView {
            kind: TextInputKind::EditTitle,
            title: "Edit title".to_string(),
            prompt: String::new(),
            input: "abcdef".to_string(),
            cursor: 5,
        };

        let rendered = inline_title_edit_cell(&editor, 5).to_string();

        assert_eq!(rendered, "cdef");
    }

    #[test]
    fn metadata_cell_shows_note_marker() {
        let mut item = task_list_item("documented");
        item.task.description = "details".to_string();
        item.notes = vec![
            crate::query::TaskNote {
                id: "note-1".to_string(),
                body: "one".to_string(),
                created_at: "001".to_string(),
            },
            crate::query::TaskNote {
                id: "note-2".to_string(),
                body: "two".to_string(),
                created_at: "002".to_string(),
            },
        ];
        item.has_notes = true;

        assert_eq!(
            metadata_cell(&item, EpicSelectionContext::default(), false).to_string(),
            "✎"
        );
    }

    #[test]
    fn metadata_cell_marks_epics() {
        let mut item = task_list_item("epic");
        item.task.is_epic = true;

        let line = metadata_cell(&item, EpicSelectionContext::default(), false);

        assert_eq!(line.to_string(), EPIC_MARKER);
        assert_eq!(line.spans[0].style.fg, Some(YELLOW));
    }

    #[test]
    fn metadata_cell_highlights_parent_epic_of_selected_child() {
        let parent_id = crate::test_support::task_id("epic-1");
        let mut parent = task_list_item("epic");
        parent.task.id = parent_id.clone();
        parent.task.is_epic = true;
        let mut child = task_list_item("child");
        child.epic_parent = Some(crate::query::TaskDependencyLink {
            project_key: "app".to_string(),
            task_id: parent_id,
            display_ref: "APP-EPIC".to_string(),
            title: "Parent epic".to_string(),
            status: "todo".to_string(),
            priority: "none".to_string(),
            unresolved: true,
        });

        let epic_selection = EpicSelectionContext::from_selected(Some(&child));
        let line = metadata_cell(&parent, epic_selection, false);

        assert!(epic_selection.highlights_parent(&parent));
        assert_eq!(line.to_string(), EPIC_MARKER);
        assert_eq!(line.spans[0].style.fg, Some(ACCENT));
        assert!(line.spans[0].style.sub_modifier.contains(Modifier::BOLD));
        assert_eq!(row_style(false, true, false, true, false), RELATED);
        assert_eq!(row_style(true, true, false, true, false), SELECTED);
    }

    #[test]
    fn metadata_cell_marks_children_of_selected_epic() {
        let mut item = task_list_item("child");
        item.epic_parent = Some(crate::query::TaskDependencyLink {
            project_key: "app".to_string(),
            task_id: crate::test_support::task_id("epic-1"),
            display_ref: "APP-EPIC".to_string(),
            title: "Selected epic".to_string(),
            status: "todo".to_string(),
            priority: "none".to_string(),
            unresolved: true,
        });

        let selected_epic = crate::test_support::task_id("epic-1");
        let epic_selection = EpicSelectionContext {
            selected_epic_id: Some(selected_epic.as_str()),
            ..EpicSelectionContext::default()
        };
        let line = metadata_cell(&item, epic_selection, false);

        assert_eq!(line.to_string(), EPIC_CHILD_MARKER);
        assert_eq!(line.spans[0].style.fg, Some(ACCENT));
        assert_eq!(
            metadata_cell(
                &item,
                EpicSelectionContext {
                    selected_epic_id: Some("other-epic"),
                    ..EpicSelectionContext::default()
                },
                false,
            )
            .to_string(),
            ""
        );
        assert_eq!(
            metadata_column_width_from_task_refs(&[&item], epic_selection, false),
            3
        );
    }

    #[test]
    fn metadata_cell_shows_dependency_counts() {
        let mut item = task_list_item("blocked");
        item.unresolved_blocker_count = 2;
        item.dependent_count = 1;

        assert_eq!(
            metadata_cell(&item, EpicSelectionContext::default(), false).to_string(),
            "←2 →1"
        );
    }

    #[test]
    fn metadata_cell_ignores_description_without_notes() {
        let mut item = task_list_item("plain");
        item.task.description = "details".to_string();

        assert_eq!(
            metadata_cell(&item, EpicSelectionContext::default(), false).to_string(),
            ""
        );
    }

    #[test]
    fn task_row_cells_insert_metadata_between_title_and_project() {
        let mut item = task_list_item("documented");
        item.task.description = "details".to_string();
        item.notes = vec![crate::query::TaskNote {
            id: "note-id".to_string(),
            body: "one".to_string(),
            created_at: "001".to_string(),
        }];
        item.has_notes = true;
        item.unresolved_blocker_count = 1;
        item.dependent_count = 1;

        let cells = build_task_row_cells(
            &item,
            TaskTimeContext {
                now_seconds: 0,
                render_mode: TaskListRenderMode::Flat,
                due_order: false,
                show_due: true,
            },
            None,
            &[12, 40, 12, 6, 9, 10, 3, 5, 6],
            TaskRowState {
                selected: false,
                focused: false,
                marked: false,
            },
            EpicSelectionContext::default(),
        );

        assert_eq!(cells.len(), TableColumn::ALL.len());
        assert_eq!(cells[3].to_string(), "←1 →1 ✎");
        assert_eq!(cells[4].to_string(), "app ");

        item.task.deleted = true;
        let cells = build_task_row_cells(
            &item,
            TaskTimeContext {
                now_seconds: 0,
                render_mode: TaskListRenderMode::Flat,
                due_order: false,
                show_due: true,
            },
            None,
            &[12, 40, 12, 6, 9, 10, 3, 5, 6],
            TaskRowState {
                selected: false,
                focused: false,
                marked: false,
            },
            EpicSelectionContext::default(),
        );
        assert_eq!(cells[3].to_string(), "× ←1 →1 ✎");
    }

    #[test]
    fn task_row_cells_use_inline_title_when_selected() {
        let item = task_list_item("original title");
        let editor = TextInputView {
            kind: TextInputKind::EditTitle,
            title: "Edit title".to_string(),
            prompt: String::new(),
            input: "edited title".to_string(),
            cursor: 12,
        };

        let cells = build_task_row_cells(
            &item,
            TaskTimeContext {
                now_seconds: 0,
                render_mode: TaskListRenderMode::Flat,
                due_order: false,
                show_due: true,
            },
            Some(&editor),
            &[12, 40, 12, 6, 9, 10, 3, 5, 6],
            TaskRowState {
                selected: false,
                focused: false,
                marked: false,
            },
            EpicSelectionContext::default(),
        );

        assert!(cells[1].to_string().contains("edited title"));
    }

    #[test]
    fn epic_child_ref_prefix_aligns_tree_with_parent_marker() {
        let item = task_list_item("child");

        let cells = build_epic_child_row_cells(
            &item,
            false,
            None,
            TaskTimeContext {
                now_seconds: 0,
                render_mode: TaskListRenderMode::Epics,
                due_order: false,
                show_due: true,
            },
            &[14, 40, 12, 6, 9, 10, 3, 5, 6],
            TaskRowState {
                selected: false,
                focused: false,
                marked: false,
            },
            EpicSelectionContext::default(),
        );

        assert_eq!(cells[0].to_string(), "   ├─ APP-1 ");
    }
}
