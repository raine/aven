use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;

use crate::choices::TaskPriority;
use crate::query::TaskListItem;
use crate::tui::store::MutationMessage;
use crate::undo::{UndoCommand, UndoContext};
use aven_core::operations::TaskUpdate;

use super::TuiStore;

#[derive(Clone, Copy)]
enum StatusRefresh {
    Default,
    PreserveTask,
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
    #[cfg(test)]
    async fn update_selected_task<F>(
        &mut self,
        index: Option<usize>,
        update: TaskUpdate,
        message: F,
    ) -> Result<Option<MutationMessage>>
    where
        F: FnOnce(&TaskListItem) -> String,
    {
        if let Some(item) = self.selected_task(index).cloned() {
            self.database
                .update_task(&self.active_workspace, &item.task.id, update)
                .await?;
            return Ok(Some(
                self.refresh_task_message(&item.task.id, message(&item))
                    .await?,
            ));
        }
        Ok(None)
    }

    pub(crate) async fn update_status(
        &mut self,
        index: Option<usize>,
        status: &str,
    ) -> Result<Option<MutationMessage>> {
        self.update_status_with_refresh(index, status, StatusRefresh::Default)
            .await
    }

    pub(crate) async fn update_status_preserving_task(
        &mut self,
        index: Option<usize>,
        status: &str,
    ) -> Result<Option<MutationMessage>> {
        self.update_status_with_refresh(index, status, StatusRefresh::PreserveTask)
            .await
    }

    async fn update_status_with_refresh(
        &mut self,
        index: Option<usize>,
        status: &str,
        refresh: StatusRefresh,
    ) -> Result<Option<MutationMessage>> {
        let Some(mut item) = self.selected_task(index).cloned() else {
            return Ok(None);
        };

        let report = self
            .database
            .mutate_tasks(
                &self.active_workspace,
                vec![(
                    item.task.id.clone(),
                    TaskUpdate {
                        status: Some(status.to_string()),
                        ..TaskUpdate::default()
                    },
                )],
                UndoContext::tui(format!("status {}", item.display_ref)),
            )
            .await?;
        let outcome = report.outcomes.into_iter().next().unwrap();
        let message = format!("set {} status={status}", item.display_ref);
        item.task = outcome.task;
        let result = match refresh {
            StatusRefresh::PreserveTask => {
                self.refresh_preserved_task_message(index, item, message)
                    .await?
            }
            StatusRefresh::Default => self.refresh_index_message(index, message).await?,
        };
        Ok(Some(result))
    }

    async fn refresh_preserved_task_message(
        &mut self,
        selected: Option<usize>,
        item: TaskListItem,
        message: impl Into<String>,
    ) -> Result<MutationMessage> {
        self.refresh(None).await?;
        let selected = match self
            .tasks
            .iter()
            .position(|task| task.task.id == item.task.id)
        {
            Some(index) => Some(index),
            None => {
                let index = selected.unwrap_or(self.tasks.len()).min(self.tasks.len());
                self.tasks.insert(index, item);
                Some(index)
            }
        };

        Ok(MutationMessage::new(message, selected))
    }

    pub(crate) async fn update_priority(
        &mut self,
        index: Option<usize>,
        reverse: bool,
    ) -> Result<Option<MutationMessage>> {
        if let Some(item) = self.selected_task(index).cloned() {
            let priority = cycled_priority(item.task.priority, reverse);
            let report = self
                .database
                .mutate_tasks(
                    &self.active_workspace,
                    vec![(
                        item.task.id.clone(),
                        TaskUpdate {
                            priority: Some(priority.as_str().to_string()),
                            ..TaskUpdate::default()
                        },
                    )],
                    UndoContext::tui(format!("priority {}", item.display_ref)),
                )
                .await?;
            let task = report.outcomes.into_iter().next().unwrap().task;
            return Ok(Some(
                self.refresh_task_message(
                    &item.task.id,
                    format!("set {} priority={}", item.display_ref, task.priority),
                )
                .await?,
            ));
        }
        Ok(None)
    }

