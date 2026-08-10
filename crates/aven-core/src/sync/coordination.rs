use std::ffi::OsString;
use std::fmt;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::time::Instant;

use crate::db::Database;

const MANUAL_SYNC_LOCK_WAIT: Duration = Duration::from_secs(2);
const SYNC_LOCK_POLL_INTERVAL: Duration = Duration::from_millis(50);
const LOCK_SUFFIX: &str = ".aven-sync.lock";
const MAX_OWNER_BYTES: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyncLockPolicy {
    Manual,
    Defer,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncSessionBusy {
    owner_pid: Option<u32>,
}

impl SyncSessionBusy {
    pub const fn owner_pid(&self) -> Option<u32> {
        self.owner_pid
    }
}

impl fmt::Display for SyncSessionBusy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.owner_pid {
            Some(pid) => write!(formatter, "error sync-session-busy owner_pid={pid}"),
            None => formatter.write_str("error sync-session-busy"),
        }
    }
}

impl std::error::Error for SyncSessionBusy {}

pub(crate) struct SyncSessionLock {
    // Each acquisition opens an independent handle so same-process sessions contend.
    _file: File,
}

pub(crate) async fn acquire(
    database: &Database,
    policy: SyncLockPolicy,
) -> Result<Option<SyncSessionLock>> {
    let Some(database_path) = database.file_identity() else {
        return Ok(None);
    };
    let path = lock_path(database_path);
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .with_context(|| format!("open sync session lock {}", path.display()))?;
    let deadline = Instant::now() + MANUAL_SYNC_LOCK_WAIT;

    loop {
        match file.try_lock() {
            Ok(()) => {
                write_owner(&mut file)
                    .with_context(|| format!("write sync session lock {}", path.display()))?;
                return Ok(Some(SyncSessionLock { _file: file }));
            }
            Err(TryLockError::WouldBlock) => {
                if policy == SyncLockPolicy::Defer {
                    return Err(SyncSessionBusy {
                        owner_pid: read_owner(&mut file),
                    }
                    .into());
                }
                let now = Instant::now();
                if now >= deadline {
                    return Err(SyncSessionBusy {
                        owner_pid: read_owner(&mut file),
                    }
                    .into());
                }
                tokio::time::sleep(SYNC_LOCK_POLL_INTERVAL.min(deadline - now)).await;
            }
            Err(TryLockError::Error(error)) => {
                return Err(error)
                    .with_context(|| format!("acquire sync session lock {}", path.display()));
            }
        }
    }
}

fn lock_path(database_path: &Path) -> PathBuf {
    let mut filename = database_path
        .file_name()
        .map_or_else(OsString::new, OsString::from);
    filename.push(LOCK_SUFFIX);
    database_path.with_file_name(filename)
}

fn write_owner(file: &mut File) -> std::io::Result<()> {
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    writeln!(file, "{}", std::process::id())?;
    file.flush()
}

fn read_owner(file: &mut File) -> Option<u32> {
    file.seek(SeekFrom::Start(0)).ok()?;
    let mut bytes = [0_u8; MAX_OWNER_BYTES + 1];
    let count = file.read(&mut bytes).ok()?;
    if count > MAX_OWNER_BYTES {
        return None;
    }
    std::str::from_utf8(&bytes[..count])
        .ok()?
        .trim()
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn file_database_lock_reports_same_process_owner() {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(&directory.path().join("aven.sqlite"))
            .await
            .unwrap();
        let _first = acquire(&database, SyncLockPolicy::Defer).await.unwrap();

        let error = match acquire(&database, SyncLockPolicy::Defer).await {
            Ok(_) => panic!("second lock acquisition must contend"),
            Err(error) => error,
        };

        assert_eq!(
            error.downcast_ref::<SyncSessionBusy>().unwrap().owner_pid(),
            Some(std::process::id())
        );
    }

    #[tokio::test]
    async fn lock_releases_when_guard_drops() {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(&directory.path().join("aven.sqlite"))
            .await
            .unwrap();
        let first = acquire(&database, SyncLockPolicy::Defer).await.unwrap();
        drop(first);

        assert!(
            acquire(&database, SyncLockPolicy::Defer)
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn in_memory_databases_bypass_file_coordination() {
        let database = Database::open(Path::new(":memory:")).await.unwrap();

        assert!(
            acquire(&database, SyncLockPolicy::Defer)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            acquire(&database, SyncLockPolicy::Defer)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn lock_path_uses_database_filename() {
        assert_eq!(
            lock_path(Path::new("/tmp/aven.sqlite")),
            Path::new("/tmp/aven.sqlite.aven-sync.lock")
        );
    }
}
