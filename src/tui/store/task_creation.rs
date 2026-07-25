use std::path::Path;

use anyhow::Result;
use tokio::task::JoinHandle;

use crate::config::TaskIntakeConfig;
use crate::operations::{TaskAttachmentAddInput, TaskDraft, TaskOutcome};
use crate::tui::authoring::PendingTaskAttachment;
use crate::undo::UndoCommand;

use super::TuiStore;

struct CreatedTaskMessage {
    message: String,
    selected: Option<usize>,
}

#[derive(Debug)]
pub(crate) struct TaskCreationCommittedError {
    source: anyhow::Error,
}

impl std::fmt::Display for TaskCreationCommittedError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "task committed but TUI finalization failed: {}",
            self.source
        )
    }
}

impl std::error::Error for TaskCreationCommittedError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

pub(crate) fn task_creation_committed(error: &anyhow::Error) -> bool {
    error.downcast_ref::<TaskCreationCommittedError>().is_some()
}

fn committed_error(source: anyhow::Error) -> anyhow::Error {
    TaskCreationCommittedError { source }.into()
}

impl TuiStore {
    pub(crate) fn spawn_task_intake(
        &self,
        config: TaskIntakeConfig,
        input: String,
        project: Option<String>,
    ) -> JoinHandle<Result<TaskDraft>> {
        let database = self.database.clone();
        let workspace = self.active_workspace.clone();
        tokio::spawn(async move {
            let context = crate::task_intake::TaskIntakeContext::load_with_database(
                &database,
                &workspace,
                project.as_deref(),
            )
            .await?;
            let output =
                crate::task_intake::run_task_intake_command(&config, &context, &input).await?;
            crate::task_intake::parsed_output_to_draft_with_database(&database, &context, &output)
                .await
        })
    }

    pub(crate) async fn create_task(
        &mut self,
        mut draft: TaskDraft,
        current_selected_index: Option<usize>,
    ) -> Result<(String, Option<usize>)> {
        if draft.project.is_none() {
            draft.project = self.inferred_add_project().await?;
        }
        let outcome = self
            .database
            .create_task(&self.active_workspace, draft)
            .await?;
        let created = self
            .finish_task_creation(outcome, Vec::new(), current_selected_index)
            .await
            .map_err(committed_error)?;
        Ok((created.message, created.selected))
    }

    pub(crate) async fn create_task_for_epic(
        &mut self,
        mut draft: TaskDraft,
        current_selected_index: Option<usize>,
        epic: &super::EpicContext,
    ) -> Result<(String, Option<usize>, crate::ids::TaskId)> {
        draft.project = Some(epic.project_key.clone());
        let outcome = self
            .database
            .create_task_for_epic(&self.active_workspace, draft, &epic.epic_id)
            .await?;
        let task_id = outcome.task.id.clone();
        let child_ref = self
            .database
            .display_ref_context(&self.active_workspace.id)
            .await?
            .display_ref(&outcome.task);
        let created = self
            .finish_epic_child_creation(
                outcome,
                Vec::new(),
                current_selected_index,
                epic,
                &child_ref,
            )
            .await
            .map_err(committed_error)?;
        Ok((created.message, created.selected, task_id))
    }

    pub(crate) async fn create_task_with_attachments(
        &mut self,
        mut draft: TaskDraft,
        current_selected_index: Option<usize>,
        blob_dir: &Path,
        lifecycle_policy: crate::attachments::lifecycle::LifecyclePolicy,
        attachments: Vec<PendingTaskAttachment>,
    ) -> Result<(String, Option<usize>)> {
        if draft.project.is_none() {
            draft.project = self.inferred_add_project().await?;
        }
        let attachment_ids = attachments
            .iter()
            .map(|attachment| attachment.attachment_id.clone())
            .collect();
        let inputs = attachments
            .into_iter()
            .map(|attachment| TaskAttachmentAddInput {
                attachment_id: attachment.attachment_id,
                input: attachment.input,
            })
            .collect();
        let outcome = self
            .database
            .create_task_with_attachments(
                &self.active_workspace,
                blob_dir,
                lifecycle_policy,
                draft,
                inputs,
            )
            .await?;
        let created = self
            .finish_task_creation(outcome, attachment_ids, current_selected_index)
            .await
            .map_err(committed_error)?;
        Ok((created.message, created.selected))
    }

