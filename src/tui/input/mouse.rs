use crossterm::event::{MouseButton, MouseEventKind};
use ratatui::layout::Rect;

use crate::choices::TaskStatus;
use crate::tui::app::{Focus, WidgetState};
use crate::tui::store::{TaskView, TuiStore};
use crate::tui::ui::{recent_action_at_position, task_at_position, task_status_at_position};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PointerTaskHit {
    pub(crate) task_index: usize,
    pub(crate) task_id: crate::ids::TaskId,
    pub(crate) viewport_row: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MouseInput {
    PrefixScroll(isize),
    OverlayScroll(MouseEventKind),
    DetailPress,
    DetailDrag,
    DetailRelease,
    PointerMove,
    StatusPress,
    Ignore,
}

pub(crate) fn route_mouse(kind: MouseEventKind, prefix_hints: bool) -> MouseInput {
    match kind {
        MouseEventKind::ScrollDown if prefix_hints => MouseInput::PrefixScroll(1),
        MouseEventKind::ScrollUp if prefix_hints => MouseInput::PrefixScroll(-1),
        MouseEventKind::ScrollDown => MouseInput::OverlayScroll(kind),
        MouseEventKind::ScrollUp => MouseInput::OverlayScroll(kind),
        MouseEventKind::Down(MouseButton::Left) => MouseInput::DetailPress,
        MouseEventKind::Drag(MouseButton::Left) => MouseInput::DetailDrag,
        MouseEventKind::Up(MouseButton::Left) => MouseInput::DetailRelease,
        MouseEventKind::Moved => MouseInput::PointerMove,
        MouseEventKind::Down(MouseButton::Right) => MouseInput::StatusPress,
        _ => MouseInput::Ignore,
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TaskSurfaceHits {
    pub(crate) lane_status: Option<TaskStatus>,
    pub(crate) recent_action: Option<usize>,
    pub(crate) status: Option<PointerTaskHit>,
    pub(crate) task: Option<PointerTaskHit>,
    pub(crate) sidebar_entry: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PointerEvent {
    MoveToColumn(TaskStatus),
    SelectRecentAction(usize),
    EditStatus(PointerTaskHit),
    SelectTask(PointerTaskHit),
    SelectSidebar(usize),
    None,
}

pub(crate) struct TaskSurfaceView<'a> {
    pub(crate) store: &'a TuiStore,
    pub(crate) widgets: &'a WidgetState,
    pub(crate) focus: Focus,
    pub(crate) sidebar_visible: bool,
    pub(crate) terminal_area: Rect,
    pub(crate) task_area: Rect,
    pub(crate) outside_sidebar: bool,
}

pub(crate) fn route_task_surface(view: TaskSurfaceView<'_>, column: u16, row: u16) -> PointerEvent {
    let TaskSurfaceView {
        store,
        widgets,
        focus,
        sidebar_visible,
        terminal_area,
        task_area,
        outside_sidebar,
    } = view;
    route_task_surface_hits(TaskSurfaceHits {
        lane_status: (outside_sidebar && store.view_state.view == TaskView::Columns)
            .then(|| {
                crate::tui::ui::column_lane_at_position(
                    store,
                    &widgets.table,
                    task_area,
                    column,
                    row,
                )
                .and_then(|lane_index| {
                    crate::tui::columns::lane_entry_status(&store.task_columns, lane_index)
                })
            })
            .flatten(),
        recent_action: (outside_sidebar && store.view_state.view == TaskView::RecentActions)
            .then(|| {
                recent_action_at_position(store, &widgets.table, task_area, column, row)
                    .map(|hit| hit.action_index)
            })
            .flatten(),
        status: outside_sidebar
            .then(|| task_status_at_position(store, &widgets.table, task_area, column, row))
            .flatten()
            .map(|hit| PointerTaskHit {
                task_index: hit.task_index,
                task_id: hit.task_id,
                viewport_row: hit.viewport_row,
            }),
        task: outside_sidebar
            .then(|| task_at_position(store, &widgets.table, task_area, column, row))
            .flatten()
            .map(|hit| PointerTaskHit {
                task_index: hit.task_index,
                task_id: hit.task_id,
                viewport_row: hit.viewport_row,
            }),
        sidebar_entry: crate::tui::ui::sidebar_click_at_for(
            &store.sidebar_entries,
            &widgets.sidebar,
            focus,
            sidebar_visible,
            terminal_area,
            column,
            row,
        )
        .map(|click| click.entry_index),
    })
}

fn route_task_surface_hits(hits: TaskSurfaceHits) -> PointerEvent {
    if let Some(status) = hits.lane_status {
        PointerEvent::MoveToColumn(status)
    } else if let Some(index) = hits.recent_action {
        PointerEvent::SelectRecentAction(index)
    } else if let Some(hit) = hits.status {
        PointerEvent::EditStatus(hit)
    } else if let Some(hit) = hits.task {
        PointerEvent::SelectTask(hit)
    } else if let Some(index) = hits.sidebar_entry {
        PointerEvent::SelectSidebar(index)
    } else {
        PointerEvent::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::TaskId;

    fn task_hit(index: usize) -> PointerTaskHit {
        PointerTaskHit {
            task_index: index,
            task_id: TaskId::new(),
            viewport_row: index as u16,
        }
    }

    #[test]
    fn scroll_routes_to_prefix_hints_before_surfaces() {
        assert_eq!(
            route_mouse(MouseEventKind::ScrollDown, true),
            MouseInput::PrefixScroll(1)
        );
        assert_eq!(
            route_mouse(MouseEventKind::ScrollDown, false),
            MouseInput::OverlayScroll(MouseEventKind::ScrollDown)
        );
    }

    #[test]
    fn status_hit_has_priority_over_task_row_hit() {
        let status = task_hit(3);
        let task = task_hit(4);
        assert_eq!(
            route_task_surface_hits(TaskSurfaceHits {
                status: Some(status.clone()),
                task: Some(task),
                ..TaskSurfaceHits::default()
            }),
            PointerEvent::EditStatus(status)
        );
    }

    #[test]
    fn lane_hit_routes_to_semantic_status() {
        assert_eq!(
            route_task_surface_hits(TaskSurfaceHits {
                lane_status: Some(TaskStatus::Done),
                task: Some(task_hit(2)),
                ..TaskSurfaceHits::default()
            }),
            PointerEvent::MoveToColumn(TaskStatus::Done)
        );
    }
}
