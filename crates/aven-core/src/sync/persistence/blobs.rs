use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result, bail};
use sqlx::SqliteConnection;

use crate::change_log::op_type;
use crate::sync::wire::{AttachmentAddPayload, ChangeWire};

pub(in crate::sync) async fn apply_server_blob_reference(
    conn: &mut SqliteConnection,
    change: &ChangeWire,
    affected_attachment_hashes: &mut HashSet<String>,
) -> Result<()> {
    match change.op_type.as_str() {
        op_type::ATTACHMENT_ADD => {
            let payload = AttachmentAddPayload::from_change(change)?;
            let previous_sha256: Option<String> = sqlx::query_scalar(
                "SELECT sha256 FROM server_blob_references
                 WHERE workspace_id = ? AND attachment_id = ?",
            )
            .bind(&payload.workspace_id)
            .bind(&payload.attachment_id)
            .fetch_optional(&mut *conn)
            .await?;
            if let Some(previous_sha256) = previous_sha256 {
                affected_attachment_hashes.insert(previous_sha256);
            }
            affected_attachment_hashes.insert(payload.sha256.clone());
            sqlx::query(
                "INSERT INTO server_blob_references(
                   workspace_id, attachment_id, task_id, sha256, byte_size, deleted
                 ) VALUES (?, ?, ?, ?, ?, 0)
                 ON CONFLICT(workspace_id, attachment_id) DO UPDATE SET
                   task_id = excluded.task_id, sha256 = excluded.sha256,
                   byte_size = excluded.byte_size, deleted = 0",
            )
            .bind(&payload.workspace_id)
            .bind(&payload.attachment_id)
            .bind(&change.entity_id)
            .bind(&payload.sha256)
            .bind(payload.byte_size)
            .execute(&mut *conn)
            .await?;
            sqlx::query(
                "DELETE FROM blob_upload_reservations WHERE workspace_id = ? AND sha256 = ?",
            )
            .bind(&payload.workspace_id)
            .bind(&payload.sha256)
            .execute(&mut *conn)
            .await?;
        }
        op_type::ATTACHMENT_DELETE => {
            let workspace_id = change.payload["workspace_id"]
                .as_str()
                .context("payload missing workspace_id")?;
            let attachment_id = change.payload["attachment_id"]
                .as_str()
                .context("payload missing attachment_id")?;
            let sha256: Option<String> = sqlx::query_scalar(
                "SELECT sha256 FROM server_blob_references
                 WHERE workspace_id = ? AND attachment_id = ?",
            )
            .bind(workspace_id)
            .bind(attachment_id)
            .fetch_optional(&mut *conn)
            .await?;
            if let Some(sha256) = sha256 {
                affected_attachment_hashes.insert(sha256);
            }
            sqlx::query(
                "UPDATE server_blob_references SET deleted = 1
                 WHERE workspace_id = ? AND attachment_id = ?",
            )
            .bind(workspace_id)
            .bind(attachment_id)
            .execute(&mut *conn)
            .await?;
        }
        op_type::SET_FIELD | op_type::RESOLVE_FIELD
            if change.field.as_deref() == Some("deleted") =>
        {
            let workspace_id = change.payload["workspace_id"]
                .as_str()
                .context("payload missing workspace_id")?;
            let hashes: Vec<String> = sqlx::query_scalar(
                "SELECT DISTINCT sha256 FROM server_blob_references
                 WHERE workspace_id = ? AND task_id = ?",
            )
            .bind(workspace_id)
            .bind(&change.entity_id)
            .fetch_all(&mut *conn)
            .await?;
            affected_attachment_hashes.extend(hashes);
            super::parent_liveness::reconcile_parent(conn, workspace_id, &change.entity_id).await?;
        }
        _ => {}
    }
    Ok(())
}

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

pub(in crate::sync) async fn prepare_server_blobs(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    changes: &[ChangeWire],
) -> Result<()> {
    let assigned_change_ids = super::load_assigned_change_ids(conn, changes).await?;
    let contracts = changes
        .iter()
        .filter(|change| {
            change.op_type == op_type::ATTACHMENT_ADD
                && !assigned_change_ids.contains(&change.change_id)
        })
        .map(|change| {
            super::super::blob::attachment_blob_contract(change)?
                .context("error attachment-blob-missing")
        })
        .collect::<Result<Vec<_>>>()?;
    for contract in super::super::blob::unique_blob_content_contracts(&contracts)? {
        validate_server_blob_before_writer(conn, blob_dir, &contract).await?;
    }
    Ok(())
}

