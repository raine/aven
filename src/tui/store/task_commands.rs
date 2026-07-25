use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use aven_core::operations::{TaskMutationReport, TaskUpdate};

use crate::choices::{TaskPriority, TaskStatus};
use crate::tui::store::MutationMessage;
use crate::tui::task_selection::TaskSelection;
use crate::undo::UndoContext;

use super::TuiStore;

#[derive(Clone, Copy)]
pub(crate) enum PriorityMutation {
    Cycle { reverse: bool },
    Set(TaskPriority),
}

#[derive(Clone, Copy)]
pub(crate) enum TaskDateField {
    Availability,
    Due,
}

#[derive(Clone, Copy)]
pub(crate) enum TaskTextField {
    Title,
    Description,
}

fn task_noun(count: usize) -> &'static str {
    if count == 1 { "task" } else { "tasks" }
}

fn cycled_priority(priority: TaskPriority, reverse: bool) -> TaskPriority {
    let index = TaskPriority::ALL
        .iter()
        .position(|candidate| *candidate == priority)
        .unwrap_or(0);
    let next = if reverse {
        (index + TaskPriority::ALL.len() - 1) % TaskPriority::ALL.len()
    } else {
        (index + 1) % TaskPriority::ALL.len()
    };
    TaskPriority::ALL[next]
}

impl TuiStore {
    pub(crate) async fn mutate_status_selection(
        &mut self,
        selection: &TaskSelection,
        status: TaskStatus,
        preserve_task: bool,
    ) -> Result<MutationMessage> {
        let updates = selection
            .ids()
            .cloned()
            .map(|task_id| {
                (
                    task_id,
                    TaskUpdate {
                        status: Some(status.to_string()),
                        ..TaskUpdate::default()
                    },
                )
            })
            .collect();
        let report = self
            .database
            .mutate_tasks(
                &self.active_workspace,
                updates,
                UndoContext::tui(format!("status {} tasks", selection.len())),
            )
            .await?;
        let changed = report.changed_count();
        let message = if selection.is_single() {
            format!("set {} status={status}", selection.targets()[0].display_ref)
        } else if changed == 0 {
            format!("status unchanged on {} tasks", selection.len())
        } else {
            format!("set status on {changed} {}", task_noun(changed))
        };
        self.refresh_task_selection(selection, report, message, preserve_task, true)
            .await
    }

    pub(crate) async fn mutate_status_changes(
        &mut self,
        selection: &TaskSelection,
        changes: &[(crate::ids::TaskId, TaskStatus)],
    ) -> Result<MutationMessage> {
        let status_by_id = changes.iter().cloned().collect::<BTreeMap<_, _>>();
        let updates = selection
            .ids()
            .filter_map(|task_id| {
                let status = status_by_id.get(task_id)?;
                Some((
                    task_id.clone(),
                    TaskUpdate {
                        status: Some(status.to_string()),
                        ..TaskUpdate::default()
                    },
                ))
            })
            .collect();
        let report = self
            .database
            .mutate_tasks(
                &self.active_workspace,
                updates,
                UndoContext::tui(format!("move {} tasks between columns", selection.len())),
            )
            .await?;
        let changed = report.changed_count();
        let message = if changed == 0 {
            format!("column unchanged on {} tasks", selection.len())
        } else {
            format!("moved {changed} tasks between columns")
        };
        self.refresh_task_selection(selection, report, message, false, false)
            .await
    }

    pub(crate) async fn mutate_priority_selection(
        &mut self,
        selection: &TaskSelection,
        mutation: PriorityMutation,
    ) -> Result<MutationMessage> {
        let updates = selection
            .targets()
            .iter()
            .map(|item| {
                let priority = match mutation {
                    PriorityMutation::Cycle { reverse } => {
                        cycled_priority(item.task.priority, reverse)
                    }
                    PriorityMutation::Set(priority) => priority,
                };
                (
                    item.task.id.clone(),
                    TaskUpdate {
                        priority: Some(priority.to_string()),
                        ..TaskUpdate::default()
                    },
                )
            })
            .collect();
        let report = self
            .database
            .mutate_tasks(
                &self.active_workspace,
                updates,
                UndoContext::tui(format!("priority {} tasks", selection.len())),
            )
            .await?;
        let changed = report.changed_count();
        let message = if selection.is_single() {
            let priority = report.outcomes[0].task.priority;
            format!(
                "set {} priority={priority}",
                selection.targets()[0].display_ref
            )
        } else if changed == 0 {
            format!("priority unchanged on {} tasks", selection.len())
        } else {
            format!("set priority on {changed} {}", task_noun(changed))
        };
        self.refresh_task_selection(selection, report, message, false, false)
            .await
    }