    pub(crate) async fn create_task_with_attachments_for_epic(
        &mut self,
        mut draft: TaskDraft,
        current_selected_index: Option<usize>,
        blob_dir: &Path,
        lifecycle_policy: crate::attachments::lifecycle::LifecyclePolicy,
        attachments: Vec<PendingTaskAttachment>,
        epic: &super::EpicContext,
    ) -> Result<(String, Option<usize>, crate::ids::TaskId)> {
        draft.project = Some(epic.project_key.clone());
        let attachment_ids = attachments
            .iter()
            .map(|attachment| attachment.attachment_id.clone())
            .collect();
        let inputs = attachments
            .into_iter()
            .map(|attachment| TaskAttachmentAddInput {
                attachment_id: attachment.attachment_id,
                input: attachment.input,
            })
            .collect();
        let outcome = self
            .database
            .create_task_with_attachments_for_epic(
                &self.active_workspace,
                blob_dir,
                lifecycle_policy,
                draft,
                inputs,
                &epic.epic_id,
            )
            .await?;
        let task_id = outcome.task.id.clone();
        let child_ref = self
            .database
            .display_ref_context(&self.active_workspace.id)
            .await?
            .display_ref(&outcome.task);
        let created = self
            .finish_epic_child_creation(
                outcome,
                attachment_ids,
                current_selected_index,
                epic,
                &child_ref,
            )
            .await
            .map_err(committed_error)?;
        Ok((created.message, created.selected, task_id))
    }

    async fn finish_epic_child_creation(
        &mut self,
        outcome: TaskOutcome,
        _attachment_ids: Vec<String>,
        current_selected_index: Option<usize>,
        epic: &super::EpicContext,
        child_ref: &str,
    ) -> Result<CreatedTaskMessage> {
        let task_id = outcome.task.id.clone();
        self.record_undo_commands(
            &format!("add {child_ref} to {}", epic.display_ref),
            vec![UndoCommand::AddEpicChild {
                epic_id: epic.epic_id.clone(),
                child_id: task_id.clone(),
            }],
        )
        .await?;
        self.view_state.collapsed_epic_ids.remove(&epic.epic_id);
        self.view_state
            .expanded_epic_ids
            .insert(epic.epic_id.clone());
        self.refresh(Some(&epic.epic_id)).await?;
        let selected = self.tasks.iter().position(|item| item.task.id == task_id);
        Ok(CreatedTaskMessage {
            message: format!("Added {child_ref} to {}", epic.display_ref),
            selected: selected.or(current_selected_index),
        })
    }

    async fn finish_task_creation(
        &mut self,
        outcome: TaskOutcome,
        attachment_ids: Vec<String>,
        current_selected_index: Option<usize>,
    ) -> Result<CreatedTaskMessage> {
        let previous_id = self
            .selected_task(current_selected_index)
            .map(|item| item.task.id.clone());
        let task_id = outcome.task.id.clone();
        let display_refs = self
            .database
            .display_ref_context(&self.active_workspace.id)
            .await?;
        let message_ref = display_refs.display_ref(&outcome.task);
        let workspace_id = self.active_workspace.id.clone();
        let snapshot = self
            .database
            .task_undo_snapshot(&workspace_id, &task_id)
            .await?;
        self.record_undo_commands(
            &format!("task {task_id}"),
            vec![UndoCommand::DeleteCreatedTask {
                task_id: task_id.clone(),
                create_change_id: outcome.create_change_id,
                attachment_ids,
                attachment_change_ids: outcome.attachment_change_ids,
                expected: snapshot,
            }],
        )
        .await?;

        self.refresh(None).await?;
        let created_index = self.tasks.iter().position(|item| item.task.id == task_id);
        if created_index.is_some() {
            return Ok(CreatedTaskMessage {
                message: format!("created task {message_ref}"),
                selected: created_index,
            });
        }

        let restored = self.restored_task_selection(previous_id.as_ref());
        Ok(CreatedTaskMessage {
            message: format!("created task {message_ref} hidden by current filters"),
            selected: restored,
        })
    }

    pub(crate) async fn add_note_to_task(
        &mut self,
        task_id: &crate::ids::TaskId,
        body: String,
    ) -> Result<String> {
        let outcome = self
            .database
            .add_note(&self.active_workspace, task_id, body)
            .await?;
        let note_change_id = outcome.change_id.clone();
        self.record_undo_commands(
            &format!("note {}", outcome.note_id),
            vec![UndoCommand::DeleteCreatedNote {
                task_id: task_id.clone(),
                note_id: outcome.note_id.clone(),
                note_add_change_id: note_change_id,
            }],
        )
        .await?;
        Ok(outcome.note_id)
    }
}
