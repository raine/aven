use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection as _, SqliteConnection};

use crate::attachments::storage::{object_path, sha256_hex};
use crate::db;
use crate::ids::now;

const BACKUP_FORMAT: &str = "aven-backup";
const BACKUP_VERSION: i64 = 1;
const DATABASE_ENTRY: &str = "database.sqlite";
const MANIFEST_ENTRY: &str = "manifest.json";

#[derive(Debug, sqlx::FromRow)]
struct ArchiveAttachmentRow {
    attachment_id: String,
    sha256: String,
    byte_size: i64,
    media_type: String,
    filename: Option<String>,
    alt_text: Option<String>,
    width: Option<i64>,
    height: Option<i64>,
    deleted: i64,
}

#[derive(Debug, sqlx::FromRow)]
struct ArchiveInventoryRow {
    sha256: String,
    byte_size: i64,
    media_type: String,
    available: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct BackupManifest {
    format: String,
    version: i64,
    created_at: String,
    database: String,
    objects: Vec<BackupObjectManifest>,
}

#[derive(Debug, Serialize, Deserialize)]
struct BackupObjectManifest {
    sha256: String,
    byte_size: i64,
    media_type: String,
}

#[derive(Debug, sqlx::FromRow)]
struct AvailableBlobRow {
    sha256: String,
    byte_size: i64,
    media_type: String,
}

pub(super) async fn create_backup_archive(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    output: &Path,
) -> Result<()> {
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    let staging = tempfile::tempdir().context("could not create backup staging directory")?;
    let database_path = staging.path().join(DATABASE_ENTRY);
    db::backup_database_with_connection(conn, &database_path).await?;

    let rows: Vec<AvailableBlobRow> = sqlx::query_as(
        "SELECT sha256, byte_size, media_type FROM blob_inventory WHERE available = 1 ORDER BY sha256",
    )
    .fetch_all(&mut *conn)
    .await?;
    let objects_dir = staging.path().join("objects").join("sha256");
    fs::create_dir_all(&objects_dir)
        .with_context(|| format!("could not create {}", objects_dir.display()))?;
    let mut objects = Vec::new();
    for row in rows {
        let source = object_path(blob_dir, &row.sha256)?;
        if !source.exists() {
            bail!("error backup-blob-missing");
        }
        let bytes = fs::read(&source).context("error backup-blob-read")?;
        validate_object_bytes(&row.sha256, row.byte_size, &row.media_type, &bytes).await?;
        fs::write(objects_dir.join(&row.sha256), &bytes).context("error backup-blob-stage")?;
        objects.push(BackupObjectManifest {
            sha256: row.sha256,
            byte_size: row.byte_size,
            media_type: row.media_type,
        });
    }

    let manifest = BackupManifest {
        format: BACKUP_FORMAT.to_string(),
        version: BACKUP_VERSION,
        created_at: now(),
        database: DATABASE_ENTRY.to_string(),
        objects,
    };
    fs::write(
        staging.path().join(MANIFEST_ENTRY),
        serde_json::to_vec(&manifest).context("could not serialize backup manifest")?,
    )?;

    let tmp = output.with_extension("tmp");
    let file =
        fs::File::create(&tmp).with_context(|| format!("could not create {}", tmp.display()))?;
    let encoder =
        zstd::stream::write::Encoder::new(file, 0).context("could not create zstd encoder")?;
    let mut tar = tar::Builder::new(encoder);
    tar.append_path_with_name(staging.path().join(MANIFEST_ENTRY), MANIFEST_ENTRY)?;
    tar.append_path_with_name(&database_path, DATABASE_ENTRY)?;
    for object in &manifest.objects {
        let entry = format!("objects/sha256/{}", object.sha256);
        tar.append_path_with_name(objects_dir.join(&object.sha256), entry)?;
    }
    let encoder = tar.into_inner().context("could not finish backup tar")?;
    encoder
        .finish()
        .context("could not finish backup compression")?;
    fs::rename(&tmp, output).with_context(|| format!("could not replace {}", output.display()))?;
    Ok(())
}

pub(super) async fn restore_backup_archive(
    db_path: &Path,
    blob_dir: &Path,
    archive: &Path,
) -> Result<PathBuf> {
    let staging = tempfile::tempdir().context("could not create restore staging directory")?;
    let entries = extract_archive(archive, staging.path())?;
    let manifest_path = staging.path().join(MANIFEST_ENTRY);
    let manifest: BackupManifest = serde_json::from_slice(
        &fs::read(&manifest_path)
            .with_context(|| format!("could not read {}", manifest_path.display()))?,
    )
    .context("could not parse backup manifest")?;
    validate_manifest(&manifest)?;
    validate_archive_entries(&entries, &manifest)?;
    let database_path = staging.path().join(&manifest.database);
    validate_sqlite_file(&database_path).await?;
    for object in &manifest.objects {
        let path = staging
            .path()
            .join("objects")
            .join("sha256")
            .join(&object.sha256);
        let bytes = fs::read(&path).context("error backup-blob-read")?;
        validate_object_bytes(&object.sha256, object.byte_size, &object.media_type, &bytes).await?;
    }
    validate_archive_attachment_metadata(&database_path, staging.path(), &manifest).await?;

    let safety = db::create_restore_safety_backup(db_path).await?;
    let sidecar_safety = db::default_sqlite_backup_path(db_path, "before-restore-blobs")?;
    if blob_dir.exists() {
        copy_dir(blob_dir, &sidecar_safety.with_extension("blobdir"))?;
    }

    let blob_parent = blob_dir
        .parent()
        .context("error backup-blob-directory-invalid")?;
    fs::create_dir_all(blob_parent).context("could not create attachment restore directory")?;
    let replacement = tempfile::Builder::new()
        .prefix(".aven-restore-blobs-")
        .tempdir_in(blob_parent)
        .context("could not create restore blob directory")?;
    let replacement_objects = replacement.path().join("objects").join("sha256");
    fs::create_dir_all(&replacement_objects)
        .context("could not create restore object directory")?;
    for object in &manifest.objects {
        let source = staging
            .path()
            .join("objects")
            .join("sha256")
            .join(&object.sha256);
        fs::copy(&source, replacement_objects.join(&object.sha256))
            .context("could not stage restored attachment object")?;
    }

    let db_tmp = db_path.with_extension("restore-staging");
    fs::copy(&database_path, &db_tmp).with_context(|| {
        format!(
            "could not copy {} -> {}",
            database_path.display(),
            db_tmp.display()
        )
    })?;
    if blob_dir.exists() {
        fs::remove_dir_all(blob_dir).context("could not replace attachment object directory")?;
    }
    fs::rename(replacement.keep(), blob_dir)
        .context("could not install restored attachment objects")?;
    for sidecar in [db::wal_path(db_path), db::shm_path(db_path)] {
        if sidecar.exists() {
            fs::remove_file(&sidecar)
                .with_context(|| format!("could not remove {}", sidecar.display()))?;
        }
    }
    fs::rename(&db_tmp, db_path)
        .with_context(|| format!("could not replace {}", db_path.display()))?;
    Ok(safety)
}

pub(super) fn is_archive_path(path: &Path) -> Result<bool> {
    let mut file =
        fs::File::open(path).with_context(|| format!("could not open {}", path.display()))?;
    let mut magic = [0_u8; 4];
    let read = file.read(&mut magic)?;
    Ok(read == 4 && magic == [0x28, 0xb5, 0x2f, 0xfd])
}

fn validate_archive_entries(entries: &HashSet<PathBuf>, manifest: &BackupManifest) -> Result<()> {
    let mut expected =
        HashSet::from([PathBuf::from(MANIFEST_ENTRY), PathBuf::from(DATABASE_ENTRY)]);
    expected.extend(
        manifest
            .objects
            .iter()
            .map(|object| PathBuf::from(format!("objects/sha256/{}", object.sha256))),
    );
    if entries != &expected {
        bail!("error backup-entry-set-mismatch");
    }
    Ok(())
}

fn validate_manifest(manifest: &BackupManifest) -> Result<()> {
    if manifest.format != BACKUP_FORMAT || manifest.version != BACKUP_VERSION {
        bail!("error backup-format-unsupported");
    }
    if manifest.database != DATABASE_ENTRY {
        bail!(
            "error backup-manifest-invalid database={}",
            manifest.database
        );
    }
    let mut seen = HashSet::new();
    for object in &manifest.objects {
        crate::attachments::validation::validate_sha256(&object.sha256)?;
        crate::attachments::validation::validate_media_type(&object.media_type)?;
        crate::attachments::validation::validate_blob_size(
            usize::try_from(object.byte_size).unwrap_or(0),
        )?;
        if !seen.insert(object.sha256.clone()) {
            bail!(
                "error backup-manifest-duplicate-object sha256={}",
                object.sha256
            );
        }
    }
    Ok(())
}

async fn validate_object_bytes(
    sha256: &str,
    byte_size: i64,
    media_type: &str,
    bytes: &[u8],
) -> Result<crate::attachments::decode::ImageFacts> {
    let sha256 = sha256.to_string();
    let media_type = media_type.to_string();
    let bytes = bytes.to_vec();
    crate::attachments::blocking::run(move || {
        validate_object_bytes_blocking(&sha256, byte_size, &media_type, &bytes)
    })
    .await
}

fn validate_object_bytes_blocking(
    sha256: &str,
    byte_size: i64,
    media_type: &str,
    bytes: &[u8],
) -> Result<crate::attachments::decode::ImageFacts> {
    let actual_size = i64::try_from(bytes.len()).context("blob size exceeds i64")?;
    if actual_size != byte_size {
        bail!("error backup-blob-size-mismatch");
    }
    let actual_sha = sha256_hex(bytes);
    if actual_sha != sha256 {
        bail!("error backup-blob-hash-mismatch");
    }
    let validated =
        crate::attachments::decode::validate_image_blocking(bytes.to_vec(), Some(media_type))
            .context("error backup-blob-image-invalid")?;
    Ok(validated.facts)
}

async fn validate_archive_attachment_metadata(
    database_path: &Path,
    staging: &Path,
    manifest: &BackupManifest,
) -> Result<()> {
    let mut conn = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(database_path)
            .read_only(true)
            .foreign_keys(true),
    )
    .await?;
    let inventory: Vec<ArchiveInventoryRow> = sqlx::query_as(
        "SELECT sha256, byte_size, media_type, available FROM blob_inventory ORDER BY sha256",
    )
    .fetch_all(&mut conn)
    .await?;
    let available = inventory
        .iter()
        .filter(|row| row.available == 1)
        .map(|row| row.sha256.clone())
        .collect::<HashSet<_>>();
    let manifested = manifest
        .objects
        .iter()
        .map(|object| object.sha256.clone())
        .collect::<HashSet<_>>();
    if available != manifested {
        bail!("error backup-inventory-object-set-mismatch");
    }
    for row in &inventory {
        crate::attachments::validation::validate_sha256(&row.sha256)?;
        crate::attachments::validation::validate_blob_size(
            usize::try_from(row.byte_size).unwrap_or(0),
        )?;
        crate::attachments::validation::validate_media_type(&row.media_type)?;
        if row.available != 0 && row.available != 1 {
            bail!("error backup-inventory-availability-invalid");
        }
        if row.available == 1 {
            let object = manifest
                .objects
                .iter()
                .find(|object| object.sha256 == row.sha256)
                .context("error backup-inventory-object-missing")?;
            if object.byte_size != row.byte_size || object.media_type != row.media_type {
                bail!("error backup-inventory-metadata-mismatch");
            }
        }
    }