    pub(crate) async fn set_exact_priority(
        &mut self,
        index: Option<usize>,
        priority: &str,
    ) -> Result<Option<MutationMessage>> {
        let Some(item) = self.selected_task(index).cloned() else {
            return Ok(None);
        };
        let report = self
            .database
            .mutate_tasks(
                &self.active_workspace,
                vec![(
                    item.task.id.clone(),
                    TaskUpdate {
                        priority: Some(priority.to_string()),
                        ..TaskUpdate::default()
                    },
                )],
                UndoContext::tui(format!("priority {}", item.display_ref)),
            )
            .await?;
        let outcome = report.outcomes.into_iter().next().unwrap();
        Ok(Some(
            self.refresh_task_message(
                &outcome.task.id,
                format!("set {} priority={priority}", item.display_ref),
            )
            .await?,
        ))
    }

    #[cfg(test)]
    pub(crate) async fn update_title(
        &mut self,
        index: Option<usize>,
        title: String,
    ) -> Result<Option<MutationMessage>> {
        let title = title.trim().to_string();
        let Some(item) = self.selected_task(index).cloned() else {
            return Ok(None);
        };
        let before = item.task.title.clone();
        if title == before {
            return Ok(Some(
                self.refresh_task_message(
                    &item.task.id,
                    format!("unchanged {} title", item.display_ref),
                )
                .await?,
            ));
        }
        let outcome = self
            .update_selected_task(
                index,
                TaskUpdate {
                    title: Some(title.clone()),
                    ..TaskUpdate::default()
                },
                |item| format!("set {} title", item.display_ref),
            )
            .await?;
        if outcome.is_some() {
            self.record_undo_commands(
                &format!("title {}", item.display_ref),
                vec![UndoCommand::SetTaskField {
                    task_id: item.task.id.clone(),
                    field: "title".to_string(),
                    before,
                    after: title,
                }],
            )
            .await?;
        }
        Ok(outcome)
    }

    pub(crate) async fn update_title_for_task(
        &mut self,
        current_selected_index: Option<usize>,
        task_id: &crate::ids::TaskId,
        title: String,
    ) -> Result<Option<MutationMessage>> {
        let title = title.trim().to_string();
        let targets = self.tasks_matching_ids(std::slice::from_ref(task_id));
        let Some(item) = targets.first() else {
            return Ok(None);
        };
        let before = item.task.title.clone();
        if title == before {
            return Ok(Some(
                self.refresh_after_task_batch(
                    current_selected_index,
                    &targets,
                    format!("unchanged {} title", item.display_ref),
                )
                .await?,
            ));
        }
        self.database
            .update_task(
                &self.active_workspace,
                task_id,
                TaskUpdate {
                    title: Some(title.clone()),
                    ..TaskUpdate::default()
                },
            )
            .await?;
        self.record_undo_commands(
            &format!("title {}", item.display_ref),
            vec![UndoCommand::SetTaskField {
                task_id: task_id.clone(),
                field: "title".to_string(),
                before,
                after: title,
            }],
        )
        .await?;
        Ok(Some(
            self.refresh_after_task_batch(
                current_selected_index,
                &targets,
                format!("set {} title", item.display_ref),
            )
            .await?,
        ))
    }

