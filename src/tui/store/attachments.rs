use std::path::Path;

use anyhow::Result;

use crate::operations::AttachmentAddInput;
use crate::tui::store::MutationMessage;

use super::TuiStore;

impl TuiStore {
    pub(crate) async fn add_attachment(
        &mut self,
        index: Option<usize>,
        blob_dir: &Path,
        lifecycle_policy: crate::attachments::lifecycle::LifecyclePolicy,
        input: AttachmentAddInput,
    ) -> Result<Option<MutationMessage>> {
        let Some(item) = self.selected_task(index).cloned() else {
            return Ok(None);
        };
        let workspace = self.active_workspace.clone();
        let add_outcome = self
            .database
            .add_task_attachment(&workspace, blob_dir, lifecycle_policy, &item.task.id, input)
            .await?;
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