    let attachments: Vec<ArchiveAttachmentRow> = sqlx::query_as(
        "SELECT attachment_id, sha256, byte_size, media_type, filename, alt_text, width, height, deleted
         FROM task_attachments",
    )
    .fetch_all(&mut conn)
    .await?;
    for attachment in attachments {
        crate::attachments::validation::validate_attachment_id(&attachment.attachment_id)?;
        crate::attachments::validation::validate_sha256(&attachment.sha256)?;
        crate::attachments::validation::validate_blob_size(
            usize::try_from(attachment.byte_size).unwrap_or(0),
        )?;
        crate::attachments::validation::validate_media_type(&attachment.media_type)?;
        crate::attachments::validation::validate_filename(attachment.filename.as_deref())?;
        crate::attachments::validation::validate_alt_text(attachment.alt_text.as_deref())?;
        crate::attachments::validation::validate_dimensions(attachment.width, attachment.height)?;
        if attachment.deleted != 0 && attachment.deleted != 1 {
            bail!("error backup-attachment-deletion-state-invalid");
        }
        let inventory_row = inventory
            .iter()
            .find(|row| row.sha256 == attachment.sha256)
            .context("error backup-attachment-inventory-missing")?;
        if inventory_row.byte_size != attachment.byte_size
            || inventory_row.media_type != attachment.media_type
        {
            bail!("error backup-attachment-metadata-mismatch");
        }
        if inventory_row.available == 1 {
            let path = staging
                .join("objects")
                .join("sha256")
                .join(&attachment.sha256);
            let bytes = fs::read(path).context("error backup-attachment-object-read")?;
            let facts = validate_object_bytes(
                &attachment.sha256,
                attachment.byte_size,
                &attachment.media_type,
                &bytes,
            )
            .await?;
            if (attachment.width, attachment.height) != (Some(facts.width), Some(facts.height)) {
                bail!("error backup-attachment-dimensions-mismatch");
            }
        }
    }
    Ok(())
}

