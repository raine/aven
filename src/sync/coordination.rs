use std::ffi::OsString;
use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use aven_core::db::Database;
use tokio::time::Instant;

const MANUAL_SYNC_LOCK_WAIT: Duration = Duration::from_secs(2);
const SYNC_LOCK_POLL_INTERVAL: Duration = Duration::from_millis(50);
const LOCK_SUFFIX: &str = ".aven-sync.lock";

pub(super) struct SyncProcessGuard {
    _file: Option<File>,
}

pub(super) async fn acquire(database: &Database) -> Result<SyncProcessGuard> {
    let Some(file) = open_lock(database)? else {
        return Ok(SyncProcessGuard { _file: None });
    };
    let deadline = Instant::now() + MANUAL_SYNC_LOCK_WAIT;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(SyncProcessGuard { _file: Some(file) }),
            Err(TryLockError::WouldBlock) => {
                let now = Instant::now();
                if now >= deadline {
                    bail!("error sync-session-busy");
                }
                tokio::time::sleep(SYNC_LOCK_POLL_INTERVAL.min(deadline - now)).await;
            }
            Err(TryLockError::Error(error)) => return Err(error).context("acquire sync lock"),
        }
    }
}

pub(super) fn try_acquire(database: &Database) -> Result<Option<SyncProcessGuard>> {
    let Some(file) = open_lock(database)? else {
        return Ok(Some(SyncProcessGuard { _file: None }));
    };
    match file.try_lock() {
        Ok(()) => Ok(Some(SyncProcessGuard { _file: Some(file) })),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => Err(error).context("acquire sync lock"),
    }
}

fn open_lock(database: &Database) -> Result<Option<File>> {
    let Some(database_path) = database.file_identity() else {
        return Ok(None);
    };
    let path = lock_path(database_path);
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .with_context(|| format!("open sync lock {}", path.display()))
        .map(Some)
}

fn lock_path(database_path: &Path) -> PathBuf {
    let mut filename = database_path
        .file_name()
        .map_or_else(OsString::new, OsString::from);
    filename.push(LOCK_SUFFIX);
    database_path.with_file_name(filename)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn lock_contends_and_releases() {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(&directory.path().join("aven.sqlite"))
            .await
            .unwrap();
        let first = try_acquire(&database).unwrap().unwrap();
        assert!(try_acquire(&database).unwrap().is_none());
        drop(first);
        assert!(try_acquire(&database).unwrap().is_some());
    }
}
