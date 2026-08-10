use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result, bail};
use sqlx::{Row, SqliteConnection};

use super::wire::{AttachmentAddPayload, BlobUploadContract, ChangeWire};
use crate::attachments::decode::validate_image;
use crate::attachments::lifecycle::{
    BoundedMaintenanceSummary, ByteCount, LifecyclePolicy, SystemClock, acquire_lease,
    ensure_local_capacity, maintain_tracked_blobs_bounded, release_lease, release_reservation,
    reserve_upload,
};
use crate::attachments::storage::{
    blob_inventory_row, object_path, sha256_hex, store_validated_blob,
};
use crate::change_log::op_type;
use crate::db::{Database, get_meta, set_meta};

#[derive(Debug, Clone)]
pub(super) struct MissingLocalBlob {
    pub sha256: String,
    pub byte_size: i64,
    pub media_type: String,
    pub width: Option<i64>,
    pub height: Option<i64>,
}

#[derive(Debug)]
pub(super) struct MissingBlobPage {
    pub blobs: Vec<MissingLocalBlob>,
    pub has_more: bool,
}

#[derive(Debug)]
pub(super) struct PreparedBlobUpload {
    pub bytes: Vec<u8>,
    pub lease_id: String,
}

#[derive(Debug)]
pub struct ServerBlobDownload {
    pub bytes: Vec<u8>,
}

pub(super) fn attachment_blob_contract(change: &ChangeWire) -> Result<Option<BlobUploadContract>> {
    if change.op_type != op_type::ATTACHMENT_ADD {
        return Ok(None);
    }
    let payload = AttachmentAddPayload::from_change(change)?;
    Ok(Some(BlobUploadContract {
        workspace_id: payload.workspace_id,
        sha256: payload.sha256,
        byte_size: payload.byte_size,
        media_type: payload.media_type,
        width: payload
            .width
            .context("validated attachment payload missing width")?,
        height: payload
            .height
            .context("validated attachment payload missing height")?,
    }))
}

pub(super) fn unique_blob_contracts(changes: &[ChangeWire]) -> Result<Vec<BlobUploadContract>> {
    let mut seen = HashSet::new();
    let mut contracts = Vec::new();
    for change in changes {
        if let Some(contract) = attachment_blob_contract(change)?
            && seen.insert((contract.workspace_id.clone(), contract.sha256.clone()))
        {
            contracts.push(contract);
        }
    }
    Ok(contracts)
}

pub(super) fn contract_by_hash(
    contracts: &[BlobUploadContract],
    sha256: &str,
) -> Result<BlobUploadContract> {
    contracts
        .iter()
        .find(|contract| contract.sha256 == sha256)
        .cloned()
        .context("payload missing attachment blob contract")
}

impl Database {
    pub(super) async fn prepare_blob_upload(
        &self,
        blob_dir: &Path,
        contract: &BlobUploadContract,
    ) -> Result<PreparedBlobUpload> {
        let (row, lease_id) = {
            let mut conn = self.acquire_writer().await?;
            let row = blob_inventory_row(&mut conn, &contract.sha256)
                .await?
                .filter(|row| row.available)
                .context("error attachment-blob-local-missing")?;
            let lease_id =
                acquire_lease(&mut conn, &contract.sha256, "transfer", &SystemClock).await?;
            (row, lease_id)
        };
        let result = async {
            let bytes = tokio::fs::read(object_path(blob_dir, &contract.sha256)?)
                .await
                .context("error attachment-blob-local-missing")?;
            if sha256_hex(&bytes) != contract.sha256
                || row.byte_size
                    != i64::try_from(bytes.len()).context("attachment bytes exceed i64")?
            {
                bail!("error attachment-blob-local-invalid");
            }
            let validated = validate_image(bytes.clone(), Some(row.media_type.clone())).await?;
            if (validated.facts.width, validated.facts.height) != (contract.width, contract.height)
                || validated.facts.media_type != contract.media_type
                || contract.byte_size != row.byte_size
            {
                bail!("error attachment-blob-local-invalid");
            }
            Ok(PreparedBlobUpload {
                bytes,
                lease_id: lease_id.clone(),
            })
        }
        .await;
        if result.is_err() {
            let _ = self.finish_blob_upload(&lease_id).await;
        }
        result
    }

    pub(super) async fn finish_blob_upload(&self, lease_id: &str) -> Result<()> {
        let mut conn = self.acquire_writer().await?;
        release_lease(&mut conn, lease_id).await
    }

    pub(super) async fn missing_local_blob_page(&self, limit: usize) -> Result<MissingBlobPage> {
        let mut conn = self.acquire_reader().await?;
        missing_local_blob_page(&mut conn, limit).await
    }

