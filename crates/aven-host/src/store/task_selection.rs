use std::collections::BTreeSet;

use crate::ids::TaskId;
use crate::query::TaskListItem;

#[derive(Debug, Clone)]
pub struct TaskSelection {
    targets: Vec<TaskListItem>,
    anchor: TaskSelectionAnchor,
    uses_marks: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TaskSelectionAnchor {
    task_id: TaskId,
    index: usize,
}

impl PartialEq for TaskSelection {
    fn eq(&self, other: &Self) -> bool {
        self.ids().eq(other.ids())
            && self.anchor == other.anchor
            && self.uses_marks == other.uses_marks
    }
}

impl Eq for TaskSelection {}

impl TaskSelection {
    pub fn resolve(
        tasks: &[TaskListItem],
        marked_task_ids: &BTreeSet<TaskId>,
        selected: Option<usize>,
    ) -> Option<Self> {
        let anchor_index = selected.filter(|index| *index < tasks.len());
        let marked_targets = tasks
            .iter()
            .filter(|item| marked_task_ids.contains(&item.task.id))
            .cloned()
            .collect::<Vec<_>>();
        let uses_marks = !marked_targets.is_empty();
        let targets = if uses_marks {
            marked_targets
        } else {
            vec![tasks.get(anchor_index?)?.clone()]
        };
        let anchor_index = anchor_index.or_else(|| {
            tasks
                .iter()
                .position(|item| item.task.id == targets[0].task.id)
        })?;

        Some(Self {
            targets,
            anchor: TaskSelectionAnchor {
                task_id: tasks[anchor_index].task.id.clone(),
                index: anchor_index,
            },
            uses_marks,
        })
    }

    pub fn resolve_single(tasks: &[TaskListItem], selected: Option<usize>) -> Option<Self> {
        let index = selected.filter(|index| *index < tasks.len())?;
        Some(Self {
            targets: vec![tasks[index].clone()],
            anchor: TaskSelectionAnchor {
                task_id: tasks[index].task.id.clone(),
                index,
            },
            uses_marks: false,
        })
    }

    pub fn for_detail_target(
        target: TaskListItem,
        anchor: &TaskListItem,
        anchor_index: usize,
    ) -> Self {
        Self {
            targets: vec![target],
            anchor: TaskSelectionAnchor {
                task_id: anchor.task.id.clone(),
                index: anchor_index,
            },
            uses_marks: false,
        }
    }

    pub fn from_captured(
        targets: Vec<TaskListItem>,
        anchor: &TaskListItem,
        anchor_index: usize,
    ) -> Option<Self> {
        Self::from_captured_with_marks(targets, anchor, anchor_index, false)
    }

    pub fn from_captured_with_marks(
        targets: Vec<TaskListItem>,
        anchor: &TaskListItem,
        anchor_index: usize,
        uses_marks: bool,
    ) -> Option<Self> {
        (!targets.is_empty()).then(|| Self {
            targets,
            anchor: TaskSelectionAnchor {
                task_id: anchor.task.id.clone(),
                index: anchor_index,
            },
            uses_marks,
        })
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn from_ids(
        tasks: &[TaskListItem],
        task_ids: &[TaskId],
        selected: Option<usize>,
    ) -> Option<Self> {
        let ids = task_ids.iter().collect::<BTreeSet<_>>();
        let targets = tasks
            .iter()
            .filter(|item| ids.contains(&item.task.id))
            .cloned()
            .collect::<Vec<_>>();
        let first = targets.first()?;
        let anchor_index = selected
            .filter(|index| *index < tasks.len())
            .or_else(|| tasks.iter().position(|item| item.task.id == first.task.id))?;
        Some(Self {
            targets,
            anchor: TaskSelectionAnchor {
                task_id: tasks[anchor_index].task.id.clone(),
                index: anchor_index,
            },
            uses_marks: false,
        })
    }

    pub fn len(&self) -> usize {
        self.targets.len()
    }

    pub fn is_single(&self) -> bool {
        self.targets.len() == 1
    }

    pub fn uses_marks(&self) -> bool {
        self.uses_marks
    }

    pub fn targets(&self) -> &[TaskListItem] {
        &self.targets
    }

    pub fn ids(&self) -> impl Iterator<Item = &TaskId> {
        self.targets.iter().map(|item| &item.task.id)
    }

    pub fn single_id(&self) -> Option<&TaskId> {
        self.is_single().then(|| &self.targets[0].task.id)
    }

    pub fn anchor_id(&self) -> &TaskId {
        &self.anchor.task_id
    }

    pub fn anchor_index(&self) -> usize {
        self.anchor.index
    }
}
