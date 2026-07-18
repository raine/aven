#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use sqlx::SqliteConnection;

use crate::ids::now;
use crate::types::BlobInventoryRow;

use super::decode::{ImageFacts, ValidatedImage, validate_image};
use super::validation::{validate_media_type, validate_sha256};

pub struct StoredBlob {
    pub sha256: String,
    pub byte_size: i64,
    pub facts: ImageFacts,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedBlob {
    pub sha256: String,
    pub byte_size: i64,
    pub created: bool,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

pub fn object_path(blob_dir: &Path, sha256: &str) -> Result<PathBuf> {
    validate_sha256(sha256)?;
    Ok(blob_dir.join("objects").join("sha256").join(sha256))
}

pub async fn store_blob(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    media_type: &str,
    bytes: &[u8],
) -> Result<StoredBlob> {
    let validated = validate_image(bytes.to_vec(), Some(media_type.to_string())).await?;
    store_validated_blob(conn, blob_dir, validated).await
}

pub async fn store_validated_blob(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    validated: ValidatedImage,
) -> Result<StoredBlob> {
    let sha256 = sha256_hex(&validated.bytes);
    let bytes = validated.bytes;
    let staged = stage_blob(blob_dir, &sha256, &bytes).await?;
    if let Err(error) = upsert_inventory_available(
        conn,
        &staged.sha256,
        staged.byte_size,
        &validated.facts.media_type,
    )
    .await
    {
        if staged.created {
            remove_staged_blob_if_unreferenced(conn, blob_dir, &staged.sha256).await;
        }
        return Err(error);
    }
    Ok(StoredBlob {
        sha256: staged.sha256,
        byte_size: staged.byte_size,
        facts: validated.facts,
    })
}

pub async fn stage_blob(blob_dir: &Path, sha256: &str, bytes: &[u8]) -> Result<StagedBlob> {
    validate_sha256(sha256)?;
    if sha256_hex(bytes) != sha256 {
        bail!("error attachment-prepared-hash-mismatch");
    }
    let path = object_path(blob_dir, sha256)?;
    let bytes = bytes.to_vec();
    let write_path = path.clone();
    let created =
        super::blocking::run(move || write_object_atomically(&write_path, &bytes)).await?;
    let byte_size = fs::metadata(&path)
        .context("could not inspect attachment object")?
        .len();
    let byte_size = i64::try_from(byte_size).context("attachment bytes exceed i64")?;
    Ok(StagedBlob {
        sha256: sha256.to_string(),
        byte_size,
        created,
    })
}

pub async fn remove_staged_blob_if_unreferenced(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    sha256: &str,
) {
    let referenced = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM task_attachments WHERE sha256 = ?)",
    )
    .bind(sha256)
    .fetch_one(&mut *conn)
    .await;
    let pending_upload = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
            SELECT 1 FROM changes
            WHERE server_seq IS NULL AND op_type = 'attachment_add'
              AND json_extract(payload, '$.sha256') = ?
         )",
    )
    .bind(sha256)
    .fetch_one(&mut *conn)
    .await;
    if matches!(referenced, Ok(false))
        && matches!(pending_upload, Ok(false))
        && let Ok(path) = object_path(blob_dir, sha256)
    {
        let _ = fs::remove_file(path);
    }
}

fn write_object_atomically(path: &Path, bytes: &[u8]) -> Result<bool> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context("could not create attachment object directory")?;
        if path.exists() {
            let existing = fs::read(path).context("could not read attachment object")?;
            if sha256_hex(&existing) != sha256_hex(bytes) {
                bail!("error attachment-object-content-mismatch");
            }
            return Ok(false);
        }
        let mut temp = tempfile::Builder::new()
            .prefix(".aven-stage-")
            .tempfile_in(parent)
            .context("could not create attachment staging file")?;
        std::io::Write::write_all(&mut temp, bytes)?;
        temp.as_file().sync_all()?;
        match temp.persist_noclobber(path) {
            Ok(_) => {
                if let Err(error) =
                    fs::File::open(parent).and_then(|directory| directory.sync_all())
                {
                    let _ = fs::remove_file(path);
                    return Err(error.into());
                }
                return Ok(true);
            }
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                let existing = fs::read(path).context("could not read attachment object")?;
                if sha256_hex(&existing) == sha256_hex(bytes) {
                    return Ok(false);
                }
                bail!("error attachment-object-content-mismatch");
            }
            Err(error) => return Err(error.error.into()),
        }
    }
    bail!("error attachment-object-path-invalid")
}

pub async fn upsert_inventory_available(
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

pub async fn blob_inventory_row(
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
    use std::io::Cursor;
    use std::path::PathBuf;

    use image::{DynamicImage, ImageFormat, RgbaImage};

    use crate::db::open_db;

    use super::*;

    fn png_bytes() -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(RgbaImage::new(2, 1))
            .write_to(&mut bytes, ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    }

    #[tokio::test]
    async fn stores_and_retrieves_blob() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("test.sqlite");
        let pool = open_db(&db_path).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();

        let blob_dir = temp.path().join("blobs");

        let bytes = png_bytes();
        let stored = store_blob(&mut conn, &blob_dir, "image/png", &bytes)
            .await
            .unwrap();

        assert_eq!(stored.sha256, sha256_hex(&bytes));
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

        let blob_dir = temp.path().join("blobs");

        let bytes = png_bytes();
        let stored1 = store_blob(&mut conn, &blob_dir, "image/png", &bytes)
            .await
            .unwrap();
        let stored2 = store_blob(&mut conn, &blob_dir, "image/png", &bytes)
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

    #[tokio::test]
    async fn inventory_failure_removes_only_newly_staged_blob() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("test.sqlite");
        let pool = open_db(&db_path).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let blob_dir = temp.path().join("blobs");
        let bytes = png_bytes();
        let sha256 = sha256_hex(&bytes);
        upsert_inventory_available(&mut conn, &sha256, 12, "image/png")
            .await
            .unwrap();

        assert!(
            store_blob(&mut conn, &blob_dir, "image/png", &bytes)
                .await
                .is_err()
        );
        assert!(!object_path(&blob_dir, &sha256).unwrap().exists());
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