    pub(super) async fn missing_local_blob_counts(&self) -> Result<ByteCount> {
        let mut conn = self.acquire_reader().await?;
        let (count, bytes): (i64, i64) = sqlx::query_as(
            "SELECT COUNT(*), COALESCE(SUM(byte_size), 0) FROM (
               SELECT ta.sha256, MAX(ta.byte_size) AS byte_size
               FROM task_attachments ta
               JOIN tasks t ON t.workspace_id = ta.workspace_id AND t.id = ta.task_id
               LEFT JOIN blob_inventory bi ON bi.sha256 = ta.sha256
               WHERE ta.deleted = 0 AND t.deleted = 0
               GROUP BY ta.sha256
               HAVING MAX(COALESCE(bi.available, 0)) = 0
             )",
        )
        .fetch_one(&mut *conn)
        .await?;
        Ok(ByteCount {
            count: u64::try_from(count)?,
            bytes: u64::try_from(bytes)?,
        })
    }

    pub(super) async fn missing_local_blobs_for_contracts(
        &self,
        blob_dir: &Path,
        contracts: &[BlobUploadContract],
    ) -> Result<Vec<MissingLocalBlob>> {
        if contracts.is_empty() {
            return Ok(Vec::new());
        }
        let mut conn = self.acquire_reader().await?;
        let mut missing = Vec::new();
        for contract in contracts {
            if !blob_available(&mut conn, blob_dir, &contract.sha256).await? {
                missing.push(MissingLocalBlob {
                    sha256: contract.sha256.clone(),
                    byte_size: contract.byte_size,
                    media_type: contract.media_type.clone(),
                    width: Some(contract.width),
                    height: Some(contract.height),
                });
            }
        }
        Ok(missing)
    }

    pub(super) async fn audit_local_blob_storage(
        &self,
        blob_dir: &Path,
        limit: usize,
    ) -> Result<Vec<MissingLocalBlob>> {
        const CURSOR_KEY: &str = "sync_blob_audit_cursor";

        let (cursor, rows) = {
            let mut conn = self.acquire_reader().await?;
            let cursor = get_meta(&mut conn, CURSOR_KEY).await?.unwrap_or_default();
            let rows = sqlx::query(
                "SELECT ta.sha256, MAX(ta.byte_size) AS byte_size,
                        MAX(ta.media_type) AS media_type, MAX(ta.width) AS width,
                        MAX(ta.height) AS height
                 FROM task_attachments ta
                 JOIN tasks t ON t.workspace_id = ta.workspace_id AND t.id = ta.task_id
                 JOIN blob_inventory bi ON bi.sha256 = ta.sha256 AND bi.available = 1
                 WHERE ta.deleted = 0 AND t.deleted = 0 AND ta.sha256 > ?
                 GROUP BY ta.sha256 ORDER BY ta.sha256 LIMIT ?",
            )
            .bind(&cursor)
            .bind(i64::try_from(limit.saturating_add(1))?)
            .fetch_all(&mut *conn)
            .await?;
            (cursor, rows)
        };
        if rows.is_empty() {
            if !cursor.is_empty() {
                let mut conn = self.acquire_writer().await?;
                set_meta(&mut conn, CURSOR_KEY, "").await?;
            }
            return Ok(Vec::new());
        }
        let has_more = rows.len() > limit;
        let mut checked = Vec::new();
        for row in rows.into_iter().take(limit) {
            let blob = MissingLocalBlob {
                sha256: row.get("sha256"),
                byte_size: row.get("byte_size"),
                media_type: row.get("media_type"),
                width: row.get("width"),
                height: row.get("height"),
            };
            let exists = object_path(blob_dir, &blob.sha256)?.exists();
            checked.push((blob, exists));
        }
        let next_cursor = checked
            .last()
            .map(|(blob, _)| blob.sha256.clone())
            .unwrap_or_default();
        let mut conn = self.acquire_writer().await?;
        let mut missing = Vec::new();
        for (blob, existed) in checked {
            if !existed && !object_path(blob_dir, &blob.sha256)?.exists() {
                sqlx::query("UPDATE blob_inventory SET available = 0 WHERE sha256 = ?")
                    .bind(&blob.sha256)
                    .execute(&mut *conn)
                    .await?;
                missing.push(blob);
            }
        }
        set_meta(
            &mut conn,
            CURSOR_KEY,
            if has_more { &next_cursor } else { "" },
        )
        .await?;
        Ok(missing)
    }

    pub(super) async fn store_downloaded_blob(
        &self,
        blob_dir: &Path,
        policy: LifecyclePolicy,
        blob: &MissingLocalBlob,
        bytes: Vec<u8>,
    ) -> Result<()> {
        let expected = usize::try_from(blob.byte_size).context("attachment bytes exceed usize")?;
        if bytes.len() != expected || sha256_hex(&bytes) != blob.sha256 {
            bail!("error attachment-blob-remote-invalid");
        }
        let validated = validate_image(bytes, Some(blob.media_type.clone()))
            .await
            .context("error attachment-blob-remote-invalid")?;
        if (blob.width, blob.height) != (Some(validated.facts.width), Some(validated.facts.height))
        {
            bail!("error attachment-blob-remote-invalid");
        }
        let mut conn = self.acquire_writer().await?;
        let reservation = ensure_local_capacity(
            &mut conn,
            blob_dir,
            &blob.sha256,
            blob.byte_size,
            policy,
            &SystemClock,
        )
        .await?;
        let result = store_validated_blob(&mut conn, blob_dir, validated).await;
        if let Some(reservation) = reservation {
            release_reservation(&mut conn, &reservation).await?;
        }
        result.map(|_| ())
    }

    pub async fn prepare_server_blob_uploads(
        &self,
        blob_dir: &Path,
        policy: LifecyclePolicy,
        blobs: &[BlobUploadContract],
    ) -> Result<Vec<String>> {
        super::wire::validate_blob_contracts(blobs)?;
        let mut conn = self.acquire_writer().await?;
        let mut missing = Vec::new();
        let mut missing_hashes = HashSet::new();
        for blob in blobs {
            if blob_available(&mut conn, blob_dir, &blob.sha256).await? {
                reserve_upload(
                    &mut conn,
                    &blob.workspace_id,
                    &blob.sha256,
                    blob.byte_size,
                    policy.quota_bytes,
                    &SystemClock,
                )
                .await?;
            } else if missing_hashes.insert(blob.sha256.as_str()) {
                missing.push(blob.sha256.clone());
            }
        }
        Ok(missing)
    }

    pub async fn store_server_blob(
        &self,
        blob_dir: &Path,
        policy: LifecyclePolicy,
        contract: &BlobUploadContract,
        bytes: Vec<u8>,
    ) -> Result<()> {
        super::wire::validate_blob_contracts(std::slice::from_ref(contract))?;
        if sha256_hex(&bytes) != contract.sha256
            || usize::try_from(contract.byte_size).ok() != Some(bytes.len())
        {
            bail!("error blob-hash-or-size-mismatch");
        }
        let validated = validate_image(bytes, Some(contract.media_type.clone()))
            .await
            .context("error blob-validation-failed")?;
        if (validated.facts.width, validated.facts.height) != (contract.width, contract.height) {
            bail!("error blob-validation-failed");
        }
        let mut conn = self.acquire_writer().await?;
        let reservation = reserve_upload(
            &mut conn,
            &contract.workspace_id,
            &contract.sha256,
            contract.byte_size,
            policy.quota_bytes,
            &SystemClock,
        )
        .await?;
        let result = store_validated_blob(&mut conn, blob_dir, validated).await;
        if result.is_err()
            && let Some(reservation) = reservation
        {
            release_reservation(&mut conn, &reservation).await?;
        }
        result.map(|_| ())
    }

    pub async fn read_server_blob(
        &self,
        blob_dir: &Path,
        sha256: &str,
    ) -> Result<Option<ServerBlobDownload>> {
        super::wire::validate_blob_hashes(&[sha256.to_string()])?;
        let lease = {
            let mut conn = self.acquire_writer().await?;
            if !blob_available(&mut conn, blob_dir, sha256).await? {
                return Ok(None);
            }
            acquire_lease(&mut conn, sha256, "transfer", &SystemClock).await?
        };
        let result = tokio::fs::read(object_path(blob_dir, sha256)?).await;
        let mut conn = self.acquire_writer().await?;
        release_lease(&mut conn, &lease).await?;
        Ok(Some(ServerBlobDownload {
            bytes: result.context("error blob-read-failed")?,
        }))
    }

    pub async fn maintain_server_blobs(
        &self,
        blob_dir: &Path,
        policy: LifecyclePolicy,
    ) -> Result<BoundedMaintenanceSummary> {
        let mut conn = self.acquire_writer().await?;
        maintain_tracked_blobs_bounded(&mut conn, blob_dir, policy, &SystemClock).await
    }
}

