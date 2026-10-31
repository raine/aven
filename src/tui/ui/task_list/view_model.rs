#[cfg(test)]
use crate::query::TaskListItem;
use ratatui::widgets::TableState;

#[cfg(test)]
use crate::tui::store::TaskListRenderMode;
pub(crate) use crate::tui::store::{TaskGroupRow, TaskListRow, TaskListView};

pub(super) struct TaskListProjection<'a> {
    pub(super) view: &'a TaskListView,
    pub(super) selected_task: Option<usize>,
    pub(super) scroll: usize,
    pub(super) viewport_rows: usize,
}

impl<'a> TaskListProjection<'a> {
    pub(super) fn from_view(
        view: &'a TaskListView,
        offset: usize,
        selected_task: Option<usize>,
        viewport_rows: usize,
    ) -> Self {
        let selected_row = selected_task
            .map(|selected| view.visual_row(selected))
            .unwrap_or(0);
        let scroll = task_list_scroll(offset, selected_row, view, viewport_rows);
        Self {
            view,
            selected_task,
            scroll,
            viewport_rows,
        }
    }

    pub(super) fn from_table_state(
        view: &'a TaskListView,
        table_state: &TableState,
        viewport_rows: usize,
    ) -> Self {
        Self::from_view(
            view,
            table_state.offset(),
            table_state.selected(),
            viewport_rows,
        )
    }

    pub(super) fn visible_rows(&self) -> Vec<(usize, &TaskListRow)> {
        task_list_visible_rows(self.view, self.scroll, self.viewport_rows)
    }

    pub(super) fn row_count(&self) -> usize {
        self.view.rows.len()
    }

    pub(super) fn top_scroll(&self) -> usize {
        task_list_top_scroll(self.view)
    }

    pub(super) fn commit_scroll(&self, table_state: &mut TableState) {
        *table_state.offset_mut() = self.scroll;
    }
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
            task_list_item_with_status_and_queue("epic", "todo", QueueBand::Epics),
            task_list_item_with_status_and_queue("finished", "done", QueueBand::Later),
            task_list_item_with_status_and_queue("canceled", "canceled", QueueBand::Later),
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
                TaskListRow::Group(TaskGroupRow {
                    label: "epics".to_string(),
                    count: 1
                }),
                TaskListRow::Task { task_index: 4 },
                TaskListRow::Group(TaskGroupRow {
                    label: "done".to_string(),
                    count: 1
                }),
                TaskListRow::Task { task_index: 5 },
                TaskListRow::Group(TaskGroupRow {
                    label: "canceled".to_string(),
                    count: 1
                }),
                TaskListRow::Task { task_index: 6 },
            ]
        );
        for index in 0..tasks.len() {
            assert_eq!(view.visual_row(index), index * 2 + 1);
            assert_eq!(view.visual_row_for(index), Some(index * 2 + 1));
            assert_eq!(view.task_index_at_visual_row(index * 2 + 1), Some(index));
            assert_eq!(view.task_index_at_visual_row(index * 2), None);
        }
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
                project_key: "app".to_string(),
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
    fn expanded_epic_includes_resolved_children() {
        for count in [1, 2] {
            let mut parent = make_epic_parent("parent", "parent-1", &["child-1", "child-2"], true);
            parent.epic_children.truncate(count);
            if count == 2 {
                parent.epic_children[0].unresolved = false;
            }
            let mut tasks = vec![
                parent,
                make_task("first", "child-1"),
                make_task("second", "child-2"),
            ];
            tasks.truncate(count + 1);
            let expanded = BTreeSet::from([crate::test_support::task_id("parent-1")]);
            let view = TaskListView::from_tasks(TaskListRenderMode::Epics, &tasks, &expanded);

            let mut expected = vec![TaskListRow::Task { task_index: 0 }];
            for (task_index, last) in if count == 1 {
                vec![(1, true)]
            } else {
                vec![(1, false), (2, true)]
            } {
                expected.push(TaskListRow::EpicChild {
                    parent_index: 0,
                    task_index,
                    last,
                });
            }
            assert_eq!(view.rows, expected);
            for index in 0..tasks.len() {
                assert_eq!(view.visual_row(index), index);
                assert_eq!(view.visual_row_for(index), Some(index));
                assert_eq!(view.task_index_at_visual_row(index), Some(index));
            }
        }
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
}
