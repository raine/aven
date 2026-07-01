use std::collections::BTreeSet;

use crate::query::TaskListItem;
use crate::tui::store::{TaskListRenderMode, TuiStore};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TaskGroupRow {
    pub(super) label: &'static str,
    pub(super) count: usize,
}

#[derive(Debug, PartialEq, Eq)]
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

pub(super) struct TaskListView {
    pub(super) rows: Vec<TaskListRow>,
    pub(super) render_mode: TaskListRenderMode,
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
        expanded_epic_ids: &BTreeSet<String>,
    ) -> Self {
        let rows = match render_mode {
            TaskListRenderMode::Queue => queue_rows(tasks),
            TaskListRenderMode::Flat => task_rows(tasks),
            TaskListRenderMode::Epics => epics_rows(tasks, expanded_epic_ids),
        };
        Self { rows, render_mode }
    }

    pub(super) fn visual_row(&self, selected_task: usize) -> usize {
        self.rows
            .iter()
            .position(|row| match row {
                TaskListRow::EpicChild { task_index, .. } | TaskListRow::Task { task_index } => {
                    *task_index == selected_task
                }
                _ => false,
            })
            .unwrap_or(0)
    }
}

pub(super) fn epics_rows(
    tasks: &[TaskListItem],
    expanded_epic_ids: &BTreeSet<String>,
) -> Vec<TaskListRow> {
    let mut rows = Vec::new();
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
                .filter(|link| link.unresolved)
                .filter_map(|link| tasks.iter().position(|t| t.task.id == link.task_id))
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

pub(super) fn queue_rows(tasks: &[TaskListItem]) -> Vec<TaskListRow> {
    let mut rows = Vec::new();
    let mut index = 0;
    while index < tasks.len() {
        let label = queue_group_label(&tasks[index]);
        let start = index;
        while index < tasks.len() && queue_group_label(&tasks[index]) == label {
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

pub(super) fn queue_group_label(item: &TaskListItem) -> &'static str {
    match item.task.status.as_str() {
        "done" => "done",
        "canceled" => "canceled",
        _ => item.queue.band.label(),
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
    row_count: usize,
    viewport_rows: usize,
) -> usize {
    if viewport_rows == 0 || row_count <= viewport_rows {
        return 0;
    }
    let max_scroll = row_count - viewport_rows;
    let scroll = current_scroll.min(max_scroll);
    if selected_row <= scroll {
        selected_row
    } else if selected_row >= scroll + viewport_rows {
        selected_row.saturating_sub(viewport_rows.saturating_sub(1))
    } else {
        scroll
    }
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
    use crate::choices::{TaskPriority, TaskStatus};
    use crate::queue::QueueBand;
    use std::collections::BTreeSet;

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
                is_epic: false,
            },
            display_ref: "APP-1".to_string(),
            labels: Vec::new(),
            notes: Vec::new(),
            has_conflict: false,
            unresolved_blocker_count: 0,
            dependent_count: 0,
            depends_on: Vec::new(),
            blocks: Vec::new(),
            epic_children: Vec::new(),
            epic_parent: None,
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

    #[test]
    fn project_filtered_queue_view_groups_by_queue_band() {
        let tasks = vec![
            task_item_with("todo high", "todo", QueueBand::Focus),
            task_item_with("inbox", "inbox", QueueBand::Triage),
            task_item_with("todo medium", "todo", QueueBand::Triage),
            task_item_with("backlog", "backlog", QueueBand::Later),
        ];

        let view = TaskListView::from_tasks(TaskListRenderMode::Queue, &tasks, &BTreeSet::new());

        assert_eq!(view.render_mode, TaskListRenderMode::Queue);
        assert_eq!(
            view.rows,
            vec![
                TaskListRow::Group(TaskGroupRow {
                    label: "focus",
                    count: 1,
                }),
                TaskListRow::Task { task_index: 0 },
                TaskListRow::Group(TaskGroupRow {
                    label: "triage",
                    count: 2,
                }),
                TaskListRow::Task { task_index: 1 },
                TaskListRow::Task { task_index: 2 },
                TaskListRow::Group(TaskGroupRow {
                    label: "later",
                    count: 1,
                }),
                TaskListRow::Task { task_index: 3 },
            ]
        );
    }

    #[test]
    fn queue_view_groups_terminal_statuses_by_status() {
        let tasks = vec![
            task_item_with("backlog", "backlog", QueueBand::Later),
            task_item_with("finished", "done", QueueBand::Later),
            task_item_with("canceled", "canceled", QueueBand::Later),
        ];

        let view = TaskListView::from_tasks(TaskListRenderMode::Queue, &tasks, &BTreeSet::new());

        assert_eq!(
            view.rows,
            vec![
                TaskListRow::Group(TaskGroupRow {
                    label: "later",
                    count: 1,
                }),
                TaskListRow::Task { task_index: 0 },
                TaskListRow::Group(TaskGroupRow {
                    label: "done",
                    count: 1,
                }),
                TaskListRow::Task { task_index: 1 },
                TaskListRow::Group(TaskGroupRow {
                    label: "canceled",
                    count: 1,
                }),
                TaskListRow::Task { task_index: 2 },
            ]
        );
    }

    #[test]
    fn non_queue_sort_does_not_emit_duplicate_status_groups() {
        let tasks = vec![
            task_item_with("todo 1", "todo", QueueBand::Focus),
            task_item_with("inbox", "inbox", QueueBand::Triage),
            task_item_with("todo 2", "todo", QueueBand::Later),
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
            task_item_with("todo high", "todo", QueueBand::Focus),
            task_item_with("inbox", "inbox", QueueBand::Triage),
            task_item_with("todo medium", "todo", QueueBand::Triage),
        ];
        let view = TaskListView::from_tasks(TaskListRenderMode::Queue, &tasks, &BTreeSet::new());

        assert_eq!(view.visual_row(0), 1);
        assert_eq!(view.visual_row(1), 3);
        assert_eq!(view.visual_row(2), 4);
    }

    #[test]
    fn queue_view_keeps_group_header_with_first_visible_task() {
        let tasks = vec![
            task_item_with("todo high", "todo", QueueBand::Focus),
            task_item_with("inbox", "inbox", QueueBand::Triage),
            task_item_with("todo medium", "todo", QueueBand::Triage),
        ];
        let view = TaskListView::from_tasks(TaskListRenderMode::Queue, &tasks, &BTreeSet::new());

        assert_eq!(
            task_list_visible_rows(&view, 1, 3),
            vec![
                (
                    0,
                    &TaskListRow::Group(TaskGroupRow {
                        label: "focus",
                        count: 1
                    })
                ),
                (1, &TaskListRow::Task { task_index: 0 }),
                (
                    2,
                    &TaskListRow::Group(TaskGroupRow {
                        label: "triage",
                        count: 2
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
                        label: "triage",
                        count: 2
                    })
                ),
                (3, &TaskListRow::Task { task_index: 1 }),
                (4, &TaskListRow::Task { task_index: 2 }),
            ]
        );
    }

    #[test]
    fn empty_task_view_has_no_rows() {
        let view = TaskListView::from_tasks(TaskListRenderMode::Queue, &[], &BTreeSet::new());

        assert!(view.rows.is_empty());
        assert_eq!(view.visual_row(0), 0);
    }

    #[test]
    fn upward_selection_from_bottom_keeps_scroll_at_bottom_until_top_edge() {
        assert_eq!(task_list_scroll(6, 9, 10, 4), 6);
        assert_eq!(task_list_scroll(6, 8, 10, 4), 6);
        assert_eq!(task_list_scroll(6, 7, 10, 4), 6);
        assert_eq!(task_list_scroll(6, 6, 10, 4), 6);
        assert_eq!(task_list_scroll(6, 5, 10, 4), 5);
    }

    #[test]
    fn returning_to_first_row_resets_scroll_to_top() {
        assert_eq!(task_list_scroll(1, 0, 10, 4), 0);
        assert_eq!(task_list_scroll(6, 6, 10, 4), 6);
    }

    #[test]
    fn downward_selection_scrolls_after_bottom_edge() {
        assert_eq!(task_list_scroll(0, 0, 10, 4), 0);
        assert_eq!(task_list_scroll(0, 1, 10, 4), 0);
        assert_eq!(task_list_scroll(0, 2, 10, 4), 0);
        assert_eq!(task_list_scroll(0, 3, 10, 4), 0);
        assert_eq!(task_list_scroll(0, 4, 10, 4), 1);
    }

    #[test]
    fn task_list_scroll_clamps_to_valid_rows() {
        assert_eq!(task_list_scroll(6, 2, 3, 4), 0);
        assert_eq!(task_list_scroll(8, 9, 10, 4), 6);
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
        let mut item = task_item(title);
        item.task.id = id.to_string();
        item
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
                task_id: child_id.to_string(),
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
        expanded.insert("parent-1".to_string());

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
    fn expanded_epic_skips_resolved_child() {
        let resolved_child = make_task("resolved", "child-1");
        let open_child = make_task("open", "child-2");
        let mut parent = make_epic_parent("parent", "parent-1", &["child-1", "child-2"], true);
        parent.epic_children[0].unresolved = false;
        let mut expanded = BTreeSet::new();
        expanded.insert("parent-1".to_string());

        let tasks = vec![parent, resolved_child, open_child];

        let view = TaskListView::from_tasks(TaskListRenderMode::Epics, &tasks, &expanded);

        assert_eq!(
            view.rows,
            vec![
                TaskListRow::Task { task_index: 0 },
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
        expanded.insert("parent-1".to_string());

        let tasks = vec![parent];

        let view = TaskListView::from_tasks(TaskListRenderMode::Epics, &tasks, &expanded);

        assert_eq!(view.rows, vec![TaskListRow::Task { task_index: 0 }]);
    }

    #[test]
    fn epics_visual_row_finds_child_row() {
        let child = make_task("child", "child-1");
        let parent = make_epic_parent("parent", "parent-1", &["child-1"], true);
        let mut expanded = BTreeSet::new();
        expanded.insert("parent-1".to_string());

        let tasks = vec![parent, child];

        let view = TaskListView::from_tasks(TaskListRenderMode::Epics, &tasks, &expanded);

        assert_eq!(view.visual_row(0), 0);
        assert_eq!(view.visual_row(1), 1);
    }
}
