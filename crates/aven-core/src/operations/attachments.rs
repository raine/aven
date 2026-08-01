use std::path::Path;

use anyhow::{Result, bail};
use sqlx::SqliteConnection;
use sqlx::{AssertSqlSafe, Row};

use crate::attachments::AttachmentBytesState;
use crate::attachments::decode::{ImageFacts, ValidatedImage, validate_image};
use crate::attachments::optimization::{ImageOptimizationPolicy, optimize_image_bytes};
use crate::attachments::storage::{blob_inventory_row, sha256_hex, store_validated_blob};
use crate::attachments::validation::{
    validate_alt_text, validate_attachment_id, validate_filename,
};
use crate::change_log::{ChangeEntity, ChangePayload, append_change, op_type};
use crate::db::{Database, begin_immediate};
use crate::ids::{TaskId, WorkspaceId, new_id, now};
use crate::types::TaskAttachment;
use crate::workspaces::Workspace;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentAddInput {
    pub filename: Option<String>,
    pub alt_text: Option<String>,
    pub declared_media_type: Option<String>,
    pub bytes: Vec<u8>,
    pub optimization_policy: ImageOptimizationPolicy,
    pub dedupe_existing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskAttachmentAddInput {
    pub attachment_id: String,
    pub input: AttachmentAddInput,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedAttachment {
    pub attachment_id: String,
    pub filename: Option<String>,
    pub alt_text: Option<String>,
    pub sha256: String,
    pub byte_size: i64,
    pub facts: ImageFacts,
    pub bytes: Vec<u8>,
    pub optimized: bool,
}

pub struct AttachmentAddOutcome {
    pub outcome: AttachmentOutcome,
    pub created: bool,
    pub optimized: bool,
}

pub struct AttachmentOutcome {
    pub attachment: TaskAttachment,
    pub has_blob: bool,
}

pub struct AttachmentReadLease {
    pub sha256: String,
    pub media_type: String,
    pub lease_id: String,
}

#[derive(Debug, Clone)]
pub struct AttachmentReadItem {
    pub attachment: TaskAttachment,
    pub bytes_state: AttachmentBytesState,
    pub has_blob: bool,
}

const ATTACHMENT_COLUMNS: &str =
    "ta.workspace_id, ta.attachment_id, ta.task_id, ta.sha256, ta.byte_size,
    ta.media_type, ta.filename, ta.alt_text, ta.width, ta.height,
    ta.created_at, ta.created_by_change_id, ta.deleted, ta.deleted_at, ta.deleted_by_change_id";

fn attachment_from_row(row: &sqlx::sqlite::SqliteRow) -> TaskAttachment {
    TaskAttachment {
        workspace_id: row.get("workspace_id"),
        attachment_id: row.get("attachment_id"),
        task_id: row.get("task_id"),
        sha256: row.get("sha256"),
        byte_size: row.get("byte_size"),
        media_type: row.get("media_type"),
        filename: row.get("filename"),
        alt_text: row.get("alt_text"),
        width: row.get("width"),
        height: row.get("height"),
        created_at: row.get("created_at"),
        created_by_change_id: row.get("created_by_change_id"),
        deleted: row.get::<i64, _>("deleted") != 0,
        deleted_at: row.get("deleted_at"),
        deleted_by_change_id: row.get("deleted_by_change_id"),
    }
}

async fn require_attachment(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    attachment_id: &str,
) -> Result<TaskAttachment> {
    let row = sqlx::query(AssertSqlSafe(format!(
        "SELECT {ATTACHMENT_COLUMNS}
         FROM task_attachments ta
         WHERE ta.workspace_id = ? AND ta.attachment_id = ?"
    )))
    .bind(workspace_id)
    .bind(attachment_id)
    .fetch_optional(&mut *conn)
    .await?;

    let Some(row) = row else {
        bail!("error attachment-not-found id={}", attachment_id);
    };
    Ok(attachment_from_row(&row))
}

async fn attachment_bytes_state(
    conn: &mut SqliteConnection,
    sha256: &str,
) -> Result<AttachmentBytesState> {
    Ok(match blob_inventory_row(conn, sha256).await? {
        Some(row) if row.available => AttachmentBytesState::Present,
        Some(_) => AttachmentBytesState::Unavailable,
        None => AttachmentBytesState::PendingDownload,
    })
}

async fn attachment_has_blob(conn: &mut SqliteConnection, sha256: &str) -> Result<bool> {
    Ok(attachment_bytes_state(conn, sha256).await? == AttachmentBytesState::Present)
}

async fn existing_live_attachments_by_sha(
    conn: &mut SqliteConnection,
    workspace_id: &crate::ids::WorkspaceId,
    task_id: &TaskId,
    sha256: &str,
) -> Result<Vec<TaskAttachment>> {
    let rows = sqlx::query(AssertSqlSafe(format!(
        "SELECT {ATTACHMENT_COLUMNS}
         FROM task_attachments ta
         WHERE ta.workspace_id = ? AND ta.task_id = ? AND ta.sha256 = ? AND ta.deleted = 0
         ORDER BY ta.created_at, ta.attachment_id"
    )))
    .bind(workspace_id)
    .bind(task_id)
    .bind(sha256)
    .fetch_all(&mut *conn)
    .await?;

    Ok(rows.iter().map(attachment_from_row).collect())
}

pub async fn prepare_task_attachment(input: TaskAttachmentAddInput) -> Result<PreparedAttachment> {
    validate_attachment_id(&input.attachment_id)?;
    validate_filename(input.input.filename.as_deref())?;
    validate_alt_text(input.input.alt_text.as_deref())?;

    let source = validate_image(input.input.bytes, input.input.declared_media_type).await?;
    let source_facts = source.facts.clone();
    let optimized = optimize_image_bytes(
        &source_facts.media_type,
        source.bytes,
        input.input.optimization_policy,
    )
    .await?;
    let optimized_flag = optimized.optimized;
    let stored_image = if optimized_flag {
        validate_image(optimized.bytes, None).await?
    } else {
        ValidatedImage {
            bytes: optimized.bytes,
            facts: source_facts,
        }
    };
    let sha256 = sha256_hex(&stored_image.bytes);
    let byte_size = i64::try_from(stored_image.bytes.len())?;
    Ok(PreparedAttachment {
        attachment_id: input.attachment_id,
        filename: input.input.filename,
        alt_text: input.input.alt_text,
        sha256,
        byte_size,
        facts: stored_image.facts,
        bytes: stored_image.bytes,
        optimized: optimized_flag,
    })
}

struct AttachmentRecordDraft<'a> {
    attachment_id: &'a str,
    sha256: &'a str,
    byte_size: i64,
    media_type: &'a str,
    filename: Option<&'a str>,
    alt_text: Option<&'a str>,
    width: i64,
    height: i64,
    created_at: &'a str,
}

async fn record_attachment_add(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &TaskId,
    draft: AttachmentRecordDraft<'_>,
) -> Result<String> {
    let change_id = append_change(
        conn,
        ChangeEntity::Task,
        task_id,
        Some("attachments"),
        op_type::ATTACHMENT_ADD,
        ChangePayload::workspace(workspace)
            .set("attachment_id", draft.attachment_id)
            .set("sha256", draft.sha256)
            .set("byte_size", draft.byte_size)
            .set("media_type", draft.media_type)
            .set("filename", draft.filename)
            .set("alt_text", draft.alt_text)
            .set("width", draft.width)
            .set("height", draft.height)
            .set("created_at", draft.created_at),
    )
    .await?;
    sqlx::query(
        "INSERT INTO task_attachments(workspace_id, attachment_id, task_id, sha256, byte_size, media_type, filename, alt_text, width, height, created_at, created_by_change_id)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&workspace.id)
    .bind(draft.attachment_id)
    .bind(task_id)
    .bind(draft.sha256)
    .bind(draft.byte_size)
    .bind(draft.media_type)
    .bind(draft.filename)
    .bind(draft.alt_text)
    .bind(draft.width)
    .bind(draft.height)
    .bind(draft.created_at)
    .bind(&change_id)
    .execute(&mut *conn)
    .await?;
    Ok(change_id)
}

pub(super) async fn insert_prepared_attachment(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &TaskId,
    prepared: &PreparedAttachment,
    created_at: &str,
) -> Result<String> {
    record_attachment_add(
        conn,
        workspace,
        task_id,
        AttachmentRecordDraft {
            attachment_id: &prepared.attachment_id,
            sha256: &prepared.sha256,
            byte_size: prepared.byte_size,
            media_type: &prepared.facts.media_type,
            filename: prepared.filename.as_deref(),
            alt_text: prepared.alt_text.as_deref(),
            width: prepared.facts.width,
            height: prepared.facts.height,
            created_at,
        },
    )
    .await
}

struct AttachmentCommitIdentity {
    attachment_id: Option<String>,
    created_at: Option<String>,
}

pub async fn add_task_attachment(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    blob_dir: &Path,
    policy: crate::attachments::lifecycle::LifecyclePolicy,
    task_id: &TaskId,
    input: AttachmentAddInput,
) -> Result<AttachmentAddOutcome> {
    add_task_attachment_inner(
        conn,
        workspace,
        blob_dir,
        policy,
        task_id,
        AttachmentCommitIdentity {
            attachment_id: None,
            created_at: None,
        },
        input,
    )
    .await
}

pub(crate) async fn add_ordered_task_attachment(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    blob_dir: &Path,
    policy: crate::attachments::lifecycle::LifecyclePolicy,
    task_id: &TaskId,
    created_at: String,
    input: TaskAttachmentAddInput,
) -> Result<AttachmentAddOutcome> {
    add_task_attachment_inner(
        conn,
        workspace,
        blob_dir,
        policy,
        task_id,
        AttachmentCommitIdentity {
            attachment_id: Some(input.attachment_id),
            created_at: Some(created_at),
        },
        input.input,
    )
    .await
}

async fn add_task_attachment_inner(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    blob_dir: &Path,
    policy: crate::attachments::lifecycle::LifecyclePolicy,
    task_id: &TaskId,
    identity: AttachmentCommitIdentity,
    input: AttachmentAddInput,
) -> Result<AttachmentAddOutcome> {
    validate_filename(input.filename.as_deref())?;
    validate_alt_text(input.alt_text.as_deref())?;

    let source = validate_image(input.bytes, input.declared_media_type).await?;
    let source_facts = source.facts.clone();
    let optimized = optimize_image_bytes(
        &source_facts.media_type,
        source.bytes,
        input.optimization_policy,
    )
    .await?;
    let optimized_flag = optimized.optimized;
    let stored_image = if optimized_flag {
        validate_image(optimized.bytes, Some(source_facts.media_type.clone())).await?
    } else {
        ValidatedImage {
            bytes: optimized.bytes,
            facts: source_facts,
        }
    };
    let sha256 = sha256_hex(&stored_image.bytes);
    let byte_size = i64::try_from(stored_image.bytes.len())?;
    let capacity_reservation = crate::attachments::lifecycle::ensure_local_capacity(
        conn,
        blob_dir,
        &sha256,
        byte_size,
        policy,
        &crate::attachments::lifecycle::SystemClock,
    )
    .await?;
    let staging_lease = crate::attachments::lifecycle::acquire_lease(
        conn,
        &sha256,
        "staging",
        &crate::attachments::lifecycle::SystemClock,
    )
    .await?;
    let stored = match store_validated_blob(conn, blob_dir, stored_image).await {
        Ok(stored) => stored,
        Err(error) => {
            let _ = crate::attachments::lifecycle::release_lease(conn, &staging_lease).await;
            if let Some(reservation_id) = capacity_reservation.as_deref() {
                let _ =
                    crate::attachments::lifecycle::release_reservation(conn, reservation_id).await;
            }
            return Err(error);
        }
    };
    if let Some(reservation_id) = capacity_reservation {
        crate::attachments::lifecycle::release_reservation(conn, &reservation_id).await?;
    }
    let database_result = async {
        let mut tx = begin_immediate(conn).await?;
        crate::operations::route_recurrence_task_field(
            &mut tx,
            workspace,
            task_id,
            "attachments",
            "",
        )
        .await?;
        let task_exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM tasks WHERE workspace_id = ? AND id = ?)",
        )
        .bind(&workspace.id)
        .bind(task_id)
        .fetch_one(&mut *tx)
        .await?;
        if !task_exists {
            bail!("error task-not-found task_id={task_id}");
        }

        if input.dedupe_existing
            && let Some(existing) =
                existing_live_attachments_by_sha(&mut tx, &workspace.id, task_id, &stored.sha256)
                    .await?
                    .first()
        {
            let attachment_id = existing.attachment_id.clone();
            tx.commit().await?;
            return Ok::<_, anyhow::Error>((attachment_id, false));
        }

        let attachment_id = identity.attachment_id.unwrap_or_else(new_id);
        let created_at = identity.created_at.unwrap_or_else(now);
        record_attachment_add(
            &mut tx,
            workspace,
            task_id,
            AttachmentRecordDraft {
                attachment_id: &attachment_id,
                sha256: &stored.sha256,
                byte_size: stored.byte_size,
                media_type: &stored.facts.media_type,
                filename: input.filename.as_deref(),
                alt_text: input.alt_text.as_deref(),
                width: stored.facts.width,
                height: stored.facts.height,
                created_at: &created_at,
            },
        )
        .await?;
        tx.commit().await?;
        Ok((attachment_id, true))
    }
    .await;
    let release_result = crate::attachments::lifecycle::release_lease(conn, &staging_lease).await;
    let (attachment_id, created) = match database_result {
        Ok(result) => result,
        Err(error) => {
            crate::attachments::storage::remove_staged_blob_if_unreferenced(
                conn,
                blob_dir,
                &stored.sha256,
            )
            .await;
            return Err(error);
        }
    };
    release_result?;
    crate::attachments::lifecycle::reconcile_liveness(
        conn,
        &crate::attachments::lifecycle::SystemClock,
    )
    .await?;
    let outcome = attachment_by_id(conn, workspace, &attachment_id).await?;
    Ok(AttachmentAddOutcome {
        outcome,
        created,
        optimized: optimized_flag,
    })
}

