#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use sqlx::SqliteConnection;

use crate::ids::now;
use crate::types::BlobInventoryRow;

use super::validation::{validate_blob_size, validate_media_type, validate_sha256};

pub(crate) struct StoredBlob {
    pub(crate) sha256: String,
    pub(crate) byte_size: i64,
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

pub(crate) fn object_path(blob_dir: &Path, sha256: &str) -> Result<PathBuf> {
    validate_sha256(sha256)?;
    Ok(blob_dir.join("objects").join("sha256").join(sha256))
}

pub(crate) async fn store_blob(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    media_type: &str,
    bytes: &[u8],
) -> Result<StoredBlob> {
    validate_media_type(media_type)?;
    validate_blob_size(bytes.len())?;
    let sha256 = sha256_hex(bytes);
    let path = object_path(blob_dir, &sha256)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    if !path.exists() {
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, bytes).with_context(|| format!("could not write {}", tmp.display()))?;
        fs::rename(&tmp, &path).with_context(|| format!("could not replace {}", path.display()))?;
    }
    let byte_size = i64::try_from(bytes.len()).context("attachment bytes exceed i64")?;
    upsert_inventory_available(conn, &sha256, byte_size, media_type).await?;
    Ok(StoredBlob { sha256, byte_size })
}

pub(crate) async fn upsert_inventory_available(
    conn: &mut SqliteConnection,
    sha256: &str,
    byte_size: i64,
    media_type: &str,
) -> Result<()> {
    validate_sha256(sha256)?;
    validate_media_type(media_type)?;
    let existing = blob_inventory_row(conn, sha256).await?;
    let timestamp = now();
    if let Some(row) = existing {
        if row.byte_size != byte_size || row.media_type != media_type {
            bail!("error blob-inventory-metadata-mismatch");
        }
        sqlx::query(
            "UPDATE blob_inventory
             SET available = 1, last_verified_at = ?
             WHERE sha256 = ?",
        )
        .bind(&timestamp)
        .bind(sha256)
        .execute(&mut *conn)
        .await?;
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO blob_inventory(sha256, byte_size, media_type, available, first_seen_at, last_verified_at)
         VALUES (?, ?, ?, 1, ?, ?)",
    )
    .bind(sha256)
    .bind(byte_size)
    .bind(media_type)
    .bind(&timestamp)
    .bind(&timestamp)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

pub(crate) async fn blob_inventory_row(
    conn: &mut SqliteConnection,
    sha256: &str,
) -> Result<Option<BlobInventoryRow>> {
    validate_sha256(sha256)?;
    let row = sqlx::query!(
        "SELECT sha256, byte_size, media_type, available, first_seen_at, last_verified_at
         FROM blob_inventory WHERE sha256 = ?",
        sha256
    )
    .fetch_optional(&mut *conn)
    .await?;
    Ok(row.map(|row| BlobInventoryRow {
        sha256: row.sha256.unwrap_or_default(),
        byte_size: row.byte_size,
        media_type: row.media_type,
        available: row.available != 0,
        first_seen_at: row.first_seen_at,
        last_verified_at: row.last_verified_at,
    }))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::config::{AppConfig, resolve_blob_dir};
    use crate::db::open_db;

    use super::*;

    #[tokio::test]
    async fn stores_and_retrieves_blob() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("test.sqlite");
        let pool = open_db(&db_path).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();

        let config = AppConfig::default();
        let blob_dir = resolve_blob_dir(&db_path, &config).unwrap();

        let bytes = b"hello world from aven attachment";
        let stored = store_blob(&mut conn, &blob_dir, "image/png", bytes)
            .await
            .unwrap();

        assert_eq!(stored.sha256, sha256_hex(bytes));
        assert!(stored.byte_size > 0);

        let obj_path = object_path(&blob_dir, &stored.sha256).unwrap();
        assert!(obj_path.exists(), "blob file should exist on disk");

        let on_disk = std::fs::read(&obj_path).unwrap();
        assert_eq!(on_disk, bytes);

        let row = blob_inventory_row(&mut conn, &stored.sha256)
            .await
            .unwrap()
            .expect("inventory row should exist");
        assert!(row.available);
        assert_eq!(row.sha256, stored.sha256);
        assert_eq!(row.byte_size, stored.byte_size);
    }

    #[tokio::test]
    async fn stores_blob_idempotently() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("test.sqlite");
        let pool = open_db(&db_path).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();

        let config = AppConfig::default();
        let blob_dir = resolve_blob_dir(&db_path, &config).unwrap();

        let bytes = b"same content";
        let stored1 = store_blob(&mut conn, &blob_dir, "image/jpeg", bytes)
            .await
            .unwrap();
        let stored2 = store_blob(&mut conn, &blob_dir, "image/jpeg", bytes)
            .await
            .unwrap();

        assert_eq!(stored1.sha256, stored2.sha256);
        let row = blob_inventory_row(&mut conn, &stored1.sha256)
            .await
            .unwrap()
            .unwrap();
        assert!(row.available);
    }

    #[tokio::test]
    async fn rejects_inventory_metadata_mismatch() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("test.sqlite");
        let pool = open_db(&db_path).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();

        let sha256 = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
        upsert_inventory_available(&mut conn, sha256, 12, "image/png")
            .await
            .unwrap();

        let error = upsert_inventory_available(&mut conn, sha256, 13, "image/png")
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(error, "error blob-inventory-metadata-mismatch");

        let error = upsert_inventory_available(&mut conn, sha256, 12, "image/jpeg")
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(error, "error blob-inventory-metadata-mismatch");
    }

    #[test]
    fn computes_sha256_hex() {
        let result = sha256_hex(b"hello");
        assert_eq!(result.len(), 64);
        assert!(result.bytes().all(|b| b.is_ascii_hexdigit()));
    }

    #[test]
    fn computes_object_path() {
        let blob_dir = Path::new("/tmp/aven");
        let hash = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
        let path = object_path(blob_dir, hash).unwrap();
        assert_eq!(
            path,
            PathBuf::from(
                "/tmp/aven/objects/sha256/abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
            )
        );
    }

    #[test]
    fn rejects_invalid_hash_in_object_path() {
        assert!(object_path(Path::new("/tmp"), "short").is_err());
    }
}