    pub(crate) async fn update_description_for_task(
        &mut self,
        current_selected_index: Option<usize>,
        task_id: &crate::ids::TaskId,
        description: String,
    ) -> Result<Option<MutationMessage>> {
        let targets = self.tasks_matching_ids(std::slice::from_ref(task_id));
        let Some(item) = targets.first() else {
            return Ok(None);
        };
        let before = item.task.description.clone();
        if description == before {
            return Ok(Some(
                self.refresh_after_task_batch(
                    current_selected_index,
                    &targets,
                    format!("unchanged {} description", item.display_ref),
                )
                .await?,
            ));
        }
        self.database
            .update_task(
                &self.active_workspace,
                task_id,
                TaskUpdate {
                    description: Some(description.clone()),
                    ..TaskUpdate::default()
                },
            )
            .await?;
        self.record_undo_commands(
            &format!("description {}", item.display_ref),
            vec![UndoCommand::SetTaskField {
                task_id: task_id.clone(),
                field: "description".to_string(),
                before,
                after: description,
            }],
        )
        .await?;
        Ok(Some(
            self.refresh_after_task_batch(
                current_selected_index,
                &targets,
                format!("set {} description", item.display_ref),
            )
            .await?,
        ))
    }

    #[cfg(test)]
    pub(crate) async fn update_description(
        &mut self,
        index: Option<usize>,
        description: String,
    ) -> Result<Option<MutationMessage>> {
        let Some(item) = self.selected_task(index).cloned() else {
            return Ok(None);
        };
        let before = item.task.description.clone();
        let outcome = self
            .update_selected_task(
                index,
                TaskUpdate {
                    description: Some(description.clone()),
                    ..TaskUpdate::default()
                },
                |item| format!("set {} description", item.display_ref),
            )
            .await?;
        if outcome.is_some() {
            self.record_undo_commands(
                &format!("description {}", item.display_ref),
                vec![UndoCommand::SetTaskField {
                    task_id: item.task.id.clone(),
                    field: "description".to_string(),
                    before,
                    after: description,
                }],
            )
            .await?;
        }
        Ok(outcome)
    }

    #[cfg(test)]
    pub(crate) async fn update_availability(
        &mut self,
        index: Option<usize>,
        available_at: String,
        preserve_task: bool,
    ) -> Result<Option<MutationMessage>> {
        let Some(mut item) = self.selected_task(index).cloned() else {
            return Ok(None);
        };
        let before = item.task.available_at.clone().unwrap_or_default();
        if available_at == before {
            return Ok(Some(
                self.refresh_task_message(
                    &item.task.id,
                    format!("unchanged {} availability", item.display_ref),
                )
                .await?,
            ));
        }

        let report = self
            .database
            .mutate_tasks(
                &self.active_workspace,
                vec![(
                    item.task.id.clone(),
                    TaskUpdate {
                        available_at: Some(
                            (!available_at.is_empty()).then(|| available_at.clone()),
                        ),
                        ..TaskUpdate::default()
                    },
                )],
                UndoContext::tui(format!("availability {}", item.display_ref)),
            )
            .await?;
        let outcome = report.outcomes.into_iter().next().unwrap();
        let message = if available_at.is_empty() {
            format!("cleared {} availability", item.display_ref)
        } else {
            format!("set {} availability", item.display_ref)
        };
        item.task = outcome.task;
        let result = if preserve_task {
            self.refresh_preserved_task_message(index, item, message)
                .await?
        } else {
            self.refresh_task_message(&item.task.id, message).await?
        };
        Ok(Some(result))
    }

    pub(crate) async fn update_availability_for_tasks(
        &mut self,
        current_selected_index: Option<usize>,
        task_ids: &[crate::ids::TaskId],
        available_at: String,
        preserve_task: bool,
    ) -> Result<Option<MutationMessage>> {
        let targets = self.tasks_matching_ids(task_ids);
        if targets.is_empty() {
            return Ok(None);
        }
        let mut updates = Vec::new();
        for item in &targets {
            let before = item.task.available_at.clone().unwrap_or_default();
            if before == available_at {
                continue;
            }
            updates.push((
                item.task.id.clone(),
                TaskUpdate {
                    available_at: Some((!available_at.is_empty()).then(|| available_at.clone())),
                    ..TaskUpdate::default()
                },
            ));
        }
        let expected_changed = updates.len();
        let report = self
            .database
            .mutate_tasks(
                &self.active_workspace,
                updates,
                UndoContext::tui(format!("availability {expected_changed} tasks")),
            )
            .await?;
        let changed = report.changed_count();
        let noun = if targets.len() == 1 { "task" } else { "tasks" };
        let message = if changed == 0 {
            format!("availability unchanged on {} {noun}", targets.len())
        } else if available_at.is_empty() {
            format!(
                "cleared availability on {changed} {}",
                if changed == 1 { "task" } else { "tasks" }
            )
        } else {
            format!(
                "set availability on {changed} {}",
                if changed == 1 { "task" } else { "tasks" }
            )
        };
        let result = if preserve_task && targets.len() == 1 && changed == 1 {
            let mut item = targets[0].clone();
            item.task = report.outcomes.into_iter().next().unwrap().task;
            self.refresh_preserved_task_message(current_selected_index, item, message)
                .await?
        } else {
            self.refresh_after_task_batch(current_selected_index, &targets, message)
                .await?
        };
        Ok(Some(result))
    }

