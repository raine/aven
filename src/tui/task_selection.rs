use std::collections::BTreeSet;

use crate::ids::TaskId;
use crate::query::TaskListItem;

#[derive(Debug, Clone)]
pub(crate) struct TaskSelection {
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
    pub(crate) fn resolve(
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

    #[cfg(test)]
    pub(crate) fn from_ids(
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

    pub(crate) fn len(&self) -> usize {
        self.targets.len()
    }

    pub(crate) fn is_single(&self) -> bool {
        self.targets.len() == 1
    }

    pub(crate) fn uses_marks(&self) -> bool {
        self.uses_marks
    }

    pub(crate) fn targets(&self) -> &[TaskListItem] {
        &self.targets
    }

    pub(crate) fn ids(&self) -> impl Iterator<Item = &TaskId> {
        self.targets.iter().map(|item| &item.task.id)
    }

    pub(crate) fn single_id(&self) -> Option<&TaskId> {
        self.is_single().then(|| &self.targets[0].task.id)
    }

    pub(crate) fn anchor_id(&self) -> &TaskId {
        &self.anchor.task_id
    }

    pub(crate) fn anchor_index(&self) -> usize {
        self.anchor.index
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::TaskSelection;
    use crate::tui::test_support::task_list_item_with_id;

    #[test]
    fn resolves_visible_marks_before_selected_row() {
        let tasks = vec![
            task_list_item_with_id("first", "task-1"),
            task_list_item_with_id("second", "task-2"),
            task_list_item_with_id("third", "task-3"),
        ];
        let marked = BTreeSet::from([tasks[0].task.id.clone(), tasks[2].task.id.clone()]);

        let selection = TaskSelection::resolve(&tasks, &marked, Some(1)).unwrap();

        assert_eq!(selection.len(), 2);
        assert_eq!(
            selection.ids().cloned().collect::<Vec<_>>(),
            vec![tasks[0].task.id.clone(), tasks[2].task.id.clone()]
        );
        assert_eq!(selection.anchor_id(), &tasks[1].task.id);
        assert_eq!(selection.anchor_index(), 1);
    }

    #[test]
    fn ignores_hidden_marks_and_requires_a_visible_target() {
        let tasks = vec![task_list_item_with_id("visible", "task-visible")];
        let hidden = task_list_item_with_id("hidden", "task-hidden").task.id;
        let marked = BTreeSet::from([hidden]);

        let selection = TaskSelection::resolve(&tasks, &marked, Some(0)).unwrap();
        assert_eq!(selection.single_id(), Some(&tasks[0].task.id));
        assert!(TaskSelection::resolve(&tasks, &marked, None).is_none());
        assert!(TaskSelection::resolve(&[], &marked, Some(0)).is_none());
    }
}