async fn validate_sqlite_file(path: &Path) -> Result<()> {
    let mut conn = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(path)
            .read_only(true)
            .foreign_keys(true),
    )
    .await
    .with_context(|| format!("could not open source {}", path.display()))?;
    let quick_check: String = sqlx::query_scalar("PRAGMA quick_check")
        .fetch_one(&mut conn)
        .await?;
    if quick_check != "ok" {
        bail!("error backup-source-corrupt quick_check={quick_check}");
    }
    Ok(())
}

fn extract_archive(archive: &Path, target: &Path) -> Result<HashSet<PathBuf>> {
    let file =
        fs::File::open(archive).with_context(|| format!("could not open {}", archive.display()))?;
    let decoder =
        zstd::stream::read::Decoder::new(file).context("could not create zstd decoder")?;
    let mut archive = tar::Archive::new(decoder);
    let mut seen = HashSet::new();
    for entry in archive.entries().context("could not read backup archive")? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            bail!("error backup-entry-unsupported");
        }
        let path = entry.path()?.into_owned();
        validate_backup_entry(&path)?;
        if !seen.insert(path.clone()) {
            bail!("error backup-entry-duplicate");
        }
        let target_path = target.join(path);
        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("could not create {}", parent.display()))?;
        }
        entry
            .unpack(target_path)
            .context("could not unpack backup entry")?;
    }
    if !seen.contains(Path::new(MANIFEST_ENTRY)) || !seen.contains(Path::new(DATABASE_ENTRY)) {
        bail!("error backup-entry-missing");
    }
    Ok(seen)
}