    pub(crate) async fn update_due_for_tasks(
        &mut self,
        current_selected_index: Option<usize>,
        task_ids: &[crate::ids::TaskId],
        due_on: String,
        preserve_task: bool,
    ) -> Result<Option<MutationMessage>> {
        let targets = self.tasks_matching_ids(task_ids);
        if targets.is_empty() {
            return Ok(None);
        }
        let mut updates = Vec::new();
        for item in &targets {
            let before = item.task.due_on.clone().unwrap_or_default();
            if before == due_on {
                continue;
            }
            updates.push((
                item.task.id.clone(),
                TaskUpdate {
                    due_on: Some((!due_on.is_empty()).then(|| due_on.clone())),
                    ..TaskUpdate::default()
                },
            ));
        }
        let expected_changed = updates.len();
        let report = self
            .database
            .mutate_tasks(
                &self.active_workspace,
                updates,
                UndoContext::tui(format!("due date {expected_changed} tasks")),
            )
            .await?;
        let changed = report.changed_count();
        let noun = if targets.len() == 1 { "task" } else { "tasks" };
        let message = if changed == 0 {
            format!("due date unchanged on {} {noun}", targets.len())
        } else if due_on.is_empty() {
            format!(
                "cleared due date on {changed} {}",
                if changed == 1 { "task" } else { "tasks" }
            )
        } else {
            format!(
                "set due date on {changed} {}",
                if changed == 1 { "task" } else { "tasks" }
            )
        };
        let result = if preserve_task && targets.len() == 1 && changed == 1 {
            let mut item = targets[0].clone();
            item.task = report.outcomes.into_iter().next().unwrap().task;
            self.refresh_preserved_task_message(current_selected_index, item, message)
                .await?
        } else {
            self.refresh_after_task_batch(current_selected_index, &targets, message)
                .await?
        };
        Ok(Some(result))
    }

    #[cfg(test)]
    pub(crate) async fn update_project(
        &mut self,
        index: Option<usize>,
        project: String,
    ) -> Result<Option<MutationMessage>> {
        let Some(item) = self.selected_task(index).cloned() else {
            return Ok(None);
        };
        self.database
            .mutate_tasks(
                &self.active_workspace,
                vec![(
                    item.task.id.clone(),
                    TaskUpdate {
                        project: Some(project),
                        ..TaskUpdate::default()
                    },
                )],
                UndoContext::tui(format!("project {}", item.display_ref)),
            )
            .await?;
        Ok(Some(
            self.refresh_task_message(&item.task.id, format!("set {} project", item.display_ref))
                .await?,
        ))
    }

