use std::collections::BTreeMap;

use anyhow::{Result, ensure};
use aven_core::operations::{TaskLabelSelection, TaskMutationReport, TaskUpdate};

use crate::choices::{TaskPriority, TaskStatus};
use crate::tui::store::{MutationMessage, types::committed_mutation_error};
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

impl TuiStore {
    pub(crate) async fn mutate_status_selection(
        &mut self,
        selection: &TaskSelection,
        status: TaskStatus,
        preserve_task: bool,
    ) -> Result<MutationMessage> {
        let selected_task_id = selection.single_id().cloned();
        let epic_fallback = (self.view_state.view == super::TaskView::Epics
            && selection.is_single()
            && selection.targets()[0].task.is_epic)
            .then(|| {
                let selected_task_id = selected_task_id.as_ref().expect("single selection");
                self.tasks
                    .iter()
                    .enumerate()
                    .filter(|(_, candidate)| {
                        &candidate.task.id != selected_task_id && candidate.epic_parent.is_none()
                    })
                    .min_by_key(|(index, _)| index.abs_diff(selection.anchor_index()))
                    .map(|(_, candidate)| candidate.task.id.clone())
            })
            .flatten();
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
                UndoContext::tui_task_mutation(
                    selection
                        .is_single()
                        .then(|| format!("status {}", selection.targets()[0].display_ref)),
                    "status",
                ),
            )
            .await?;
        let changed = report.changed_count();
        let message = if selection.is_single() {
            let verb = if changed == 0 { "unchanged" } else { "set" };
            format!(
                "{verb} {} status={status}",
                selection.targets()[0].display_ref
            )
        } else if changed == 0 {
            format!("status unchanged on {} tasks", selection.len())
        } else {
            format!("set status on {changed} {}", task_noun(changed))
        };
        let mut result = self
            .refresh_task_selection(selection, report, message, preserve_task, true)
            .await?;
        if selected_task_id.is_some_and(|selected_task_id| {
            self.tasks
                .iter()
                .all(|candidate| candidate.task.id != selected_task_id)
        }) && let Some(fallback) = epic_fallback
        {
            result.selected = self
                .tasks
                .iter()
                .position(|candidate| candidate.task.id == fallback);
        }
        Ok(result)
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
                UndoContext::tui_task_mutation(
                    selection.is_single().then(|| {
                        format!(
                            "move {} between columns",
                            selection.targets()[0].display_ref
                        )
                    }),
                    "move between columns",
                ),
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
            .ids()
            .cloned()
            .map(|task_id| {
                let update = match mutation {
                    PriorityMutation::Cycle { reverse } => TaskUpdate {
                        cycle_priority: Some(reverse),
                        ..TaskUpdate::default()
                    },
                    PriorityMutation::Set(priority) => TaskUpdate {
                        priority: Some(priority.to_string()),
                        ..TaskUpdate::default()
                    },
                };
                (task_id, update)
            })
            .collect();
        let report = self
            .database
            .mutate_tasks(
                &self.active_workspace,
                updates,
                UndoContext::tui_task_mutation(
                    selection
                        .is_single()
                        .then(|| format!("priority {}", selection.targets()[0].display_ref)),
                    "priority",
                ),
            )
            .await?;
        let changed = report.changed_count();
        let message = if selection.is_single() {
            let priority = report.outcomes[0].task.priority;
            let verb = if changed == 0 { "unchanged" } else { "set" };
            format!(
                "{verb} {} priority={priority}",
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
        ensure!(
            selection.is_single(),
            "text mutation requires exactly one task"
        );
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
                UndoContext::tui_task_mutation(
                    selection
                        .is_single()
                        .then(|| format!("{field_name} {}", selection.targets()[0].display_ref)),
                    field_name,
                ),
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
                UndoContext::tui_task_mutation(
                    selection
                        .is_single()
                        .then(|| format!("project {}", selection.targets()[0].display_ref)),
                    "project",
                ),
            )
            .await?;
        let changed = report.changed_count();
        let message = if selection.is_single() {
            let verb = if changed == 0 { "unchanged" } else { "set" };
            format!("{verb} {} project", selection.targets()[0].display_ref)
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
        let updates = selection
            .ids()
            .cloned()
            .map(|task_id| {
                (
                    task_id,
                    TaskUpdate {
                        label_selection: Some(TaskLabelSelection {
                            selected: selected_labels.clone(),
                            partial: partial_labels.clone(),
                        }),
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
                UndoContext::tui_task_mutation(
                    selection
                        .is_single()
                        .then(|| format!("labels {}", selection.targets()[0].display_ref)),
                    "labels",
                ),
            )
            .await?;
        let changed = report.changed_count();
        let message = if selection.is_single() {
            let verb = if changed == 0 { "unchanged" } else { "set" };
            format!("{verb} {} labels", selection.targets()[0].display_ref)
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
        preserve_task: bool,
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
                UndoContext::tui_task_mutation(
                    selection.is_single().then(|| {
                        format!(
                            "{} {}",
                            if deleted { "delete" } else { "restore" },
                            selection.targets()[0].display_ref
                        )
                    }),
                    if deleted { "delete" } else { "restore" },
                ),
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
        let direct_single = selection.single_id() == Some(selection.anchor_id());
        let preserve_task = direct_single && (preserve_task || deleted);
        let restore_by_index = deleted && !direct_single;
        self.refresh_task_selection(selection, report, message, preserve_task, restore_by_index)
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
            self.refresh_preserving_visible_deleted(None)
                .await
                .map_err(committed_mutation_error)?;
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
            self.refresh(None).await.map_err(committed_mutation_error)?;
            let selected = self.restored_task_selection_at_index(Some(selection.anchor_index()));
            return Ok(MutationMessage::new(message, selected));
        }
        let selected = self
            .refresh(Some(selection.anchor_id()))
            .await
            .map_err(committed_mutation_error)?;
        Ok(MutationMessage::new(message, selected))
    }

    #[cfg(test)]
    pub(crate) async fn add_dependency(
        &mut self,
        index: Option<usize>,
        depends_on_task_id: &crate::ids::TaskId,
    ) -> Result<Option<MutationMessage>> {
        let Some(selection) = self.test_selected(index) else {
            return Ok(None);
        };
        Ok(Some(
            self.add_dependency_to_selection(&selection, depends_on_task_id)
                .await?,
        ))
    }

    pub(crate) async fn add_dependency_to_selection(
        &mut self,
        selection: &TaskSelection,
        depends_on_task_id: &crate::ids::TaskId,
    ) -> Result<MutationMessage> {
        ensure!(
            selection.is_single(),
            "dependency mutation requires exactly one task"
        );
        let task_id = selection.single_id().expect("single selection");
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

    #[cfg(test)]
    pub(crate) async fn remove_dependency(
        &mut self,
        index: Option<usize>,
        depends_on_task_id: &crate::ids::TaskId,
    ) -> Result<Option<MutationMessage>> {
        let Some(selection) = self.test_selected(index) else {
            return Ok(None);
        };
        Ok(Some(
            self.remove_dependency_from_selection(&selection, depends_on_task_id)
                .await?,
        ))
    }

    pub(crate) async fn remove_dependency_from_selection(
        &mut self,
        selection: &TaskSelection,
        depends_on_task_id: &crate::ids::TaskId,
    ) -> Result<MutationMessage> {
        ensure!(
            selection.is_single(),
            "dependency mutation requires exactly one task"
        );
        let item = &selection.targets()[0];
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
        self.refresh_task_message(
            &item.task.id,
            format!(
                "{verb} dependency {} depends_on {depends_on_ref}",
                item.display_ref
            ),
        )
        .await
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
            self.mutate_deleted_selection(&selection, deleted, false)
                .await?,
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
            self.mutate_deleted_selection(&selection, deleted, false)
                .await?,
        ))
    }
}
