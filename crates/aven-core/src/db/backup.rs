use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection as _, SqliteConnection, SqlitePool};

use super::MIGRATOR;

const MIGRATION_BACKUP_KEEP: usize = 20;

pub(super) async fn backup_before_pending_migrations(
    path: &Path,
    existed_before_open: bool,
    pool: &SqlitePool,
) -> Result<()> {
    if !migration_backups_enabled() || !existed_before_open || !has_pending_migrations(pool).await?
    {
        return Ok(());
    }
    let backup_path = migration_backup_path(path)?;
    let mut conn = pool.acquire().await?;
    backup_database_with_connection(&mut conn, &backup_path).await?;
    prune_migration_backups(path)?;
    Ok(())
}

fn migration_backups_enabled() -> bool {
    std::env::var_os("AVEN_DEV_MIGRATION_BACKUPS").is_some()
}

async fn has_pending_migrations(pool: &SqlitePool) -> Result<bool> {
    let applied_versions =
        match sqlx::query_scalar::<_, i64>("SELECT version FROM _sqlx_migrations")
            .fetch_all(pool)
            .await
        {
            Ok(versions) => versions,
            Err(error) => {
                let Some(db_error) = error.as_database_error() else {
                    return Err(error.into());
                };
                if db_error.code().as_deref() == Some("1") {
                    return Ok(MIGRATOR.iter().next().is_some());
                }
                return Err(error.into());
            }
        };
    Ok(MIGRATOR
        .iter()
        .any(|migration| !applied_versions.contains(&migration.version)))
}

fn migration_backup_path(path: &Path) -> Result<PathBuf> {
    default_sqlite_backup_path(path, "before-migrate")
}

pub fn default_backup_path(path: &Path, reason: &str) -> Result<PathBuf> {
    backup_path_with_extension(path, reason, "aven-backup.tar.zst")
}

pub fn default_sqlite_backup_path(path: &Path, reason: &str) -> Result<PathBuf> {
    backup_path_with_extension(path, reason, "sqlite")
}

fn backup_path_with_extension(path: &Path, reason: &str, extension: &str) -> Result<PathBuf> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let backup_dir = parent.join("backups");
    fs::create_dir_all(&backup_dir)
        .with_context(|| format!("could not create {}", backup_dir.display()))?;
    let stem = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("db.sqlite");
    Ok(backup_dir.join(format!(
        "{stem}.{reason}-{}.{}",
        backup_timestamp()?,
        extension
    )))
}

pub async fn backup_database(source: &Path, backup: &Path) -> Result<()> {
    if !source.is_file() {
        bail!("could not open source {}", source.display());
    }
    let _installation = super::installation::InstallationGuard::acquire_plaintext(source)?;
    backup_database_unlocked(source, backup).await
}

async fn backup_database_unlocked(source: &Path, backup: &Path) -> Result<()> {
    if !source.is_file() {
        bail!("could not open source {}", source.display());
    }
    if let Some(parent) = backup.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    let mut conn = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(source)
            .read_only(true)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5)),
    )
    .await
    .with_context(|| format!("could not open source {}", source.display()))?;
    backup_database_with_connection(&mut conn, backup).await
}

pub fn wal_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}-wal", path.display()))
}

pub fn shm_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}-shm", path.display()))
}

pub async fn restore_database_file(target: &Path, source: &Path) -> Result<PathBuf> {
    let _installation = super::installation::InstallationGuard::acquire(target)?;
    _installation.ensure_unbound()?;
    validate_sqlite_source(source).await?;
    ensure_file_has_no_active_local_shared_capture(source, "source").await?;
    ensure_file_has_no_active_local_shared_capture(target, "target").await?;
    let safety = create_restore_safety_backup(target).await?;
    let staging = target.with_extension("restore-staging");
    if staging.exists() {
        fs::remove_file(&staging)
            .with_context(|| format!("could not remove {}", staging.display()))?;
    }
    fs::copy(source, &staging).with_context(|| {
        format!(
            "could not copy {} -> {}",
            source.display(),
            staging.display()
        )
    })?;
    for sidecar in [wal_path(target), shm_path(target)] {
        if sidecar.exists() {
            fs::remove_file(&sidecar)
                .with_context(|| format!("could not remove {}", sidecar.display()))?;
        }
    }
    fs::rename(&staging, target)
        .with_context(|| format!("could not replace {}", target.display()))?;
    Ok(safety)
}

pub(crate) async fn create_restore_safety_backup(target: &Path) -> Result<PathBuf> {
    let safety = default_sqlite_backup_path(target, "before-restore")?;
    if target.exists() {
        backup_database_unlocked(target, &safety).await?;
    } else {
        SqliteConnection::connect_with(
            &SqliteConnectOptions::new()
                .filename(&safety)
                .create_if_missing(true),
        )
        .await
        .with_context(|| format!("could not create {}", safety.display()))?
        .close()
        .await?;
    }
    Ok(safety)
}

