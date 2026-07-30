use std::collections::{BTreeSet, HashMap};

use crate::query::TaskListItem;
use crate::tui::store::{TaskListRenderMode, TuiStore};
use ratatui::widgets::TableState;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TaskGroupRow {
    pub(super) label: String,
    pub(super) count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TaskListRow {
    Group(TaskGroupRow),
    Task {
        task_index: usize,
    },
    EpicChild {
        parent_index: usize,
        task_index: usize,
        last: bool,
    },
}

#[derive(Debug, Clone)]
pub(super) struct TaskListView {
    pub(super) rows: Vec<TaskListRow>,
    pub(super) render_mode: TaskListRenderMode,
}

#[derive(Debug)]
pub(super) struct TaskListProjection {
    pub(super) view: TaskListView,
    pub(super) selected_task: Option<usize>,
    pub(super) scroll: usize,
    pub(super) viewport_rows: usize,
}

impl TaskListProjection {
    pub(super) fn from_view(
        view: TaskListView,
        offset: usize,
        selected_task: Option<usize>,
        viewport_rows: usize,
    ) -> Self {
        let selected_row = selected_task
            .map(|selected| view.visual_row(selected))
            .unwrap_or(0);
        let scroll = task_list_scroll(offset, selected_row, &view, viewport_rows);
        Self {
            view,
            selected_task,
            scroll,
            viewport_rows,
        }
    }

    pub(super) fn from_table_state(
        store: &TuiStore,
        table_state: &TableState,
        viewport_rows: usize,
    ) -> Self {
        Self::from_view(
            TaskListView::new(store),
            table_state.offset(),
            table_state.selected(),
            viewport_rows,
        )
    }

    pub(super) fn visible_rows(&self) -> Vec<(usize, &TaskListRow)> {
        task_list_visible_rows(&self.view, self.scroll, self.viewport_rows)
    }

    pub(super) fn row_count(&self) -> usize {
        self.view.rows.len()
    }

    pub(super) fn top_scroll(&self) -> usize {
        task_list_top_scroll(&self.view)
    }

    pub(super) fn commit_scroll(&self, table_state: &mut TableState) {
        *table_state.offset_mut() = self.scroll;
    }
}

impl TaskListView {
    pub(super) fn new(store: &TuiStore) -> Self {
        Self::from_tasks(
            store.view_state.render_mode(),
            &store.tasks,
            &store.view_state.expanded_epic_ids,
        )
    }

    pub(super) fn from_tasks(
        render_mode: TaskListRenderMode,
        tasks: &[TaskListItem],
        expanded_epic_ids: &BTreeSet<crate::ids::TaskId>,
    ) -> Self {
        let rows = match render_mode {
            TaskListRenderMode::Queue => queue_rows(tasks),
            TaskListRenderMode::Upcoming => upcoming_rows(tasks, crate::queue::now_seconds()),
            TaskListRenderMode::Flat | TaskListRenderMode::Columns => task_rows(tasks),
            TaskListRenderMode::Epics => epics_rows(tasks, expanded_epic_ids),
        };
        Self { rows, render_mode }
    }

    pub(super) fn visual_row(&self, selected_task: usize) -> usize {
        self.visual_row_for(selected_task).unwrap_or(0)
    }

    pub(super) fn visual_row_for(&self, selected_task: usize) -> Option<usize> {
        self.rows.iter().position(|row| match row {
            TaskListRow::EpicChild { task_index, .. } | TaskListRow::Task { task_index } => {
                *task_index == selected_task
            }
            _ => false,
        })
    }

    pub(super) fn task_index_at_visual_row(&self, visual_row: usize) -> Option<usize> {
        match self.rows.get(visual_row)? {
            TaskListRow::EpicChild { task_index, .. } | TaskListRow::Task { task_index } => {
                Some(*task_index)
            }
            TaskListRow::Group(_) => None,
        }
    }
}

pub(super) fn epics_rows(
    tasks: &[TaskListItem],
    expanded_epic_ids: &BTreeSet<crate::ids::TaskId>,
) -> Vec<TaskListRow> {
    let mut rows = Vec::new();
    let task_indices = tasks
        .iter()
        .enumerate()
        .map(|(index, item)| (item.task.id.as_str(), index))
        .collect::<HashMap<_, _>>();
    for (parent_index, item) in tasks.iter().enumerate() {
        let is_epic_parent = item.task.is_epic;
        if !is_epic_parent {
            continue;
        }
        rows.push(TaskListRow::Task {
            task_index: parent_index,
        });
        if expanded_epic_ids.contains(&item.task.id) {
            let child_task_indices = item
                .epic_children
                .iter()
                .filter_map(|link| task_indices.get(link.task_id.as_str()).copied())
                .collect::<Vec<_>>();
            let last_child_index = child_task_indices.len().saturating_sub(1);
            for (child_index, task_index) in child_task_indices.into_iter().enumerate() {
                rows.push(TaskListRow::EpicChild {
                    parent_index,
                    task_index,
                    last: child_index == last_child_index,
                });
            }
        }
    }
    rows
}

pub(super) fn task_rows(tasks: &[TaskListItem]) -> Vec<TaskListRow> {
    tasks
        .iter()
        .enumerate()
        .map(|(task_index, _)| TaskListRow::Task { task_index })
        .collect()
}

fn grouped_task_rows_by<F>(tasks: &[TaskListItem], label_for: F) -> Vec<TaskListRow>
where
    F: Fn(&TaskListItem) -> String,
{
    let mut rows = Vec::new();
    let mut index = 0;
    while index < tasks.len() {
        let label = label_for(&tasks[index]);
        let start = index;
        while index < tasks.len() && label_for(&tasks[index]) == label {
            index += 1;
        }
        rows.push(TaskListRow::Group(TaskGroupRow {
            label,
            count: index - start,
        }));
        rows.extend((start..index).map(|task_index| TaskListRow::Task { task_index }));
    }
    rows
}

pub(super) fn queue_rows(tasks: &[TaskListItem]) -> Vec<TaskListRow> {
    grouped_task_rows_by(tasks, queue_group_label)
}

pub(super) fn queue_group_label(item: &TaskListItem) -> String {
    match item.task.status.as_str() {
        "done" => "done".to_string(),
        "canceled" => "canceled".to_string(),
        _ => item.queue.band.label().to_string(),
    }
}

pub(super) fn upcoming_rows(tasks: &[TaskListItem], now_seconds: i64) -> Vec<TaskListRow> {
    grouped_task_rows_by(tasks, |item| {
        crate::tui::time::available_day_label(
            item.task.available_at.as_deref().unwrap_or(""),
            now_seconds,
        )
    })
}

pub(super) fn task_list_visible_rows(
    view: &TaskListView,
    scroll: usize,
    viewport_rows: usize,
) -> Vec<(usize, &TaskListRow)> {
    let mut rows = Vec::new();
    if let Some(TaskListRow::Task { .. }) = view.rows.get(scroll)
        && let Some(group @ TaskListRow::Group(_)) = view.rows.get(scroll.saturating_sub(1))
    {
        rows.push((scroll.saturating_sub(1), group));
    }
    rows.extend(
        view.rows
            .iter()
            .enumerate()
            .skip(scroll)
            .take(viewport_rows.saturating_sub(rows.len())),
    );
    rows
}

pub(super) fn task_list_scroll(
    current_scroll: usize,
    selected_row: usize,
    view: &TaskListView,
    viewport_rows: usize,
) -> usize {
    if viewport_rows == 0 || view.rows.len() <= viewport_rows {
        return 0;
    }
    let hard_max_scroll = view.rows.len().saturating_sub(1);
    let base_max_scroll = view.rows.len().saturating_sub(viewport_rows);
    let max_scroll = if matches!(
        view.rows.get(base_max_scroll),
        Some(TaskListRow::Task { .. })
    ) && matches!(
        view.rows.get(base_max_scroll.saturating_sub(1)),
        Some(TaskListRow::Group(_))
    ) {
        base_max_scroll.saturating_add(1).min(hard_max_scroll)
    } else {
        base_max_scroll
    };
    let scroll = current_scroll.min(max_scroll);
    if task_list_visible_rows(view, scroll, viewport_rows)
        .iter()
        .any(|(row_index, _)| *row_index == selected_row)
    {
        return scroll;
    }
    if selected_row < scroll {
        return selected_row.min(hard_max_scroll);
    }
    for candidate in scroll.saturating_add(1)..=selected_row.min(hard_max_scroll) {
        if task_list_visible_rows(view, candidate, viewport_rows)
            .iter()
            .any(|(row_index, _)| *row_index == selected_row)
        {
            return candidate;
        }
    }
    selected_row.min(hard_max_scroll)
}

pub(super) fn task_list_top_scroll(view: &TaskListView) -> usize {
    match view.rows.first() {
        Some(TaskListRow::Group(_)) => 1,
        _ => 0,
    }
}

pub(super) fn scrollbar_position(
    scroll: usize,
    row_count: usize,
    viewport_rows: usize,
    top_scroll: usize,
) -> usize {
    if viewport_rows == 0 || row_count <= viewport_rows || scroll <= top_scroll {
        0
    } else {
        scroll.saturating_mul(row_count.saturating_sub(1)) / (row_count - viewport_rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::QueueBand;
    use crate::tui::test_support::{
        task_list_item, task_list_item_with_id, task_list_item_with_status_and_queue,
    };
    use std::collections::BTreeSet;

    #[test]
    fn project_filtered_queue_view_groups_by_queue_band() {
        let tasks = vec![
            task_list_item_with_status_and_queue("todo high", "todo", QueueBand::Focus),
            task_list_item_with_status_and_queue("todo medium", "todo", QueueBand::Soon),
            task_list_item_with_status_and_queue("inbox", "inbox", QueueBand::Triage),
            task_list_item_with_status_and_queue("backlog", "backlog", QueueBand::Later),
        ];

        let view = TaskListView::from_tasks(TaskListRenderMode::Queue, &tasks, &BTreeSet::new());

        assert_eq!(view.render_mode, TaskListRenderMode::Queue);
        assert_eq!(
            view.rows,
            vec![
                TaskListRow::Group(TaskGroupRow {
                    label: "focus".to_string(),
                    count: 1,
                }),
                TaskListRow::Task { task_index: 0 },
                TaskListRow::Group(TaskGroupRow {
                    label: "soon".to_string(),
                    count: 1,
                }),
                TaskListRow::Task { task_index: 1 },
                TaskListRow::Group(TaskGroupRow {
                    label: "triage".to_string(),
                    count: 1,
                }),
                TaskListRow::Task { task_index: 2 },
                TaskListRow::Group(TaskGroupRow {
                    label: "later".to_string(),
                    count: 1,
                }),
                TaskListRow::Task { task_index: 3 },
            ]
        );
    }

    #[test]
    fn queue_view_keeps_nonadjacent_equal_bands_in_separate_groups() {
        let tasks = vec![
            task_list_item_with_status_and_queue("focus 1", "todo", QueueBand::Focus),
            task_list_item_with_status_and_queue("soon", "todo", QueueBand::Soon),
            task_list_item_with_status_and_queue("focus 2", "todo", QueueBand::Focus),
        ];

        let view = TaskListView::from_tasks(TaskListRenderMode::Queue, &tasks, &BTreeSet::new());

        assert_eq!(
            view.rows,
            vec![
                TaskListRow::Group(TaskGroupRow {
                    label: "focus".to_string(),
                    count: 1,
                }),
                TaskListRow::Task { task_index: 0 },
                TaskListRow::Group(TaskGroupRow {
                    label: "soon".to_string(),
                    count: 1,
                }),
                TaskListRow::Task { task_index: 1 },
                TaskListRow::Group(TaskGroupRow {
                    label: "focus".to_string(),
                    count: 1,
                }),
                TaskListRow::Task { task_index: 2 },
            ]
        );
    }

    #[test]
    fn queue_view_groups_epics_separately() {
        let tasks = vec![
            task_list_item_with_status_and_queue("backlog", "backlog", QueueBand::Later),
            task_list_item_with_status_and_queue("epic", "todo", QueueBand::Epics),
        ];

        let view = TaskListView::from_tasks(TaskListRenderMode::Queue, &tasks, &BTreeSet::new());

        assert_eq!(
            view.rows,
            vec![
                TaskListRow::Group(TaskGroupRow {
                    label: "later".to_string(),
                    count: 1,
                }),
                TaskListRow::Task { task_index: 0 },
                TaskListRow::Group(TaskGroupRow {
                    label: "epics".to_string(),
                    count: 1,
                }),
                TaskListRow::Task { task_index: 1 },
            ]
        );
    }

    #[test]
    fn queue_view_groups_terminal_statuses_by_status() {
        let tasks = vec![
            task_list_item_with_status_and_queue("backlog", "backlog", QueueBand::Later),
            task_list_item_with_status_and_queue("finished", "done", QueueBand::Later),
            task_list_item_with_status_and_queue("canceled", "canceled", QueueBand::Later),
        ];

        let view = TaskListView::from_tasks(TaskListRenderMode::Queue, &tasks, &BTreeSet::new());

        assert_eq!(
            view.rows,
            vec![
                TaskListRow::Group(TaskGroupRow {
                    label: "later".to_string(),
                    count: 1,
                }),
                TaskListRow::Task { task_index: 0 },
                TaskListRow::Group(TaskGroupRow {
                    label: "done".to_string(),
                    count: 1,
                }),
                TaskListRow::Task { task_index: 1 },
                TaskListRow::Group(TaskGroupRow {
                    label: "canceled".to_string(),
                    count: 1,
                }),
                TaskListRow::Task { task_index: 2 },
            ]
        );
    }

    #[test]
    fn non_queue_sort_does_not_emit_duplicate_status_groups() {
        let tasks = vec![
            task_list_item_with_status_and_queue("todo 1", "todo", QueueBand::Focus),
            task_list_item_with_status_and_queue("inbox", "inbox", QueueBand::Triage),
            task_list_item_with_status_and_queue("todo 2", "todo", QueueBand::Later),
        ];

        let view = TaskListView::from_tasks(TaskListRenderMode::Flat, &tasks, &BTreeSet::new());

        assert_eq!(view.render_mode, TaskListRenderMode::Flat);
        assert_eq!(
            view.rows,
            vec![
                TaskListRow::Task { task_index: 0 },
                TaskListRow::Task { task_index: 1 },
                TaskListRow::Task { task_index: 2 },
            ]
        );
    }

    #[test]
    fn visual_row_uses_planned_rows() {
        let tasks = vec![
            task_list_item_with_status_and_queue("todo high", "todo", QueueBand::Focus),
            task_list_item_with_status_and_queue("todo medium", "todo", QueueBand::Soon),
            task_list_item_with_status_and_queue("inbox", "inbox", QueueBand::Triage),
        ];
        let view = TaskListView::from_tasks(TaskListRenderMode::Queue, &tasks, &BTreeSet::new());

        assert_eq!(view.visual_row(0), 1);
        assert_eq!(view.visual_row(1), 3);
        assert_eq!(view.visual_row(2), 5);
    }

    #[test]
    fn queue_view_keeps_group_header_with_first_visible_task() {
        let tasks = vec![
            task_list_item_with_status_and_queue("todo high", "todo", QueueBand::Focus),
            task_list_item_with_status_and_queue("todo medium", "todo", QueueBand::Soon),
            task_list_item_with_status_and_queue("inbox", "inbox", QueueBand::Triage),
        ];
        let view = TaskListView::from_tasks(TaskListRenderMode::Queue, &tasks, &BTreeSet::new());

        assert_eq!(
            task_list_visible_rows(&view, 1, 3),
            vec![
                (
                    0,
                    &TaskListRow::Group(TaskGroupRow {
                        label: "focus".to_string(),
                        count: 1
                    })
                ),
                (1, &TaskListRow::Task { task_index: 0 }),
                (
                    2,
                    &TaskListRow::Group(TaskGroupRow {
                        label: "soon".to_string(),
                        count: 1
                    })
                ),
            ]
        );
        assert_eq!(
            task_list_visible_rows(&view, 3, 3),
            vec![
                (
                    2,
                    &TaskListRow::Group(TaskGroupRow {
                        label: "soon".to_string(),
                        count: 1
                    })
                ),
                (3, &TaskListRow::Task { task_index: 1 }),
                (
                    4,
                    &TaskListRow::Group(TaskGroupRow {
                        label: "triage".to_string(),
                        count: 1
                    })
                ),
            ]
        );
    }

    #[test]
    fn empty_task_view_has_no_rows() {
        let view = TaskListView::from_tasks(TaskListRenderMode::Queue, &[], &BTreeSet::new());

        assert!(view.rows.is_empty());
        assert_eq!(view.visual_row(0), 0);
    }

    fn flat_view(row_count: usize) -> TaskListView {
        let tasks = (0..row_count)
            .map(|index| task_list_item(&format!("task {index}")))
            .collect::<Vec<_>>();
        TaskListView::from_tasks(TaskListRenderMode::Flat, &tasks, &BTreeSet::new())
    }

    #[test]
    fn upward_selection_from_bottom_keeps_scroll_at_bottom_until_top_edge() {
        let view = flat_view(10);

        assert_eq!(task_list_scroll(6, 9, &view, 4), 6);
        assert_eq!(task_list_scroll(6, 8, &view, 4), 6);
        assert_eq!(task_list_scroll(6, 7, &view, 4), 6);
        assert_eq!(task_list_scroll(6, 6, &view, 4), 6);
        assert_eq!(task_list_scroll(6, 5, &view, 4), 5);
    }

    #[test]
    fn returning_to_first_row_resets_scroll_to_top() {
        let view = flat_view(10);

        assert_eq!(task_list_scroll(1, 0, &view, 4), 0);
        assert_eq!(task_list_scroll(6, 6, &view, 4), 6);
    }

    #[test]
    fn downward_selection_scrolls_after_bottom_edge() {
        let view = flat_view(10);

        assert_eq!(task_list_scroll(0, 0, &view, 4), 0);
        assert_eq!(task_list_scroll(0, 1, &view, 4), 0);
        assert_eq!(task_list_scroll(0, 2, &view, 4), 0);
        assert_eq!(task_list_scroll(0, 3, &view, 4), 0);
        assert_eq!(task_list_scroll(0, 4, &view, 4), 1);
    }

    #[test]
    fn queue_sticky_header_counts_toward_scroll_visibility() {
        let tasks = vec![
            task_list_item_with_status_and_queue("backlog", "backlog", QueueBand::Later),
            task_list_item_with_status_and_queue("epic", "todo", QueueBand::Epics),
        ];
        let view = TaskListView::from_tasks(TaskListRenderMode::Queue, &tasks, &BTreeSet::new());

        let scroll = task_list_scroll(0, 3, &view, 3);

        assert_eq!(scroll, 2);
        assert!(
            task_list_visible_rows(&view, scroll, 3)
                .iter()
                .any(|(row_index, _)| *row_index == 3)
        );
    }

    #[test]
    fn upward_selection_from_bottom_keeps_final_queue_group_visible() {
        let mut epic = task_list_item_with_status_and_queue("epic", "todo", QueueBand::Epics);
        epic.task.is_epic = true;
        let tasks = vec![
            task_list_item_with_status_and_queue("focus", "todo", QueueBand::Focus),
            task_list_item_with_status_and_queue("soon", "todo", QueueBand::Soon),
            task_list_item_with_status_and_queue("triage", "inbox", QueueBand::Triage),
            task_list_item_with_status_and_queue("later", "backlog", QueueBand::Later),
            epic,
        ];
        let view = TaskListView::from_tasks(TaskListRenderMode::Queue, &tasks, &BTreeSet::new());

        let bottom_scroll = task_list_scroll(0, 9, &view, 5);
        let upward_scroll = task_list_scroll(bottom_scroll, 7, &view, 5);
        let visible_rows = task_list_visible_rows(&view, upward_scroll, 5)
            .into_iter()
            .map(|(index, _)| index)
            .collect::<Vec<_>>();

        assert_eq!(bottom_scroll, 6);
        assert_eq!(upward_scroll, bottom_scroll);
        assert_eq!(visible_rows, vec![6, 7, 8, 9]);
    }

    #[test]
    fn task_list_scroll_clamps_to_valid_rows() {
        let short_view = flat_view(3);
        let view = flat_view(10);

        assert_eq!(task_list_scroll(6, 2, &short_view, 4), 0);
        assert_eq!(task_list_scroll(8, 9, &view, 4), 6);
    }

    #[test]
    fn scrollbar_position_maps_max_scroll_to_end() {
        assert_eq!(scrollbar_position(0, 10, 4, 0), 0);
        assert_eq!(scrollbar_position(6, 10, 4, 0), 9);
        assert_eq!(scrollbar_position(0, 3, 4, 0), 0);
    }

    #[test]
    fn grouped_queue_top_scroll_keeps_scrollbar_at_top() {
        assert_eq!(scrollbar_position(1, 10, 4, 1), 0);
    }

    fn make_task(title: &str, id: &str) -> TaskListItem {
        task_list_item_with_id(title, id)
    }

    fn make_epic_parent(
        title: &str,
        id: &str,
        child_ids: &[&str],
        unresolved: bool,
    ) -> TaskListItem {
        let mut item = make_task(title, id);
        item.task.is_epic = true;
        item.epic_children = child_ids
            .iter()
            .map(|child_id| crate::query::TaskDependencyLink {
                task_id: crate::test_support::task_id(child_id),
                display_ref: format!("APP-{}", &child_id[..4]),
                title: "child".to_string(),
                status: "todo".to_string(),
                priority: "none".to_string(),
                unresolved,
            })
            .collect();
        item
    }

    #[test]
    fn collapsed_epic_parent_emits_only_parent_row() {
        let child = make_task("child", "child-1");
        let parent = make_epic_parent("parent", "parent-1", &["child-1"], true);

        let tasks = vec![parent, child];

        let view = TaskListView::from_tasks(TaskListRenderMode::Epics, &tasks, &BTreeSet::new());

        assert_eq!(view.rows, vec![TaskListRow::Task { task_index: 0 }]);
    }

    #[test]
    fn expanded_epic_parent_emits_child_row() {
        let child = make_task("child", "child-1");
        let parent = make_epic_parent("parent", "parent-1", &["child-1"], true);
        let mut expanded = BTreeSet::new();
        expanded.insert(crate::test_support::task_id("parent-1"));

        let tasks = vec![parent, child];

        let view = TaskListView::from_tasks(TaskListRenderMode::Epics, &tasks, &expanded);

        assert_eq!(
            view.rows,
            vec![
                TaskListRow::Task { task_index: 0 },
                TaskListRow::EpicChild {
                    parent_index: 0,
                    task_index: 1,
                    last: true,
                },
            ]
        );
    }

    #[test]
    fn expanded_epic_includes_resolved_children() {
        let resolved_child = make_task("resolved", "child-1");
        let open_child = make_task("open", "child-2");
        let mut parent = make_epic_parent("parent", "parent-1", &["child-1", "child-2"], true);
        parent.epic_children[0].unresolved = false;
        let mut expanded = BTreeSet::new();
        expanded.insert(crate::test_support::task_id("parent-1"));

        let tasks = vec![parent, resolved_child, open_child];

        let view = TaskListView::from_tasks(TaskListRenderMode::Epics, &tasks, &expanded);

        assert_eq!(
            view.rows,
            vec![
                TaskListRow::Task { task_index: 0 },
                TaskListRow::EpicChild {
                    parent_index: 0,
                    task_index: 1,
                    last: false,
                },
                TaskListRow::EpicChild {
                    parent_index: 0,
                    task_index: 2,
                    last: true,
                },
            ]
        );
    }

    #[test]
    fn expanded_epic_skips_missing_child_tasks() {
        let parent = make_epic_parent("parent", "parent-1", &["missing-child"], true);
        let mut expanded = BTreeSet::new();
        expanded.insert(crate::test_support::task_id("parent-1"));

        let tasks = vec![parent];

        let view = TaskListView::from_tasks(TaskListRenderMode::Epics, &tasks, &expanded);

        assert_eq!(view.rows, vec![TaskListRow::Task { task_index: 0 }]);
    }

    #[test]
    fn epics_visual_row_finds_child_row() {
        let child = make_task("child", "child-1");
        let parent = make_epic_parent("parent", "parent-1", &["child-1"], true);
        let mut expanded = BTreeSet::new();
        expanded.insert(crate::test_support::task_id("parent-1"));

        let tasks = vec![parent, child];

        let view = TaskListView::from_tasks(TaskListRenderMode::Epics, &tasks, &expanded);

        assert_eq!(view.visual_row(0), 0);
        assert_eq!(view.visual_row(1), 1);
    }
}
