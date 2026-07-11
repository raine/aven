use std::path::Path;

use anyhow::Result;

use crate::attachments::storage::sha256_hex;
use crate::operations::{AttachmentAddInput, add_task_attachment};
use crate::tui::store::MutationMessage;
use crate::undo::UndoCommand;

use super::TuiStore;

async fn attachment_sha_exists_for_task(
    conn: &mut sqlx::SqliteConnection,
    workspace_id: &crate::ids::WorkspaceId,
    task_id: &crate::ids::TaskId,
    sha256: &str,
) -> Result<bool> {
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM task_attachments
         WHERE workspace_id = ? AND task_id = ? AND sha256 = ? AND deleted = 0",
    )
    .bind(workspace_id)
    .bind(task_id)
    .bind(sha256)
    .fetch_one(&mut *conn)
    .await?;
    Ok(count > 0)
}

impl TuiStore {
    pub(crate) async fn add_attachment(
        &mut self,
        index: Option<usize>,
        blob_dir: &Path,
        input: AttachmentAddInput,
    ) -> Result<Option<MutationMessage>> {
        let Some(item) = self.selected_task(index).cloned() else {
            return Ok(None);
        };
        let before = item.task.description.clone();
        let sha256 = sha256_hex(&input.bytes);
        let mut conn = self.pool.acquire().await?;
        if attachment_sha_exists_for_task(
            &mut conn,
            &self.active_workspace.id,
            &item.task.id,
            &sha256,
        )
        .await?
        {
            return Ok(Some(MutationMessage::new("image already attached", index)));
        }
        let outcome = add_task_attachment(
            &mut conn,
            &self.active_workspace,
            blob_dir,
            &item.task.id,
            input,
        )
        .await?;
        drop(conn);
        self.record_undo_commands(
            &format!("attachment {}", item.display_ref),
            vec![UndoCommand::SetTaskField {
                task_id: item.task.id.clone(),
                field: "description".to_string(),
                before,
                after: outcome.task.description.clone(),
            }],
        )
        .await?;
        Ok(Some(
            self.refresh_task_message(&item.task.id, "attached image")
                .await?,
        ))
    }
}