    pub(crate) async fn mutate_text_selection(
        &mut self,
        selection: &TaskSelection,
        field: TaskTextField,
        value: String,
    ) -> Result<MutationMessage> {
        let item = &selection.targets()[0];
        let (update, field_name) = match field {
            TaskTextField::Title => (
                TaskUpdate {
                    title: Some(value),
                    ..TaskUpdate::default()
                },
                "title",
            ),
            TaskTextField::Description => (
                TaskUpdate {
                    description: Some(value),
                    ..TaskUpdate::default()
                },
                "description",
            ),
        };
        let report = self
            .database
            .mutate_tasks(
                &self.active_workspace,
                vec![(item.task.id.clone(), update)],
                UndoContext::tui(format!("{field_name} {}", item.display_ref)),
            )
            .await?;
        let verb = if report.changed_count() == 0 {
            "unchanged"
        } else {
            "set"
        };
        let message = format!("{verb} {} {field_name}", item.display_ref);
        self.refresh_task_selection(selection, report, message, false, false)
            .await
    }

    pub(crate) async fn mutate_date_selection(
        &mut self,
        selection: &TaskSelection,
        field: TaskDateField,
        value: Option<String>,
        preserve_task: bool,
    ) -> Result<MutationMessage> {
        let updates = selection
            .ids()
            .cloned()
            .map(|task_id| {
                let update = match field {
                    TaskDateField::Availability => TaskUpdate {
                        available_at: Some(value.clone()),
                        ..TaskUpdate::default()
                    },
                    TaskDateField::Due => TaskUpdate {
                        due_on: Some(value.clone()),
                        ..TaskUpdate::default()
                    },
                };
                (task_id, update)
            })
            .collect();
        let field_name = match field {
            TaskDateField::Availability => "availability",
            TaskDateField::Due => "due date",
        };
        let report = self
            .database
            .mutate_tasks(
                &self.active_workspace,
                updates,
                UndoContext::tui(format!("{field_name} {} tasks", selection.len())),
            )
            .await?;
        let changed = report.changed_count();
        let message = if selection.is_single() {
            let verb = match (changed, value.is_none()) {
                (0, _) => "unchanged",
                (_, true) => "cleared",
                (_, false) => "set",
            };
            format!("{verb} {} {field_name}", selection.targets()[0].display_ref)
        } else if changed == 0 {
            format!("{field_name} unchanged on {} tasks", selection.len())
        } else {
            let verb = if value.is_none() { "cleared" } else { "set" };
            format!("{verb} {field_name} on {changed} {}", task_noun(changed))
        };
        self.refresh_task_selection(selection, report, message, preserve_task, false)
            .await
    }

    pub(crate) async fn mutate_project_selection(
        &mut self,
        selection: &TaskSelection,
        project: String,
    ) -> Result<MutationMessage> {
        let updates = selection
            .ids()
            .cloned()
            .map(|task_id| {
                (
                    task_id,
                    TaskUpdate {
                        project: Some(project.clone()),
                        ..TaskUpdate::default()
                    },
                )
            })
            .collect();
        let report = self
            .database
            .mutate_tasks(
                &self.active_workspace,
                updates,
                UndoContext::tui(format!("project {} tasks", selection.len())),
            )
            .await?;
        let changed = report.changed_count();
        let message = if selection.is_single() {
            format!("set {} project", selection.targets()[0].display_ref)
        } else if changed == 0 {
            format!("project unchanged on {} tasks", selection.len())
        } else {
            format!("set project on {changed} {}", task_noun(changed))
        };
        self.refresh_task_selection(selection, report, message, false, false)
            .await
    }

    pub(crate) async fn mutate_labels_selection(
        &mut self,
        selection: &TaskSelection,
        selected_labels: Vec<String>,
        partial_labels: Vec<String>,
    ) -> Result<MutationMessage> {
        let selected = selected_labels.iter().collect::<BTreeSet<_>>();
        let partial = partial_labels.iter().collect::<BTreeSet<_>>();
        let updates = selection
            .targets()
            .iter()
            .map(|item| {
                let add_labels = selected_labels
                    .iter()
                    .filter(|label| !item.labels.contains(label))
                    .cloned()
                    .collect();
                let remove_labels = item
                    .labels
                    .iter()
                    .filter(|label| !selected.contains(label) && !partial.contains(label))
                    .cloned()
                    .collect();
                (
                    item.task.id.clone(),
                    TaskUpdate {
                        add_labels,
                        remove_labels,
                        ..TaskUpdate::default()
                    },
                )
            })
            .collect();
        let report = self
            .database
            .mutate_tasks(
                &self.active_workspace,
                updates,
                UndoContext::tui(format!("labels {} tasks", selection.len())),
            )
            .await?;
        let changed = report.changed_count();
        let message = if selection.is_single() {
            format!("set {} labels", selection.targets()[0].display_ref)
        } else if changed == 0 {
            format!("labels unchanged on {} tasks", selection.len())
        } else {
            format!("set labels on {changed} {}", task_noun(changed))
        };
        self.refresh_task_selection(selection, report, message, false, false)
            .await
    }

