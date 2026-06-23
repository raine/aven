use anyhow::Result;
use tokio::task::JoinHandle;

use crate::config::TaskIntakeConfig;
use crate::operations::{
    TaskDraft, add_note as add_note_operation, create_task as create_task_operation,
};
use crate::refs::display_ref;
use crate::undo::{UndoCommand, task_snapshot};

use super::TuiStore;

impl TuiStore {
    pub(crate) fn spawn_task_intake(
        &self,
        config: TaskIntakeConfig,
        input: String,
        project: Option<String>,
    ) -> JoinHandle<Result<TaskDraft>> {
        self.activate_workspace();
        let pool = self.pool.clone();
        let workspace = self.active_workspace.clone();
        tokio::spawn(async move {
            crate::workspaces::set_active_workspace(workspace);
            let mut conn = pool.acquire().await?;
            crate::task_intake::parse_task_intake_with_project(
                &mut conn,
                &config,
                &input,
                project.as_deref(),
            )
            .await
        })
    }

    pub(crate) async fn create_task(
        &mut self,
        draft: TaskDraft,
        current_selected_index: Option<usize>,
    ) -> Result<(String, Option<usize>)> {
        let previous_id = self
            .selected_task(current_selected_index)
            .map(|item| item.task.id.clone());
        self.activate_workspace();
        let mut conn = self.pool.acquire().await?;
        let outcome = create_task_operation(&mut conn, draft).await?;
        let task_id = outcome.task.id.clone();
        let message_ref = display_ref(&mut conn, &outcome.task).await?;
        let workspace_id = self.active_workspace.id.clone();
        let snapshot = task_snapshot(&mut conn, &workspace_id, &task_id).await?;
        drop(conn);
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

        let restored = self.restored_task_selection(previous_id.as_deref());
        Ok((
            format!("created task {message_ref} hidden by current filters"),
            restored,
        ))
    }

    pub(crate) async fn add_note_to_task(&mut self, task_id: &str, body: String) -> Result<String> {
        self.activate_workspace();
        let workspace_id = self.active_workspace.id.clone();
        let mut conn = self.pool.acquire().await?;
        let outcome = add_note_operation(&mut conn, task_id, body).await?;
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
                task_id: task_id.to_string(),
                note_id: outcome.note_id.clone(),
                note_add_change_id: note_change_id,
            }],
        )
        .await?;
        Ok(outcome.note_id)
    }
}