    #[cfg(test)]
    pub(crate) async fn update_labels(
        &mut self,
        index: Option<usize>,
        selected_labels: Vec<String>,
    ) -> Result<Option<MutationMessage>> {
        let Some(item) = self.selected_task(index).cloned() else {
            return Ok(None);
        };
        let add_labels = selected_labels
            .iter()
            .filter(|label| !item.labels.contains(label))
            .cloned()
            .collect::<Vec<_>>();
        let remove_labels = item
            .labels
            .iter()
            .filter(|label| !selected_labels.contains(label))
            .cloned()
            .collect::<Vec<_>>();
        self.database
            .mutate_tasks(
                &self.active_workspace,
                vec![(
                    item.task.id.clone(),
                    TaskUpdate {
                        add_labels,
                        remove_labels,
                        ..TaskUpdate::default()
                    },
                )],
                UndoContext::tui(format!("labels {}", item.display_ref)),
            )
            .await?;
        Ok(Some(
            self.refresh_task_message(&item.task.id, format!("set {} labels", item.display_ref))
                .await?,
        ))
    }

    pub(crate) async fn update_deleted(
        &mut self,
        index: Option<usize>,
        deleted: bool,
    ) -> Result<Option<MutationMessage>> {
        if let Some(item) = self.selected_task(index).cloned() {
            if item.task.deleted == deleted {
                return Ok(Some(MutationMessage::new(
                    if deleted {
                        format!("already deleted {}", item.display_ref)
                    } else {
                        format!("already restored {}", item.display_ref)
                    },
                    index,
                )));
            }

            let before = if item.task.deleted { "1" } else { "0" };
            self.database
                .set_task_deleted_state(&self.active_workspace, &item.task, deleted)
                .await?;
            let summary = if deleted {
                format!("delete {}", item.display_ref)
            } else {
                format!("restore {}", item.display_ref)
            };
            self.record_undo_commands(
                &summary,
                vec![UndoCommand::SetTaskField {
                    task_id: item.task.id.clone(),
                    field: "deleted".to_string(),
                    before: before.to_string(),
                    after: if deleted { "1" } else { "0" }.to_string(),
                }],
            )
            .await?;
            if deleted {
                if let Some(index) = index
                    && let Some(current) = self.tasks.get_mut(index)
                {
                    current.task.deleted = true;
                }
                return Ok(Some(MutationMessage::new(
                    format!("deleted {}", item.display_ref),
                    index,
                )));
            }
            return Ok(Some(
                self.refresh_task_message(&item.task.id, format!("restored {}", item.display_ref))
                    .await?,
            ));
        }
        Ok(None)
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
        let outcome = self
            .database
            .add_task_dependency(&self.active_workspace, task_id, depends_on_task_id)
            .await?;
        let display_refs = self
            .database
            .display_ref_context(&self.active_workspace.id)
            .await?;
        let task_ref = display_refs.display_ref(&outcome.task);
        let depends_on_ref = display_refs.display_ref(&outcome.depends_on);
        if outcome.changed {
            self.record_undo_commands(
                &format!("dependency {task_ref}"),
                vec![UndoCommand::AddTaskDependency {
                    task_id: outcome.task.id.clone(),
                    depends_on_task_id: outcome.depends_on.id.clone(),
                }],
            )
            .await?;
        }
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
            .remove_task_dependency(&self.active_workspace, &item.task.id, depends_on_task_id)
            .await?;
        let display_refs = self
            .database
            .display_ref_context(&self.active_workspace.id)
            .await?;
        let depends_on_ref = display_refs.display_ref(&outcome.depends_on);
        if outcome.changed {
            self.record_undo_commands(
                &format!("dependency {}", item.display_ref),
                vec![UndoCommand::RemoveTaskDependency {
                    task_id: item.task.id.clone(),
                    depends_on_task_id: outcome.depends_on.id.clone(),
                }],
            )
            .await?;
        }
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

    fn tasks_matching_ids(&self, task_ids: &[crate::ids::TaskId]) -> Vec<TaskListItem> {
        let task_ids = task_ids.iter().collect::<BTreeSet<_>>();
        self.tasks
            .iter()
            .filter(|item| task_ids.contains(&item.task.id))
            .cloned()
            .collect()
    }

    async fn refresh_after_task_batch(
        &mut self,
        current_selected_index: Option<usize>,
        targets: &[TaskListItem],
        message: String,
    ) -> Result<MutationMessage> {
        let selected_id = self
            .selected_task(current_selected_index)
            .map(|item| item.task.id.clone());
        let fallback_id = targets.first().map(|item| item.task.id.clone());
        let selected = self
            .refresh(selected_id.as_ref().or(fallback_id.as_ref()))
            .await?;
        Ok(MutationMessage::new(message, selected))
    }

    pub(crate) async fn update_status_for_tasks(
        &mut self,
        current_selected_index: Option<usize>,
        task_ids: &[crate::ids::TaskId],
        status: &str,
    ) -> Result<Option<MutationMessage>> {
        self.update_status_for_tasks_with_refresh(
            current_selected_index,
            task_ids,
            status,
            StatusRefresh::Default,
        )
        .await
    }

    pub(crate) async fn update_status_for_tasks_preserving_task(
        &mut self,
        current_selected_index: Option<usize>,
        task_ids: &[crate::ids::TaskId],
        status: &str,
    ) -> Result<Option<MutationMessage>> {
        self.update_status_for_tasks_with_refresh(
            current_selected_index,
            task_ids,
            status,
            StatusRefresh::PreserveTask,
        )
        .await
    }

    async fn update_status_for_tasks_with_refresh(
        &mut self,
        current_selected_index: Option<usize>,
        task_ids: &[crate::ids::TaskId],
        status: &str,
        refresh: StatusRefresh,
    ) -> Result<Option<MutationMessage>> {
        let targets = self.tasks_matching_ids(task_ids);
        if targets.is_empty() {
            return Ok(None);
        }

        let mut updates = Vec::new();
        for item in &targets {
            let before = item.task.status.as_str();
            if before == status {
                continue;
            }
            updates.push((
                item.task.id.clone(),
                TaskUpdate {
                    status: Some(status.to_string()),
                    ..TaskUpdate::default()
                },
            ));
        }
        let expected_changed = updates.len();
        let report = self
            .database
            .mutate_tasks(
                &self.active_workspace,
                updates,
                UndoContext::tui(format!("status {expected_changed} tasks")),
            )
            .await?;

        let changed = report.changed_count();
        let message = if changed == 0 {
            format!(
                "status unchanged on {} {}",
                targets.len(),
                task_noun(targets.len())
            )
        } else {
            format!("set status on {changed} {}", task_noun(changed))
        };
        let result = match refresh {
            StatusRefresh::Default => {
                self.refresh_index_message(current_selected_index, message)
                    .await?
            }
            StatusRefresh::PreserveTask => {
                self.refresh_after_task_batch(current_selected_index, &targets, message)
                    .await?
            }
        };
        Ok(Some(result))
    }

    pub(crate) async fn update_status_changes_for_tasks(
        &mut self,
        current_selected_index: Option<usize>,
        changes: &[(crate::ids::TaskId, String)],
    ) -> Result<Option<MutationMessage>> {
        let status_by_id = changes.iter().cloned().collect::<BTreeMap<_, _>>();
        let task_ids = status_by_id.keys().cloned().collect::<Vec<_>>();
        let targets = self.tasks_matching_ids(&task_ids);
        if targets.is_empty() {
            return Ok(None);
        }

        let mut updates = Vec::new();
        for item in &targets {
            let Some(status) = status_by_id.get(&item.task.id) else {
                continue;
            };
            if item.task.status.as_str() == status {
                continue;
            }
            updates.push((
                item.task.id.clone(),
                TaskUpdate {
                    status: Some(status.clone()),
                    ..TaskUpdate::default()
                },
            ));
        }
        let expected_changed = updates.len();
        let report = self
            .database
            .mutate_tasks(
                &self.active_workspace,
                updates,
                UndoContext::tui(format!("move {expected_changed} tasks between columns")),
            )
            .await?;

        let changed = report.changed_count();
        let message = if changed == 0 {
            format!("column unchanged on {} tasks", targets.len())
        } else {
            format!("moved {changed} tasks between columns")
        };
        Ok(Some(
            self.refresh_after_task_batch(current_selected_index, &targets, message)
                .await?,
        ))
    }

    pub(crate) async fn update_priority_for_tasks(
        &mut self,
        current_selected_index: Option<usize>,
        task_ids: &[crate::ids::TaskId],
        reverse: bool,
    ) -> Result<Option<MutationMessage>> {
        let targets = self.tasks_matching_ids(task_ids);
        if targets.is_empty() {
            return Ok(None);
        }

        let updates = targets
            .iter()
            .map(|item| {
                let priority = cycled_priority(item.task.priority, reverse);
                (
                    item.task.id.clone(),
                    TaskUpdate {
                        priority: Some(priority.as_str().to_string()),
                        ..TaskUpdate::default()
                    },
                )
            })
            .collect::<Vec<_>>();
        let expected_changed = updates.len();
        let report = self
            .database
            .mutate_tasks(
                &self.active_workspace,
                updates,
                UndoContext::tui(format!("priority {expected_changed} tasks")),
            )
            .await?;

        let changed = report.changed_count();
        Ok(Some(
            self.refresh_after_task_batch(
                current_selected_index,
                &targets,
                format!("set priority on {changed} {}", task_noun(changed)),
            )
            .await?,
        ))
    }

    pub(crate) async fn set_exact_priority_for_tasks(
        &mut self,
        current_selected_index: Option<usize>,
        task_ids: &[crate::ids::TaskId],
        priority: &str,
    ) -> Result<Option<MutationMessage>> {
        let targets = self.tasks_matching_ids(task_ids);
        if targets.is_empty() {
            return Ok(None);
        }

        let mut updates = Vec::new();
        for item in &targets {
            if item.task.priority.as_str() == priority {
                continue;
            }
            updates.push((
                item.task.id.clone(),
                TaskUpdate {
                    priority: Some(priority.to_string()),
                    ..TaskUpdate::default()
                },
            ));
        }
        let expected_changed = updates.len();
        let report = self
            .database
            .mutate_tasks(
                &self.active_workspace,
                updates,
                UndoContext::tui(format!("priority {expected_changed} tasks")),
            )
            .await?;

        let changed = report.changed_count();
        let message = if changed == 0 {
            format!(
                "priority unchanged on {} {}",
                targets.len(),
                task_noun(targets.len())
            )
        } else {
            format!("set priority on {changed} {}", task_noun(changed))
        };
        Ok(Some(
            self.refresh_after_task_batch(current_selected_index, &targets, message)
                .await?,
        ))
    }

    pub(crate) async fn update_project_for_tasks(
        &mut self,
        current_selected_index: Option<usize>,
        task_ids: &[crate::ids::TaskId],
        project: String,
    ) -> Result<Option<MutationMessage>> {
        let targets = self.tasks_matching_ids(task_ids);
        if targets.is_empty() {
            return Ok(None);
        }

        let updates = targets
            .iter()
            .map(|item| {
                (
                    item.task.id.clone(),
                    TaskUpdate {
                        project: Some(project.clone()),
                        ..TaskUpdate::default()
                    },
                )
            })
            .collect::<Vec<_>>();
        let expected_changed = updates.len();
        let report = self
            .database
            .mutate_tasks(
                &self.active_workspace,
                updates,
                UndoContext::tui(format!("project {expected_changed} tasks")),
            )
            .await?;

        let changed = report.changed_count();
        let message = if changed == 0 {
            format!(
                "project unchanged on {} {}",
                targets.len(),
                task_noun(targets.len())
            )
        } else {
            format!("set project on {changed} {}", task_noun(changed))
        };
        Ok(Some(
            self.refresh_after_task_batch(current_selected_index, &targets, message)
                .await?,
        ))
    }

    pub(crate) async fn update_deleted_for_tasks(
        &mut self,
        current_selected_index: Option<usize>,
        task_ids: &[crate::ids::TaskId],
        deleted: bool,
    ) -> Result<Option<MutationMessage>> {
        let targets = self.tasks_matching_ids(task_ids);
        if targets.is_empty() {
            return Ok(None);
        }

        let mut undo_commands = Vec::new();
        let mut updates = Vec::new();
        for item in &targets {
            if item.task.deleted == deleted {
                continue;
            }
            let before = if item.task.deleted { "1" } else { "0" };
            let after = if deleted { "1" } else { "0" };
            updates.push((
                item.task.id.clone(),
                "deleted".to_string(),
                after.to_string(),
            ));
            undo_commands.push(UndoCommand::SetTaskField {
                task_id: item.task.id.clone(),
                field: "deleted".to_string(),
                before: before.to_string(),
                after: after.to_string(),
            });
        }
        self.database
            .set_task_fields(&self.active_workspace, &updates)
            .await?;

        let changed = undo_commands.len();
        if changed > 0 {
            let summary = if deleted { "delete" } else { "restore" };
            self.record_undo_commands(&format!("{summary} {changed} tasks"), undo_commands)
                .await?;
        }
        let message = match (deleted, changed) {
            (true, 0) => format!("already deleted {} tasks", targets.len()),
            (false, 0) => format!("already restored {} tasks", targets.len()),
            (true, _) => format!("deleted {changed} tasks"),
            (false, _) => format!("restored {changed} tasks"),
        };
        Ok(Some(
            self.refresh_after_task_batch(current_selected_index, &targets, message)
                .await?,
        ))
    }

    pub(crate) async fn update_labels_for_tasks(
        &mut self,
        current_selected_index: Option<usize>,
        task_ids: &[crate::ids::TaskId],
        selected_labels: Vec<String>,
        partial_labels: Vec<String>,
    ) -> Result<Option<MutationMessage>> {
        if task_ids.is_empty() {
            return Ok(None);
        }
        let selected_id = self
            .selected_task(current_selected_index)
            .map(|item| item.task.id.clone());
        let task_ids = task_ids.iter().collect::<BTreeSet<_>>();
        let selected_label_set = selected_labels.iter().collect::<BTreeSet<_>>();
        let partial_label_set = partial_labels.iter().collect::<BTreeSet<_>>();
        let targets = self
            .tasks
            .iter()
            .filter(|item| task_ids.contains(&item.task.id))
            .cloned()
            .collect::<Vec<_>>();
        if targets.is_empty() {
            return Ok(None);
        }

        let mut updates = Vec::new();
        for item in &targets {
            let before = &item.labels;
            let add_labels = selected_labels
                .iter()
                .filter(|label| !before.contains(label))
                .cloned()
                .collect::<Vec<_>>();
            let remove_labels = before
                .iter()
                .filter(|label| {
                    !selected_label_set.contains(label) && !partial_label_set.contains(label)
                })
                .cloned()
                .collect::<Vec<_>>();
            if add_labels.is_empty() && remove_labels.is_empty() {
                continue;
            }
            updates.push((
                item.task.id.clone(),
                TaskUpdate {
                    add_labels,
                    remove_labels,
                    ..TaskUpdate::default()
                },
            ));
        }
        let expected_changed = updates.len();
        let report = self
            .database
            .mutate_tasks(
                &self.active_workspace,
                updates,
                UndoContext::tui(format!("labels {expected_changed} tasks")),
            )
            .await?;

        let changed = report.changed_count();
        let fallback_id = targets.first().map(|item| item.task.id.clone());
        let selected = self
            .refresh(selected_id.as_ref().or(fallback_id.as_ref()))
            .await?;
        let message = if changed == 0 {
            format!(
                "labels unchanged on {} {}",
                targets.len(),
                task_noun(targets.len())
            )
        } else {
            format!("set labels on {changed} {}", task_noun(changed))
        };
        Ok(Some(MutationMessage::new(message, selected)))
    }
}
