use std::collections::{HashMap, HashSet};
use std::fs;
use std::future::Future;
use std::io::{Read, Write as _};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection as _, SqliteConnection};

use crate::attachments::storage::{object_path, sha256_hex};
use crate::db;
use crate::ids::now;
use crate::private_fs;

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
    create_backup_archive_with_snapshot_hook(conn, blob_dir, output, || async { Ok(()) }).await
}

async fn create_backup_archive_with_snapshot_hook<F, Fut>(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    output: &Path,
    after_snapshot: F,
) -> Result<()>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<()>>,
{
    if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
        private_fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    let staging = private_fs::tempdir().context("could not create backup staging directory")?;
    let database_path = staging.path().join(DATABASE_ENTRY);
    db::backup_database_with_connection(conn, &database_path).await?;
    after_snapshot().await?;

    let mut snapshot = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(&database_path)
            .read_only(true)
            .foreign_keys(true),
    )
    .await?;
    let rows: Vec<AvailableBlobRow> = sqlx::query_as(
        "SELECT sha256, byte_size, media_type FROM blob_inventory WHERE available = 1 ORDER BY sha256",
    )
    .fetch_all(&mut snapshot)
    .await?;
    let objects_dir = staging.path().join("objects").join("sha256");
    private_fs::create_dir_all(&objects_dir)
        .with_context(|| format!("could not create {}", objects_dir.display()))?;
    let mut objects = Vec::new();
    for row in rows {
        let source = object_path(blob_dir, &row.sha256)?;
        if !source.exists() {
            bail!("error backup-blob-missing");
        }
        let bytes = fs::read(&source).context("error backup-blob-read")?;
        validate_object_bytes(&row.sha256, row.byte_size, &row.media_type, &bytes).await?;
        private_fs::write_new_file(&objects_dir.join(&row.sha256), &bytes)
            .context("error backup-blob-stage")?;
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
    private_fs::write_new_file(
        &staging.path().join(MANIFEST_ENTRY),
        &serde_json::to_vec(&manifest).context("could not serialize backup manifest")?,
    )?;

    let tmp = private_fs::sibling_tempfile(output)
        .with_context(|| format!("could not create temporary file for {}", output.display()))?;
    let encoder = zstd::stream::write::Encoder::new(tmp.as_file(), 0)
        .context("could not create zstd encoder")?;
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
    tmp.as_file()
        .sync_all()
        .context("could not flush backup archive")?;
    tmp.persist(output)
        .with_context(|| format!("could not replace {}", output.display()))?;
    Ok(())
}