fn validate_backup_entry(path: &Path) -> Result<()> {
    validate_relative_entry(path)?;
    if path == Path::new(MANIFEST_ENTRY) || path == Path::new(DATABASE_ENTRY) {
        return Ok(());
    }
    let components = path.components().collect::<Vec<_>>();
    if components.len() == 3
        && components[0] == Component::Normal("objects".as_ref())
        && components[1] == Component::Normal("sha256".as_ref())
        && let Component::Normal(name) = components[2]
        && let Some(name) = name.to_str()
    {
        crate::attachments::validation::validate_sha256(name)?;
        return Ok(());
    }
    bail!("error backup-entry-unexpected");
}

fn validate_relative_entry(path: &Path) -> Result<()> {
    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::Prefix(_)))
    {
        bail!("error backup-entry-invalid");
    }
    Ok(())
}

fn copy_dir(source: &Path, target: &Path) -> Result<()> {
    fs::create_dir_all(target).with_context(|| format!("could not create {}", target.display()))?;
    for entry in
        fs::read_dir(source).with_context(|| format!("could not read {}", source.display()))?
    {
        let entry = entry?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&source_path, &target_path)?;
        } else {
            fs::copy(&source_path, &target_path).with_context(|| {
                format!(
                    "could not copy {} -> {}",
                    source_path.display(),
                    target_path.display()
                )
            })?;
        }
    }
    Ok(())
}
