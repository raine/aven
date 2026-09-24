use std::collections::HashSet;

use anyhow::{Context, Result};
use sqlx::SqliteConnection;

use crate::change_log::op_type;
use crate::sync::wire::{AttachmentAddPayload, ChangeWire};

/// Collects attachment hashes whose liveness an applied change may affect.
pub(in crate::sync) async fn collect_attachment_liveness_hashes(
    conn: &mut SqliteConnection,
    change: &ChangeWire,
    affected_attachment_hashes: &mut HashSet<String>,
) -> Result<()> {
    match change.op_type.as_str() {
        op_type::ATTACHMENT_ADD => {
            let payload = AttachmentAddPayload::from_change(change)?;
            affected_attachment_hashes.insert(payload.sha256);
        }
        op_type::ATTACHMENT_DELETE => {
            let workspace_id = change.payload["workspace_id"]
                .as_str()
                .context("payload missing workspace_id")?;
            let attachment_id = change.payload["attachment_id"]
                .as_str()
                .context("payload missing attachment_id")?;
            let sha256: Option<String> = sqlx::query_scalar(
                "SELECT sha256 FROM task_attachments
                 WHERE workspace_id = ? AND attachment_id = ?",
            )
            .bind(workspace_id)
            .bind(attachment_id)
            .fetch_optional(&mut *conn)
            .await?;
            if let Some(sha256) = sha256 {
                affected_attachment_hashes.insert(sha256);
            }
        }
        op_type::SET_FIELD | op_type::RESOLVE_FIELD
            if change.field.as_deref() == Some("deleted") =>
        {
            let workspace_id = change.payload["workspace_id"]
                .as_str()
                .context("payload missing workspace_id")?;
            let hashes: Vec<String> = sqlx::query_scalar(
                "SELECT DISTINCT sha256 FROM task_attachments
                 WHERE workspace_id = ? AND task_id = ?",
            )
            .bind(workspace_id)
            .bind(&change.entity_id)
            .fetch_all(&mut *conn)
            .await?;
            affected_attachment_hashes.extend(hashes);
        }
        _ => {}
    }
    Ok(())
}