async fn missing_local_blob_page(
    conn: &mut SqliteConnection,
    limit: usize,
) -> Result<MissingBlobPage> {
    let rows = sqlx::query(
        "SELECT ta.sha256, MAX(ta.byte_size) AS byte_size,
                MAX(ta.media_type) AS media_type, MAX(ta.width) AS width,
                MAX(ta.height) AS height
         FROM task_attachments ta
         JOIN tasks t ON t.workspace_id = ta.workspace_id AND t.id = ta.task_id
         LEFT JOIN blob_inventory bi ON bi.sha256 = ta.sha256
         WHERE ta.deleted = 0 AND t.deleted = 0
         GROUP BY ta.sha256
         HAVING MAX(COALESCE(bi.available, 0)) = 0
         ORDER BY ta.sha256 LIMIT ?",
    )
    .bind(i64::try_from(limit.saturating_add(1))?)
    .fetch_all(&mut *conn)
    .await?;
    let has_more = rows.len() > limit;
    let blobs = rows
        .into_iter()
        .take(limit)
        .map(|row| MissingLocalBlob {
            sha256: row.get("sha256"),
            byte_size: row.get("byte_size"),
            media_type: row.get("media_type"),
            width: row.get("width"),
            height: row.get("height"),
        })
        .collect();
    Ok(MissingBlobPage { blobs, has_more })
}

