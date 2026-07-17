use std::cmp::Ordering;

use crate::choices::{TaskPriority, TaskStatus};
use crate::config::TaskColumnConfig;
use crate::query::TaskListItem;

#[derive(Debug)]
pub(crate) struct TaskColumn<'a> {
    pub(crate) config: &'a TaskColumnConfig,
    pub(crate) task_indices: Vec<usize>,
}

#[derive(Debug)]
pub(crate) struct ColumnBoard<'a> {
    pub(crate) columns: Vec<TaskColumn<'a>>,
}

#[derive(Clone, Copy)]
enum LaneSort {
    Oldest,
    PriorityThenOldest,
    StaleThenPriority,
    RecentTerminal,
    Preserve,
}

fn sort_lane(config: &TaskColumnConfig, tasks: &[TaskListItem], task_indices: &mut [usize]) {
    let sort = lane_sort(config);
    task_indices.sort_by(|left, right| {
        if matches!(sort, LaneSort::Preserve) {
            return Ordering::Equal;
        }
        let left = &tasks[*left].task;
        let right = &tasks[*right].task;
        let ordering = match sort {
            LaneSort::Oldest => left.created_at.cmp(&right.created_at),
            LaneSort::PriorityThenOldest => priority_cmp(left.priority, right.priority)
                .then_with(|| left.created_at.cmp(&right.created_at)),
            LaneSort::StaleThenPriority => left
                .queue_activity_at
                .cmp(&right.queue_activity_at)
                .then_with(|| priority_cmp(left.priority, right.priority)),
            LaneSort::RecentTerminal => right
                .queue_activity_at
                .cmp(&left.queue_activity_at)
                .then_with(|| right.updated_at.cmp(&left.updated_at)),
            LaneSort::Preserve => Ordering::Equal,
        };
        ordering.then_with(|| left.id.cmp(&right.id))
    });
}

fn lane_sort(config: &TaskColumnConfig) -> LaneSort {
    let statuses = config
        .statuses
        .iter()
        .filter_map(|status| TaskStatus::parse(status).ok())
        .collect::<Vec<_>>();
    if statuses.iter().all(|status| *status == TaskStatus::Inbox) {
        LaneSort::Oldest
    } else if statuses
        .iter()
        .all(|status| matches!(status, TaskStatus::Backlog | TaskStatus::Todo))
    {
        LaneSort::PriorityThenOldest
    } else if statuses.iter().all(|status| *status == TaskStatus::Active) {
        LaneSort::StaleThenPriority
    } else if statuses.iter().all(|status| status.is_terminal()) {
        LaneSort::RecentTerminal
    } else {
        LaneSort::Preserve
    }
}

fn priority_cmp(left: TaskPriority, right: TaskPriority) -> Ordering {
    priority_rank(right).cmp(&priority_rank(left))
}

fn priority_rank(priority: TaskPriority) -> u8 {
    match priority {
        TaskPriority::None => 0,
        TaskPriority::Low => 1,
        TaskPriority::Medium => 2,
        TaskPriority::High => 3,
        TaskPriority::Urgent => 4,
    }
}

pub(crate) fn lane_index_for_status(
    columns: &[TaskColumnConfig],
    status: TaskStatus,
) -> Option<usize> {
    columns.iter().position(|column| {
        column
            .statuses
            .iter()
            .any(|candidate| candidate == status.as_str())
    })
}

pub(crate) fn lane_entry_status(
    columns: &[TaskColumnConfig],
    column_index: usize,
) -> Option<TaskStatus> {
    columns
        .get(column_index)?
        .statuses
        .first()
        .and_then(|status| TaskStatus::parse(status).ok())
}

pub(crate) fn adjacent_lane_entry_status(
    columns: &[TaskColumnConfig],
    status: TaskStatus,
    delta: isize,
) -> Option<TaskStatus> {
    if delta == 0 {
        return None;
    }
    let source = lane_index_for_status(columns, status)?;
    let target = source.checked_add_signed(delta.signum())?;
    lane_entry_status(columns, target)
}

impl<'a> ColumnBoard<'a> {
    pub(crate) fn new(columns: &'a [TaskColumnConfig], tasks: &[TaskListItem]) -> Self {
        let columns = columns
            .iter()
            .map(|config| {
                let mut task_indices = tasks
                    .iter()
                    .enumerate()
                    .filter(|(_, item)| {
                        config
                            .statuses
                            .iter()
                            .any(|status| status == item.task.status.as_str())
                    })
                    .map(|(index, _)| index)
                    .collect::<Vec<_>>();
                sort_lane(config, tasks, &mut task_indices);
                TaskColumn {
                    config,
                    task_indices,
                }
            })
            .collect();
        Self { columns }
    }

    pub(crate) fn position(&self, task_index: usize) -> Option<(usize, usize)> {
        self.columns.iter().enumerate().find_map(|(column, lane)| {
            lane.task_indices
                .iter()
                .position(|index| *index == task_index)
                .map(|row| (column, row))
        })
    }