async fn validate_server_blob_before_writer(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    contract: &crate::sync::wire::BlobUploadContract,
) -> Result<()> {
    let Some(row) = crate::attachments::storage::blob_inventory_row(conn, &contract.sha256).await?
    else {
        bail!("error attachment-blob-missing");
    };
    validate_server_blob_inventory(&row, contract)?;
    let path = crate::attachments::object_path(blob_dir, &contract.sha256)?;
    let bytes = match tokio::fs::read(path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            bail!("error attachment-blob-missing")
        }
        Err(error) => return Err(error.into()),
    };
    if i64::try_from(bytes.len()).ok() != Some(contract.byte_size)
        || crate::attachments::storage::sha256_hex(&bytes) != contract.sha256
    {
        bail!("error attachment-blob-content-mismatch");
    }
    let validated =
        crate::attachments::decode::validate_image(bytes, Some(contract.media_type.clone()))
            .await?;
    if (validated.facts.width, validated.facts.height) != (contract.width, contract.height) {
        bail!("error blob-inventory-metadata-mismatch");
    }
    Ok(())
}

fn validate_server_blob_inventory(
    row: &crate::types::BlobInventoryRow,
    contract: &crate::sync::wire::BlobUploadContract,
) -> Result<()> {
    if !row.available {
        bail!("error attachment-blob-missing");
    }
    if row.byte_size != contract.byte_size || row.media_type != contract.media_type {
        bail!("error blob-inventory-metadata-mismatch");
    }
    Ok(())
}