pub(super) async fn blob_available(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    sha256: &str,
) -> Result<bool> {
    let Some(row) = blob_inventory_row(conn, sha256).await? else {
        return Ok(false);
    };
    Ok(row.available && object_path(blob_dir, sha256)?.exists())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn attachment_change(op_type: &str) -> ChangeWire {
        ChangeWire {
            change_id: "AAAAAAAAAAAAAAA0".to_string(),
            client_id: "client".to_string(),
            local_seq: 1,
            entity_type: "task".to_string(),
            entity_id: "BBBBBBBBBBBBBBBB".to_string(),
            field: Some("attachments".to_string()),
            op_type: op_type.to_string(),
            payload: json!({
                "workspace_id": "0000000000000000",
                "workspace_key": "default",
                "attachment_id": "7KQ9A1X4MV2P8D6R",
                "sha256": "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
                "byte_size": 12,
                "media_type": "image/png",
                "filename": null,
                "alt_text": null,
                "width": 320,
                "height": 240,
                "created_at": "2026-06-01T00:00:00Z"
            }),
            base_version: None,
            created_at: "2026-06-01T00:00:00Z".to_string(),
            server_seq: None,
        }
    }

    #[test]
    fn attachment_blob_contract_uses_typed_add_payload() {
        let add = attachment_change(op_type::ATTACHMENT_ADD);
        let contract = attachment_blob_contract(&add).unwrap().unwrap();
        assert_eq!(contract.workspace_id, "0000000000000000");
        assert_eq!(
            contract.sha256,
            "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
        );
        assert_eq!(contract.byte_size, 12);
        assert_eq!(contract.media_type, "image/png");
        assert_eq!((contract.width, contract.height), (320, 240));

        let mut malformed = add;
        malformed.payload["sha256"] = json!("invalid");
        assert!(
            attachment_blob_contract(&malformed)
                .unwrap_err()
                .to_string()
                .contains("invalid-sha256")
        );

        assert!(
            attachment_blob_contract(&attachment_change(op_type::ATTACHMENT_DELETE))
                .unwrap()
                .is_none()
        );
        assert!(
            attachment_blob_contract(&attachment_change(op_type::NOTE_ADD))
                .unwrap()
                .is_none()
        );
    }

    async fn seed_missing_attachments(database: &Database, count: usize) {
        let mut conn = database.acquire_writer().await.unwrap();
        for index in 0..count {
            let task_id = format!("{index:016X}");
            let attachment_id = format!("{:016X}", index + 100);
            let sha256 = format!("{index:064x}");
            sqlx::query(
                "INSERT INTO tasks(
                   workspace_id, id, title, description, project_id, status, priority,
                   created_at, updated_at, queue_activity_at
                 ) VALUES ('0000000000000000', ?, 'task', '', 'project', 'inbox', 'none',
                           '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z',
                           '2026-01-01T00:00:00Z')",
            )
            .bind(&task_id)
            .execute(&mut *conn)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO task_attachments(
                   workspace_id, attachment_id, task_id, sha256, byte_size, media_type,
                   width, height, created_at
                 ) VALUES ('0000000000000000', ?, ?, ?, 4, 'image/png', 1, 1,
                           '2026-01-01T00:00:00Z')",
            )
            .bind(attachment_id)
            .bind(task_id)
            .bind(sha256)
            .execute(&mut *conn)
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn missing_blob_page_is_bounded_and_reports_more() {
        let temp = tempfile::tempdir().unwrap();
        let database = Database::open(&temp.path().join("replica.sqlite"))
            .await
            .unwrap();
        seed_missing_attachments(&database, 3).await;

        let page = database.missing_local_blob_page(2).await.unwrap();
        assert_eq!(page.blobs.len(), 2);
        assert!(page.has_more);
        assert_eq!(
            database.missing_local_blob_counts().await.unwrap(),
            ByteCount {
                count: 3,
                bytes: 12
            }
        );
    }
}