async fn validate_sqlite_source(source: &Path) -> Result<()> {
    let mut conn = sqlx::SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(source)
            .read_only(true)
            .foreign_keys(true),
    )
    .await
    .with_context(|| format!("could not open source {}", source.display()))?;
    let quick_check: String = sqlx::query_scalar("PRAGMA quick_check")
        .fetch_one(&mut conn)
        .await?;
    if quick_check != "ok" {
        bail!("error backup-source-corrupt quick_check={quick_check}");
    }
    Ok(())
}

fn backup_timestamp() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before unix epoch")?
        .as_secs())
}

pub(crate) async fn backup_database_with_connection(
    conn: &mut SqliteConnection,
    backup: &Path,
) -> Result<()> {
    ensure_connection_has_no_active_local_shared_capture(conn, "backup source").await?;
    wait_at_backup_precheck_boundary(backup).await;
    let parent = backup.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).with_context(|| format!("could not create {}", parent.display()))?;
    let staging_dir = tempfile::Builder::new()
        .prefix(".aven-sqlite-backup-")
        .tempdir_in(parent)
        .with_context(|| format!("could not create backup staging in {}", parent.display()))?;
    let staging = staging_dir.path().join("database.sqlite");
    sqlx::query("VACUUM INTO ?")
        .bind(staging.display().to_string())
        .execute(&mut *conn)
        .await
        .with_context(|| format!("could not back up database to {}", backup.display()))?;
    ensure_file_has_no_active_local_shared_capture(&staging, "backup-snapshot").await?;
    fs::rename(&staging, backup)
        .with_context(|| format!("could not replace {}", backup.display()))?;
    Ok(())
}

async fn ensure_connection_has_no_active_local_shared_capture(
    conn: &mut SqliteConnection,
    role: &str,
) -> Result<()> {
    let table_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(
             SELECT 1 FROM sqlite_master
             WHERE type = 'table' AND name = 'local_shared_capture_journal'
         )",
    )
    .fetch_one(&mut *conn)
    .await?;
    if !table_exists {
        return Ok(());
    }
    let source_table: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = 'local_seed_source')",
    )
    .fetch_one(&mut *conn)
    .await?;
    if source_table {
        crate::sync::shared_state::adoption::ensure_unbound(conn).await?;
    }
    let active: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM local_shared_capture_journal WHERE singleton = 1)",
    )
    .fetch_one(&mut *conn)
    .await?;
    if active {
        bail!(
            "error local-shared-capture-active role={} hint=cancel-never-dispatched-capture-first",
            role.replace(' ', "-")
        );
    }
    Ok(())
}

pub(crate) async fn ensure_file_has_no_active_local_shared_capture(
    path: &Path,
    role: &str,
) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let mut conn = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(path)
            .read_only(true)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5)),
    )
    .await
    .with_context(|| format!("could not open {} {}", role, path.display()))?;
    ensure_connection_has_no_active_local_shared_capture(&mut conn, role).await
}

#[cfg(test)]
type BackupPrecheckBarrier = (
    PathBuf,
    tokio::sync::oneshot::Sender<()>,
    tokio::sync::oneshot::Receiver<()>,
);

#[cfg(test)]
static BACKUP_PRECHECK_BARRIER: std::sync::Mutex<Option<BackupPrecheckBarrier>> =
    std::sync::Mutex::new(None);

#[cfg(test)]
async fn wait_at_backup_precheck_boundary(backup: &Path) {
    let barrier = {
        let mut guard = BACKUP_PRECHECK_BARRIER
            .lock()
            .expect("backup test barrier poisoned");
        if guard
            .as_ref()
            .is_some_and(|(expected, _, _)| expected == backup)
        {
            guard.take()
        } else {
            None
        }
    };
    if let Some((_, reached, resume)) = barrier {
        let _ = reached.send(());
        let _ = resume.await;
    }
}

#[cfg(not(test))]
async fn wait_at_backup_precheck_boundary(_backup: &Path) {}

