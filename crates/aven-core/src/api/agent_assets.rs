use super::*;

pub use crate::attachments::ImageOptimizationPolicy;

/// An image an assistant session holds before attaching it to tasks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentAssetInput {
    pub bytes: Vec<u8>,
    pub declared_media_type: Option<String>,
    pub optimization_policy: ImageOptimizationPolicy,
}

/// Facts describe the stored bytes, after optimization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedAgentAsset {
    pub pin_id: String,
    pub sha256: String,
    pub byte_size: i64,
    pub media_type: String,
    pub width: i64,
    pub height: i64,
}

impl Store {
    /// Validates and optimizes the image, writes its blob and pins it, so a later
    /// `ChangeOp::AddAttachment` only touches the database. The pin outlives
    /// applied change sets; release it when the session no longer needs the image.
    pub async fn stage_agent_asset(
        &self,
        workspace_id: &WorkspaceId,
        input: AgentAssetInput,
    ) -> Result<StagedAgentAsset, Error> {
        self.workspace(workspace_id).await?;
        let pin = self
            .database
            .stage_agent_asset(
                workspace_id,
                &crate::attachments::default_blob_dir(self.database.path()),
                crate::attachments::LifecyclePolicy::default(),
                input.bytes,
                input.declared_media_type,
                input.optimization_policy,
            )
            .await
            .map_err(Error::from_internal)?;
        Ok(StagedAgentAsset {
            pin_id: pin.pin_id,
            sha256: pin.sha256,
            byte_size: pin.byte_size,
            media_type: pin.media_type,
            width: pin.width,
            height: pin.height,
        })
    }

    /// Drops the pin and removes the blob unless something else references it.
    /// Returns false when the pin was already released.
    pub async fn release_agent_asset(&self, pin_id: &str) -> Result<bool, Error> {
        self.database
            .release_agent_asset(
                &crate::attachments::default_blob_dir(self.database.path()),
                pin_id,
            )
            .await
            .map_err(Error::from_internal)
    }

    /// Releases every pin staged more than `older_than` ago and returns the count.
    pub async fn sweep_agent_assets(&self, older_than: std::time::Duration) -> Result<u32, Error> {
        let Some(cutoff) = chrono::Duration::from_std(older_than)
            .ok()
            .and_then(|age| crate::ids::now_utc().checked_sub_signed(age))
        else {
            return Ok(0);
        };
        let released = self
            .database
            .sweep_agent_assets(
                &crate::attachments::default_blob_dir(self.database.path()),
                &cutoff.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            )
            .await
            .map_err(Error::from_internal)?;
        Ok(u32::try_from(released).unwrap_or(u32::MAX))
    }
}
