use std::path::Path;

use anyhow::{Result, bail};
use sqlx::Row;
use sqlx::SqliteConnection;

use crate::attachments::optimization::{ImageOptimizationPolicy, optimize_image_bytes};
use crate::attachments::storage::{blob_inventory_row, store_blob};
use crate::attachments::validation::{
    validate_alt_text, validate_blob_size, validate_dimensions, validate_filename,
    validate_media_type,
};
use crate::change_log::{ChangeEntity, ChangePayload, append_change, op_type};
use crate::db::begin_immediate;
use crate::ids::{TaskId, new_id, now};
use crate::refs::get_task_in_workspace;
use crate::types::{Task, TaskAttachment};
use crate::workspaces::Workspace;

pub(crate) struct AttachmentAddInput {
    pub(crate) filename: Option<String>,
    pub(crate) alt_text: Option<String>,
    pub(crate) media_type: String,
    pub(crate) width: Option<i64>,
    pub(crate) height: Option<i64>,
    pub(crate) bytes: Vec<u8>,
    pub(crate) optimization_policy: ImageOptimizationPolicy,
    pub(crate) dedupe_existing: bool,
}

pub(crate) struct AttachmentAddOutcome {
    pub(crate) outcome: AttachmentOutcome,
    pub(crate) description_changed: bool,
    pub(crate) optimized: bool,
}

pub(crate) struct AttachmentOutcome {
    pub(crate) task: Task,
    pub(crate) attachment: TaskAttachment,
    pub(crate) has_blob: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct AttachmentReadItem {
    pub(crate) attachment: TaskAttachment,
    pub(crate) has_blob: bool,
}

pub(crate) fn markdown_attachment_ref(attachment_id: &str, alt_text: Option<&str>) -> String {
    let escaped = alt_text
        .unwrap_or("")
        .replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]");
    format!("![{}](aven-attachment:{})", escaped, attachment_id)
}

pub(crate) fn append_attachment_ref(description: &str, ref_text: &str) -> String {
    if description.is_empty() {
        ref_text.to_string()
    } else if description.ends_with('\n') {
        format!("{}{}", description, ref_text)
    } else {
        format!("{}\n\n{}", description, ref_text)
    }
}

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

async fn attachment_has_blob(conn: &mut SqliteConnection, sha256: &str) -> Result<bool> {
    Ok(blob_inventory_row(conn, sha256)
        .await?
        .map(|row| row.available)
        .unwrap_or(false))
}

async fn existing_live_attachments_by_sha(
    conn: &mut SqliteConnection,
    workspace_id: &crate::ids::WorkspaceId,
    task_id: &TaskId,
    sha256: &str,
) -> Result<Vec<TaskAttachment>> {
    let rows = sqlx::query(
        "SELECT ta.workspace_id, ta.attachment_id, ta.task_id, ta.sha256, ta.byte_size,
                ta.media_type, ta.filename, ta.alt_text, ta.width, ta.height,
                ta.created_at, ta.created_by_change_id, ta.deleted, ta.deleted_at, ta.deleted_by_change_id
         FROM task_attachments ta
         WHERE ta.workspace_id = ? AND ta.task_id = ? AND ta.sha256 = ? AND ta.deleted = 0
         ORDER BY ta.created_at, ta.attachment_id",
    )
    .bind(workspace_id)
    .bind(task_id)
    .bind(sha256)
    .fetch_all(&mut *conn)
    .await?;

    Ok(rows.iter().map(attachment_from_row).collect())
}

fn description_references_attachment(description: &str, attachment_id: &str) -> bool {
    description.contains(&format!("](aven-attachment:{attachment_id})"))
}