pub(super) async fn restore_backup_archive(
    db_path: &Path,
    blob_dir: &Path,
    archive: &Path,
) -> Result<PathBuf> {
    let _installation = db::installation::InstallationGuard::acquire(db_path)?;
    _installation.ensure_restore_target_unbound()?;
    let staging = private_fs::tempdir().context("could not create restore staging directory")?;
    let entries = extract_archive(archive, staging.path())?;
    let manifest_path = staging.path().join(MANIFEST_ENTRY);
    let manifest: BackupManifest = serde_json::from_slice(
        &fs::read(&manifest_path)
            .with_context(|| format!("could not read {}", manifest_path.display()))?,
    )
    .context("could not parse backup manifest")?;
    let manifest_by_hash = validate_manifest(&manifest)?;
    validate_archive_entries(&entries, &manifest)?;
    let database_path = staging.path().join(&manifest.database);
    validate_sqlite_file(&database_path).await?;
    db::detach_backup_snapshot(&database_path).await?;
    db::ensure_file_has_no_active_local_shared_capture(db_path, "restore-target").await?;
    let mut facts_by_hash = HashMap::with_capacity(manifest.objects.len());
    for object in &manifest.objects {
        let path = staging
            .path()
            .join("objects")
            .join("sha256")
            .join(&object.sha256);
        let bytes = fs::read(&path).context("error backup-blob-read")?;
        let facts =
            validate_object_bytes(&object.sha256, object.byte_size, &object.media_type, &bytes)
                .await?;
        facts_by_hash.insert(object.sha256.as_str(), facts);
    }
    validate_archive_attachment_metadata(&database_path, &manifest_by_hash, &facts_by_hash).await?;

    let safety = db::create_restore_safety_backup(db_path).await?;
    let sidecar_safety = db::default_sqlite_backup_path(db_path, "before-restore-blobs")?;
    if blob_dir.exists() {
        copy_dir(blob_dir, &sidecar_safety.with_extension("blobdir"))?;
    }

    let blob_parent = blob_dir
        .parent()
        .context("error backup-blob-directory-invalid")?;
    private_fs::create_dir_all(blob_parent)
        .context("could not create attachment restore directory")?;
    let replacement = private_fs::tempdir_in(blob_parent, ".aven-restore-blobs-")
        .context("could not create restore blob directory")?;
    let replacement_objects = replacement.path().join("objects").join("sha256");
    private_fs::create_dir_all(&replacement_objects)
        .context("could not create restore object directory")?;
    for object in &manifest.objects {
        let source = staging
            .path()
            .join("objects")
            .join("sha256")
            .join(&object.sha256);
        private_fs::copy_to_new_file(&source, &replacement_objects.join(&object.sha256))
            .context("could not stage restored attachment object")?;
    }

    let mut db_tmp = private_fs::sibling_tempfile(db_path)
        .with_context(|| format!("could not stage {}", db_path.display()))?;
    std::io::copy(&mut fs::File::open(&database_path)?, db_tmp.as_file_mut())
        .with_context(|| format!("could not copy {}", database_path.display()))?;
    db_tmp.as_file_mut().flush()?;
    db_tmp.as_file().sync_all()?;
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
    db_tmp
        .persist(db_path)
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

fn validate_manifest(manifest: &BackupManifest) -> Result<HashMap<&str, &BackupObjectManifest>> {
    if manifest.format != BACKUP_FORMAT || manifest.version != BACKUP_VERSION {
        bail!("error backup-format-unsupported");
    }
    if manifest.database != DATABASE_ENTRY {
        bail!(
            "error backup-manifest-invalid database={}",
            manifest.database
        );
    }
    let mut objects_by_hash = HashMap::with_capacity(manifest.objects.len());
    for object in &manifest.objects {
        crate::attachments::validation::validate_sha256(&object.sha256)?;
        crate::attachments::validation::validate_media_type(&object.media_type)?;
        crate::attachments::validation::validate_blob_size(
            usize::try_from(object.byte_size).unwrap_or(0),
        )?;
        if objects_by_hash
            .insert(object.sha256.as_str(), object)
            .is_some()
        {
            bail!(
                "error backup-manifest-duplicate-object sha256={}",
                object.sha256
            );
        }
    }
    Ok(objects_by_hash)
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
    manifest_by_hash: &HashMap<&str, &BackupObjectManifest>,
    facts_by_hash: &HashMap<&str, crate::attachments::decode::ImageFacts>,
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
        .map(|row| row.sha256.as_str())
        .collect::<HashSet<_>>();
    let manifested = manifest_by_hash.keys().copied().collect::<HashSet<_>>();
    if available != manifested {
        bail!("error backup-inventory-object-set-mismatch");
    }
    let mut inventory_by_hash = HashMap::with_capacity(inventory.len());
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
            let object = manifest_by_hash
                .get(row.sha256.as_str())
                .context("error backup-inventory-object-missing")?;
            if object.byte_size != row.byte_size || object.media_type != row.media_type {
                bail!("error backup-inventory-metadata-mismatch");
            }
        }
        if inventory_by_hash.insert(row.sha256.as_str(), row).is_some() {
            bail!(
                "error backup-inventory-duplicate-object sha256={}",
                row.sha256
            );
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
        let inventory_row = inventory_by_hash
            .get(attachment.sha256.as_str())
            .context("error backup-attachment-inventory-missing")?;
        if inventory_row.byte_size != attachment.byte_size
            || inventory_row.media_type != attachment.media_type
        {
            bail!("error backup-attachment-metadata-mismatch");
        }
        if inventory_row.available == 1 {
            let facts = facts_by_hash
                .get(attachment.sha256.as_str())
                .context("error backup-attachment-object-unvalidated")?;
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
            private_fs::create_dir_all(parent)
                .with_context(|| format!("could not create {}", parent.display()))?;
        }
        let mut file =
            private_fs::create_new_file(&target_path).context("could not unpack backup entry")?;
        std::io::copy(&mut entry, &mut file).context("could not unpack backup entry")?;
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
    if !fs::symlink_metadata(target).is_ok_and(|metadata| metadata.is_dir()) {
        private_fs::create_dir(target)
            .with_context(|| format!("could not create {}", target.display()))?;
    }
    for entry in
        fs::read_dir(source).with_context(|| format!("could not read {}", source.display()))?
    {
        let entry = entry?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&source_path, &target_path)?;
        } else {
            if fs::symlink_metadata(&target_path).is_ok() {
                fs::remove_file(&target_path)
                    .with_context(|| format!("could not replace {}", target_path.display()))?;
            }
            private_fs::copy_to_new_file(&source_path, &target_path).with_context(|| {
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

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use image::{DynamicImage, ImageFormat, RgbaImage};

    use crate::attachments::storage::upsert_inventory_available;

    use super::*;

    fn png_bytes() -> Vec<u8> {
        let image = DynamicImage::ImageRgba8(RgbaImage::new(1, 1));
        let mut bytes = Cursor::new(Vec::new());
        image.write_to(&mut bytes, ImageFormat::Png).unwrap();
        bytes.into_inner()
    }

    #[tokio::test]
    async fn setup_incomplete_archive_restores_as_local_only() {
        let temp = tempfile::tempdir().unwrap();
        let source_path = temp.path().join("source.sqlite");
        let source = db::Database::open(&source_path).await.unwrap();
        let mut conn = source.acquire_writer().await.unwrap();
        db::set_meta(&mut conn, "backup-test", "preserved")
            .await
            .unwrap();
        sqlx::query("INSERT INTO local_seed_genesis_pin VALUES (1, zeroblob(32))")
            .execute(&mut *conn)
            .await
            .unwrap();
        drop(conn);
        source
            .capture_local_shared_state_never_dispatched()
            .await
            .unwrap();
        let installation = db::installation::InstallationGuard::acquire(&source_path).unwrap();
        installation.fence().unwrap();
        drop(installation);
        let archive_path = temp.path().join("backup.aven-backup.tar.zst");
        source
            .create_backup_archive(&temp.path().join("source-blobs"), &archive_path)
            .await
            .unwrap();

        let target_path = temp.path().join("restored.sqlite");
        restore_backup_archive(
            &target_path,
            &temp.path().join("restored-blobs"),
            &archive_path,
        )
        .await
        .unwrap();
        let restored = db::Database::open(&target_path).await.unwrap();
        assert_eq!(
            restored.meta("backup-test").await.unwrap().as_deref(),
            Some("preserved")
        );
        assert!(
            restored
                .local_seed_genesis_commitment()
                .await
                .unwrap()
                .is_none()
        );
        assert!(restored.enrollment_pin().await.unwrap().is_none());
        assert_eq!(
            restored.meta("sync_generation").await.unwrap().as_deref(),
            Some("0")
        );
    }

    #[tokio::test]
    async fn live_inventory_mutation_after_snapshot_does_not_change_archive_inventory() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("live.sqlite");
        let pool = db::open_db(&db_path).await.unwrap();
        let mut backup_conn = pool.acquire().await.unwrap();
        let mut mutation_conn =
            SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(&db_path))
                .await
                .unwrap();
        let blob_dir = temp.path().join("blobs");
        let bytes = png_bytes();
        let hash = sha256_hex(&bytes);
        let object = object_path(&blob_dir, &hash).unwrap();
        fs::create_dir_all(object.parent().unwrap()).unwrap();
        fs::write(object, &bytes).unwrap();
        let archive_path = temp.path().join("backup.aven-backup.tar.zst");
        let mutation_hash = hash.clone();
        let byte_size = i64::try_from(bytes.len()).unwrap();

        create_backup_archive_with_snapshot_hook(
            &mut backup_conn,
            &blob_dir,
            &archive_path,
            || async move {
                upsert_inventory_available(
                    &mut mutation_conn,
                    &mutation_hash,
                    byte_size,
                    "image/png",
                )
                .await
            },
        )
        .await
        .unwrap();

        let extracted = tempfile::tempdir().unwrap();
        let entries = extract_archive(&archive_path, extracted.path()).unwrap();
        let manifest: BackupManifest =
            serde_json::from_slice(&fs::read(extracted.path().join(MANIFEST_ENTRY)).unwrap())
                .unwrap();
        assert!(manifest.objects.is_empty());
        assert_eq!(
            entries,
            HashSet::from([PathBuf::from(MANIFEST_ENTRY), PathBuf::from(DATABASE_ENTRY)])
        );
        let manifest_by_hash = validate_manifest(&manifest).unwrap();
        let facts_by_hash = HashMap::new();
        validate_archive_attachment_metadata(
            &extracted.path().join(DATABASE_ENTRY),
            &manifest_by_hash,
            &facts_by_hash,
        )
        .await
        .unwrap();

        let live_available: i64 =
            sqlx::query_scalar("SELECT available FROM blob_inventory WHERE sha256 = ?")
                .bind(hash)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(live_available, 1);
    }

    async fn source_with_image(root: &Path) -> (db::Database, PathBuf) {
        let db_path = root.join("source.sqlite");
        let source = db::Database::open(&db_path).await.unwrap();
        let blob_dir = root.join("source-blobs");
        let bytes = png_bytes();
        let hash = sha256_hex(&bytes);
        let object = object_path(&blob_dir, &hash).unwrap();
        fs::create_dir_all(object.parent().unwrap()).unwrap();
        fs::write(object, &bytes).unwrap();
        let mut conn = source.acquire_writer().await.unwrap();
        upsert_inventory_available(
            &mut conn,
            &hash,
            i64::try_from(bytes.len()).unwrap(),
            "image/png",
        )
        .await
        .unwrap();
        drop(conn);
        (source, blob_dir)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn archive_and_restore_create_owner_only_files_under_permissive_umask() {
        use crate::private_fs::test_umask::{Umask, mode};

        let _umask = Umask::set(0o022);
        let temp = tempfile::tempdir().unwrap();
        let (source, blob_dir) = source_with_image(temp.path()).await;
        for path in [
            temp.path().join("source.sqlite"),
            db::wal_path(&temp.path().join("source.sqlite")),
            db::shm_path(&temp.path().join("source.sqlite")),
            temp.path().join("source.sqlite.aven-installation.lock"),
        ] {
            if path.exists() {
                assert_eq!(mode(&path), 0o600, "{}", path.display());
            }
        }
        let archive_path = temp.path().join("out").join("backup.aven-backup.tar.zst");
        let mut conn = source.acquire_writer().await.unwrap();
        let staged_modes = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed = staged_modes.clone();
        create_backup_archive_with_snapshot_hook(
            &mut conn,
            &blob_dir,
            &archive_path,
            || async move {
                for entry in fs::read_dir(std::env::temp_dir()).unwrap().flatten() {
                    let name = entry.file_name();
                    if name.to_string_lossy().starts_with(".aven-") {
                        observed.lock().unwrap().push(mode(&entry.path()));
                    }
                }
                Ok(())
            },
        )
        .await
        .unwrap();
        drop(conn);
        {
            let staged_modes = staged_modes.lock().unwrap();
            assert!(!staged_modes.is_empty());
            assert!(staged_modes.iter().all(|mode| *mode == 0o700));
        }
        assert_eq!(mode(&archive_path), 0o600);
        assert_eq!(mode(archive_path.parent().unwrap()), 0o700);
        let archive = fs::File::open(&archive_path).unwrap();
        let mut entries = tar::Archive::new(zstd::stream::read::Decoder::new(archive).unwrap());
        for entry in entries.entries().unwrap() {
            assert_eq!(entry.unwrap().header().mode().unwrap() & 0o777, 0o600);
        }

        let target = temp.path().join("restored.sqlite");
        let restored_blobs = temp.path().join("restored-blobs");
        restore_backup_archive(&target, &restored_blobs, &archive_path)
            .await
            .unwrap();
        assert_eq!(mode(&target), 0o600);
        assert_eq!(mode(&restored_blobs), 0o700);
        assert_eq!(mode(&temp.path().join("backups")), 0o700);
        for entry in fs::read_dir(temp.path().join("backups")).unwrap().flatten() {
            let expected = if entry.file_type().unwrap().is_dir() {
                0o700
            } else {
                0o600
            };
            assert_eq!(mode(&entry.path()), expected, "{}", entry.path().display());
        }
        let restored = db::Database::open(&target).await.unwrap();
        drop(restored);
        for path in [db::wal_path(&target), db::shm_path(&target)] {
            if path.exists() {
                assert_eq!(mode(&path), 0o600, "{}", path.display());
            }
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn archive_backup_replaces_planted_symlinks_without_following_them() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let (source, blob_dir) = source_with_image(temp.path()).await;
        let victim = temp.path().join("victim");
        fs::write(&victim, b"sentinel").unwrap();
        let archive_path = temp.path().join("backup.aven-backup.tar.zst");
        symlink(&victim, archive_path.with_extension("tmp")).unwrap();
        symlink(&victim, &archive_path).unwrap();

        source
            .create_backup_archive(&blob_dir, &archive_path)
            .await
            .unwrap();

        assert_eq!(fs::read(&victim).unwrap(), b"sentinel");
        let metadata = fs::symlink_metadata(&archive_path).unwrap();
        assert!(metadata.is_file() && !metadata.file_type().is_symlink());
        assert!(is_archive_path(&archive_path).unwrap());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn archive_restore_ignores_planted_staging_symlink() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let (source, blob_dir) = source_with_image(temp.path()).await;
        let archive_path = temp.path().join("backup.aven-backup.tar.zst");
        source
            .create_backup_archive(&blob_dir, &archive_path)
            .await
            .unwrap();
        let victim = temp.path().join("victim");
        fs::write(&victim, b"sentinel").unwrap();
        let target = temp.path().join("restored.sqlite");
        symlink(&victim, target.with_extension("restore-staging")).unwrap();

        restore_backup_archive(&target, &temp.path().join("restored-blobs"), &archive_path)
            .await
            .unwrap();

        assert_eq!(fs::read(&victim).unwrap(), b"sentinel");
        let metadata = fs::symlink_metadata(&target).unwrap();
        assert!(metadata.is_file() && !metadata.file_type().is_symlink());
        validate_sqlite_file(&target).await.unwrap();
    }
}
