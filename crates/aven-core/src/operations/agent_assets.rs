use std::path::Path;

use anyhow::Result;
use sqlx::SqliteConnection;

use super::attachments::{
    AttachmentRecordDraft, existing_live_attachments_by_sha, prepare_stored_image,
    record_attachment_add, store_image_with_capacity,
};
use crate::attachments::ImageOptimizationPolicy;
use crate::attachments::lifecycle::{LifecyclePolicy, SystemClock};
use crate::db::{Database, begin_immediate};
use crate::error::CoreError;
use crate::ids::{TaskId, WorkspaceId, new_id, now};
use crate::workspaces::Workspace;

/// A staged image whose blob stays on disk until the pin is released.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub(crate) struct AgentAssetPin {
    pub pin_id: String,
    pub workspace_id: WorkspaceId,
    pub sha256: String,
    pub byte_size: i64,
    pub media_type: String,
    pub width: i64,
    pub height: i64,
    pub created_at: String,
}

pub(crate) struct PinnedAttachmentAdd {
    pub attachment_id: String,
    pub inserted: bool,
}

async fn find_pin(conn: &mut SqliteConnection, pin_id: &str) -> Result<Option<AgentAssetPin>> {
    Ok(sqlx::query_as(
        "SELECT pin_id, workspace_id, sha256, byte_size, media_type, width, height, created_at
         FROM local_agent_asset_pins WHERE pin_id = ?",
    )
    .bind(pin_id)
    .fetch_optional(&mut *conn)
    .await?)
}

/// Loads a pin of this workspace whose blob is available locally.
pub(crate) async fn require_available_pin(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    pin_id: &str,
) -> Result<AgentAssetPin> {
    let pin = find_pin(conn, pin_id)
        .await?
        .filter(|pin| pin.workspace_id == *workspace_id)
        .ok_or_else(|| CoreError::not_found(format!("error agent-asset-not-found pin={pin_id}")))?;
    let available: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM blob_inventory WHERE sha256 = ? AND available = 1)",
    )
    .bind(&pin.sha256)
    .fetch_one(&mut *conn)
    .await?;
    if !available {
        return Err(CoreError::not_found(format!(
            "error agent-asset-blob-unavailable pin={pin_id}"
        ))
        .into());
    }
    Ok(pin)
}

/// Adds the pinned image to the task unless a live attachment already holds
/// the same bytes. The caller routes recurrence and reconciles liveness.
pub(crate) async fn add_pinned_attachment_in_transaction(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &TaskId,
    attachment_id: &str,
    pin: &AgentAssetPin,
    filename: Option<&str>,
    alt_text: Option<&str>,
) -> Result<PinnedAttachmentAdd> {
    if let Some(existing) =
        existing_live_attachments_by_sha(conn, &workspace.id, task_id, &pin.sha256)
            .await?
            .into_iter()
            .next()
    {
        return Ok(PinnedAttachmentAdd {
            attachment_id: existing.attachment_id,
            inserted: false,
        });
    }
    let created_at = now();
    record_attachment_add(
        conn,
        workspace,
        task_id,
        AttachmentRecordDraft {
            attachment_id,
            sha256: &pin.sha256,
            byte_size: pin.byte_size,
            media_type: &pin.media_type,
            filename,
            alt_text,
            width: pin.width,
            height: pin.height,
            created_at: &created_at,
        },
    )
    .await?;
    Ok(PinnedAttachmentAdd {
        attachment_id: attachment_id.to_string(),
        inserted: true,
    })
}