fn prune_migration_backups(path: &Path) -> Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    let backup_dir = parent.join("backups");
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return Ok(());
    };
    let prefix = format!("{file_name}.before-migrate-");
    let mut backups = fs::read_dir(&backup_dir)
        .with_context(|| format!("could not read {}", backup_dir.display()))?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(".sqlite"))
        })
        .collect::<Vec<_>>();
    backups.sort_by_key(|entry| entry.file_name());
    let remove_count = backups.len().saturating_sub(MIGRATION_BACKUP_KEEP);
    for entry in backups.into_iter().take(remove_count) {
        let path = entry.path();
        fs::remove_file(&path).with_context(|| format!("could not remove {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::{Database, begin_immediate, get_meta, set_meta};
    use super::*;

    #[tokio::test]
    async fn in_process_backup_captures_wal_and_replaces_destination() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.sqlite");
        let backup = temp.path().join("backup.sqlite");
        let database = Database::open(&source).await.unwrap();
        let mut writer = database.acquire_writer().await.unwrap();
        let mut tx = begin_immediate(&mut writer).await.unwrap();
        set_meta(&mut tx, "backup-test", "first").await.unwrap();
        tx.commit().await.unwrap();
        drop(writer);
        assert!(wal_path(&source).exists());

        fs::write(&backup, b"existing destination").unwrap();
        backup_database(&source, &backup).await.unwrap();
        let mut backup_conn = SqliteConnection::connect_with(
            &SqliteConnectOptions::new()
                .filename(&backup)
                .read_only(true),
        )
        .await
        .unwrap();
        assert_eq!(
            get_meta(&mut backup_conn, "backup-test")
                .await
                .unwrap()
                .as_deref(),
            Some("first")
        );
        drop(backup_conn);

        let mut writer = database.acquire_writer().await.unwrap();
        set_meta(&mut writer, "backup-test", "second")
            .await
            .unwrap();
        drop(writer);
        backup_database(&source, &backup).await.unwrap();
        let mut backup_conn = SqliteConnection::connect_with(
            &SqliteConnectOptions::new()
                .filename(&backup)
                .read_only(true),
        )
        .await
        .unwrap();
        assert_eq!(
            get_meta(&mut backup_conn, "backup-test")
                .await
                .unwrap()
                .as_deref(),
            Some("second")
        );
    }

    #[tokio::test]
    async fn in_process_backup_rejects_missing_source() {
        let temp = tempfile::tempdir().unwrap();
        let backup = temp.path().join("backup.sqlite");
        let error = backup_database(&temp.path().join("missing.sqlite"), &backup)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("could not open source"));
        assert!(!backup.exists());
    }

    #[tokio::test]
    async fn restore_replaces_sidecars_and_preserves_safety_copy() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target.sqlite");
        let source = temp.path().join("source.sqlite");
        let target_database = Database::open(&target).await.unwrap();
        let source_database = Database::open(&source).await.unwrap();
        let mut target_writer = target_database.acquire_writer().await.unwrap();
        set_meta(&mut target_writer, "restore-test", "target")
            .await
            .unwrap();
        drop(target_writer);
        let mut source_writer = source_database.acquire_writer().await.unwrap();
        set_meta(&mut source_writer, "restore-test", "source")
            .await
            .unwrap();
        drop(source_writer);
        target_database.pool.close().await;
        source_database.pool.close().await;
        fs::write(wal_path(&target), b"stale wal").unwrap();
        fs::write(shm_path(&target), b"stale shm").unwrap();

        let safety = restore_database_file(&target, &source).await.unwrap();
        assert!(!wal_path(&target).exists());
        assert!(!shm_path(&target).exists());

        let mut restored = SqliteConnection::connect_with(
            &SqliteConnectOptions::new()
                .filename(&target)
                .read_only(true),
        )
        .await
        .unwrap();
        assert_eq!(
            get_meta(&mut restored, "restore-test")
                .await
                .unwrap()
                .as_deref(),
            Some("source")
        );
        let mut preserved = SqliteConnection::connect_with(
            &SqliteConnectOptions::new()
                .filename(&safety)
                .read_only(true),
        )
        .await
        .unwrap();
        assert_eq!(
            get_meta(&mut preserved, "restore-test")
                .await
                .unwrap()
                .as_deref(),
            Some("target")
        );
    }

    #[tokio::test]
    async fn completed_snapshot_rejects_capture_committed_after_backup_precheck() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.sqlite");
        let backup = temp.path().join("backup.sqlite");
        let database = Database::open(&source).await.unwrap();
        fs::write(&backup, b"existing destination").unwrap();
        let existing = fs::read(&backup).unwrap();
        let (reached_tx, reached_rx) = tokio::sync::oneshot::channel();
        let (resume_tx, resume_rx) = tokio::sync::oneshot::channel();
        *BACKUP_PRECHECK_BARRIER
            .lock()
            .expect("backup test barrier poisoned") = Some((backup.clone(), reached_tx, resume_rx));

        let source_for_backup = source.clone();
        let backup_for_worker = backup.clone();
        let worker =
            tokio::spawn(
                async move { backup_database(&source_for_backup, &backup_for_worker).await },
            );
        reached_rx.await.unwrap();
        database
            .capture_local_shared_state_never_dispatched()
            .await
            .unwrap();
        resume_tx.send(()).unwrap();

        let error = worker.await.unwrap().unwrap_err();
        assert!(error.to_string().contains("role=backup-snapshot"));
        assert_eq!(fs::read(&backup).unwrap(), existing);
        assert!(
            fs::read_dir(temp.path())
                .unwrap()
                .filter_map(|entry| entry.ok())
                .all(|entry| !entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".aven-sqlite-backup-"))
        );
    }
}
