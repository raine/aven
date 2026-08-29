#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, SystemTime};

use anyhow::{Result, bail};
use chrono::{DateTime, SecondsFormat, Utc};
use sqlx::{Row, SqliteConnection};

use crate::attachments::storage::object_path;
use crate::attachments::validation::validate_sha256;
use crate::db::begin_immediate;
use crate::ids::new_id;

pub const DEFAULT_LOCAL_GRACE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
pub const DEFAULT_ORIGINAL_QUOTA_BYTES: i64 = 10 * 1024 * 1024 * 1024;
pub const DEFAULT_PREVIEW_QUOTA_BYTES: u64 = 512 * 1024 * 1024;
pub const DEFAULT_MAINTENANCE_LIMIT: usize = 128;
const LEASE_TTL: Duration = Duration::from_secs(10 * 60);

pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct LifecyclePolicy {
    pub grace: Duration,
    pub quota_bytes: i64,
    pub preview_quota_bytes: u64,
    pub maintenance_limit: usize,
}

impl Default for LifecyclePolicy {
    fn default() -> Self {
        Self {
            grace: DEFAULT_LOCAL_GRACE,
            quota_bytes: DEFAULT_ORIGINAL_QUOTA_BYTES,
            preview_quota_bytes: DEFAULT_PREVIEW_QUOTA_BYTES,
            maintenance_limit: DEFAULT_MAINTENANCE_LIMIT,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ByteCount {
    pub count: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LifecycleReport {
    pub referenced: ByteCount,
    pub protected: ByteCount,
    pub grace_period: ByteCount,
    pub eligible: ByteCount,
    pub staging: ByteCount,
    pub trash: ByteCount,
    pub reservations: ByteCount,
    pub quota: ByteCount,
    pub inconsistencies: ByteCount,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PruneSummary {
    pub eligible: ByteCount,
    pub pruned: ByteCount,
}

fn timestamp(now: DateTime<Utc>) -> String {
    now.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn cutoff(now: DateTime<Utc>, grace: Duration) -> Result<String> {
    let grace = chrono::Duration::from_std(grace)?;
    Ok(timestamp(now - grace))
}

fn trash_dir(blob_dir: &Path) -> PathBuf {
    blob_dir.join("trash")
}

fn staging_dir(blob_dir: &Path) -> PathBuf {
    blob_dir.join("objects").join("sha256")
}

struct ScannedFile {
    path: PathBuf,
    name: String,
    len: u64,
    modified: SystemTime,
}

#[derive(Default)]
struct DirectoryCursor {
    entries: Option<fs::ReadDir>,
}

static DIRECTORY_CURSORS: LazyLock<Mutex<HashMap<PathBuf, DirectoryCursor>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

async fn scan_directory_all(dir: PathBuf) -> Result<Vec<ScannedFile>> {
    crate::attachments::blocking::run(move || {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut files = Vec::new();
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let metadata = entry.metadata()?;
            files.push(ScannedFile {
                path: entry.path(),
                name: entry.file_name().to_string_lossy().into_owned(),
                len: metadata.len(),
                modified: metadata.modified()?,
            });
        }
        Ok(files)
    })
    .await
}

async fn scan_directory_page(dir: PathBuf, limit: usize) -> Result<Vec<ScannedFile>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    crate::attachments::blocking::run(move || {
        let key = fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
        let mut cursors = DIRECTORY_CURSORS
            .lock()
            .map_err(|_| anyhow::anyhow!("attachment directory cursor poisoned"))?;
        let result = {
            let cursor = cursors.entry(key.clone()).or_default();
            if cursor.entries.is_none() {
                cursor.entries = match fs::read_dir(&dir) {
                    Ok(entries) => Some(entries),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        return Ok(Vec::new());
                    }
                    Err(error) => return Err(error.into()),
                };
            }
            let mut files = Vec::new();
            for _ in 0..limit {
                let Some(entry) = cursor.entries.as_mut().and_then(Iterator::next) else {
                    cursor.entries = None;
                    break;
                };
                let entry = entry?;
                if !entry.file_type()?.is_file() {
                    continue;
                }
                let metadata = entry.metadata()?;
                files.push(ScannedFile {
                    path: entry.path(),
                    name: entry.file_name().to_string_lossy().into_owned(),
                    len: metadata.len(),
                    modified: metadata.modified()?,
                });
            }
            Ok(files)
        };
        if result.is_err() {
            cursors.remove(&key);
        }
        result
    })
    .await
}

async fn remove_files(files: Vec<PathBuf>) -> Result<()> {
    crate::attachments::blocking::run(move || {
        for path in files {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    })
    .await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileMove {
    Moved,
    SourceMissing,
    TargetExists,
}

fn move_file_without_replacing(source: &Path, target: &Path) -> Result<FileMove> {
    match fs::hard_link(source, target) {
        Ok(()) => {
            if let Err(error) = fs::remove_file(source) {
                let _ = fs::remove_file(target);
                return Err(error.into());
            }
            Ok(FileMove::Moved)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            Ok(FileMove::TargetExists)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if source.exists() {
                Err(error.into())
            } else {
                Ok(FileMove::SourceMissing)
            }
        }
        Err(error) => Err(error.into()),
    }
}

async fn restore_trashed_files(files: Vec<(PathBuf, PathBuf)>) {
    let _ = crate::attachments::blocking::run(move || {
        for (source, target) in files {
            if !target.exists() {
                continue;
            }
            if source.exists() {
                fs::remove_file(target)?;
            } else {
                let _ = move_file_without_replacing(&target, &source)?;
            }
        }
        Ok(())
    })
    .await;
}

fn live_blob_references_sql(sha256_expr: &str) -> String {
    format!(
        "EXISTS(
           SELECT 1 FROM task_attachments ta
           JOIN tasks t ON t.workspace_id = ta.workspace_id AND t.id = ta.task_id
           WHERE ta.sha256 = {sha256_expr} AND ta.deleted = 0 AND t.deleted = 0
         ) OR EXISTS(
           SELECT 1 FROM server_blob_references sbr
           LEFT JOIN server_task_tombstones st
             ON st.workspace_id = sbr.workspace_id AND st.task_id = sbr.task_id
           WHERE sbr.sha256 = {sha256_expr} AND sbr.deleted = 0
             AND COALESCE(st.deleted, 0) = 0
         )"
    )
}

pub async fn reconcile_liveness(conn: &mut SqliteConnection, clock: &dyn Clock) -> Result<()> {
    let mut tx = begin_immediate(conn).await?;
    reconcile_liveness_in_transaction(&mut tx, clock).await?;
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn reconcile_liveness_in_transaction(
    conn: &mut SqliteConnection,
    clock: &dyn Clock,
) -> Result<()> {
    let now = timestamp(clock.now());
    sqlx::query("DELETE FROM blob_leases WHERE expires_at <= ?")
        .bind(&now)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM blob_upload_reservations WHERE expires_at <= ?")
        .bind(&now)
        .execute(&mut *conn)
        .await?;
    sqlx::query(
        "INSERT OR IGNORE INTO blob_lifecycle(sha256, unreferenced_at)
         SELECT sha256, NULL FROM blob_inventory",
    )
    .execute(&mut *conn)
    .await?;
    let live_blob_references = live_blob_references_sql("blob_lifecycle.sha256");
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE blob_lifecycle SET unreferenced_at = NULL
         WHERE unreferenced_at IS NOT NULL
           AND ({live_blob_references})"
    )))
    .execute(&mut *conn)
    .await?;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE blob_lifecycle SET unreferenced_at = ?
         WHERE unreferenced_at IS NULL
           AND NOT ({live_blob_references})"
    )))
    .bind(&now)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

