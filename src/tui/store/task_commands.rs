use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;

use crate::mutation::{cycle_priority, set_deleted, set_priority, set_status};
use crate::operations::{TaskUpdate, update_task as update_task_operation};
use crate::query::TaskListItem;
use crate::tui::store::MutationMessage;
use crate::undo::UndoCommand;

use super::TuiStore;

#[derive(Clone, Copy)]
enum StatusRefresh {
    Default,
    PreserveTask,
}

impl TuiStore {
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
            self.activate_workspace();
            let mut conn = self.pool.acquire().await?;
            update_task_operation(&mut conn, &item.task.id, update).await?;
            drop(conn);
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

        let before = item.task.status.as_str().to_string();
        self.activate_workspace();
        let mut conn = self.pool.acquire().await?;
        let task = set_status(&mut conn, &item.task, status).await?;
        drop(conn);
        self.record_undo_commands(
            &format!("status {}", item.display_ref),
            vec![UndoCommand::SetTaskField {
                task_id: item.task.id.clone(),
                field: "status".to_string(),
                before,
                after: status.to_string(),
            }],
        )
        .await?;
        let message = format!("set {} status={status}", item.display_ref);
        item.task = task;
        let result = match (status, refresh) {
            ("done", StatusRefresh::PreserveTask) => {
                self.refresh_preserved_task_message(index, item, message)
                    .await?
            }
            ("done", StatusRefresh::Default) => self.refresh_index_message(index, message).await?,
            _ => self.refresh_task_message(&item.task.id, message).await?,
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
            let before = item.task.priority.as_str().to_string();
            self.activate_workspace();
            let mut conn = self.pool.acquire().await?;
            let task = cycle_priority(&mut conn, &item.task, reverse).await?;
            drop(conn);
            self.record_undo_commands(
                &format!("priority {}", item.display_ref),
                vec![UndoCommand::SetTaskField {
                    task_id: item.task.id.clone(),
                    field: "priority".to_string(),
                    before,
                    after: task.priority.as_str().to_string(),
                }],
            )
            .await?;
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
        let before = item.task.priority.as_str().to_string();
        let outcome = self
            .update_selected_task(
                index,
                TaskUpdate {
                    priority: Some(priority.to_string()),
                    ..TaskUpdate::default()
                },
                |item| format!("set {} priority={priority}", item.display_ref),
            )
            .await?;
        if outcome.is_some() {
            self.record_undo_commands(
                &format!("priority {}", item.display_ref),
                vec![UndoCommand::SetTaskField {
                    task_id: item.task.id.clone(),
                    field: "priority".to_string(),
                    before,
                    after: priority.to_string(),
                }],
            )
            .await?;
        }
        Ok(outcome)
    }

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

