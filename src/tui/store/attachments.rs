use std::path::Path;

use anyhow::Result;

use crate::attachments::export::{LeasedImageExport, lease_image_export};
use crate::operations::TaskAttachmentAddInput;

use super::TuiStore;

#[derive(Clone)]
pub(crate) struct AttachmentWorkerContext {
    pool: sqlx::SqlitePool,
    workspace: crate::workspaces::Workspace,
}

impl AttachmentWorkerContext {
    pub(crate) async fn add_ordered_attachment(
        &self,
        blob_dir: &Path,
        lifecycle_policy: crate::attachments::lifecycle::LifecyclePolicy,
        task_id: &crate::ids::TaskId,
        created_at: String,
        input: TaskAttachmentAddInput,
    ) -> Result<bool> {
        let mut conn = self.pool.acquire().await?;
        Ok(crate::operations::add_ordered_task_attachment(
            &mut conn,
            &self.workspace,
            blob_dir,
            lifecycle_policy,
            task_id,
            created_at,
            input,
        )
        .await?
        .created)
    }
}

impl TuiStore {
    pub(crate) fn attachment_worker_context(&self) -> AttachmentWorkerContext {
        AttachmentWorkerContext {
            pool: self.pool.clone(),
            workspace: self.active_workspace.clone(),
        }
    }

    pub(crate) async fn lease_image_export(
        &self,
        blob_dir: &Path,
        attachment_id: &str,
    ) -> Result<LeasedImageExport> {
        lease_image_export(&self.pool, &self.active_workspace, blob_dir, attachment_id).await
    }

    pub(crate) async fn release_image_export(&self, export: &mut LeasedImageExport) -> Result<()> {
        export.release().await
    }
}