pub(crate) async fn reconcile_liveness_for_hashes_in_transaction(
    conn: &mut SqliteConnection,
    hashes: &[String],
    clock: &dyn Clock,
) -> Result<()> {
    if hashes.is_empty() {
        return Ok(());
    }
    let now = timestamp(clock.now());
    let hashes = serde_json::to_string(hashes)?;
    let live_inventory_references = live_blob_references_sql("bi.sha256");
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT OR IGNORE INTO blob_lifecycle(sha256, unreferenced_at)
         SELECT bi.sha256,
                CASE WHEN ({live_inventory_references}) THEN NULL ELSE ? END
         FROM blob_inventory bi
         JOIN (SELECT DISTINCT value AS sha256 FROM json_each(?)) affected
           ON affected.sha256 = bi.sha256"
    )))
    .bind(&now)
    .bind(&hashes)
    .execute(&mut *conn)
    .await?;

    let live_blob_references = live_blob_references_sql("blob_lifecycle.sha256");
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE blob_lifecycle SET unreferenced_at = NULL
         WHERE unreferenced_at IS NOT NULL
           AND sha256 IN (SELECT value FROM json_each(?))
           AND ({live_blob_references})"
    )))
    .bind(&hashes)
    .execute(&mut *conn)
    .await?;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE blob_lifecycle SET unreferenced_at = ?
         WHERE unreferenced_at IS NULL
           AND sha256 IN (SELECT value FROM json_each(?))
           AND NOT ({live_blob_references})"
    )))
    .bind(&now)
    .bind(&hashes)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

async fn is_protected(conn: &mut SqliteConnection, sha256: &str, now: &str) -> Result<bool> {
    let live_blob_references = live_blob_references_sql("?");
    Ok(sqlx::query_scalar::<_, bool>(sqlx::AssertSqlSafe(format!(
        "SELECT
           {live_blob_references} OR EXISTS(
             SELECT 1 FROM changes
             WHERE server_seq IS NULL AND op_type = 'attachment_add'
               AND json_extract(payload, '$.sha256') = ?
           ) OR EXISTS(
             SELECT 1 FROM blob_leases WHERE sha256 = ? AND expires_at > ?
           ) OR EXISTS(
             SELECT 1 FROM blob_upload_reservations WHERE sha256 = ? AND expires_at > ?
           )"
    )))
    .bind(sha256)
    .bind(sha256)
    .bind(sha256)
    .bind(sha256)
    .bind(now)
    .bind(sha256)
    .bind(now)
    .fetch_one(&mut *conn)
    .await?)
}