async fn delete_pins_and_unreferenced_blobs(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    pins: Vec<AgentAssetPin>,
) -> Result<()> {
    let mut tx = begin_immediate(conn).await?;
    for pin in &pins {
        sqlx::query("DELETE FROM local_agent_asset_pins WHERE pin_id = ?")
            .bind(&pin.pin_id)
            .execute(&mut *tx)
            .await?;
    }
    let mut hashes = pins.into_iter().map(|pin| pin.sha256).collect::<Vec<_>>();
    hashes.sort();
    hashes.dedup();
    crate::attachments::lifecycle::reconcile_liveness_for_hashes_in_transaction(
        &mut tx,
        &hashes,
        &SystemClock,
    )
    .await?;
    tx.commit().await?;
    for sha256 in hashes {
        crate::attachments::storage::remove_staged_blob_if_unreferenced(conn, blob_dir, &sha256)
            .await;
    }
    Ok(())
}

impl Database {
    pub(crate) async fn stage_agent_asset(
        &self,
        workspace_id: &WorkspaceId,
        blob_dir: &Path,
        policy: LifecyclePolicy,
        bytes: Vec<u8>,
        declared_media_type: Option<String>,
        optimization_policy: ImageOptimizationPolicy,
    ) -> Result<AgentAssetPin> {
        let (image, _) = prepare_stored_image(bytes, declared_media_type, optimization_policy)
            .await
            .map_err(|error| CoreError::validation(error.to_string()))?;
        let mut conn = self.acquire_writer().await?;
        let (stored, staging_lease) =
            store_image_with_capacity(&mut conn, blob_dir, policy, image).await?;
        let pin = AgentAssetPin {
            pin_id: new_id(),
            workspace_id: workspace_id.clone(),
            sha256: stored.sha256,
            byte_size: stored.byte_size,
            media_type: stored.facts.media_type,
            width: stored.facts.width,
            height: stored.facts.height,
            created_at: now(),
        };
        let database_result = async {
            let mut tx = begin_immediate(&mut conn).await?;
            sqlx::query(
                "INSERT INTO local_agent_asset_pins(
                   pin_id, workspace_id, sha256, byte_size, media_type, width, height, created_at
                 ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&pin.pin_id)
            .bind(&pin.workspace_id)
            .bind(&pin.sha256)
            .bind(pin.byte_size)
            .bind(&pin.media_type)
            .bind(pin.width)
            .bind(pin.height)
            .bind(&pin.created_at)
            .execute(&mut *tx)
            .await?;
            crate::attachments::lifecycle::reconcile_liveness_for_hashes_in_transaction(
                &mut tx,
                std::slice::from_ref(&pin.sha256),
                &SystemClock,
            )
            .await?;
            tx.commit().await?;
            Ok::<_, anyhow::Error>(())
        }
        .await;
        let release_result =
            crate::attachments::lifecycle::release_lease(&mut conn, &staging_lease).await;
        if let Err(error) = database_result {
            crate::attachments::storage::remove_staged_blob_if_unreferenced(
                &mut conn,
                blob_dir,
                &pin.sha256,
            )
            .await;
            return Err(error);
        }
        release_result?;
        Ok(pin)
    }

    /// Returns false when the pin was already released.
    pub(crate) async fn release_agent_asset(&self, blob_dir: &Path, pin_id: &str) -> Result<bool> {
        let mut conn = self.acquire_writer().await?;
        let Some(pin) = find_pin(&mut conn, pin_id).await? else {
            return Ok(false);
        };
        delete_pins_and_unreferenced_blobs(&mut conn, blob_dir, vec![pin]).await?;
        Ok(true)
    }

    /// Releases pins created before `cutoff` and returns how many were released.
    pub(crate) async fn sweep_agent_assets(&self, blob_dir: &Path, cutoff: &str) -> Result<usize> {
        let mut conn = self.acquire_writer().await?;
        let pins: Vec<AgentAssetPin> = sqlx::query_as(
            "SELECT pin_id, workspace_id, sha256, byte_size, media_type, width, height, created_at
             FROM local_agent_asset_pins WHERE created_at < ?",
        )
        .bind(cutoff)
        .fetch_all(&mut *conn)
        .await?;
        let count = pins.len();
        if count > 0 {
            delete_pins_and_unreferenced_blobs(&mut conn, blob_dir, pins).await?;
        }
        Ok(count)
    }
}