    pub(crate) async fn update_availability(
        &mut self,
        index: Option<usize>,
        available_at: String,
        preserve_task: bool,
    ) -> Result<Option<MutationMessage>> {
        let Some(mut item) = self.selected_task(index).cloned() else {
            return Ok(None);
        };
        let before = item.task.available_at.clone();
        if available_at == before {
            return Ok(Some(
                self.refresh_task_message(
                    &item.task.id,
                    format!("unchanged {} availability", item.display_ref),
                )
                .await?,
            ));
        }

        self.activate_workspace();
        let mut conn = self.pool.acquire().await?;
        let outcome = update_task_operation(
            &mut conn,
            &item.task.id,
            TaskUpdate {
                available_at: Some(available_at.clone()),
                ..TaskUpdate::default()
            },
        )
        .await?;
        drop(conn);
        self.record_undo_commands(
            &format!("availability {}", item.display_ref),
            vec![UndoCommand::SetTaskField {
                task_id: item.task.id.clone(),
                field: "available_at".to_string(),
                before,
                after: available_at.clone(),
            }],
        )
        .await?;
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

    pub(crate) async fn update_project(
        &mut self,
        index: Option<usize>,
        project: String,
    ) -> Result<Option<MutationMessage>> {
        let Some(item) = self.selected_task(index).cloned() else {
            return Ok(None);
        };
        let before = item.task.project_id.clone();
        self.activate_workspace();
        let mut conn = self.pool.acquire().await?;
        let outcome = update_task_operation(
            &mut conn,
            &item.task.id,
            TaskUpdate {
                project: Some(project.clone()),
                ..TaskUpdate::default()
            },
        )
        .await?;
        drop(conn);
        self.record_undo_commands(
            &format!("project {}", item.display_ref),
            vec![UndoCommand::SetTaskField {
                task_id: item.task.id.clone(),
                field: "project".to_string(),
                before,
                after: outcome.task.project_id.clone(),
            }],
        )
        .await?;
        Ok(Some(
            self.refresh_task_message(&item.task.id, format!("set {} project", item.display_ref))
                .await?,
        ))
    }

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
        let outcome = self
            .update_selected_task(
                index,
                TaskUpdate {
                    add_labels,
                    remove_labels,
                    ..TaskUpdate::default()
                },
                |item| format!("set {} labels", item.display_ref),
            )
            .await?;
        if outcome.is_some() {
            self.record_undo_commands(
                &format!("labels {}", item.display_ref),
                vec![UndoCommand::SetTaskLabels {
                    task_id: item.task.id.clone(),
                    before: item.labels.clone(),
                    after: selected_labels,
                }],
            )
            .await?;
        }
        Ok(outcome)
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
            self.activate_workspace();
            let mut conn = self.pool.acquire().await?;
            set_deleted(&mut conn, &item.task, deleted).await?;
            drop(conn);
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
        depends_on_task_id: &str,
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
        task_id: &str,
        depends_on_task_id: &str,
    ) -> Result<MutationMessage> {
        self.activate_workspace();
        let mut conn = self.pool.acquire().await?;
        let outcome =
            crate::operations::add_task_dependency(&mut conn, task_id, depends_on_task_id).await?;
        let task_ref = crate::refs::display_ref(&mut conn, &outcome.task).await?;
        let depends_on_ref = crate::refs::display_ref(&mut conn, &outcome.depends_on).await?;
        drop(conn);
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
        depends_on_task_id: &str,
    ) -> Result<Option<MutationMessage>> {
        let Some(item) = self.selected_task(index).cloned() else {
            return Ok(None);
        };
        self.activate_workspace();
        let mut conn = self.pool.acquire().await?;
        let outcome =
            crate::operations::remove_task_dependency(&mut conn, &item.task.id, depends_on_task_id)
                .await?;
        let depends_on_ref = crate::refs::display_ref(&mut conn, &outcome.depends_on).await?;
        drop(conn);
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

    pub(crate) fn union_labels_for_tasks(&self, task_ids: &[String]) -> Vec<String> {
        let task_ids = task_ids.iter().collect::<BTreeSet<_>>();
        self.tasks
            .iter()
            .filter(|item| task_ids.contains(&item.task.id))
            .flat_map(|item| item.labels.iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    fn tasks_matching_ids(&self, task_ids: &[String]) -> Vec<TaskListItem> {
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
            .refresh(selected_id.as_deref().or(fallback_id.as_deref()))
            .await?;
        Ok(MutationMessage::new(message, selected))
    }

    pub(crate) async fn update_status_for_tasks(
        &mut self,
        current_selected_index: Option<usize>,
        task_ids: &[String],
        status: &str,
    ) -> Result<Option<MutationMessage>> {
        let targets = self.tasks_matching_ids(task_ids);
        if targets.is_empty() {
            return Ok(None);
        }

        self.activate_workspace();
        let mut undo_commands = Vec::new();
        let mut conn = self.pool.acquire().await?;
        for item in &targets {
            let before = item.task.status.as_str().to_string();
            if before == status {
                continue;
            }
            set_status(&mut conn, &item.task, status).await?;
            undo_commands.push(UndoCommand::SetTaskField {
                task_id: item.task.id.clone(),
                field: "status".to_string(),
                before,
                after: status.to_string(),
            });
        }
        drop(conn);

        let changed = undo_commands.len();
        if changed > 0 {
            self.record_undo_commands(&format!("status {changed} tasks"), undo_commands)
                .await?;
        }
        let message = if changed == 0 {
            format!("status unchanged on {} tasks", targets.len())
        } else {
            format!("set status on {changed} tasks")
        };
        Ok(Some(
            self.refresh_after_task_batch(current_selected_index, &targets, message)
                .await?,
        ))
    }

    pub(crate) async fn update_status_changes_for_tasks(
        &mut self,
        current_selected_index: Option<usize>,
        changes: &[(String, String)],
    ) -> Result<Option<MutationMessage>> {
        let status_by_id = changes.iter().cloned().collect::<BTreeMap<_, _>>();
        let task_ids = status_by_id.keys().cloned().collect::<Vec<_>>();
        let targets = self.tasks_matching_ids(&task_ids);
        if targets.is_empty() {
            return Ok(None);
        }

        self.activate_workspace();
        let mut undo_commands = Vec::new();
        let mut conn = self.pool.acquire().await?;
        for item in &targets {
            let Some(status) = status_by_id.get(&item.task.id) else {
                continue;
            };
            let before = item.task.status.as_str().to_string();
            if before == *status {
                continue;
            }
            set_status(&mut conn, &item.task, status).await?;
            undo_commands.push(UndoCommand::SetTaskField {
                task_id: item.task.id.clone(),
                field: "status".to_string(),
                before,
                after: status.clone(),
            });
        }
        drop(conn);

        let changed = undo_commands.len();
        if changed > 0 {
            self.record_undo_commands(
                &format!("move {changed} tasks between columns"),
                undo_commands,
            )
            .await?;
        }
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
        task_ids: &[String],
        reverse: bool,
    ) -> Result<Option<MutationMessage>> {
        let targets = self.tasks_matching_ids(task_ids);
        if targets.is_empty() {
            return Ok(None);
        }

        self.activate_workspace();
        let mut undo_commands = Vec::new();
        let mut conn = self.pool.acquire().await?;
        for item in &targets {
            let before = item.task.priority.as_str().to_string();
            let task = cycle_priority(&mut conn, &item.task, reverse).await?;
            undo_commands.push(UndoCommand::SetTaskField {
                task_id: item.task.id.clone(),
                field: "priority".to_string(),
                before,
                after: task.priority.as_str().to_string(),
            });
        }
        drop(conn);

        let changed = undo_commands.len();
        self.record_undo_commands(&format!("priority {changed} tasks"), undo_commands)
            .await?;
        Ok(Some(
            self.refresh_after_task_batch(
                current_selected_index,
                &targets,
                format!("set priority on {changed} tasks"),
            )
            .await?,
        ))
    }

    pub(crate) async fn set_exact_priority_for_tasks(
        &mut self,
        current_selected_index: Option<usize>,
        task_ids: &[String],
        priority: &str,
    ) -> Result<Option<MutationMessage>> {
        let targets = self.tasks_matching_ids(task_ids);
        if targets.is_empty() {
            return Ok(None);
        }

        self.activate_workspace();
        let mut undo_commands = Vec::new();
        let mut conn = self.pool.acquire().await?;
        for item in &targets {
            let before = item.task.priority.as_str().to_string();
            if before == priority {
                continue;
            }
            let task = set_priority(&mut conn, &item.task, priority).await?;
            undo_commands.push(UndoCommand::SetTaskField {
                task_id: item.task.id.clone(),
                field: "priority".to_string(),
                before,
                after: task.priority.as_str().to_string(),
            });
        }
        drop(conn);

        let changed = undo_commands.len();
        if changed > 0 {
            self.record_undo_commands(&format!("priority {changed} tasks"), undo_commands)
                .await?;
        }
        let message = if changed == 0 {
            format!("priority unchanged on {} tasks", targets.len())
        } else {
            format!("set priority on {changed} tasks")
        };
        Ok(Some(
            self.refresh_after_task_batch(current_selected_index, &targets, message)
                .await?,
        ))
    }

    pub(crate) async fn update_project_for_tasks(
        &mut self,
        current_selected_index: Option<usize>,
        task_ids: &[String],
        project: String,
    ) -> Result<Option<MutationMessage>> {
        let targets = self.tasks_matching_ids(task_ids);
        if targets.is_empty() {
            return Ok(None);
        }

        self.activate_workspace();
        let mut undo_commands = Vec::new();
        let mut conn = self.pool.acquire().await?;
        for item in &targets {
            let before = item.task.project_id.clone();
            let outcome = update_task_operation(
                &mut conn,
                &item.task.id,
                TaskUpdate {
                    project: Some(project.clone()),
                    ..TaskUpdate::default()
                },
            )
            .await?;
            if !outcome.changed {
                continue;
            }
            undo_commands.push(UndoCommand::SetTaskField {
                task_id: item.task.id.clone(),
                field: "project".to_string(),
                before,
                after: outcome.task.project_id.clone(),
            });
        }
        drop(conn);

        let changed = undo_commands.len();
        if changed > 0 {
            self.record_undo_commands(&format!("project {changed} tasks"), undo_commands)
                .await?;
        }
        let message = if changed == 0 {
            format!("project unchanged on {} tasks", targets.len())
        } else {
            format!("set project on {changed} tasks")
        };
        Ok(Some(
            self.refresh_after_task_batch(current_selected_index, &targets, message)
                .await?,
        ))
    }

    pub(crate) async fn update_deleted_for_tasks(
        &mut self,
        current_selected_index: Option<usize>,
        task_ids: &[String],
        deleted: bool,
    ) -> Result<Option<MutationMessage>> {
        let targets = self.tasks_matching_ids(task_ids);
        if targets.is_empty() {
            return Ok(None);
        }

        self.activate_workspace();
        let mut undo_commands = Vec::new();
        let mut conn = self.pool.acquire().await?;
        for item in &targets {
            if item.task.deleted == deleted {
                continue;
            }
            let before = if item.task.deleted { "1" } else { "0" };
            set_deleted(&mut conn, &item.task, deleted).await?;
            undo_commands.push(UndoCommand::SetTaskField {
                task_id: item.task.id.clone(),
                field: "deleted".to_string(),
                before: before.to_string(),
                after: if deleted { "1" } else { "0" }.to_string(),
            });
        }
        drop(conn);

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
        task_ids: &[String],
        selected_labels: Vec<String>,
    ) -> Result<Option<MutationMessage>> {
        if task_ids.is_empty() {
            return Ok(None);
        }
        let selected_id = self
            .selected_task(current_selected_index)
            .map(|item| item.task.id.clone());
        let task_ids = task_ids.iter().collect::<BTreeSet<_>>();
        let selected_label_set = selected_labels.iter().collect::<BTreeSet<_>>();
        let targets = self
            .tasks
            .iter()
            .filter(|item| task_ids.contains(&item.task.id))
            .cloned()
            .collect::<Vec<_>>();
        if targets.is_empty() {
            return Ok(None);
        }

        self.activate_workspace();
        let mut undo_commands = Vec::new();
        let mut conn = self.pool.acquire().await?;
        for item in &targets {
            let before = item.labels.clone();
            let add_labels = selected_labels
                .iter()
                .filter(|label| !before.contains(label))
                .cloned()
                .collect::<Vec<_>>();
            let remove_labels = before
                .iter()
                .filter(|label| !selected_label_set.contains(label))
                .cloned()
                .collect::<Vec<_>>();
            if add_labels.is_empty() && remove_labels.is_empty() {
                continue;
            }
            update_task_operation(
                &mut conn,
                &item.task.id,
                TaskUpdate {
                    add_labels,
                    remove_labels,
                    ..TaskUpdate::default()
                },
            )
            .await?;
            undo_commands.push(UndoCommand::SetTaskLabels {
                task_id: item.task.id.clone(),
                before,
                after: selected_labels.clone(),
            });
        }
        drop(conn);

        let changed = undo_commands.len();
        if changed > 0 {
            self.record_undo_commands(&format!("labels {changed} tasks"), undo_commands)
                .await?;
        }
        let fallback_id = targets.first().map(|item| item.task.id.clone());
        let selected = self
            .refresh(selected_id.as_deref().or(fallback_id.as_deref()))
            .await?;
        let message = if changed == 0 {
            format!("labels unchanged on {} tasks", targets.len())
        } else {
            format!("set labels on {changed} tasks")
        };
        Ok(Some(MutationMessage::new(message, selected)))
    }
}