pub async fn acquire_lease(
    conn: &mut SqliteConnection,
    sha256: &str,
    kind: &str,
    clock: &dyn Clock,
) -> Result<String> {
    validate_sha256(sha256)?;
    if !matches!(kind, "staging" | "read" | "backup" | "transfer") {
        bail!("error attachment-lease-kind-invalid");
    }
    let lease_id = new_id();
    let now = clock.now();
    let expires = now + chrono::Duration::from_std(LEASE_TTL)?;
    sqlx::query(
        "INSERT INTO blob_leases(lease_id, sha256, kind, created_at, expires_at)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&lease_id)
    .bind(sha256)
    .bind(kind)
    .bind(timestamp(now))
    .bind(timestamp(expires))
    .execute(&mut *conn)
    .await?;
    Ok(lease_id)
}

pub async fn release_lease(conn: &mut SqliteConnection, lease_id: &str) -> Result<()> {
    sqlx::query("DELETE FROM blob_leases WHERE lease_id = ?")
        .bind(lease_id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

pub async fn reserve_upload(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    sha256: &str,
    byte_size: i64,
    quota_bytes: i64,
    clock: &dyn Clock,
) -> Result<Option<String>> {
    validate_sha256(sha256)?;
    let mut tx = begin_immediate(conn).await?;
    let existing: bool = sqlx::query_scalar(
        "SELECT EXISTS(
           SELECT 1 FROM server_blob_references sbr
           LEFT JOIN server_task_tombstones st
             ON st.workspace_id = sbr.workspace_id AND st.task_id = sbr.task_id
           WHERE sbr.workspace_id = ? AND sbr.sha256 = ? AND sbr.deleted = 0
             AND COALESCE(st.deleted, 0) = 0
         )",
    )
    .bind(workspace_id)
    .bind(sha256)
    .fetch_one(&mut *tx)
    .await?;
    if existing {
        tx.commit().await?;
        return Ok(None);
    }
    let used: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(byte_size), 0) FROM (
           SELECT sbr.sha256, MAX(sbr.byte_size) AS byte_size
           FROM server_blob_references sbr
           LEFT JOIN server_task_tombstones st
             ON st.workspace_id = sbr.workspace_id AND st.task_id = sbr.task_id
           WHERE sbr.workspace_id = ? AND sbr.deleted = 0 AND COALESCE(st.deleted, 0) = 0
           GROUP BY sbr.sha256
         )",
    )
    .bind(workspace_id)
    .fetch_one(&mut *tx)
    .await?;
    let reserved: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(byte_size), 0) FROM blob_upload_reservations
         WHERE workspace_id = ? AND sha256 != ? AND expires_at > ?",
    )
    .bind(workspace_id)
    .bind(sha256)
    .bind(timestamp(clock.now()))
    .fetch_one(&mut *tx)
    .await?;
    if used.saturating_add(reserved).saturating_add(byte_size) > quota_bytes {
        tx.rollback().await?;
        bail!("error attachment-quota-exceeded");
    }
    let reservation_id = new_id();
    let now = clock.now();
    let expires = now + chrono::Duration::from_std(LEASE_TTL)?;
    sqlx::query(
        "INSERT INTO blob_upload_reservations(
           reservation_id, workspace_id, sha256, byte_size, created_at, expires_at
         ) VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(workspace_id, sha256) DO UPDATE SET
           reservation_id = excluded.reservation_id,
           byte_size = excluded.byte_size,
           created_at = excluded.created_at,
           expires_at = excluded.expires_at",
    )
    .bind(&reservation_id)
    .bind(workspace_id)
    .bind(sha256)
    .bind(byte_size)
    .bind(timestamp(now))
    .bind(timestamp(expires))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(reservation_id))
}

pub async fn release_reservation(conn: &mut SqliteConnection, reservation_id: &str) -> Result<()> {
    sqlx::query("DELETE FROM blob_upload_reservations WHERE reservation_id = ?")
        .bind(reservation_id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

pub async fn local_unique_bytes(conn: &mut SqliteConnection) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT COALESCE(SUM(byte_size), 0) FROM blob_inventory WHERE available = 1",
    )
    .fetch_one(&mut *conn)
    .await?)
}

pub async fn ensure_local_capacity(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    sha256: &str,
    byte_size: i64,
    policy: LifecyclePolicy,
    clock: &dyn Clock,
) -> Result<Option<String>> {
    let used = local_unique_bytes(conn).await?;
    if used.saturating_add(byte_size) > policy.quota_bytes {
        prune(conn, blob_dir, policy, true, clock).await?;
    }
    let now = clock.now();
    let mut tx = begin_immediate(conn).await?;
    let existing: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM blob_inventory WHERE sha256 = ? AND available = 1)",
    )
    .bind(sha256)
    .fetch_one(&mut *tx)
    .await?;
    if existing {
        tx.commit().await?;
        return Ok(None);
    }
    let used: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(byte_size), 0) FROM blob_inventory WHERE available = 1",
    )
    .fetch_one(&mut *tx)
    .await?;
    let reserved: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(byte_size), 0) FROM blob_upload_reservations
         WHERE workspace_id = '__local__' AND sha256 != ? AND expires_at > ?",
    )
    .bind(sha256)
    .bind(timestamp(now))
    .fetch_one(&mut *tx)
    .await?;
    if used.saturating_add(reserved).saturating_add(byte_size) > policy.quota_bytes {
        tx.rollback().await?;
        bail!("error attachment-quota-exceeded");
    }
    let reservation_id = new_id();
    let expires = now + chrono::Duration::from_std(LEASE_TTL)?;
    sqlx::query(
        "INSERT INTO blob_upload_reservations(
           reservation_id, workspace_id, sha256, byte_size, created_at, expires_at
         ) VALUES (?, '__local__', ?, ?, ?, ?)
         ON CONFLICT(workspace_id, sha256) DO UPDATE SET
           reservation_id = excluded.reservation_id,
           byte_size = excluded.byte_size,
           created_at = excluded.created_at,
           expires_at = excluded.expires_at",
    )
    .bind(&reservation_id)
    .bind(sha256)
    .bind(byte_size)
    .bind(timestamp(now))
    .bind(timestamp(expires))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(reservation_id))
}

