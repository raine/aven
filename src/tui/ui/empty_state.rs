use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::query::RecurrenceSeriesLifecycleFilter;
use crate::tui::event::{Action, CommandContext, preferred_shortcut_label};
use crate::tui::store::{
    ClosedTaskVisibility, RefreshHealth, TaskProjectionOrigin, TaskScope, TaskView, TuiStore,
};
use crate::tui::theme::{ACCENT, BG, BG_ALT, FG, FG_DIM, FG_MUTED, RED};

use crate::tui::text::truncate_width;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EmptyStateReason {
    LoadFailed,
    EmptyScope,
    NoFilterMatches,
    SearchPrompt,
    NoSearchResults,
    TaskUnavailable,
    NoDeletedTasks,
    DeferredTasks,
    NamedView,
    RecurrenceSearch,
    RecurrenceLifecycle,
    RecentActions,
    ColumnConfiguration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EmptyStateAction {
    pub(crate) action: Action,
    pub(crate) label: &'static str,
    pub(crate) compact_label: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EmptyState {
    pub(crate) reason: EmptyStateReason,
    pub(crate) title: &'static str,
    pub(crate) detail: &'static str,
    pub(crate) action: Option<EmptyStateAction>,
}

impl EmptyState {
    fn new(
        reason: EmptyStateReason,
        title: &'static str,
        detail: &'static str,
        action: Action,
        label: &'static str,
        compact_label: &'static str,
    ) -> Self {
        Self {
            reason,
            title,
            detail,
            action: Some(EmptyStateAction {
                action,
                label,
                compact_label,
            }),
        }
    }
}

pub(crate) fn task_empty_state(store: &TuiStore) -> EmptyState {
    if store.refresh_health() == RefreshHealth::Failed {
        return load_failed_state();
    }

    let modifiers = &store.view_state.filter_modifiers;
    let has_narrowing_filters = modifiers.deleted_only
        || modifiers.label.is_some()
        || modifiers.priority.is_some()
        || (modifiers.closed == ClosedTaskVisibility::Only
            && store.view_state.view != TaskView::Epics);
    match &store.view_state.projection_origin {
        TaskProjectionOrigin::SearchPrompt => {
            return EmptyState::new(
                EmptyStateReason::SearchPrompt,
                "Search tasks",
                "Find tasks by title, notes, labels, or ID.",
                Action::BeginSearch,
                "Start a search",
                "search",
            );
        }
        TaskProjectionOrigin::Search { task_ids, .. }
            if task_ids.is_empty() || !has_narrowing_filters =>
        {
            return EmptyState::new(
                EmptyStateReason::NoSearchResults,
                "No search results",
                "Try another title, note, label, or task ID.",
                Action::BeginSearch,
                "Search again",
                "search",
            );
        }
        TaskProjectionOrigin::ExactTasks(task_ids)
            if task_ids.is_empty() || !has_narrowing_filters =>
        {
            return EmptyState::new(
                EmptyStateReason::TaskUnavailable,
                "Task unavailable",
                "That task is not available in this workspace.",
                Action::ShowView(TaskView::Queue),
                "View queue",
                "queue",
            );
        }
        TaskProjectionOrigin::NamedView
        | TaskProjectionOrigin::Search { .. }
        | TaskProjectionOrigin::ExactTasks(_) => {}
    }

    if modifiers.deleted_only {
        let detail = if modifiers.label.is_some() || modifiers.priority.is_some() {
            "No deleted tasks match the other filters."
        } else {
            "This scope has no deleted tasks."
        };
        return EmptyState::new(
            EmptyStateReason::NoDeletedTasks,
            "No deleted tasks",
            detail,
            Action::ToggleDeletedFilter,
            "Show active tasks",
            "active tasks",
        );
    }

    if modifiers.label.is_some()
        || modifiers.priority.is_some()
        || (modifiers.closed == ClosedTaskVisibility::Only
            && store.view_state.view != TaskView::Epics)
    {
        return EmptyState::new(
            EmptyStateReason::NoFilterMatches,
            "No tasks match these filters",
            "Clear the active filters to widen this view.",
            Action::ClearFilters,
            "Clear filters",
            "clear filters",
        );
    }

    if matches!(
        store.view_state.view,
        TaskView::Queue | TaskView::Open | TaskView::Columns
    ) && store.counts.upcoming > 0
    {
        return EmptyState::new(
            EmptyStateReason::DeferredTasks,
            "No tasks available yet",
            "Scheduled tasks are waiting in Upcoming.",
            Action::ShowView(TaskView::Upcoming),
            "View upcoming tasks",
            "upcoming",
        );
    }

    if matches!(
        store.view_state.view,
        TaskView::Queue | TaskView::Open | TaskView::Columns
    ) && store.counts.open == 0
        && store.counts.done == 0
        && store.counts.upcoming == 0
    {
        return scope_empty_state(&store.view_state.scope);
    }

    match store.view_state.view {
        TaskView::Queue => named_state(
            "Queue is clear",
            "Nothing is actionable in this scope.",
            Action::BeginAddTask,
            "Add a task",
            "add task",
        ),
        TaskView::Columns => named_state(
            "No tasks in these columns",
            "Available tasks in this scope will appear by status.",
            Action::BeginAddTask,
            "Add a task",
            "add task",
        ),
        TaskView::Open => named_state(
            "No open tasks",
            "This scope has no available unfinished tasks.",
            Action::BeginAddTask,
            "Add a task",
            "add task",
        ),
        TaskView::Inbox => named_state(
            "Inbox is clear",
            "Capture anything you do not want to lose.",
            Action::BeginAddTask,
            "Add a task",
            "add task",
        ),
        TaskView::Active => named_state(
            "No active tasks",
            "Start the next task from your queue.",
            Action::ShowView(TaskView::Queue),
            "View queue",
            "queue",
        ),
        TaskView::Backlog => named_state(
            "Backlog is clear",
            "There are no parked tasks in this scope.",
            Action::ShowView(TaskView::Queue),
            "View queue",
            "queue",
        ),
        TaskView::Todo => named_state(
            "No todo tasks",
            "Choose the next task from your queue.",
            Action::ShowView(TaskView::Queue),
            "View queue",
            "queue",
        ),
        TaskView::Done => named_state(
            "Nothing completed yet",
            "Finished and canceled tasks collect here.",
            Action::ShowView(TaskView::Queue),
            "View queue",
            "queue",
        ),
        TaskView::Upcoming => named_state(
            "Nothing scheduled",
            "No open tasks are waiting for a future date.",
            Action::ShowView(TaskView::Queue),
            "View queue",
            "queue",
        ),
        TaskView::Conflicts => named_state(
            "No conflicts",
            "Task changes agree across devices.",
            Action::ShowView(TaskView::Queue),
            "View queue",
            "queue",
        ),
        TaskView::Search => named_state(
            "Search tasks",
            "Find tasks by title, notes, labels, or ID.",
            Action::BeginSearch,
            "Start a search",
            "search",
        ),
        TaskView::Epics => {
            let (title, detail) = match modifiers.closed {
                ClosedTaskVisibility::Only => (
                    "No closed epics",
                    "Finished and canceled epics collect here.",
                ),
                ClosedTaskVisibility::Included => (
                    "No epics",
                    "There are no open or closed epics in this scope.",
                ),
                ClosedTaskVisibility::Default => (
                    "No open epics",
                    "Finished epics are available with the closed filter.",
                ),
            };
            named_state(
                title,
                detail,
                Action::ToggleClosedFilter,
                "Change closed visibility",
                "closed filter",
            )
        }
        TaskView::Recurring | TaskView::RecentActions => named_state(
            "Nothing to show",
            "This view has no rows in the current scope.",
            Action::ShowView(TaskView::Queue),
            "View queue",
            "queue",
        ),
    }
}

pub(crate) fn recurrence_empty_state(store: &TuiStore) -> EmptyState {
    if store.refresh_health() == RefreshHealth::Failed {
        return load_failed_state();
    }
    if store.view_state.recurring.search.is_some() {
        return EmptyState::new(
            EmptyStateReason::RecurrenceSearch,
            "No recurring series match this search",
            "Try another title, description, project, or label.",
            Action::BeginSearch,
            "Search again",
            "search",
        );
    }
    let lifecycle = store.view_state.recurring.lifecycle;
    if lifecycle != RecurrenceSeriesLifecycleFilter::ActiveOrPaused {
        let title = match lifecycle {
            RecurrenceSeriesLifecycleFilter::ActiveOrPaused => {
                "No active or paused recurring series"
            }
            RecurrenceSeriesLifecycleFilter::Active => "No active recurring series",
            RecurrenceSeriesLifecycleFilter::Paused => "No paused recurring series",
            RecurrenceSeriesLifecycleFilter::Stopped => "No stopped recurring series",
            RecurrenceSeriesLifecycleFilter::All => "No recurring series",
        };
        return EmptyState::new(
            EmptyStateReason::RecurrenceLifecycle,
            title,
            "Change the lifecycle filter to see other series.",
            Action::CycleRecurringLifecycleFilter,
            "Change lifecycle filter",
            "lifecycle filter",
        );
    }
    let title = match store.view_state.scope {
        TaskScope::Workspace => "No active or paused recurring series in this workspace",
        TaskScope::Project(_) => "No active or paused recurring series in this project",
    };
    EmptyState::new(
        EmptyStateReason::RecurrenceLifecycle,
        title,
        "Stopped series are available through the lifecycle filter.",
        Action::BeginAddTask,
        "Add a recurring task",
        "add task",
    )
}

pub(crate) fn recent_actions_empty_state(store: &TuiStore) -> EmptyState {
    if store.refresh_health() == RefreshHealth::Failed {
        return load_failed_state();
    }
    let detail = match store.view_state.scope {
        TaskScope::Workspace => "Task changes in this workspace will appear here.",
        TaskScope::Project(_) => "Task changes in this project will appear here.",
    };
    EmptyState::new(
        EmptyStateReason::RecentActions,
        "No recent actions in this scope",
        detail,
        Action::BeginAddTask,
        "Add a task",
        "add task",
    )
}

pub(crate) fn column_board_empty_state(store: &TuiStore) -> EmptyState {
    if store.refresh_health() == RefreshHealth::Failed || store.tasks.is_empty() {
        task_empty_state(store)
    } else {
        EmptyState::new(
            EmptyStateReason::ColumnConfiguration,
            "Tasks do not fit these columns",
            "The column configuration does not include their statuses.",
            Action::ShowConfigInfo,
            "Review configuration",
            "config",
        )
    }
}

pub(crate) fn render_empty_state(frame: &mut Frame, area: Rect, state: EmptyState) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    frame.render_widget(Block::new().style(Style::new().bg(BG)), area);

    let compact = area.width < 28;
    let max_width = usize::from(area.width.saturating_sub(4).max(1));
    let action_line = state.action.and_then(|action| {
        preferred_shortcut_label(action.action, CommandContext::Normal).map(|key| {
            let label = if compact {
                action.compact_label
            } else {
                action.label
            };
            Line::from(vec![
                Span::styled(
                    format!(" {key} "),
                    Style::new().fg(FG).bg(BG_ALT).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(
                        " {}",
                        truncate_width(
                            label,
                            max_width.saturating_sub(UnicodeWidthStr::width(key) + 4),
                        )
                    ),
                    Style::new().fg(FG_MUTED),
                ),
            ])
        })
    });

    let title = truncate_width(state.title, max_width);
    let detail = truncate_width(state.detail, max_width);
    let title_line = if compact {
        Line::from(Span::styled(
            title,
            Style::new()
                .fg(if state.reason == EmptyStateReason::LoadFailed {
                    RED
                } else {
                    FG
                })
                .add_modifier(Modifier::BOLD),
        ))
    } else {
        let (marker, accent) = if state.reason == EmptyStateReason::LoadFailed {
            ("!  ", RED)
        } else {
            ("◆  ", ACCENT)
        };
        Line::from(vec![
            Span::styled(marker, Style::new().fg(accent).add_modifier(Modifier::BOLD)),
            Span::styled(
                title,
                Style::new()
                    .fg(if state.reason == EmptyStateReason::LoadFailed {
                        RED
                    } else {
                        FG
                    })
                    .add_modifier(Modifier::BOLD),
            ),
        ])
    };

    let mut lines = if area.height == 1 {
        action_line
            .clone()
            .map(|line| vec![line])
            .unwrap_or_else(|| vec![title_line.clone()])
    } else if area.height == 2 {
        vec![title_line.clone()]
    } else {
        vec![
            title_line.clone(),
            Line::from(Span::styled(detail, Style::new().fg(FG_DIM))),
        ]
    };
    if area.height >= 5 && action_line.is_some() {
        lines.push(Line::from(""));
    }
    if area.height >= 2
        && let Some(action_line) = action_line
    {
        lines.push(action_line);
    }
    lines.truncate(area.height as usize);

    let content = Rect::new(
        area.x,
        area.y
            .saturating_add(area.height.saturating_sub(lines.len() as u16) / 2),
        area.width,
        lines.len() as u16,
    );
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .alignment(Alignment::Center)
            .style(Style::new().bg(BG)),
        content,
    );
}

