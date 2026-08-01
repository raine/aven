use std::path::Path;

use anyhow::Result;
use aven_core::db::Database;

use crate::attachments::export::{LeasedImageExport, lease_image_export};
use crate::operations::TaskAttachmentAddInput;

use super::TuiStore;

#[derive(Clone)]
pub(crate) struct AttachmentWorkerContext {
    database: Database,
    workspace: crate::workspaces::Workspace,
    app_config: crate::config::AppConfig,
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
        let outcome = self
            .database
            .add_ordered_task_attachment(
                &self.workspace,
                blob_dir,
                lifecycle_policy,
                task_id,
                created_at,
                input,
            )
            .await?;
        crate::daemon::wake_if_enabled(&self.app_config, self.database.path());
        Ok(outcome.created)
    }
}

impl TuiStore {
    pub(crate) fn attachment_worker_context(&self) -> AttachmentWorkerContext {
        AttachmentWorkerContext {
            database: self.database.clone(),
            workspace: self.active_workspace.clone(),
            app_config: self.app_config.clone(),
        }
    }

    pub(crate) async fn delete_attachment(&self, attachment_id: &str) -> Result<()> {
        self.database
            .delete_task_attachment(&self.active_workspace, attachment_id)
            .await?;
        self.wake_after_mutation();
        Ok(())
    }

    pub(crate) async fn lease_image_export(
        &self,
        blob_dir: &Path,
        attachment_id: &str,
    ) -> Result<LeasedImageExport> {
        lease_image_export(
            &self.database,
            &self.active_workspace,
            blob_dir,
            attachment_id,
        )
        .await
    }

    pub(crate) async fn release_image_export(&self, export: &mut LeasedImageExport) -> Result<()> {
        export.release().await
    }
}
