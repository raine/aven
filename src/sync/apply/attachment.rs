use anyhow::{Result, bail};
use sqlx::{Row, SqliteConnection};

use crate::sync::wire::ChangeWire;

use super::payload::{AttachmentAddPayload, AttachmentDeletePayload};
use super::shared::task_field_workspace_id_payload;

pub(super) async fn add_attachment(conn: &mut SqliteConnection, change: &ChangeWire) -> Result<()> {
    let workspace_id = task_field_workspace_id_payload(conn, change).await?;
    ensure_attachment_task_exists(conn, workspace_id.as_str(), &change.entity_id).await?;
    let payload = AttachmentAddPayload::from_change(change)?;
    if let Some(row) =
        existing_attachment(conn, workspace_id.as_str(), &payload.attachment_id).await?
    {
        return ensure_same_attachment(&row, &change.entity_id, &payload);
    }

    sqlx::query(
        "INSERT INTO task_attachments(workspace_id, attachment_id, task_id, sha256, byte_size, media_type, filename, alt_text, width, height, created_at, created_by_change_id)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&workspace_id)
    .bind(&payload.attachment_id)
    .bind(&change.entity_id)
    .bind(&payload.sha256)
    .bind(payload.byte_size)
    .bind(&payload.media_type)
    .bind(&payload.filename)
    .bind(&payload.alt_text)
    .bind(payload.width)
    .bind(payload.height)
    .bind(&payload.created_at)
    .bind(&change.change_id)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

pub(super) async fn delete_attachment(
    conn: &mut SqliteConnection,
    change: &ChangeWire,
) -> Result<()> {
    let workspace_id = task_field_workspace_id_payload(conn, change).await?;
    let payload = AttachmentDeletePayload::from_change(change)?;
    let updated = sqlx::query(
        "UPDATE task_attachments
         SET deleted = 1,
             deleted_at = COALESCE(deleted_at, ?),
             deleted_by_change_id = COALESCE(deleted_by_change_id, ?)
         WHERE workspace_id = ? AND attachment_id = ? AND task_id = ?",
    )
    .bind(&payload.deleted_at)
    .bind(&change.change_id)
    .bind(&workspace_id)
    .bind(&payload.attachment_id)
    .bind(&change.entity_id)
    .execute(&mut *conn)
    .await?
    .rows_affected();

    if updated == 0
        && attachment_exists(conn, workspace_id.as_str(), &payload.attachment_id).await?
    {
        bail!(
            "error attachment-identity-conflict attachment_id={} task_id={}",
            payload.attachment_id,
            change.entity_id
        );
    }
    Ok(())
}

async fn ensure_attachment_task_exists(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    task_id: &str,
) -> Result<()> {
    let count: i64 = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM tasks WHERE workspace_id = ? AND id = ?",
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_one(&mut *conn)
    .await?;
    if count == 0 {
        bail!("error attachment-missing-task task_id={task_id}");
    }
    Ok(())
}

async fn existing_attachment(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    attachment_id: &str,
) -> Result<Option<sqlx::sqlite::SqliteRow>> {
    Ok(sqlx::query(
        "SELECT task_id, sha256, byte_size, media_type, filename, alt_text, width, height, created_at
         FROM task_attachments
         WHERE workspace_id = ? AND attachment_id = ?",
    )
    .bind(workspace_id)
    .bind(attachment_id)
    .fetch_optional(&mut *conn)
    .await?)
}

fn ensure_same_attachment(
    row: &sqlx::sqlite::SqliteRow,
    task_id: &str,
    payload: &AttachmentAddPayload,
) -> Result<()> {
    let row_task_id: String = row.get("task_id");
    let row_sha256: String = row.get("sha256");
    let row_byte_size: i64 = row.get("byte_size");
    let row_media_type: String = row.get("media_type");
    let row_filename: Option<String> = row.get("filename");
    let row_alt_text: Option<String> = row.get("alt_text");
    let row_width: Option<i64> = row.get("width");
    let row_height: Option<i64> = row.get("height");
    let row_created_at: String = row.get("created_at");

    if row_task_id == task_id
        && row_sha256 == payload.sha256
        && row_byte_size == payload.byte_size
        && row_media_type == payload.media_type
        && row_filename == payload.filename
        && row_alt_text == payload.alt_text
        && row_width == payload.width
        && row_height == payload.height
        && row_created_at == payload.created_at
    {
        return Ok(());
    }

    bail!(
        "error attachment-identity-conflict attachment_id={} task_id={task_id}",
        payload.attachment_id
    )
}

async fn attachment_exists(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    attachment_id: &str,
) -> Result<bool> {
    let count: i64 = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM task_attachments WHERE workspace_id = ? AND attachment_id = ?",
    )
    .bind(workspace_id)
    .bind(attachment_id)
    .fetch_one(&mut *conn)
    .await?;
    Ok(count > 0)
}
