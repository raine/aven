use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result, bail};
use sqlx::{Row, SqliteConnection};

use super::wire::{BlobUploadContract, ChangeWire};
use crate::attachments::decode::validate_image;
use crate::attachments::lifecycle::{
    ByteCount, LifecyclePolicy, SystemClock, acquire_lease, ensure_local_capacity, prune,
    release_lease, release_reservation, reserve_upload,
};
use crate::attachments::storage::{
    blob_inventory_row, object_path, sha256_hex, store_validated_blob,
};
use crate::change_log::op_type;
use crate::db::Database;

#[derive(Debug, Clone)]
pub(super) struct MissingLocalBlob {
    pub sha256: String,
    pub byte_size: i64,
    pub media_type: String,
    pub width: Option<i64>,
    pub height: Option<i64>,
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
    Ok(Some(BlobUploadContract {
        workspace_id: payload_string(change, "workspace_id")?,
        sha256: payload_string(change, "sha256")?,
        byte_size: payload_i64(change, "byte_size")?,
        media_type: payload_string(change, "media_type")?,
        width: payload_i64(change, "width")?,
        height: payload_i64(change, "height")?,
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

fn payload_string(change: &ChangeWire, key: &str) -> Result<String> {
    change
        .payload
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .with_context(|| format!("payload missing {key}"))
}

fn payload_i64(change: &ChangeWire, key: &str) -> Result<i64> {
    change
        .payload
        .get(key)
        .and_then(serde_json::Value::as_i64)
        .with_context(|| format!("payload missing {key}"))
}

impl Database {
    pub(super) async fn prepare_blob_upload(
        &self,
        blob_dir: &Path,
        contract: &BlobUploadContract,
    ) -> Result<PreparedBlobUpload> {
        let mut conn = self.acquire_writer().await?;
        let row = blob_inventory_row(&mut conn, &contract.sha256)
            .await?
            .filter(|row| row.available)
            .context("error attachment-blob-local-missing")?;
        let bytes = tokio::fs::read(object_path(blob_dir, &contract.sha256)?)
            .await
            .context("error attachment-blob-local-missing")?;
        if sha256_hex(&bytes) != contract.sha256
            || row.byte_size != i64::try_from(bytes.len()).context("attachment bytes exceed i64")?
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
        let lease_id = acquire_lease(&mut conn, &contract.sha256, "transfer", &SystemClock).await?;
        Ok(PreparedBlobUpload { bytes, lease_id })
    }

    pub(super) async fn finish_blob_upload(&self, lease_id: &str) -> Result<()> {
        let mut conn = self.acquire_writer().await?;
        release_lease(&mut conn, lease_id).await
    }

    pub(super) async fn missing_local_blobs(
        &self,
        blob_dir: &Path,
    ) -> Result<Vec<MissingLocalBlob>> {
        let mut conn = self.acquire_reader().await?;
        missing_local_blobs(&mut conn, blob_dir).await
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
        prune(&mut conn, blob_dir, policy, true, &SystemClock).await?;
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
        prune(&mut conn, blob_dir, policy, true, &SystemClock).await?;
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
        let mut conn = self.acquire_writer().await?;
        if !blob_available(&mut conn, blob_dir, sha256).await? {
            return Ok(None);
        }
        let lease = acquire_lease(&mut conn, sha256, "transfer", &SystemClock).await?;
        let result = tokio::fs::read(object_path(blob_dir, sha256)?).await;
        release_lease(&mut conn, &lease).await?;
        Ok(Some(ServerBlobDownload {
            bytes: result.context("error blob-read-failed")?,
        }))
    }

    pub async fn maintain_server_blobs(
        &self,
        blob_dir: &Path,
        policy: LifecyclePolicy,
    ) -> Result<()> {
        let mut conn = self.acquire_writer().await?;
        prune(&mut conn, blob_dir, policy, true, &SystemClock)
            .await
            .map(|_| ())
    }
}

async fn missing_local_blobs(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
) -> Result<Vec<MissingLocalBlob>> {
    let rows = sqlx::query(
        "SELECT ta.sha256, MAX(ta.byte_size) AS byte_size,
                MAX(ta.media_type) AS media_type, MAX(ta.width) AS width,
                MAX(ta.height) AS height, MAX(COALESCE(bi.available, 0)) AS available
         FROM task_attachments ta
         JOIN tasks t ON t.workspace_id = ta.workspace_id AND t.id = ta.task_id
         LEFT JOIN blob_inventory bi ON bi.sha256 = ta.sha256
         WHERE ta.deleted = 0 AND t.deleted = 0
         GROUP BY ta.sha256
         ORDER BY ta.sha256",
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut missing = Vec::new();
    for row in rows {
        let sha256: String = row.get("sha256");
        let available: i64 = row.get("available");
        if available == 0 || !object_path(blob_dir, &sha256)?.exists() {
            missing.push(MissingLocalBlob {
                sha256,
                byte_size: row.get("byte_size"),
                media_type: row.get("media_type"),
                width: row.get("width"),
                height: row.get("height"),
            });
        }
    }
    Ok(missing)
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

pub(super) fn missing_counts(blobs: &[MissingLocalBlob]) -> ByteCount {
    ByteCount {
        count: blobs.len() as u64,
        bytes: blobs
            .iter()
            .map(|blob| u64::try_from(blob.byte_size).unwrap_or(0))
            .sum(),
    }
}