async fn reconcile_trash_files(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    files: Vec<ScannedFile>,
) -> Result<()> {
    let hashes = files
        .iter()
        .filter(|file| validate_sha256(&file.name).is_ok())
        .map(|file| file.name.clone())
        .collect::<Vec<_>>();
    let available = if hashes.is_empty() {
        HashSet::new()
    } else {
        sqlx::query_scalar::<_, String>(
            "SELECT sha256 FROM blob_inventory
             WHERE available = 1 AND sha256 IN (SELECT value FROM json_each(?))",
        )
        .bind(serde_json::to_string(&hashes)?)
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .collect()
    };
    let blob_dir = blob_dir.to_path_buf();
    crate::attachments::blocking::run(move || {
        for file in files {
            if validate_sha256(&file.name).is_err() {
                continue;
            }
            let target = object_path(&blob_dir, &file.name)?;
            if available.contains(&file.name) {
                if target.exists() {
                    fs::remove_file(file.path)?;
                } else {
                    if let Some(parent) = target.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    match move_file_without_replacing(&file.path, &target)? {
                        FileMove::Moved | FileMove::SourceMissing | FileMove::TargetExists => {}
                    }
                }
            } else {
                match fs::remove_file(file.path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }
        Ok(())
    })
    .await
}

pub async fn reconcile_trash(conn: &mut SqliteConnection, blob_dir: &Path) -> Result<()> {
    reconcile_trash_files(
        conn,
        blob_dir,
        scan_directory_all(trash_dir(blob_dir)).await?,
    )
    .await
}

async fn reconcile_trash_page(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    limit: usize,
) -> Result<()> {
    reconcile_trash_files(
        conn,
        blob_dir,
        scan_directory_page(trash_dir(blob_dir), limit).await?,
    )
    .await
}

pub async fn reconcile_staging(blob_dir: &Path) -> Result<ByteCount> {
    let files = scan_directory_all(staging_dir(blob_dir)).await?;
    let stale = files
        .into_iter()
        .filter(|file| {
            file.name.starts_with(".aven-stage-")
                && file.modified.elapsed().is_ok_and(|age| age >= LEASE_TTL)
        })
        .collect::<Vec<_>>();
    let removed = ByteCount {
        count: u64::try_from(stale.len())?,
        bytes: stale.iter().map(|file| file.len).sum(),
    };
    remove_files(stale.into_iter().map(|file| file.path).collect()).await?;
    Ok(removed)
}

pub async fn reconcile_missing_objects(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    clock: &dyn Clock,
) -> Result<ByteCount> {
    let rows = sqlx::query(
        "SELECT sha256, byte_size FROM blob_inventory
         WHERE available = 1 ORDER BY sha256",
    )
    .fetch_all(&mut *conn)
    .await?;
    let verified_at = timestamp(clock.now());
    let mut missing = ByteCount::default();
    for row in rows {
        let sha256: String = row.get("sha256");
        if object_path(blob_dir, &sha256)?.exists() {
            continue;
        }
        sqlx::query(
            "UPDATE blob_inventory SET available = 0, last_verified_at = ?
             WHERE sha256 = ? AND available = 1",
        )
        .bind(&verified_at)
        .bind(&sha256)
        .execute(&mut *conn)
        .await?;
        missing.count += 1;
        missing.bytes += u64::try_from(row.get::<i64, _>("byte_size"))?;
    }
    Ok(missing)
}

async fn reconcile_object_directory(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    grace: Duration,
    limit: usize,
    clock: &dyn Clock,
) -> Result<ByteCount> {
    let files = scan_directory_page(staging_dir(blob_dir), limit).await?;
    let stale_staging = files
        .iter()
        .filter(|file| {
            file.name.starts_with(".aven-stage-")
                && file.modified.elapsed().is_ok_and(|age| age >= LEASE_TTL)
        })
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    remove_files(stale_staging).await?;

    let cutoff = clock.now() - chrono::Duration::from_std(grace)?;
    let canonical = files
        .iter()
        .filter(|file| {
            validate_sha256(&file.name).is_ok() && DateTime::<Utc>::from(file.modified) <= cutoff
        })
        .collect::<Vec<_>>();
    if canonical.is_empty() {
        return Ok(ByteCount::default());
    }
    let hashes = canonical
        .iter()
        .map(|file| file.name.clone())
        .collect::<Vec<_>>();
    let candidates = serde_json::to_string(&hashes)?;
    let now = timestamp(clock.now());
    let live_blob_references = live_blob_references_sql("candidate.value");
    let protected = sqlx::query_scalar::<_, String>(sqlx::AssertSqlSafe(format!(
        "SELECT candidate.value FROM json_each(?) candidate
         WHERE EXISTS(SELECT 1 FROM blob_inventory bi WHERE bi.sha256 = candidate.value)
            OR {live_blob_references}
            OR EXISTS(
              SELECT 1 FROM changes
              WHERE server_seq IS NULL AND op_type = 'attachment_add'
                AND json_extract(payload, '$.sha256') = candidate.value
            )
            OR EXISTS(
              SELECT 1 FROM blob_leases
              WHERE sha256 = candidate.value AND expires_at > ?
            )
            OR EXISTS(
              SELECT 1 FROM blob_upload_reservations
              WHERE sha256 = candidate.value AND expires_at > ?
            )"
    )))
    .bind(candidates)
    .bind(&now)
    .bind(&now)
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .collect::<HashSet<_>>();
    let removable = canonical
        .into_iter()
        .filter(|file| !protected.contains(&file.name))
        .map(|file| (file.path.clone(), file.name.clone(), file.len))
        .collect::<Vec<_>>();
    if removable.is_empty() {
        return Ok(ByteCount::default());
    }
    let blob_dir = blob_dir.to_path_buf();
    crate::attachments::blocking::run(move || {
        let trash = trash_dir(&blob_dir);
        fs::create_dir_all(&trash)?;
        let mut removed = ByteCount::default();
        for (source, name, len) in removable {
            let target = trash.join(name);
            if move_file_without_replacing(&source, &target)? == FileMove::Moved {
                fs::remove_file(target)?;
                removed.count += 1;
                removed.bytes += len;
            }
        }
        Ok(removed)
    })
    .await
}

pub async fn reconcile_orphan_objects(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    grace: Duration,
    clock: &dyn Clock,
) -> Result<ByteCount> {
    reconcile_object_directory(conn, blob_dir, grace, usize::MAX, clock).await
}

pub async fn prune(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    policy: LifecyclePolicy,
    apply: bool,
    clock: &dyn Clock,
) -> Result<PruneSummary> {
    if apply {
        reconcile_trash_page(conn, blob_dir, policy.maintenance_limit).await?;
        reconcile_object_directory(
            conn,
            blob_dir,
            policy.grace,
            policy.maintenance_limit,
            clock,
        )
        .await?;
        reconcile_missing_objects(conn, blob_dir, clock).await?;
    }
    reconcile_liveness_bounded(conn, policy.maintenance_limit, clock).await?;
    let now = timestamp(clock.now());
    let cutoff = cutoff(clock.now(), policy.grace)?;
    let rows = sqlx::query(
        "SELECT bi.sha256, bi.byte_size
         FROM blob_inventory bi
         JOIN blob_lifecycle bl ON bl.sha256 = bi.sha256
         WHERE bi.available = 1 AND bl.unreferenced_at IS NOT NULL
           AND bl.unreferenced_at <= ?
         ORDER BY bl.unreferenced_at, bi.sha256 LIMIT ?",
    )
    .bind(&cutoff)
    .bind(i64::try_from(policy.maintenance_limit)?)
    .fetch_all(&mut *conn)
    .await?;
    let mut summary = PruneSummary::default();
    for row in rows {
        let sha256: String = row.get("sha256");
        let byte_size: i64 = row.get("byte_size");
        if is_protected(conn, &sha256, &now).await? {
            continue;
        }
        summary.eligible.count += 1;
        summary.eligible.bytes += u64::try_from(byte_size)?;
        if !apply {
            continue;
        }
        let mut tx = begin_immediate(conn).await?;
        let still_eligible: bool = sqlx::query_scalar(
            "SELECT EXISTS(
               SELECT 1 FROM blob_inventory bi
               JOIN blob_lifecycle bl ON bl.sha256 = bi.sha256
               WHERE bi.sha256 = ? AND bi.available = 1
                 AND bl.unreferenced_at IS NOT NULL AND bl.unreferenced_at <= ?
             )",
        )
        .bind(&sha256)
        .bind(&cutoff)
        .fetch_one(&mut *tx)
        .await?;
        if !still_eligible || is_protected(&mut tx, &sha256, &now).await? {
            tx.rollback().await?;
            continue;
        }
        let source = object_path(blob_dir, &sha256)?;
        let trash = trash_dir(blob_dir);
        let trashed = trash.join(&sha256);
        let source_for_move = source.clone();
        let trashed_for_move = trashed.clone();
        let moved = crate::attachments::blocking::run(move || {
            fs::create_dir_all(&trash)?;
            move_file_without_replacing(&source_for_move, &trashed_for_move)
        })
        .await?;
        if moved == FileMove::TargetExists {
            tx.rollback().await?;
            continue;
        }
        if let Err(error) = sqlx::query(
            "UPDATE blob_inventory SET available = 0, last_verified_at = ? WHERE sha256 = ?",
        )
        .bind(&now)
        .bind(&sha256)
        .execute(&mut *tx)
        .await
        {
            let _ = tx.rollback().await;
            if moved == FileMove::Moved {
                restore_trashed_files(vec![(source, trashed)]).await;
            }
            return Err(error.into());
        }
        if let Err(error) = tx.commit().await {
            if moved == FileMove::Moved {
                restore_trashed_files(vec![(source, trashed)]).await;
            }
            return Err(error.into());
        }
        if moved == FileMove::Moved {
            remove_files(vec![trashed]).await?;
        }
        summary.pruned.count += 1;
        summary.pruned.bytes += u64::try_from(byte_size)?;
    }
    if apply {
        let blob_dir = blob_dir.to_path_buf();
        crate::attachments::blocking::run(move || {
            prune_preview_cache(&blob_dir, policy.preview_quota_bytes)
        })
        .await?;
    }
    Ok(summary)
}

pub fn prune_preview_cache(blob_dir: &Path, quota: u64) -> Result<ByteCount> {
    let root = blob_dir.join("cache").join("previews");
    if !root.exists() {
        return Ok(ByteCount::default());
    }
    let mut files = Vec::new();
    let mut dirs = vec![root];
    while let Some(dir) = dirs.pop() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                dirs.push(entry.path());
            } else if entry.file_type()?.is_file() {
                let metadata = entry.metadata()?;
                files.push((metadata.modified()?, metadata.len(), entry.path()));
            }
        }
    }
    let mut total: u64 = files.iter().map(|(_, size, _)| *size).sum();
    files.sort_by_key(|(modified, _, path)| (*modified, path.clone()));
    let mut removed = ByteCount::default();
    for (_, size, path) in files {
        if total <= quota {
            break;
        }
        fs::remove_file(path)?;
        total -= size;
        removed.count += 1;
        removed.bytes += size;
    }
    Ok(removed)
}

