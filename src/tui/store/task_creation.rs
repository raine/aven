use std::path::Path;

use anyhow::Result;
use tokio::task::JoinHandle;

use crate::config::TaskIntakeConfig;
use crate::operations::{
    TaskAttachmentAddInput, TaskDraft, TaskOutcome, add_note as add_note_operation,
    create_task as create_task_operation,
    create_task_with_attachments as create_task_with_attachments_operation,
};
use crate::refs::DisplayRefContext;
use crate::tui::authoring::PendingTaskAttachment;
use crate::undo::{UndoCommand, task_snapshot};

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
        let pool = self.pool.clone();
        let workspace = self.active_workspace.clone();
        tokio::spawn(async move {
            let context = {
                let mut conn = pool.acquire().await?;
                crate::task_intake::TaskIntakeContext::load_with_project(
                    &mut conn,
                    &workspace,
                    project.as_deref(),
                )
                .await?
            };
            let output =
                crate::task_intake::run_task_intake_command(&config, &context, &input).await?;
            let mut conn = pool.acquire().await?;
            crate::task_intake::parsed_output_to_draft(&mut conn, &context, &output).await
        })
    }

    pub(crate) async fn create_task(
        &mut self,
        draft: TaskDraft,
        current_selected_index: Option<usize>,
    ) -> Result<(String, Option<usize>)> {
        let mut conn = self.pool.acquire().await?;
        let outcome = create_task_operation(&mut conn, &self.active_workspace, draft).await?;
        drop(conn);
        let created = self
            .finish_task_creation(outcome, Vec::new(), current_selected_index)
            .await
            .map_err(committed_error)?;
        Ok((created.message, created.selected))
    }

    pub(crate) async fn create_task_with_attachments(
        &mut self,
        draft: TaskDraft,
        current_selected_index: Option<usize>,
        blob_dir: &Path,
        attachments: Vec<PendingTaskAttachment>,
    ) -> Result<(String, Option<usize>)> {
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
        let mut conn = self.pool.acquire().await?;
        let outcome = create_task_with_attachments_operation(
            &mut conn,
            &self.active_workspace,
            blob_dir,
            draft,
            inputs,
        )
        .await?;
        drop(conn);
        let created = self
            .finish_task_creation(outcome, attachment_ids, current_selected_index)
            .await
            .map_err(committed_error)?;
        Ok((created.message, created.selected))
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
        let mut conn = self.pool.acquire().await?;
        let display_refs =
            DisplayRefContext::for_workspace(&mut conn, &self.active_workspace.id).await?;
        let message_ref = display_refs.display_ref(&outcome.task);
        let workspace_id = self.active_workspace.id.clone();
        let snapshot = task_snapshot(&mut conn, &workspace_id, &task_id).await?;
        drop(conn);
        self.record_undo_commands(
            &format!("task {task_id}"),
            vec![UndoCommand::DeleteCreatedTask {
                task_id: task_id.clone(),
                create_change_id: outcome.create_change_id,
                expected: snapshot,
                attachment_ids,
                attachment_change_ids: outcome.attachment_change_ids,
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
        let workspace_id = self.active_workspace.id.clone();
        let mut conn = self.pool.acquire().await?;
        let outcome = add_note_operation(&mut conn, &self.active_workspace, task_id, body).await?;
        let note_change_id: String = sqlx::query_scalar(
            "SELECT change_id FROM notes WHERE workspace_id = ? AND id = ? AND task_id = ?",
        )
        .bind(&workspace_id)
        .bind(&outcome.note_id)
        .bind(task_id)
        .fetch_one(&mut *conn)
        .await?;
        drop(conn);
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
