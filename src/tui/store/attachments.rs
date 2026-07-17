use std::path::Path;

use anyhow::Result;

use crate::operations::{AttachmentAddInput, add_task_attachment};
use crate::tui::store::MutationMessage;

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
        let workspace = self.active_workspace.clone();
        let mut conn = self.pool.acquire().await?;
        let add_outcome =
            add_task_attachment(&mut conn, &workspace, blob_dir, &item.task.id, input).await?;
        drop(conn);
        let message = if add_outcome.created {
            "attached image"
        } else {
            "image already attached"
        };
        Ok(Some(
            self.refresh_task_message(&item.task.id, message).await?,
        ))
    }
}
