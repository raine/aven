use ratatui::layout::Rect;
#[cfg(test)]
use ratatui::widgets::TableState;

use crate::tui::store::TuiStore;

#[cfg(test)]
use super::view_model::TaskListView;
use super::view_model::{TaskListProjection, TaskListRow};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskListHit {
    pub(crate) task_index: usize,
    pub(crate) task_id: crate::ids::TaskId,
    pub(crate) viewport_row: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TaskListHitCandidate {
    pub(super) task_index: usize,
    pub(super) viewport_row: u16,
}

pub(super) fn task_list_hit_in_projection(
    projection: &TaskListProjection,
    table_area: Rect,
    column: u16,
    row: u16,
) -> Option<TaskListHitCandidate> {
    if column < table_area.x || column >= table_area.x.saturating_add(table_area.width) {
        return None;
    }
    if row <= table_area.y || row >= table_area.y.saturating_add(table_area.height) {
        return None;
    }
    if projection.row_count() > projection.viewport_rows
        && column
            == table_area
                .x
                .saturating_add(table_area.width)
                .saturating_sub(1)
    {
        return None;
    }
    if projection.viewport_rows == 0 {
        return None;
    }

    let visual_row = row - table_area.y - 1;
    let visual_row = usize::from(visual_row);
    if visual_row >= projection.viewport_rows {
        return None;
    }

    let visible_rows = projection.visible_rows();
    let (_, row) = *visible_rows.get(visual_row)?;
    let viewport_row = u16::try_from(visual_row).ok()?;
    match row {
        TaskListRow::Task { task_index } | TaskListRow::EpicChild { task_index, .. } => {
            Some(TaskListHitCandidate {
                task_index: *task_index,
                viewport_row,
            })
        }
        TaskListRow::Group(_) => None,
    }
}

#[cfg(test)]
pub(super) fn task_list_hit_in_view(
    view: &TaskListView,
    table_state: &TableState,
    table_area: Rect,
    column: u16,
    row: u16,
) -> Option<TaskListHitCandidate> {
    let viewport_rows = table_area.height.saturating_sub(1) as usize;
    let projection = TaskListProjection::from_view(
        view.clone(),
        table_state.offset(),
        table_state.selected(),
        viewport_rows,
    );
    task_list_hit_in_projection(&projection, table_area, column, row)
}

pub(super) fn task_list_hit(
    store: &TuiStore,
    candidate: TaskListHitCandidate,
) -> Option<TaskListHit> {
    let task_id = store
        .tasks
        .get(candidate.task_index)
        .map(|item| item.task.id.clone())?;
    Some(TaskListHit {
        task_index: candidate.task_index,
        task_id,
        viewport_row: candidate.viewport_row,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::QueueBand;
    use crate::tui::store::TaskListRenderMode;
    use crate::tui::test_support::{
        task_list_item, task_list_item_with_id, task_list_item_with_id_and_status_and_queue,
    };
    use std::collections::BTreeSet;

    #[test]
    fn task_at_position_skips_queue_group_rows() {
        let tasks = vec![
            task_list_item_with_id_and_status_and_queue(
                "todo high",
                "task-1",
                "todo",
                QueueBand::Focus,
            ),
            task_list_item_with_id_and_status_and_queue(
                "todo medium",
                "task-2",
                "todo",
                QueueBand::Focus,
            ),
            task_list_item_with_id_and_status_and_queue(
                "inbox",
                "task-3",
                "inbox",
                QueueBand::Triage,
            ),
        ];
        let view = TaskListView::from_tasks(TaskListRenderMode::Queue, &tasks, &BTreeSet::new());
        let table_area = Rect::new(0, 0, 80, 10);
        let table_state = TableState::default();

        let header_hit = task_list_hit_in_view(
            &view,
            &table_state,
            table_area,
            table_area.x + 1,
            table_area.y + 1,
        );
        assert!(header_hit.is_none());

        let first_task = task_list_hit_in_view(
            &view,
            &table_state,
            table_area,
            table_area.x + 1,
            table_area.y + 2,
        )
        .unwrap();
        assert_eq!(first_task.task_index, 0);
        assert_eq!(first_task.viewport_row, 1);
    }

    #[test]
    fn task_at_position_respects_scroll_position() {
        let tasks = (0..20)
            .map(|index| {
                task_list_item_with_id(&format!("task {index}"), &format!("task-{index:02}"))
            })
            .collect::<Vec<_>>();
        let view = TaskListView::from_tasks(TaskListRenderMode::Flat, &tasks, &BTreeSet::new());
        let mut table_state = TableState::default();
        table_state.select(Some(10));

        let hit = task_list_hit_in_view(&view, &table_state, Rect::new(0, 0, 80, 5), 1, 4).unwrap();
        assert_eq!(hit.task_index, 10);
        assert_eq!(hit.viewport_row, 3);
    }

    #[test]
    fn task_at_position_ignores_scrollbar_column() {
        let tasks = (0..20)
            .map(|index| task_list_item(&format!("task {index}")))
            .collect::<Vec<_>>();
        let view = TaskListView::from_tasks(TaskListRenderMode::Flat, &tasks, &BTreeSet::new());
        let mut table_state = TableState::default();
        table_state.select(Some(10));

        let hit = task_list_hit_in_view(&view, &table_state, Rect::new(0, 0, 80, 5), 79, 4);

        assert!(hit.is_none());
    }
}