    pub(crate) async fn mutate_deleted_selection(
        &mut self,
        selection: &TaskSelection,
        deleted: bool,
    ) -> Result<MutationMessage> {
        let updates = selection
            .ids()
            .cloned()
            .map(|task_id| {
                (
                    task_id,
                    TaskUpdate {
                        deleted: Some(deleted),
                        ..TaskUpdate::default()
                    },
                )
            })
            .collect();
        let report = self
            .database
            .mutate_tasks(
                &self.active_workspace,
                updates,
                UndoContext::tui(format!(
                    "{} {} tasks",
                    if deleted { "delete" } else { "restore" },
                    selection.len()
                )),
            )
            .await?;
        let changed = report.changed_count();
        let message = if selection.is_single() {
            let verb = match (deleted, changed) {
                (true, 0) => "already deleted",
                (false, 0) => "already restored",
                (true, _) => "deleted",
                (false, _) => "restored",
            };
            format!("{verb} {}", selection.targets()[0].display_ref)
        } else {
            match (deleted, changed) {
                (true, 0) => format!("already deleted {} tasks", selection.len()),
                (false, 0) => format!("already restored {} tasks", selection.len()),
                (true, _) => format!("deleted {changed} tasks"),
                (false, _) => format!("restored {changed} tasks"),
            }
        };
        if deleted && selection.is_single() && !selection.uses_marks() {
            if let Some(item) = self
                .tasks
                .iter_mut()
                .find(|item| item.task.id == selection.targets()[0].task.id)
            {
                item.task.deleted = true;
            }
            return Ok(MutationMessage::new(
                message,
                Some(selection.anchor_index()),
            ));
        }
        self.refresh_task_selection(selection, report, message, false, false)
            .await
    }

    async fn refresh_task_selection(
        &mut self,
        selection: &TaskSelection,
        report: TaskMutationReport,
        message: String,
        preserve_task: bool,
        restore_by_index: bool,
    ) -> Result<MutationMessage> {
        if preserve_task && selection.is_single() && report.changed_count() == 1 {
            let mut item = selection.targets()[0].clone();
            item.task = report.outcomes.into_iter().next().unwrap().task;
            self.refresh(None).await?;
            let selected = match self
                .tasks
                .iter()
                .position(|task| task.task.id == item.task.id)
            {
                Some(index) => Some(index),
                None => {
                    let index = selection.anchor_index().min(self.tasks.len());
                    self.tasks.insert(index, item);
                    Some(index)
                }
            };
            return Ok(MutationMessage::new(message, selected));
        }
        if restore_by_index && !preserve_task {
            self.refresh(None).await?;
            let selected = self.restored_task_selection_at_index(Some(selection.anchor_index()));
            return Ok(MutationMessage::new(message, selected));
        }
        let selected = self.refresh(Some(selection.anchor_id())).await?;
        Ok(MutationMessage::new(message, selected))
    }

    pub(crate) async fn add_dependency(
        &mut self,
        index: Option<usize>,
        depends_on_task_id: &crate::ids::TaskId,
    ) -> Result<Option<MutationMessage>> {
        let Some(item) = self.selected_task(index).cloned() else {
            return Ok(None);
        };
        Ok(Some(
            self.add_dependency_to_task(&item.task.id, depends_on_task_id)
                .await?,
        ))
    }

    pub(crate) async fn add_dependency_to_task(
        &mut self,
        task_id: &crate::ids::TaskId,
        depends_on_task_id: &crate::ids::TaskId,
    ) -> Result<MutationMessage> {
        let display_refs = self
            .database
            .display_ref_context(&self.active_workspace.id)
            .await?;
        let task = self
            .database
            .resolve_task_ref(&self.active_workspace, task_id.as_str())
            .await?;
        let task_ref = display_refs.display_ref(&task);
        let outcome = self
            .database
            .add_task_dependency_with_undo(
                &self.active_workspace,
                task_id,
                depends_on_task_id,
                UndoContext::tui(format!("dependency {task_ref}")),
            )
            .await?;
        let depends_on_ref = display_refs.display_ref(&outcome.depends_on);
        let verb = if outcome.changed { "added" } else { "kept" };
        self.refresh_task_message(
            &outcome.task.id,
            format!("{verb} dependency {task_ref} depends_on {depends_on_ref}"),
        )
        .await
    }