    pub(crate) fn move_vertical(&self, selected: Option<usize>, delta: isize) -> Option<usize> {
        let Some(selected) = selected else {
            return if delta < 0 { self.last() } else { self.first() };
        };
        let (column, row) = self.position(selected)?;
        let tasks = &self.columns[column].task_indices;
        let next = if delta < 0 {
            row.checked_sub(delta.unsigned_abs() % tasks.len())
                .unwrap_or_else(|| tasks.len() - (delta.unsigned_abs() - row) % tasks.len())
                % tasks.len()
        } else {
            (row + delta.unsigned_abs()) % tasks.len()
        };
        tasks.get(next).copied()
    }

    pub(crate) fn move_vertical_bounded(
        &self,
        selected: Option<usize>,
        delta: isize,
    ) -> Option<usize> {
        let Some(selected) = selected else {
            return if delta < 0 { self.last() } else { self.first() };
        };
        let (column, row) = self.position(selected)?;
        let tasks = &self.columns[column].task_indices;
        let next = if delta < 0 {
            row.saturating_sub(delta.unsigned_abs())
        } else {
            row.saturating_add(delta.unsigned_abs())
                .min(tasks.len() - 1)
        };
        tasks.get(next).copied()
    }

    pub(crate) fn move_horizontal(&self, selected: Option<usize>, delta: isize) -> Option<usize> {
        let selected = selected.or_else(|| self.first())?;
        let (column, row) = self.position(selected)?;
        let mut target = column as isize + delta.signum();
        while target >= 0 && (target as usize) < self.columns.len() {
            let tasks = &self.columns[target as usize].task_indices;
            if !tasks.is_empty() {
                return tasks.get(row.min(tasks.len() - 1)).copied();
            }
            target += delta.signum();
        }
        None
    }

    pub(crate) fn edge(&self, selected: Option<usize>, last: bool) -> Option<usize> {
        let Some(selected) = selected else {
            return if last { self.last() } else { self.first() };
        };
        let (column, _) = self.position(selected)?;
        if last {
            self.columns[column].task_indices.last().copied()
        } else {
            self.columns[column].task_indices.first().copied()
        }
    }

    pub(crate) fn first(&self) -> Option<usize> {
        self.columns
            .iter()
            .find_map(|column| column.task_indices.first().copied())
    }