pub(in crate::sync) async fn ensure_attachment_blobs_admitted(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    changes: &[ChangeWire],
) -> Result<()> {
    let contracts = super::super::blob::attachment_blob_contracts(changes)?;
    for contract in super::super::blob::unique_blob_content_contracts(&contracts)? {
        let Some(row) =
            crate::attachments::storage::blob_inventory_row(conn, &contract.sha256).await?
        else {
            bail!("error attachment-blob-missing");
        };
        validate_server_blob_inventory(&row, &contract)?;
        if !crate::attachments::object_path(blob_dir, &contract.sha256)?.exists() {
            bail!("error attachment-blob-missing");
        }
    }
    for contract in super::super::blob::unique_blob_admission_contracts(&contracts) {
        let admitted: bool = sqlx::query_scalar(
            "SELECT EXISTS(
               SELECT 1 FROM server_blob_references sbr
               LEFT JOIN server_task_tombstones st
                 ON st.workspace_id = sbr.workspace_id AND st.task_id = sbr.task_id
               WHERE sbr.workspace_id = ? AND sbr.sha256 = ? AND sbr.deleted = 0
                 AND COALESCE(st.deleted, 0) = 0
             ) OR EXISTS(
               SELECT 1 FROM blob_upload_reservations
               WHERE workspace_id = ? AND sha256 = ? AND byte_size = ? AND expires_at > ?
             )",
        )
        .bind(&contract.workspace_id)
        .bind(&contract.sha256)
        .bind(&contract.workspace_id)
        .bind(&contract.sha256)
        .bind(contract.byte_size)
        .bind(crate::ids::now())
        .fetch_one(&mut *conn)
        .await?;
        if !admitted {
            bail!("error attachment-blob-unreserved");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::io::Cursor;
    use std::time::{Duration, Instant};

    use image::{DynamicImage, ImageFormat};

    use super::super::{ServerSyncPage, assign_server_sequences};
    use super::*;
    use crate::attachments::storage::{object_path, sha256_hex, upsert_inventory_available};
    use crate::change_log::op_type;
    use crate::db::Database;
    use crate::sync::wire::{ChangeWire, SYNC_PROTOCOL_VERSION, SyncRequest};
    use serde_json::json;

    async fn corrupt_attachment_page() -> (
        tempfile::TempDir,
        Database,
        std::path::PathBuf,
        ServerSyncPage,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let database = Database::open(&temp.path().join("server.sqlite"))
            .await
            .unwrap();
        let blob_dir = temp.path().join("blobs");
        let expected = b"expected";
        let sha256 = sha256_hex(expected);
        {
            let mut conn = database.acquire_writer().await.unwrap();
            upsert_inventory_available(&mut conn, &sha256, expected.len() as i64, "image/png")
                .await
                .unwrap();
        }
        let path = object_path(&blob_dir, &sha256).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"corrupt!").unwrap();
        let change = ChangeWire {
            change_id: "0123456789ABCDEF".to_string(),
            client_id: "client-a".to_string(),
            local_seq: 1,
            entity_type: "task".to_string(),
            entity_id: "0123456789ABCDE0".to_string(),
            field: Some("attachments".to_string()),
            op_type: op_type::ATTACHMENT_ADD.to_string(),
            payload: json!({
                "workspace_id": "0000000000000000",
                "workspace_key": "default",
                "attachment_id": "7KQ9A1X4MV2P8D6R",
                "sha256": sha256,
                "byte_size": expected.len(),
                "media_type": "image/png",
                "filename": "photo.png",
                "alt_text": "photo",
                "width": 1,
                "height": 1,
                "created_at": "2026-01-01T00:00:00Z"
            }),
            base_version: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            server_seq: None,
        };
        let page = ServerSyncPage {
            request: SyncRequest {
                protocol_version: Some(SYNC_PROTOCOL_VERSION),
                client_id: "client-a".to_string(),
                after: 0,
                pull_limit: Some(100),
                changes: vec![change],
            },
        };
        (temp, database, blob_dir, page)
    }

    fn page_for_contract(contract: &crate::sync::wire::BlobUploadContract) -> ServerSyncPage {
        let change = ChangeWire {
            change_id: "0123456789ABCDEF".to_string(),
            client_id: "client-a".to_string(),
            local_seq: 1,
            entity_type: "task".to_string(),
            entity_id: "0123456789ABCDE0".to_string(),
            field: Some("attachments".to_string()),
            op_type: op_type::ATTACHMENT_ADD.to_string(),
            payload: json!({
                "workspace_id": contract.workspace_id,
                "workspace_key": "default",
                "attachment_id": "7KQ9A1X4MV2P8D6R",
                "sha256": contract.sha256,
                "byte_size": contract.byte_size,
                "media_type": contract.media_type,
                "filename": "photo.png",
                "alt_text": "photo",
                "width": contract.width,
                "height": contract.height,
                "created_at": "2026-01-01T00:00:00Z"
            }),
            base_version: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            server_seq: None,
        };
        ServerSyncPage {
            request: SyncRequest {
                protocol_version: Some(SYNC_PROTOCOL_VERSION),
                client_id: "client-a".to_string(),
                after: 0,
                pull_limit: Some(100),
                changes: vec![change],
            },
        }
    }

    #[tokio::test]
    async fn corrupt_server_blob_is_rejected_before_writer_acquisition() {
        let (_temp, database, blob_dir, page) = corrupt_attachment_page().await;
        let writer = database.acquire_writer().await.unwrap();
        let task = tokio::spawn({
            let database = database.clone();
            async move {
                database
                    .persist_server_sync_page_with_blobs(page, &blob_dir)
                    .await
            }
        });
        // The rejection must land while this test still owns the writer gate. Poll to a
        // generous deadline so the assertion tracks gate ordering rather than machine load.
        let deadline = Instant::now() + Duration::from_secs(10);
        while !task.is_finished() {
            assert!(
                Instant::now() < deadline,
                "corrupt content validation should not wait for the writer gate"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        drop(writer);
        let error = task.await.unwrap().unwrap_err();
        assert!(
            error
                .to_string()
                .contains("attachment-blob-content-mismatch")
        );
    }

    #[tokio::test]
    async fn reservation_is_rechecked_after_blob_preparation() {
        let temp = tempfile::tempdir().unwrap();
        let database = Database::open(&temp.path().join("server.sqlite"))
            .await
            .unwrap();
        let blob_dir = temp.path().join("blobs");
        let mut encoded = Cursor::new(Vec::new());
        DynamicImage::new_rgba8(1, 1)
            .write_to(&mut encoded, ImageFormat::Png)
            .unwrap();
        let bytes = encoded.into_inner();
        let contract = crate::sync::wire::BlobUploadContract {
            workspace_id: "0000000000000000".to_string(),
            sha256: sha256_hex(&bytes),
            byte_size: bytes.len() as i64,
            media_type: "image/png".to_string(),
            width: 1,
            height: 1,
        };
        database
            .store_server_blob(
                &blob_dir,
                crate::attachments::lifecycle::LifecyclePolicy::default(),
                &contract,
                bytes,
            )
            .await
            .unwrap();
        let page = page_for_contract(&contract);
        {
            let mut reader = database.acquire_reader().await.unwrap();
            prepare_server_blobs(&mut reader, &blob_dir, &page.request.changes)
                .await
                .unwrap();
        }
        {
            let mut writer = database.acquire_writer().await.unwrap();
            sqlx::query("DELETE FROM blob_upload_reservations")
                .execute(&mut *writer)
                .await
                .unwrap();
        }
        let mut writer = database.acquire_writer().await.unwrap();
        let error = assign_server_sequences(&mut writer, page.request.changes, Some(&blob_dir))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("attachment-blob-unreserved"));
        let accepted: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM changes")
            .fetch_one(&mut *writer)
            .await
            .unwrap();
        assert_eq!(accepted, 0);
    }

    #[tokio::test]
    async fn server_task_deletion_operations_reconcile_attachment_liveness() {
        let (_temp, mut conn) = crate::test_support::test_conn().await;
        for (index, operation) in [op_type::SET_FIELD, op_type::RESOLVE_FIELD]
            .into_iter()
            .enumerate()
        {
            let task_id = format!("BBBBBBBBBBBBBBB{index}");
            let attachment_id = format!("CCCCCCCCCCCCCCC{index}");
            let sha256 = format!("{index:064x}");
            upsert_inventory_available(&mut conn, &sha256, 1, "image/png")
                .await
                .unwrap();
            sqlx::query("INSERT INTO blob_lifecycle(sha256, unreferenced_at) VALUES (?, NULL)")
                .bind(&sha256)
                .execute(&mut *conn)
                .await
                .unwrap();
            sqlx::query(
                "INSERT INTO server_blob_references(
                   workspace_id, attachment_id, task_id, sha256, byte_size, deleted
                 ) VALUES ('0000000000000000', ?, ?, ?, 1, 0)",
            )
            .bind(attachment_id)
            .bind(&task_id)
            .bind(&sha256)
            .execute(&mut *conn)
            .await
            .unwrap();

            let create_id = format!("DDDDDDDDDDDDDDD{index}");
            let creation = ChangeWire {
                change_id: create_id.clone(),
                client_id: "client".to_string(),
                local_seq: 1,
                entity_type: "task".to_string(),
                entity_id: task_id.clone(),
                field: None,
                op_type: op_type::CREATE_TASK.to_string(),
                payload: json!({"workspace_id": "0000000000000000"}),
                base_version: None,
                created_at: "2026-01-01T00:00:00Z".to_string(),
                server_seq: Some(index as i64 * 3 + 1),
            };
            super::super::insert_wire_change(&mut conn, &creation)
                .await
                .unwrap();
            let deletion_change = |value: &str| ChangeWire {
                change_id: format!("AAAAAAAAAAAAAA{index}{value}"),
                client_id: "client".to_string(),
                local_seq: 1,
                entity_type: "task".to_string(),
                entity_id: task_id.clone(),
                field: Some("deleted".to_string()),
                op_type: operation.to_string(),
                payload: json!({
                    "workspace_id": "0000000000000000",
                    "workspace_key": "default",
                    "value": value,
                }),
                base_version: Some(if value == "1" {
                    create_id.clone()
                } else {
                    format!("AAAAAAAAAAAAAA{index}1")
                }),
                created_at: "2026-01-01T00:00:00Z".to_string(),
                server_seq: Some(index as i64 * 3 + if value == "1" { 2 } else { 3 }),
            };

            super::super::insert_wire_change(&mut conn, &deletion_change("1"))
                .await
                .unwrap();
            let mut affected_hashes = HashSet::new();
            apply_server_blob_reference(&mut conn, &deletion_change("1"), &mut affected_hashes)
                .await
                .unwrap();
            let affected_hashes = affected_hashes.into_iter().collect::<Vec<_>>();
            crate::attachments::lifecycle::reconcile_liveness_for_hashes_in_transaction(
                &mut conn,
                &affected_hashes,
                &crate::attachments::lifecycle::SystemClock,
            )
            .await
            .unwrap();
            let deleted: bool = sqlx::query_scalar(
                "SELECT deleted FROM server_task_tombstones
                 WHERE workspace_id = '0000000000000000' AND task_id = ?",
            )
            .bind(&task_id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
            let unreferenced_at: Option<String> =
                sqlx::query_scalar("SELECT unreferenced_at FROM blob_lifecycle WHERE sha256 = ?")
                    .bind(&sha256)
                    .fetch_one(&mut *conn)
                    .await
                    .unwrap();
            assert!(deleted, "{operation} must apply live-to-deleted state");
            assert!(unreferenced_at.is_some());

            super::super::insert_wire_change(&mut conn, &deletion_change("0"))
                .await
                .unwrap();
            let mut affected_hashes = HashSet::new();
            apply_server_blob_reference(&mut conn, &deletion_change("0"), &mut affected_hashes)
                .await
                .unwrap();
            let affected_hashes = affected_hashes.into_iter().collect::<Vec<_>>();
            crate::attachments::lifecycle::reconcile_liveness_for_hashes_in_transaction(
                &mut conn,
                &affected_hashes,
                &crate::attachments::lifecycle::SystemClock,
            )
            .await
            .unwrap();
            let deleted: bool = sqlx::query_scalar(
                "SELECT deleted FROM server_task_tombstones
                 WHERE workspace_id = '0000000000000000' AND task_id = ?",
            )
            .bind(&task_id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
            let unreferenced_at: Option<String> =
                sqlx::query_scalar("SELECT unreferenced_at FROM blob_lifecycle WHERE sha256 = ?")
                    .bind(&sha256)
                    .fetch_one(&mut *conn)
                    .await
                    .unwrap();
            assert!(!deleted, "{operation} must apply deleted-to-live state");
            assert_eq!(unreferenced_at, None);
        }
    }
}