    pub(crate) async fn remove_dependency(
        &mut self,
        index: Option<usize>,
        depends_on_task_id: &crate::ids::TaskId,
    ) -> Result<Option<MutationMessage>> {
        let Some(item) = self.selected_task(index).cloned() else {
            return Ok(None);
        };
        let outcome = self
            .database
            .remove_task_dependency_with_undo(
                &self.active_workspace,
                &item.task.id,
                depends_on_task_id,
                UndoContext::tui(format!("dependency {}", item.display_ref)),
            )
            .await?;
        let display_refs = self
            .database
            .display_ref_context(&self.active_workspace.id)
            .await?;
        let depends_on_ref = display_refs.display_ref(&outcome.depends_on);
        let verb = if outcome.changed { "removed" } else { "kept" };
        Ok(Some(
            self.refresh_task_message(
                &item.task.id,
                format!(
                    "{verb} dependency {} depends_on {depends_on_ref}",
                    item.display_ref
                ),
            )
            .await?,
        ))
    }
}

#[cfg(test)]
impl TuiStore {
    fn test_selection(
        &self,
        selected: Option<usize>,
        task_ids: &[crate::ids::TaskId],
    ) -> Option<TaskSelection> {
        TaskSelection::from_ids(&self.tasks, task_ids, selected)
    }

    fn test_selected(&self, selected: Option<usize>) -> Option<TaskSelection> {
        let id = self.selected_task(selected)?.task.id.clone();
        self.test_selection(selected, &[id])
    }

    pub(crate) async fn update_status(
        &mut self,
        selected: Option<usize>,
        status: &str,
    ) -> Result<Option<MutationMessage>> {
        let Some(selection) = self.test_selected(selected) else {
            return Ok(None);
        };
        Ok(Some(
            self.mutate_status_selection(&selection, TaskStatus::parse(status)?, false)
                .await?,
        ))
    }

    pub(crate) async fn update_status_preserving_task(
        &mut self,
        selected: Option<usize>,
        status: &str,
    ) -> Result<Option<MutationMessage>> {
        let Some(selection) = self.test_selected(selected) else {
            return Ok(None);
        };
        Ok(Some(
            self.mutate_status_selection(&selection, TaskStatus::parse(status)?, true)
                .await?,
        ))
    }

    pub(crate) async fn update_status_for_tasks(
        &mut self,
        selected: Option<usize>,
        task_ids: &[crate::ids::TaskId],
        status: &str,
    ) -> Result<Option<MutationMessage>> {
        let Some(selection) = self.test_selection(selected, task_ids) else {
            return Ok(None);
        };
        Ok(Some(
            self.mutate_status_selection(&selection, TaskStatus::parse(status)?, false)
                .await?,
        ))
    }

    pub(crate) async fn set_exact_priority(
        &mut self,
        selected: Option<usize>,
        priority: &str,
    ) -> Result<Option<MutationMessage>> {
        let Some(selection) = self.test_selected(selected) else {
            return Ok(None);
        };
        Ok(Some(
            self.mutate_priority_selection(
                &selection,
                PriorityMutation::Set(TaskPriority::parse(priority)?),
            )
            .await?,
        ))
    }

    pub(crate) async fn set_exact_priority_for_tasks(
        &mut self,
        selected: Option<usize>,
        task_ids: &[crate::ids::TaskId],
        priority: &str,
    ) -> Result<Option<MutationMessage>> {
        let Some(selection) = self.test_selection(selected, task_ids) else {
            return Ok(None);
        };
        Ok(Some(
            self.mutate_priority_selection(
                &selection,
                PriorityMutation::Set(TaskPriority::parse(priority)?),
            )
            .await?,
        ))
    }

    pub(crate) async fn update_title(
        &mut self,
        selected: Option<usize>,
        title: String,
    ) -> Result<Option<MutationMessage>> {
        self.test_mutate_text(selected, TaskTextField::Title, title.trim().to_string())
            .await
    }

    pub(crate) async fn update_description(
        &mut self,
        selected: Option<usize>,
        description: String,
    ) -> Result<Option<MutationMessage>> {
        self.test_mutate_text(selected, TaskTextField::Description, description)
            .await
    }