pub(crate) async fn add_task_attachment(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    blob_dir: &Path,
    task_id: &TaskId,
    input: AttachmentAddInput,
) -> Result<AttachmentAddOutcome> {
    validate_media_type(&input.media_type)?;
    validate_blob_size(input.bytes.len())?;
    validate_filename(input.filename.as_deref())?;
    validate_alt_text(input.alt_text.as_deref())?;
    validate_dimensions(input.width, input.height)?;

    let optimized =
        optimize_image_bytes(&input.media_type, input.bytes, input.optimization_policy).await?;
    validate_blob_size(optimized.bytes.len())?;

    let stored = store_blob(conn, blob_dir, &input.media_type, &optimized.bytes).await?;
    let mut tx = begin_immediate(conn).await?;
    let task = get_task_in_workspace(&mut tx, workspace, task_id).await?;

    if input.dedupe_existing {
        let existing_attachments =
            existing_live_attachments_by_sha(&mut tx, &workspace.id, task_id, &stored.sha256)
                .await?;
        let referenced_existing = existing_attachments
            .iter()
            .find(|attachment| {
                description_references_attachment(&task.description, &attachment.attachment_id)
            })
            .or_else(|| existing_attachments.first());

        if let Some(existing) = referenced_existing {
            if description_references_attachment(&task.description, &existing.attachment_id) {
                tx.commit().await?;
                let outcome =
                    attachment_by_id(conn, workspace, &existing.attachment_id).await?;
                return Ok(AttachmentAddOutcome {
                    outcome,
                    description_changed: false,
                    optimized: optimized.optimized,
                });
            }

            let markdown_ref =
                markdown_attachment_ref(&existing.attachment_id, existing.alt_text.as_deref());
            let description = append_attachment_ref(&task.description, &markdown_ref);
            crate::mutation::set_task_field(
                &mut tx,
                workspace,
                task_id,
                "description",
                &description,
            )
            .await?;
            tx.commit().await?;
            let outcome = attachment_by_id(conn, workspace, &existing.attachment_id).await?;
            return Ok(AttachmentAddOutcome {
                outcome,
                description_changed: true,
                optimized: optimized.optimized,
            });
        }
    }

    let attachment_id = new_id();
    let created_at = now();
    let markdown_ref = markdown_attachment_ref(&attachment_id, input.alt_text.as_deref());
    let description = append_attachment_ref(&task.description, &markdown_ref);

    crate::mutation::set_task_field(&mut tx, workspace, task_id, "description", &description)
        .await?;

    let change_id = append_change(
        &mut tx,
        ChangeEntity::Task,
        task_id,
        Some("attachments"),
        op_type::ATTACHMENT_ADD,
        ChangePayload::workspace(workspace)
            .set("attachment_id", &attachment_id)
            .set("sha256", &stored.sha256)
            .set("byte_size", stored.byte_size)
            .set("media_type", &input.media_type)
            .set("filename", &input.filename)
            .set("alt_text", &input.alt_text)
            .set("width", input.width)
            .set("height", input.height)
            .set("created_at", &created_at),
    )
    .await?;

    sqlx::query(
        "INSERT INTO task_attachments(workspace_id, attachment_id, task_id, sha256, byte_size, media_type, filename, alt_text, width, height, created_at, created_by_change_id)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&workspace.id)
    .bind(&attachment_id)
    .bind(task_id)
    .bind(&stored.sha256)
    .bind(stored.byte_size)
    .bind(&input.media_type)
    .bind(&input.filename)
    .bind(&input.alt_text)
    .bind(input.width)
    .bind(input.height)
    .bind(&created_at)
    .bind(&change_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    let outcome = attachment_by_id(conn, workspace, &attachment_id).await?;
    Ok(AttachmentAddOutcome {
        outcome,
        description_changed: true,
        optimized: optimized.optimized,
    })
}

pub(crate) async fn attachment_by_id(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    attachment_id: &str,
) -> Result<AttachmentOutcome> {
    let row = sqlx::query(
        "SELECT ta.workspace_id, ta.attachment_id, ta.task_id, ta.sha256, ta.byte_size,
                ta.media_type, ta.filename, ta.alt_text, ta.width, ta.height,
                ta.created_at, ta.created_by_change_id, ta.deleted, ta.deleted_at, ta.deleted_by_change_id
         FROM task_attachments ta
         WHERE ta.workspace_id = ? AND ta.attachment_id = ?",
    )
    .bind(&workspace.id)
    .bind(attachment_id)
    .fetch_optional(&mut *conn)
    .await?;

    let Some(row) = row else {
        bail!("error attachment-not-found id={}", attachment_id);
    };

    let attachment = attachment_from_row(&row);
    let has_blob = attachment_has_blob(conn, &attachment.sha256).await?;
    let task_id = attachment.task_id.parse()?;
    let task = get_task_in_workspace(conn, workspace, &task_id).await?;

    Ok(AttachmentOutcome {
        task,
        attachment,
        has_blob,
    })
}

pub(crate) async fn delete_task_attachment(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    attachment_id: &str,
) -> Result<AttachmentOutcome> {
    let row = sqlx::query(
        "SELECT ta.workspace_id, ta.attachment_id, ta.task_id, ta.sha256, ta.byte_size,
                ta.media_type, ta.filename, ta.alt_text, ta.width, ta.height,
                ta.created_at, ta.created_by_change_id, ta.deleted, ta.deleted_at, ta.deleted_by_change_id
         FROM task_attachments ta
         WHERE ta.workspace_id = ? AND ta.attachment_id = ?",
    )
    .bind(&workspace.id)
    .bind(attachment_id)
    .fetch_optional(&mut *conn)
    .await?;

    let Some(row) = row else {
        bail!("error attachment-not-found id={}", attachment_id);
    };

    let deleted: bool = row.get::<i64, _>("deleted") != 0;
    if deleted {
        let attachment = attachment_from_row(&row);
        let has_blob = attachment_has_blob(conn, &attachment.sha256).await?;
        let task_id = attachment.task_id.parse()?;
        let task = get_task_in_workspace(conn, workspace, &task_id).await?;
        return Ok(AttachmentOutcome {
            task,
            attachment,
            has_blob,
        });
    }

    let task_id: String = row.get("task_id");
    let deleted_at = now();

    let mut tx = begin_immediate(conn).await?;

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

    attachment_by_id(conn, workspace, attachment_id).await
}

pub(crate) async fn attachment_read_items_by_task(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    task_id: &str,
    include_deleted: bool,
) -> Result<Vec<AttachmentReadItem>> {
    let attachments = attachments_by_task(conn, workspace_id, task_id, include_deleted).await?;
    let mut items = Vec::with_capacity(attachments.len());
    for attachment in attachments {
        let has_blob = attachment_has_blob(conn, &attachment.sha256).await?;
        items.push(AttachmentReadItem {
            attachment,
            has_blob,
        });
    }
    Ok(items)
}

pub(crate) async fn attachments_by_task(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    task_id: &str,
    include_deleted: bool,
) -> Result<Vec<TaskAttachment>> {
    let rows = if include_deleted {
        sqlx::query(
            "SELECT ta.workspace_id, ta.attachment_id, ta.task_id, ta.sha256, ta.byte_size,
                    ta.media_type, ta.filename, ta.alt_text, ta.width, ta.height,
                    ta.created_at, ta.created_by_change_id, ta.deleted, ta.deleted_at, ta.deleted_by_change_id
             FROM task_attachments ta
             WHERE ta.workspace_id = ? AND ta.task_id = ?
             ORDER BY ta.created_at",
        )
        .bind(workspace_id)
        .bind(task_id)
        .fetch_all(&mut *conn)
        .await?
    } else {
        sqlx::query(
            "SELECT ta.workspace_id, ta.attachment_id, ta.task_id, ta.sha256, ta.byte_size,
                    ta.media_type, ta.filename, ta.alt_text, ta.width, ta.height,
                    ta.created_at, ta.created_by_change_id, ta.deleted, ta.deleted_at, ta.deleted_by_change_id
             FROM task_attachments ta
             WHERE ta.workspace_id = ? AND ta.task_id = ? AND ta.deleted = 0
             ORDER BY ta.created_at",
        )
        .bind(workspace_id)
        .bind(task_id)
        .fetch_all(&mut *conn)
        .await?
    };

    let mut attachments = Vec::with_capacity(rows.len());
    for row in &rows {
        attachments.push(attachment_from_row(row));
    }
    Ok(attachments)
}