    pub(crate) fn last(&self) -> Option<usize> {
        self.columns
            .iter()
            .rev()
            .find_map(|column| column.task_indices.last().copied())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::choices::{TaskPriority, TaskStatus};

    fn item(id: usize, status: &str) -> TaskListItem {
        TaskListItem {
            task: crate::types::Task {
                id: id.to_string(),
                workspace_id: "0000000000000001".parse().unwrap(),
                title: format!("task {id}"),
                description: String::new(),
                project_id: "0000000000000001".parse().unwrap(),
                project_key: "app".into(),
                project_prefix: "APP".into(),
                status: TaskStatus::parse(status).unwrap(),
                priority: TaskPriority::None,
                created_at: String::new(),
                updated_at: String::new(),
                queue_activity_at: String::new(),
                available_at: String::new(),
                due_on: String::new(),
                deleted: false,
                is_epic: false,
            },
            display_ref: format!("APP-{id}"),
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

    fn item_with_sort(
        id: usize,
        status: &str,
        priority: TaskPriority,
        created_at: &str,
        activity_at: &str,
    ) -> TaskListItem {
        let mut item = item(id, status);
        item.task.priority = priority;
        item.task.created_at = created_at.into();
        item.task.updated_at = activity_at.into();
        item.task.queue_activity_at = activity_at.into();
        item
    }

    fn columns() -> Vec<TaskColumnConfig> {
        vec![
            TaskColumnConfig {
                name: "One".into(),
                statuses: vec!["inbox".into()],
            },
            TaskColumnConfig {
                name: "Empty".into(),
                statuses: vec!["backlog".into()],
            },
            TaskColumnConfig {
                name: "Two".into(),
                statuses: vec!["todo".into(), "active".into()],
            },
        ]
    }

    #[test]
    fn groups_in_configured_and_task_order() {
        let tasks = vec![item(0, "todo"), item(1, "inbox"), item(2, "active")];
        let config = columns();
        let board = ColumnBoard::new(&config, &tasks);

        assert_eq!(board.columns[0].task_indices, [1]);
        assert!(board.columns[1].task_indices.is_empty());
        assert_eq!(board.columns[2].task_indices, [0, 2]);
    }

    #[test]
    fn semantic_lanes_use_workflow_specific_sorting() {
        let tasks = vec![
            item_with_sort(0, "inbox", TaskPriority::Urgent, "2026-02-01", "2026-02-01"),
            item_with_sort(1, "inbox", TaskPriority::Low, "2026-01-01", "2026-01-01"),
            item_with_sort(2, "todo", TaskPriority::Low, "2026-01-01", "2026-01-01"),
            item_with_sort(3, "todo", TaskPriority::Urgent, "2026-02-01", "2026-02-01"),
            item_with_sort(4, "active", TaskPriority::Low, "2026-01-01", "2026-02-01"),
            item_with_sort(
                5,
                "active",
                TaskPriority::Urgent,
                "2026-02-01",
                "2026-01-01",
            ),
            item_with_sort(6, "done", TaskPriority::None, "2026-01-01", "2026-01-01"),
            item_with_sort(
                7,
                "canceled",
                TaskPriority::None,
                "2026-01-01",
                "2026-02-01",
            ),
        ];
        let config = vec![
            TaskColumnConfig {
                name: "Inbox".into(),
                statuses: vec!["inbox".into()],
            },
            TaskColumnConfig {
                name: "Ready".into(),
                statuses: vec!["backlog".into(), "todo".into()],
            },
            TaskColumnConfig {
                name: "In progress".into(),
                statuses: vec!["active".into()],
            },
            TaskColumnConfig {
                name: "Closed".into(),
                statuses: vec!["done".into(), "canceled".into()],
            },
        ];

        let board = ColumnBoard::new(&config, &tasks);

        assert_eq!(board.columns[0].task_indices, [1, 0]);
        assert_eq!(board.columns[1].task_indices, [3, 2]);
        assert_eq!(board.columns[2].task_indices, [5, 4]);
        assert_eq!(board.columns[3].task_indices, [7, 6]);
    }

    #[test]
    fn mixed_policy_custom_lane_preserves_query_order() {
        let tasks = vec![item(0, "todo"), item(1, "active"), item(2, "todo")];
        let config = vec![TaskColumnConfig {
            name: "Work".into(),
            statuses: vec!["todo".into(), "active".into()],
        }];

        let board = ColumnBoard::new(&config, &tasks);

        assert_eq!(board.columns[0].task_indices, [0, 1, 2]);
    }

    #[test]
    fn lane_moves_follow_config_order_and_first_status() {
        let config = vec![
            TaskColumnConfig {
                name: "Closed".into(),
                statuses: vec!["done".into(), "canceled".into()],
            },
            TaskColumnConfig {
                name: "Work".into(),
                statuses: vec!["todo".into(), "active".into()],
            },
            TaskColumnConfig {
                name: "Inbox".into(),
                statuses: vec!["inbox".into()],
            },
        ];

        assert_eq!(
            lane_index_for_status(&config, TaskStatus::Canceled),
            Some(0)
        );
        assert_eq!(lane_entry_status(&config, 0), Some(TaskStatus::Done));
        assert_eq!(lane_entry_status(&config, 1), Some(TaskStatus::Todo));
        assert_eq!(
            adjacent_lane_entry_status(&config, TaskStatus::Active, -1),
            Some(TaskStatus::Done)
        );
        assert_eq!(
            adjacent_lane_entry_status(&config, TaskStatus::Active, 1),
            Some(TaskStatus::Inbox)
        );
        assert_eq!(
            adjacent_lane_entry_status(&config, TaskStatus::Done, -1),
            None
        );
        assert_eq!(
            adjacent_lane_entry_status(&config, TaskStatus::Inbox, 1),
            None
        );
        assert_eq!(
            adjacent_lane_entry_status(&config, TaskStatus::Todo, 0),
            None
        );
    }

    #[test]
    fn navigation_skips_empty_columns_and_clamps_rows() {
        let tasks = vec![item(0, "inbox"), item(1, "inbox"), item(2, "todo")];
        let config = columns();
        let board = ColumnBoard::new(&config, &tasks);

        assert_eq!(board.move_horizontal(Some(1), 1), Some(2));
        assert_eq!(board.move_horizontal(Some(2), -1), Some(0));
        assert_eq!(board.move_horizontal(Some(2), 1), None);
        assert_eq!(board.move_vertical(Some(0), -1), Some(1));
        assert_eq!(board.move_vertical(Some(1), 1), Some(0));
        assert_eq!(board.move_vertical_bounded(Some(0), -1), Some(0));
        assert_eq!(board.move_vertical_bounded(Some(1), 1), Some(1));
        assert_eq!(board.move_vertical_bounded(Some(0), 1), Some(1));
    }

    #[test]
    fn edges_and_missing_selection_use_non_empty_lanes() {
        let tasks = vec![item(0, "inbox"), item(1, "inbox"), item(2, "todo")];
        let config = columns();
        let board = ColumnBoard::new(&config, &tasks);

        assert_eq!(board.move_vertical(None, 1), Some(0));
        assert_eq!(board.move_vertical(None, -1), Some(2));
        assert_eq!(board.edge(Some(1), false), Some(0));
        assert_eq!(board.edge(Some(0), true), Some(1));
        assert_eq!(board.edge(None, true), Some(2));
    }

    #[test]
    fn empty_board_has_no_selection() {
        let config = columns();
        let board = ColumnBoard::new(&config, &[]);
        assert_eq!(board.move_vertical(None, 1), None);
        assert_eq!(board.move_horizontal(None, 1), None);
        assert_eq!(board.edge(None, false), None);
    }
}
