use ratatui::layout::Rect;
use ratatui::widgets::TableState;

use crate::tui::store::TuiStore;

use super::view_model::{TaskListRow, TaskListView, task_list_scroll, task_list_visible_rows};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskListHit {
    pub(crate) task_index: usize,
    pub(crate) task_id: String,
    pub(crate) viewport_row: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TaskListHitCandidate {
    pub(super) task_index: usize,
    pub(super) viewport_row: u16,
}

pub(super) fn task_list_hit_in_view(
    view: &TaskListView,
    table_state: &TableState,
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
    let viewport_rows = table_area.height.saturating_sub(1) as usize;
    if view.rows.len() > viewport_rows
        && column
            == table_area
                .x
                .saturating_add(table_area.width)
                .saturating_sub(1)
    {
        return None;
    }
    if viewport_rows == 0 {
        return None;
    }
    let scroll = task_list_scroll(
        table_state.offset(),
        table_state
            .selected()
            .map(|selected| view.visual_row(selected))
            .unwrap_or(0),
        view.rows.len(),
        viewport_rows,
    );

    let visual_row = row - table_area.y - 1;
    let visual_row = usize::from(visual_row);
    if visual_row >= viewport_rows {
        return None;
    }

    let viewport_rows = task_list_visible_rows(view, scroll, viewport_rows);
    let (_, row) = *viewport_rows.get(visual_row)?;
    let viewport_row = u16::try_from(visual_row).ok()?;
    match row {
        TaskListRow::Task { task_index } => Some(TaskListHitCandidate {
            task_index: *task_index,
            viewport_row,
        }),
        TaskListRow::Group(_) => None,
    }
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
    use crate::choices::{TaskPriority, TaskStatus};
    use crate::queue::QueueBand;
    use crate::tui::store::TaskListRenderMode;

    fn task_item(title: &str) -> TaskListItem {
        TaskListItem {
            task: crate::types::Task {
                id: "task-1".to_string(),
                workspace_id: "workspace-1".to_string(),
                title: title.to_string(),
                description: String::new(),
                project_id: "project-id".to_string(),
                project_key: "app".to_string(),
                project_prefix: "APP".to_string(),
                status: TaskStatus::Todo,
                priority: TaskPriority::None,
                created_at: "2026-06-20T00:00:00Z".to_string(),
                updated_at: "2026-06-20T00:00:00Z".to_string(),
                queue_activity_at: "2026-06-20T00:00:00Z".to_string(),
                deleted: false,
            },
            display_ref: "APP-1".to_string(),
            labels: Vec::new(),
            notes: Vec::new(),
            has_conflict: false,
            unresolved_blocker_count: 0,
            dependent_count: 0,
            depends_on: Vec::new(),
            blocks: Vec::new(),
            queue: Default::default(),
        }
    }

    fn task_item_with(title: &str, status: &str, band: QueueBand) -> TaskListItem {
        let mut item = task_item(title);
        item.task.title = title.to_string();
        item.task.status = TaskStatus::parse(status).expect("valid status");
        item.queue.band = band;
        item
    }

    fn task_id(task: &mut TaskListItem, id: &str) {
        task.task.id = id.to_string();
    }

    use crate::query::TaskListItem;

    #[test]
    fn task_at_position_skips_queue_group_rows() {
        let mut tasks = vec![
            task_item_with("todo high", "todo", QueueBand::Focus),
            task_item_with("todo medium", "todo", QueueBand::Focus),
            task_item_with("inbox", "inbox", QueueBand::Triage),
        ];
        task_id(&mut tasks[0], "task-1");
        task_id(&mut tasks[1], "task-2");
        task_id(&mut tasks[2], "task-3");
        let view = TaskListView::from_tasks(TaskListRenderMode::Queue, &tasks);
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
        let mut tasks = Vec::new();
        for index in 0..20 {
            let mut item = task_item(&format!("task {index}"));
            task_id(&mut item, &format!("task-{index:02}"));
            tasks.push(item);
        }
        let view = TaskListView::from_tasks(TaskListRenderMode::Flat, &tasks);
        let mut table_state = TableState::default();
        table_state.select(Some(10));

        let hit = task_list_hit_in_view(&view, &table_state, Rect::new(0, 0, 80, 5), 1, 4).unwrap();
        assert_eq!(hit.task_index, 10);
        assert_eq!(hit.viewport_row, 3);
    }

    #[test]
    fn task_at_position_ignores_scrollbar_column() {
        let tasks = (0..20)
            .map(|index| task_item(&format!("task {index}")))
            .collect::<Vec<_>>();
        let view = TaskListView::from_tasks(TaskListRenderMode::Flat, &tasks);
        let mut table_state = TableState::default();
        table_state.select(Some(10));

        let hit = task_list_hit_in_view(&view, &table_state, Rect::new(0, 0, 80, 5), 79, 4);

        assert!(hit.is_none());
    }
}
