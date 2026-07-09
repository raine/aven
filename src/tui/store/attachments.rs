use std::path::Path;

use anyhow::Result;

use crate::operations::{AttachmentAddInput, add_task_attachment};
use crate::tui::store::MutationMessage;
use crate::undo::UndoCommand;

use super::TuiStore;

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
        let workspace = self.active_workspace.clone();
        let mut conn = self.pool.acquire().await?;
        let add_outcome =
            add_task_attachment(&mut conn, &workspace, blob_dir, &item.task.id, input).await?;
        let description_changed = add_outcome.description_changed;
        let outcome = add_outcome.outcome;
        drop(conn);
        if !description_changed {
            return Ok(Some(
                self.refresh_task_message(&item.task.id, "image already attached")
                    .await?,
            ));
        }
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