pub async fn attachment_by_id(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    attachment_id: &str,
) -> Result<AttachmentOutcome> {
    let attachment = require_attachment(conn, &workspace.id, attachment_id).await?;
    let has_blob = attachment_has_blob(conn, &attachment.sha256).await?;
    Ok(AttachmentOutcome {
        attachment,
        has_blob,
    })
}

pub async fn delete_task_attachment(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    attachment_id: &str,
) -> Result<AttachmentOutcome> {
    let attachment = require_attachment(conn, &workspace.id, attachment_id).await?;
    if attachment.deleted {
        let has_blob = attachment_has_blob(conn, &attachment.sha256).await?;
        return Ok(AttachmentOutcome {
            attachment,
            has_blob,
        });
    }

    let task_id = attachment.task_id.clone();
    let deleted_at = now();

    let mut tx = begin_immediate(conn).await?;
    crate::operations::route_recurrence_task_field(&mut tx, workspace, &task_id, "attachments", "")
        .await?;

    let change_id = append_change(
        &mut tx,
        ChangeEntity::Task,
        &task_id,
        Some("attachments"),
        op_type::ATTACHMENT_DELETE,
        ChangePayload::workspace(workspace)
            .set("attachment_id", attachment_id)
            .set("deleted_at", &deleted_at),
    )
    .await?;

    sqlx::query(
        "UPDATE task_attachments SET deleted = 1, deleted_at = ?, deleted_by_change_id = ? WHERE workspace_id = ? AND attachment_id = ?",
    )
    .bind(&deleted_at)
    .bind(&change_id)
    .bind(&workspace.id)
    .bind(attachment_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    crate::attachments::lifecycle::reconcile_liveness(
        conn,
        &crate::attachments::lifecycle::SystemClock,
    )
    .await?;

    attachment_by_id(conn, workspace, attachment_id).await
}

pub async fn attachment_read_items_by_task(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    task_id: &str,
    include_deleted: bool,
) -> Result<Vec<AttachmentReadItem>> {
    let attachments = attachments_by_task(conn, workspace_id, task_id, include_deleted).await?;
    let mut items = Vec::with_capacity(attachments.len());
    for attachment in attachments {
        let bytes_state = attachment_bytes_state(conn, &attachment.sha256).await?;
        items.push(AttachmentReadItem {
            attachment,
            has_blob: bytes_state == AttachmentBytesState::Present,
            bytes_state,
        });
    }
    Ok(items)
}

pub async fn attachments_by_task(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    task_id: &str,
    include_deleted: bool,
) -> Result<Vec<TaskAttachment>> {
    let deleted_predicate = if include_deleted {
        ""
    } else {
        " AND ta.deleted = 0"
    };
    let rows = sqlx::query(AssertSqlSafe(format!(
        "SELECT {ATTACHMENT_COLUMNS}
         FROM task_attachments ta
         WHERE ta.workspace_id = ? AND ta.task_id = ?{deleted_predicate}
         ORDER BY ta.created_at, ta.attachment_id"
    )))
    .bind(workspace_id)
    .bind(task_id)
    .fetch_all(&mut *conn)
    .await?;

    let mut attachments = Vec::with_capacity(rows.len());
    for row in &rows {
        attachments.push(attachment_from_row(row));
    }
    Ok(attachments)
}

impl Database {
    pub async fn add_task_attachment(
        &self,
        workspace: &Workspace,
        blob_dir: &Path,
        policy: crate::attachments::lifecycle::LifecyclePolicy,
        task_id: &TaskId,
        input: AttachmentAddInput,
    ) -> Result<AttachmentAddOutcome> {
        let mut conn = self.acquire_writer().await?;
        add_task_attachment(&mut conn, workspace, blob_dir, policy, task_id, input).await
    }

    pub async fn add_ordered_task_attachment(
        &self,
        workspace: &Workspace,
        blob_dir: &Path,
        policy: crate::attachments::lifecycle::LifecyclePolicy,
        task_id: &TaskId,
        created_at: String,
        input: TaskAttachmentAddInput,
    ) -> Result<AttachmentAddOutcome> {
        let mut conn = self.acquire_writer().await?;
        add_ordered_task_attachment(
            &mut conn, workspace, blob_dir, policy, task_id, created_at, input,
        )
        .await
    }

    pub async fn attachment_by_id(
        &self,
        workspace: &Workspace,
        attachment_id: &str,
    ) -> Result<AttachmentOutcome> {
        let mut conn = self.acquire_reader().await?;
        attachment_by_id(&mut conn, workspace, attachment_id).await
    }

    pub async fn delete_task_attachment(
        &self,
        workspace: &Workspace,
        attachment_id: &str,
    ) -> Result<AttachmentOutcome> {
        let mut conn = self.acquire_writer().await?;
        delete_task_attachment(&mut conn, workspace, attachment_id).await
    }

    pub async fn attachment_read_items_by_task(
        &self,
        workspace_id: &WorkspaceId,
        task_id: &TaskId,
        include_deleted: bool,
    ) -> Result<Vec<AttachmentReadItem>> {
        let mut conn = self.acquire_reader().await?;
        attachment_read_items_by_task(
            &mut conn,
            workspace_id.as_str(),
            task_id.as_str(),
            include_deleted,
        )
        .await
    }

    pub async fn prune_attachments(
        &self,
        blob_dir: &Path,
        policy: crate::attachments::lifecycle::LifecyclePolicy,
        apply: bool,
    ) -> Result<crate::attachments::lifecycle::PruneSummary> {
        let mut conn = self.acquire_writer().await?;
        crate::attachments::lifecycle::prune(
            &mut conn,
            blob_dir,
            policy,
            apply,
            &crate::attachments::lifecycle::SystemClock,
        )
        .await
    }

    pub async fn acquire_attachment_lease(&self, sha256: &str, purpose: &str) -> Result<String> {
        let mut conn = self.acquire_writer().await?;
        crate::attachments::lifecycle::acquire_lease(
            &mut conn,
            sha256,
            purpose,
            &crate::attachments::lifecycle::SystemClock,
        )
        .await
    }

    pub async fn acquire_live_attachment_read_lease(
        &self,
        workspace: &Workspace,
        attachment_id: &str,
    ) -> Result<AttachmentReadLease> {
        let mut conn = self.acquire_writer().await?;
        let row = sqlx::query(
            "SELECT ta.sha256, ta.media_type, bi.available
             FROM task_attachments ta
             JOIN tasks t ON t.workspace_id = ta.workspace_id AND t.id = ta.task_id
             LEFT JOIN blob_inventory bi ON bi.sha256 = ta.sha256
             WHERE ta.workspace_id = ? AND ta.attachment_id = ?
               AND ta.deleted = 0 AND t.deleted = 0",
        )
        .bind(&workspace.id)
        .bind(attachment_id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(|| anyhow::anyhow!("error attachment-invalidated"))?;
        if !row.try_get::<bool, _>("available").unwrap_or(false) {
            bail!("error attachment-blob-unavailable");
        }
        let sha256: String = row.get("sha256");
        let media_type: String = row.get("media_type");
        let lease_id = crate::attachments::lifecycle::acquire_lease(
            &mut conn,
            &sha256,
            "read",
            &crate::attachments::lifecycle::SystemClock,
        )
        .await?;
        Ok(AttachmentReadLease {
            sha256,
            media_type,
            lease_id,
        })
    }

    pub async fn release_attachment_lease(&self, lease_id: &str) -> Result<()> {
        let mut conn = self.acquire_writer().await?;
        crate::attachments::lifecycle::release_lease(&mut conn, lease_id).await
    }

    pub async fn attachment_lifecycle_report(
        &self,
        blob_dir: &Path,
        policy: crate::attachments::lifecycle::LifecyclePolicy,
    ) -> Result<crate::attachments::lifecycle::LifecycleReport> {
        let mut conn = self.acquire_writer().await?;
        crate::attachments::lifecycle::lifecycle_report(
            &mut conn,
            blob_dir,
            policy,
            &crate::attachments::lifecycle::SystemClock,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn lifecycle_report_waits_for_guarded_writer() {
        let temp = tempfile::tempdir().unwrap();
        let database = Database::open(&temp.path().join("lifecycle.sqlite"))
            .await
            .unwrap();
        let writer = database.acquire_writer().await.unwrap();

        let reporting_database = database.clone();
        let blob_dir = temp.path().join("blobs");
        let mut report = tokio::spawn(async move {
            reporting_database
                .attachment_lifecycle_report(
                    &blob_dir,
                    crate::attachments::lifecycle::LifecyclePolicy::default(),
                )
                .await
        });

        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut report)
                .await
                .is_err(),
            "lifecycle reporting should wait for the guarded writer"
        );

        drop(writer);
        tokio::time::timeout(Duration::from_secs(1), report)
            .await
            .expect("lifecycle reporting should proceed after the writer is released")
            .unwrap()
            .unwrap();
    }
}