fn load_failed_state() -> EmptyState {
    EmptyState::new(
        EmptyStateReason::LoadFailed,
        "Could not load this view",
        "Try loading the current scope again.",
        Action::Refresh,
        "Refresh",
        "refresh",
    )
}

fn scope_empty_state(scope: &TaskScope) -> EmptyState {
    let (title, detail) = match scope {
        TaskScope::Workspace => (
            "No tasks in this workspace",
            "Add the first task to start building your queue.",
        ),
        TaskScope::Project(_) => (
            "No tasks in this project",
            "Add the first task for this project.",
        ),
    };
    EmptyState::new(
        EmptyStateReason::EmptyScope,
        title,
        detail,
        Action::BeginAddTask,
        "Add a task",
        "add task",
    )
}

fn named_state(
    title: &'static str,
    detail: &'static str,
    action: Action,
    label: &'static str,
    compact_label: &'static str,
) -> EmptyState {
    EmptyState::new(
        EmptyStateReason::NamedView,
        title,
        detail,
        action,
        label,
        compact_label,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn render_state(state: EmptyState, width: u16, height: u16) -> ratatui::buffer::Buffer {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_empty_state(frame, frame.area(), state))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn buffer_text(buffer: &ratatui::buffer::Buffer) -> String {
        buffer.content.iter().map(|cell| cell.symbol()).collect()
    }

    #[test]
    fn advertised_actions_resolve_through_the_catalog() {
        let actions = [
            Action::BeginAddTask,
            Action::BeginSearch,
            Action::ClearFilters,
            Action::ToggleClosedFilter,
            Action::ToggleDeletedFilter,
            Action::CycleRecurringLifecycleFilter,
            Action::ShowView(TaskView::Queue),
            Action::ShowView(TaskView::Upcoming),
            Action::Refresh,
            Action::ShowConfigInfo,
        ];
        for action in actions {
            assert!(
                preferred_shortcut_label(action, CommandContext::Normal).is_some(),
                "missing normal shortcut for {action:?}"
            );
        }
        assert_eq!(
            preferred_shortcut_label(Action::BeginAddTask, CommandContext::Normal),
            Some("a")
        );
    }

    #[test]
    fn renderer_adapts_to_full_narrow_and_short_areas() {
        let state = scope_empty_state(&TaskScope::Workspace);

        let full = buffer_text(&render_state(state, 80, 7));
        let narrow = buffer_text(&render_state(state, 20, 4));
        let short = buffer_text(&render_state(state, 20, 1));

        assert!(full.contains("No tasks in this workspace"));
        assert!(full.contains("Add the first task"));
        assert!(full.contains("Add a task"));
        assert!(narrow.contains("No tasks in"));
        assert!(narrow.contains("add task"));
        assert!(!narrow.contains('◆'));
        assert!(short.contains("a  add task"));
        assert!(!short.contains("No tasks"));
    }

    #[test]
    fn load_error_has_distinct_marker_and_color() {
        let buffer = render_state(load_failed_state(), 48, 5);
        let text = buffer_text(&buffer);
        let marker = buffer
            .content
            .iter()
            .find(|cell| cell.symbol() == "!")
            .unwrap();

        assert!(text.contains("Could not load this view"));
        assert_eq!(marker.style().fg, Some(RED));
    }
}
