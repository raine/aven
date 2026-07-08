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
    db_path: &Path,
    blob_dir: &Path,
    output: &Path,
) -> Result<()> {
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    let staging = tempfile::tempdir().context("could not create backup staging directory")?;
    let database_path = staging.path().join(DATABASE_ENTRY);
    db::backup_database(db_path, &database_path)?;

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
            bail!("error backup-blob-missing sha256={}", row.sha256);
        }
        let bytes =
            fs::read(&source).with_context(|| format!("could not read {}", source.display()))?;
        validate_object_bytes(&row.sha256, row.byte_size, &bytes)?;
        fs::write(objects_dir.join(&row.sha256), &bytes)
            .with_context(|| format!("could not stage blob {}", row.sha256))?;
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
    extract_archive(archive, staging.path())?;
    let manifest_path = staging.path().join(MANIFEST_ENTRY);
    let manifest: BackupManifest = serde_json::from_slice(
        &fs::read(&manifest_path)
            .with_context(|| format!("could not read {}", manifest_path.display()))?,
    )
    .context("could not parse backup manifest")?;
    validate_manifest(&manifest)?;
    let database_path = staging.path().join(&manifest.database);
    validate_sqlite_file(&database_path).await?;
    for object in &manifest.objects {
        let path = staging
            .path()
            .join("objects")
            .join("sha256")
            .join(&object.sha256);
        let bytes =
            fs::read(&path).with_context(|| format!("could not read {}", path.display()))?;
        validate_object_bytes(&object.sha256, object.byte_size, &bytes)?;
    }

    let safety = db::default_sqlite_backup_path(db_path, "before-restore")?;
    db::backup_database(db_path, &safety)?;
    let sidecar_safety = db::default_sqlite_backup_path(db_path, "before-restore-blobs")?;
    if blob_dir.exists() {
        copy_dir(blob_dir, &sidecar_safety.with_extension("blobdir"))?;
    }

    for object in &manifest.objects {
        let target = object_path(blob_dir, &object.sha256)?;
        let source = staging
            .path()
            .join("objects")
            .join("sha256")
            .join(&object.sha256);
        if target.exists() {
            let existing = fs::read(&target)
                .with_context(|| format!("could not read {}", target.display()))?;
            validate_object_bytes(&object.sha256, object.byte_size, &existing)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("could not create {}", parent.display()))?;
            let tmp = tempfile::NamedTempFile::new_in(parent).with_context(|| {
                format!("could not create temporary file in {}", parent.display())
            })?;
            fs::copy(&source, tmp.path()).with_context(|| {
                format!(
                    "could not copy {} -> {}",
                    source.display(),
                    tmp.path().display()
                )
            })?;
            tmp.persist(&target)
                .map_err(|error| error.error)
                .with_context(|| format!("could not replace {}", target.display()))?;
        }
    }

    let db_tmp = db_path.with_extension("restore-staging");
    fs::copy(&database_path, &db_tmp).with_context(|| {
        format!(
            "could not copy {} -> {}",
            database_path.display(),
            db_tmp.display()
        )
    })?;
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

fn validate_object_bytes(sha256: &str, byte_size: i64, bytes: &[u8]) -> Result<()> {
    let actual_size = i64::try_from(bytes.len()).context("blob size exceeds i64")?;
    if actual_size != byte_size {
        bail!("error backup-blob-size-mismatch sha256={sha256}");
    }
    let actual_sha = sha256_hex(bytes);
    if actual_sha != sha256 {
        bail!("error backup-blob-hash-mismatch sha256={sha256}");
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

fn extract_archive(archive: &Path, target: &Path) -> Result<()> {
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
            bail!("error backup-entry-duplicate path={}", path.display());
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
    Ok(())
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
    bail!("error backup-entry-unexpected path={}", path.display());
}

fn validate_relative_entry(path: &Path) -> Result<()> {
    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::Prefix(_)))
    {
        bail!("error backup-entry-invalid path={}", path.display());
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