    async fn test_mutate_text(
        &mut self,
        selected: Option<usize>,
        field: TaskTextField,
        value: String,
    ) -> Result<Option<MutationMessage>> {
        let Some(selection) = self.test_selected(selected) else {
            return Ok(None);
        };
        Ok(Some(
            self.mutate_text_selection(&selection, field, value).await?,
        ))
    }

    pub(crate) async fn update_availability(
        &mut self,
        selected: Option<usize>,
        value: String,
        preserve_task: bool,
    ) -> Result<Option<MutationMessage>> {
        let Some(selection) = self.test_selected(selected) else {
            return Ok(None);
        };
        Ok(Some(
            self.mutate_date_selection(
                &selection,
                TaskDateField::Availability,
                (!value.is_empty()).then_some(value),
                preserve_task,
            )
            .await?,
        ))
    }

    pub(crate) async fn update_availability_for_tasks(
        &mut self,
        selected: Option<usize>,
        task_ids: &[crate::ids::TaskId],
        value: String,
        preserve_task: bool,
    ) -> Result<Option<MutationMessage>> {
        self.test_mutate_date(
            selected,
            task_ids,
            TaskDateField::Availability,
            value,
            preserve_task,
        )
        .await
    }

    pub(crate) async fn update_due_for_tasks(
        &mut self,
        selected: Option<usize>,
        task_ids: &[crate::ids::TaskId],
        value: String,
        preserve_task: bool,
    ) -> Result<Option<MutationMessage>> {
        self.test_mutate_date(selected, task_ids, TaskDateField::Due, value, preserve_task)
            .await
    }

    async fn test_mutate_date(
        &mut self,
        selected: Option<usize>,
        task_ids: &[crate::ids::TaskId],
        field: TaskDateField,
        value: String,
        preserve_task: bool,
    ) -> Result<Option<MutationMessage>> {
        let Some(selection) = self.test_selection(selected, task_ids) else {
            return Ok(None);
        };
        Ok(Some(
            self.mutate_date_selection(
                &selection,
                field,
                (!value.is_empty()).then_some(value),
                preserve_task,
            )
            .await?,
        ))
    }

    pub(crate) async fn update_project(
        &mut self,
        selected: Option<usize>,
        project: String,
    ) -> Result<Option<MutationMessage>> {
        let Some(selection) = self.test_selected(selected) else {
            return Ok(None);
        };
        Ok(Some(
            self.mutate_project_selection(&selection, project).await?,
        ))
    }

    pub(crate) async fn update_project_for_tasks(
        &mut self,
        selected: Option<usize>,
        task_ids: &[crate::ids::TaskId],
        project: String,
    ) -> Result<Option<MutationMessage>> {
        let Some(selection) = self.test_selection(selected, task_ids) else {
            return Ok(None);
        };
        Ok(Some(
            self.mutate_project_selection(&selection, project).await?,
        ))
    }

    pub(crate) async fn update_labels(
        &mut self,
        selected: Option<usize>,
        labels: Vec<String>,
    ) -> Result<Option<MutationMessage>> {
        let Some(selection) = self.test_selected(selected) else {
            return Ok(None);
        };
        Ok(Some(
            self.mutate_labels_selection(&selection, labels, Vec::new())
                .await?,
        ))
    }

    pub(crate) async fn update_labels_for_tasks(
        &mut self,
        selected: Option<usize>,
        task_ids: &[crate::ids::TaskId],
        labels: Vec<String>,
        partial_labels: Vec<String>,
    ) -> Result<Option<MutationMessage>> {
        let Some(selection) = self.test_selection(selected, task_ids) else {
            return Ok(None);
        };
        Ok(Some(
            self.mutate_labels_selection(&selection, labels, partial_labels)
                .await?,
        ))
    }

    pub(crate) async fn update_deleted(
        &mut self,
        selected: Option<usize>,
        deleted: bool,
    ) -> Result<Option<MutationMessage>> {
        let Some(selection) = self.test_selected(selected) else {
            return Ok(None);
        };
        Ok(Some(
            self.mutate_deleted_selection(&selection, deleted).await?,
        ))
    }

    pub(crate) async fn update_deleted_for_tasks(
        &mut self,
        selected: Option<usize>,
        task_ids: &[crate::ids::TaskId],
        deleted: bool,
    ) -> Result<Option<MutationMessage>> {
        let Some(selection) = self.test_selection(selected, task_ids) else {
            return Ok(None);
        };
        Ok(Some(
            self.mutate_deleted_selection(&selection, deleted).await?,
        ))
    }
}
