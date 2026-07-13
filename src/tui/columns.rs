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

impl<'a> ColumnBoard<'a> {
    pub(crate) fn new(columns: &'a [TaskColumnConfig], tasks: &[TaskListItem]) -> Self {
        let columns = columns
            .iter()
            .map(|config| TaskColumn {
                config,
                task_indices: tasks
                    .iter()
                    .enumerate()
                    .filter(|(_, item)| {
                        config
                            .statuses
                            .iter()
                            .any(|status| status == item.task.status.as_str())
                    })
                    .map(|(index, _)| index)
                    .collect(),
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
                workspace_id: "workspace".into(),
                title: format!("task {id}"),
                description: String::new(),
                project_id: "project".into(),
                project_key: "app".into(),
                project_prefix: "APP".into(),
                status: TaskStatus::parse(status).unwrap(),
                priority: TaskPriority::None,
                created_at: String::new(),
                updated_at: String::new(),
                queue_activity_at: String::new(),
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