pub async fn lifecycle_report(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    policy: LifecyclePolicy,
    clock: &dyn Clock,
) -> Result<LifecycleReport> {
    reconcile_liveness(conn, clock).await?;
    let now = timestamp(clock.now());
    let cutoff = cutoff(clock.now(), policy.grace)?;
    let live_blob_references = live_blob_references_sql("bi.sha256");
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT bi.sha256, bi.byte_size, bi.available, bl.unreferenced_at,
           {live_blob_references} AS referenced
         FROM blob_inventory bi LEFT JOIN blob_lifecycle bl ON bl.sha256 = bi.sha256"
    )))
    .fetch_all(&mut *conn)
    .await?;
    let mut report = LifecycleReport::default();
    for row in rows {
        let sha256: String = row.get("sha256");
        let bytes = u64::try_from(row.get::<i64, _>("byte_size"))?;
        let available = row.get::<i64, _>("available") != 0;
        let referenced = row.get::<i64, _>("referenced") != 0;
        let unreferenced_at: Option<String> = row.get("unreferenced_at");
        if available {
            report.quota.count += 1;
            report.quota.bytes += bytes;
        }
        if referenced {
            report.referenced.count += 1;
            report.referenced.bytes += bytes;
        } else if is_protected(conn, &sha256, &now).await? {
            report.protected.count += 1;
            report.protected.bytes += bytes;
        } else if unreferenced_at
            .as_deref()
            .is_some_and(|at| at <= cutoff.as_str())
        {
            report.eligible.count += 1;
            report.eligible.bytes += bytes;
        } else {
            report.grace_period.count += 1;
            report.grace_period.bytes += bytes;
        }
        if referenced && unreferenced_at.is_some() || !referenced && unreferenced_at.is_none() {
            report.inconsistencies.count += 1;
            report.inconsistencies.bytes += bytes;
        }
    }
    let (reservation_count, reservation_bytes): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(SUM(byte_size), 0)
         FROM blob_upload_reservations WHERE expires_at > ?",
    )
    .bind(&now)
    .fetch_one(&mut *conn)
    .await?;
    report.reservations = ByteCount {
        count: u64::try_from(reservation_count)?,
        bytes: u64::try_from(reservation_bytes)?,
    };
    for entry in fs::read_dir(staging_dir(blob_dir))
        .into_iter()
        .flatten()
        .flatten()
    {
        let name = entry.file_name().to_string_lossy().to_string();
        let metadata = entry.metadata()?;
        if name.starts_with(".aven-stage-") {
            report.staging.count += 1;
            report.staging.bytes += metadata.len();
        } else if validate_sha256(&name).is_ok() {
            let tracked: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM blob_inventory WHERE sha256 = ?)")
                    .bind(&name)
                    .fetch_one(&mut *conn)
                    .await?;
            if !tracked {
                report.staging.count += 1;
                report.staging.bytes += metadata.len();
                report.inconsistencies.count += 1;
                report.inconsistencies.bytes += metadata.len();
            }
        }
    }
    for entry in fs::read_dir(trash_dir(blob_dir))
        .into_iter()
        .flatten()
        .flatten()
    {
        if entry.file_type()?.is_file() {
            report.trash.count += 1;
            report.trash.bytes += entry.metadata()?.len();
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use chrono::DateTime;
    use sqlx::Connection as _;
    use sqlx::sqlite::SqliteConnectOptions;

    use crate::attachments::storage::{object_path, upsert_inventory_available};
    use crate::db::open_db;

    use super::*;

    #[derive(Clone)]
    struct TestClock(Arc<Mutex<DateTime<Utc>>>);

    impl TestClock {
        fn at(value: &str) -> Self {
            Self(Arc::new(Mutex::new(
                DateTime::parse_from_rfc3339(value).unwrap().to_utc(),
            )))
        }

        fn advance(&self, duration: chrono::Duration) {
            let mut now = self.0.lock().unwrap();
            *now += duration;
        }
    }

    impl Clock for TestClock {
        fn now(&self) -> DateTime<Utc> {
            *self.0.lock().unwrap()
        }
    }

    async fn insert_task(conn: &mut SqliteConnection, task_id: &str) {
        sqlx::query(
            "INSERT INTO tasks(
               workspace_id, id, title, description, project_id, status, priority,
               created_at, updated_at, queue_activity_at
             ) VALUES ('0000000000000000', ?, 'task', '', 'project', 'inbox', 'none',
                       '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        )
        .bind(task_id)
        .execute(conn)
        .await
        .unwrap();
    }

    async fn insert_attachment(
        conn: &mut SqliteConnection,
        attachment_id: &str,
        task_id: &str,
        sha256: &str,
        deleted: bool,
    ) {
        sqlx::query(
            "INSERT INTO task_attachments(
               workspace_id, attachment_id, task_id, sha256, byte_size, media_type, width, height,
               created_at, deleted, deleted_at
             ) VALUES ('0000000000000000', ?, ?, ?, 4, 'image/png', 1, 1,
                       '2026-01-01T00:00:00Z', ?, ?)",
        )
        .bind(attachment_id)
        .bind(task_id)
        .bind(sha256)
        .bind(i64::from(deleted))
        .bind(deleted.then_some("2026-01-01T00:00:00Z"))
        .execute(conn)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn final_live_reference_starts_grace_once_and_restore_clears_it() {
        let temp = tempfile::tempdir().unwrap();
        let pool = open_db(&temp.path().join("test.sqlite")).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let hash = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        upsert_inventory_available(&mut conn, hash, 4, "image/png")
            .await
            .unwrap();
        insert_task(&mut conn, "0000000000000001").await;
        insert_task(&mut conn, "0000000000000002").await;
        insert_attachment(
            &mut conn,
            "0000000000000011",
            "0000000000000001",
            hash,
            false,
        )
        .await;
        insert_attachment(
            &mut conn,
            "0000000000000012",
            "0000000000000002",
            hash,
            false,
        )
        .await;
        let clock = TestClock::at("2026-07-01T00:00:00Z");

        reconcile_liveness(&mut conn, &clock).await.unwrap();
        sqlx::query("UPDATE task_attachments SET deleted = 1, deleted_at = 'x' WHERE attachment_id = '0000000000000011'")
            .execute(&mut *conn).await.unwrap();
        reconcile_liveness(&mut conn, &clock).await.unwrap();
        let value: Option<String> =
            sqlx::query_scalar("SELECT unreferenced_at FROM blob_lifecycle WHERE sha256 = ?")
                .bind(hash)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(value, None, "one live reference keeps the hash live");

        sqlx::query("UPDATE task_attachments SET deleted = 1, deleted_at = 'x' WHERE attachment_id = '0000000000000012'")
            .execute(&mut *conn).await.unwrap();
        reconcile_liveness(&mut conn, &clock).await.unwrap();
        let first: String =
            sqlx::query_scalar("SELECT unreferenced_at FROM blob_lifecycle WHERE sha256 = ?")
                .bind(hash)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        clock.advance(chrono::Duration::days(1));
        reconcile_liveness(&mut conn, &clock).await.unwrap();
        let second: String =
            sqlx::query_scalar("SELECT unreferenced_at FROM blob_lifecycle WHERE sha256 = ?")
                .bind(hash)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(first, second, "grace starts exactly once");

        sqlx::query("UPDATE task_attachments SET deleted = 0, deleted_at = NULL WHERE attachment_id = '0000000000000012'")
            .execute(&mut *conn).await.unwrap();
        reconcile_liveness(&mut conn, &clock).await.unwrap();
        let restored: Option<String> =
            sqlx::query_scalar("SELECT unreferenced_at FROM blob_lifecycle WHERE sha256 = ?")
                .bind(hash)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(restored, None);
    }

    #[tokio::test]
    async fn affected_liveness_reconciliation_is_scoped_and_write_minimal() {
        let temp = tempfile::tempdir().unwrap();
        let pool = open_db(&temp.path().join("test.sqlite")).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let affected =
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string();
        let unrelated =
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string();
        let clock = TestClock::at("2026-07-01T00:00:00Z");
        for hash in [&affected, &unrelated] {
            upsert_inventory_available(&mut conn, hash, 4, "image/png")
                .await
                .unwrap();
        }
        insert_task(&mut conn, "0000000000000001").await;
        insert_attachment(
            &mut conn,
            "0000000000000011",
            "0000000000000001",
            &affected,
            false,
        )
        .await;
        for hash in [&affected, &unrelated] {
            sqlx::query("INSERT INTO blob_lifecycle(sha256, unreferenced_at) VALUES (?, ?)")
                .bind(hash)
                .bind("2026-06-01T00:00:00Z")
                .execute(&mut *conn)
                .await
                .unwrap();
        }
        sqlx::query(
            "CREATE TABLE lifecycle_updates(count INTEGER NOT NULL DEFAULT 0);
             CREATE TRIGGER count_lifecycle_updates
             AFTER UPDATE OF unreferenced_at ON blob_lifecycle
             BEGIN
                 INSERT INTO lifecycle_updates(count) VALUES (1);
             END",
        )
        .execute(&mut *conn)
        .await
        .unwrap();

        reconcile_liveness_for_hashes_in_transaction(&mut conn, &[affected.clone()], &clock)
            .await
            .unwrap();
        let unrelated_at: String =
            sqlx::query_scalar("SELECT unreferenced_at FROM blob_lifecycle WHERE sha256 = ?")
                .bind(&unrelated)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(unrelated_at, "2026-06-01T00:00:00Z");
        let updates: i64 = sqlx::query_scalar("SELECT count(*) FROM lifecycle_updates")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(updates, 1, "the live affected row changes once");

        reconcile_liveness_for_hashes_in_transaction(&mut conn, &[affected.clone()], &clock)
            .await
            .unwrap();
        let repeated_updates: i64 = sqlx::query_scalar("SELECT count(*) FROM lifecycle_updates")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(repeated_updates, 1, "an already-live row is not rewritten");

        sqlx::query(
            "UPDATE task_attachments SET deleted = 1, deleted_at = 'x'
             WHERE attachment_id = '0000000000000011'",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
        reconcile_liveness_for_hashes_in_transaction(&mut conn, &[affected.clone()], &clock)
            .await
            .unwrap();
        let first_unreferenced: String =
            sqlx::query_scalar("SELECT unreferenced_at FROM blob_lifecycle WHERE sha256 = ?")
                .bind(&affected)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(first_unreferenced, "2026-07-01T00:00:00Z");
        clock.advance(chrono::Duration::days(1));
        reconcile_liveness_for_hashes_in_transaction(&mut conn, &[affected.clone()], &clock)
            .await
            .unwrap();
        let second_unreferenced: String =
            sqlx::query_scalar("SELECT unreferenced_at FROM blob_lifecycle WHERE sha256 = ?")
                .bind(&affected)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(first_unreferenced, second_unreferenced);

        sqlx::query(
            "UPDATE task_attachments SET deleted = 0, deleted_at = NULL
             WHERE attachment_id = '0000000000000011'",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
        reconcile_liveness(&mut conn, &clock).await.unwrap();
        let restored: Option<String> =
            sqlx::query_scalar("SELECT unreferenced_at FROM blob_lifecycle WHERE sha256 = ?")
                .bind(&affected)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(restored, None);
        let full_updates: i64 = sqlx::query_scalar("SELECT count(*) FROM lifecycle_updates")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(full_updates, 3, "the full reset writes only the transition");
    }

    #[tokio::test]
    async fn lease_protects_expired_unreferenced_blob_until_release() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("test.sqlite");
        let pool = open_db(&db_path).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let blob_dir = temp.path().join("blobs");
        let hash = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        upsert_inventory_available(&mut conn, hash, 4, "image/png")
            .await
            .unwrap();
        let path = object_path(&blob_dir, hash).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"blob").unwrap();
        let clock = TestClock::at("2026-07-10T00:00:00Z");
        reconcile_liveness(&mut conn, &clock).await.unwrap();
        clock.advance(chrono::Duration::days(8));
        let lease = acquire_lease(&mut conn, hash, "backup", &clock)
            .await
            .unwrap();
        let policy = LifecyclePolicy::default();
        let blocked = prune(&mut conn, &blob_dir, policy, true, &clock)
            .await
            .unwrap();
        assert_eq!(blocked.pruned.count, 0);
        assert!(path.exists());

        release_lease(&mut conn, &lease).await.unwrap();
        let pruned = prune(&mut conn, &blob_dir, policy, true, &clock)
            .await
            .unwrap();
        assert_eq!(pruned.pruned.count, 1);
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn quota_is_unique_by_hash_and_reservations_are_workspace_scoped() {
        let temp = tempfile::tempdir().unwrap();
        let pool = open_db(&temp.path().join("test.sqlite")).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let hash = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
        let clock = TestClock::at("2026-07-01T00:00:00Z");
        let first = reserve_upload(&mut conn, "workspace-a", hash, 8, 8, &clock)
            .await
            .unwrap();
        assert!(first.is_some());
        let replacement = reserve_upload(&mut conn, "workspace-a", hash, 8, 8, &clock)
            .await
            .unwrap();
        assert!(replacement.is_some());
        let other = reserve_upload(&mut conn, "workspace-b", hash, 8, 8, &clock)
            .await
            .unwrap();
        assert!(other.is_some());
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM blob_upload_reservations")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn concurrent_attach_wins_prune_recheck() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("test.sqlite");
        let pool = open_db(&db_path).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let blob_dir = temp.path().join("blobs");
        let hash = "abababababababababababababababababababababababababababababababab";
        upsert_inventory_available(&mut conn, hash, 4, "image/png")
            .await
            .unwrap();
        insert_task(&mut conn, "0000000000000003").await;
        let path = object_path(&blob_dir, hash).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"blob").unwrap();
        let clock = TestClock::at("2026-07-01T00:00:00Z");
        reconcile_liveness(&mut conn, &clock).await.unwrap();
        clock.advance(chrono::Duration::days(8));
        let options = SqliteConnectOptions::new()
            .filename(&db_path)
            .busy_timeout(Duration::from_secs(5));
        let prune_conn = SqliteConnection::connect_with(&options).await.unwrap();

        let mut tx = begin_immediate(&mut conn).await.unwrap();
        insert_attachment(&mut tx, "0000000000000013", "0000000000000003", hash, false).await;
        let prune_dir = blob_dir.clone();
        let prune_clock = clock.clone();
        let pruning = tokio::spawn(async move {
            let mut prune_conn = prune_conn;
            prune(
                &mut prune_conn,
                &prune_dir,
                LifecyclePolicy::default(),
                true,
                &prune_clock,
            )
            .await
            .unwrap()
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        tx.commit().await.unwrap();

        let summary = pruning.await.unwrap();
        assert_eq!(summary.pruned.count, 0);
        assert!(path.exists());
    }

    #[tokio::test]
    async fn accepted_server_reference_keeps_blob_live() {
        let temp = tempfile::tempdir().unwrap();
        let pool = open_db(&temp.path().join("test.sqlite")).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let hash = "acacacacacacacacacacacacacacacacacacacacacacacacacacacacacacacac";
        upsert_inventory_available(&mut conn, hash, 4, "image/png")
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO server_blob_references(
               workspace_id, attachment_id, task_id, sha256, byte_size
             ) VALUES ('workspace', '0000000000000042', '0000000000000004', ?, 4)",
        )
        .bind(hash)
        .execute(&mut *conn)
        .await
        .unwrap();
        let clock = TestClock::at("2026-07-01T00:00:00Z");

        reconcile_liveness(&mut conn, &clock).await.unwrap();
        let unreferenced_at: Option<String> =
            sqlx::query_scalar("SELECT unreferenced_at FROM blob_lifecycle WHERE sha256 = ?")
                .bind(hash)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(unreferenced_at, None);
    }

    #[tokio::test]
    async fn local_quota_boundary_is_hash_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("test.sqlite");
        let pool = open_db(&db_path).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let blob_dir = temp.path().join("blobs");
        let existing = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
        let new_hash = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
        upsert_inventory_available(&mut conn, existing, 8, "image/png")
            .await
            .unwrap();
        let clock = TestClock::at("2026-07-01T00:00:00Z");
        let policy = LifecyclePolicy {
            quota_bytes: 8,
            ..LifecyclePolicy::default()
        };

        ensure_local_capacity(&mut conn, &blob_dir, existing, 8, policy, &clock)
            .await
            .unwrap();
        let error = ensure_local_capacity(&mut conn, &blob_dir, new_hash, 1, policy, &clock)
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "error attachment-quota-exceeded");
    }

    #[tokio::test]
    async fn interrupted_atomic_create_object_is_reconciled_after_grace() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("test.sqlite");
        let pool = open_db(&db_path).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let blob_dir = temp.path().join("blobs");
        let hash = "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";
        let path = object_path(&blob_dir, hash).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"orphan").unwrap();
        let clock = TestClock::at("2030-07-01T00:00:00Z");
        let policy = LifecyclePolicy {
            grace: Duration::ZERO,
            ..LifecyclePolicy::default()
        };

        prune(&mut conn, &blob_dir, policy, true, &clock)
            .await
            .unwrap();
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn orphan_traversal_obeys_limit_and_resumes_between_steps() {
        let temp = tempfile::tempdir().unwrap();
        let pool = open_db(&temp.path().join("test.sqlite")).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let blob_dir = temp.path().join("blobs");
        for digit in ['6', '7', '8', '9', 'a'] {
            let hash = digit.to_string().repeat(64);
            let path = object_path(&blob_dir, &hash).unwrap();
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"orphan").unwrap();
        }
        let clock = TestClock::at("2030-07-01T00:00:00Z");
        let policy = LifecyclePolicy {
            grace: Duration::ZERO,
            maintenance_limit: 2,
            ..LifecyclePolicy::default()
        };

        prune(&mut conn, &blob_dir, policy, true, &clock)
            .await
            .unwrap();
        let after_one = fs::read_dir(staging_dir(&blob_dir)).unwrap().count();
        assert!(after_one >= 3);

        for _ in 0..4 {
            prune(&mut conn, &blob_dir, policy, true, &clock)
                .await
                .unwrap();
        }
        assert_eq!(fs::read_dir(staging_dir(&blob_dir)).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn interrupted_trash_move_restores_available_object() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("test.sqlite");
        let pool = open_db(&db_path).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let blob_dir = temp.path().join("blobs");
        let hash = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
        upsert_inventory_available(&mut conn, hash, 4, "image/png")
            .await
            .unwrap();
        let source = object_path(&blob_dir, hash).unwrap();
        let trash = trash_dir(&blob_dir).join(hash);
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::create_dir_all(trash.parent().unwrap()).unwrap();
        fs::write(&source, b"blob").unwrap();
        fs::rename(&source, &trash).unwrap();

        reconcile_trash(&mut conn, &blob_dir).await.unwrap();
        assert!(source.exists());
        assert!(!trash.exists());
    }
}
