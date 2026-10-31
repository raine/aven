use std::collections::{BTreeSet, HashMap};

use chrono::{Datelike, Local, TimeZone};

use super::TaskListRenderMode;
use crate::query::TaskListItem;
use crate::queue::unix_seconds;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskGroupRow {
    pub(crate) label: String,
    pub(crate) count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TaskListRow {
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

#[derive(Debug)]
pub(crate) struct TaskListView {
    pub(crate) rows: Vec<TaskListRow>,
    pub(crate) render_mode: TaskListRenderMode,
    visual_rows: Vec<Option<usize>>,
}

impl TaskListView {
    pub(crate) fn from_tasks(
        render_mode: TaskListRenderMode,
        tasks: &[TaskListItem],
        expanded_epic_ids: &BTreeSet<crate::ids::TaskId>,
    ) -> Self {
        Self::from_tasks_at(
            render_mode,
            tasks,
            expanded_epic_ids,
            crate::queue::now_seconds(),
        )
    }

    pub(crate) fn from_tasks_at(
        render_mode: TaskListRenderMode,
        tasks: &[TaskListItem],
        expanded_epic_ids: &BTreeSet<crate::ids::TaskId>,
        now_seconds: i64,
    ) -> Self {
        let rows = match render_mode {
            TaskListRenderMode::Queue => queue_rows(tasks),
            TaskListRenderMode::Upcoming => upcoming_rows(tasks, now_seconds),
            TaskListRenderMode::Flat => task_rows(tasks),
            TaskListRenderMode::Epics => epics_rows(tasks, expanded_epic_ids),
        };
        let mut visual_rows = vec![None; tasks.len()];
        for (visual_row, row) in rows.iter().enumerate() {
            let task_index = match row {
                TaskListRow::EpicChild { task_index, .. } | TaskListRow::Task { task_index } => {
                    *task_index
                }
                TaskListRow::Group(_) => continue,
            };
            if let Some(slot) = visual_rows.get_mut(task_index) {
                slot.get_or_insert(visual_row);
            }
        }
        Self {
            rows,
            render_mode,
            visual_rows,
        }
    }

    pub(crate) fn visual_row(&self, selected_task: usize) -> usize {
        self.visual_row_for(selected_task).unwrap_or(0)
    }

    pub(crate) fn visual_row_for(&self, selected_task: usize) -> Option<usize> {
        self.visual_rows.get(selected_task).copied().flatten()
    }

    pub(crate) fn row_count(&self) -> usize {
        self.rows.len()
    }

    #[cfg(test)]
    pub(crate) fn group_labels(&self) -> Vec<&str> {
        self.rows
            .iter()
            .filter_map(|row| match row {
                TaskListRow::Group(group) => Some(group.label.as_str()),
                TaskListRow::Task { .. } | TaskListRow::EpicChild { .. } => None,
            })
            .collect()
    }

    pub(crate) fn task_index_at_visual_row(&self, visual_row: usize) -> Option<usize> {
        match self.rows.get(visual_row)? {
            TaskListRow::EpicChild { task_index, .. } | TaskListRow::Task { task_index } => {
                Some(*task_index)
            }
            TaskListRow::Group(_) => None,
        }
    }
}

pub(crate) fn epics_rows(
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

pub(crate) fn task_rows(tasks: &[TaskListItem]) -> Vec<TaskListRow> {
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

pub(crate) fn queue_rows(tasks: &[TaskListItem]) -> Vec<TaskListRow> {
    grouped_task_rows_by(tasks, queue_group_label)
}

pub(crate) fn queue_group_label(item: &TaskListItem) -> String {
    match item.task.status.as_str() {
        "done" => "done".to_string(),
        "canceled" => "canceled".to_string(),
        _ => item.queue.band.label().to_string(),
    }
}

pub(crate) fn upcoming_rows(tasks: &[TaskListItem], now_seconds: i64) -> Vec<TaskListRow> {
    grouped_task_rows_by(tasks, |item| {
        available_day_label(item.task.available_at.as_deref().unwrap_or(""), now_seconds)
    })
}

pub(crate) fn available_day_label(available_at: &str, now_seconds: i64) -> String {
    let Some(available_seconds) = unix_seconds(available_at) else {
        return "later".to_string();
    };
    let Some(available_date) = local_date(available_seconds) else {
        return "later".to_string();
    };
    let Some(today) = local_date(now_seconds) else {
        return "later".to_string();
    };
    let days = (available_date - today).num_days();
    match days {
        i64::MIN..=-1 => "ready".to_string(),
        0 => "today".to_string(),
        1 => "tomorrow".to_string(),
        2..=6 => available_date.format("%A").to_string().to_lowercase(),
        _ if available_date.year() == today.year() => available_date.format("%b %-d").to_string(),
        _ => available_date.format("%b %-d, %Y").to_string(),
    }
}

pub(crate) fn local_date(seconds: i64) -> Option<chrono::NaiveDate> {
    Some(Local.timestamp_opt(seconds, 0).single()?.date_naive())
}
