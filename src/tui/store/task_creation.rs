use anyhow::Result;
use tokio::task::JoinHandle;

use crate::config::TaskIntakeConfig;
use crate::operations::TaskDraft;
use crate::undo::UndoCommand;

use super::TuiStore;

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
        let previous_id = self
            .selected_task(current_selected_index)
            .map(|item| item.task.id.clone());
        if draft.project.is_none() {
            draft.project = self.inferred_add_project().await?;
        }
        let outcome = self
            .database
            .create_task(&self.active_workspace, draft)
            .await?;
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
                expected: snapshot,
            }],
        )
        .await?;

        self.refresh(None).await?;
        let created_index = self.tasks.iter().position(|item| item.task.id == task_id);
        if created_index.is_some() {
            return Ok((format!("created task {message_ref}"), created_index));
        }

        let restored = self.restored_task_selection(previous_id.as_ref());
        Ok((
            format!("created task {message_ref} hidden by current filters"),
            restored,
        ))
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
