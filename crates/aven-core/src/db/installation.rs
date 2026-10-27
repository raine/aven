//! Denial-only exclusion for explicitly selected E2EE installations.
//!
//! A marker survives SQLite replacement. It never authorizes initialization or
//! adoption. Locks serialize supported replacement with source preparation, not
//! arbitrary database users. Ordinary operations share the lock; source setup
//! and replacement acquire it exclusively. Protected key store operations also
//! acquire it exclusively, which keeps them out of backups and replacement and
//! keeps one another from blocking a thread on the store lock. They wait briefly
//! for another holder instead of failing, so overlapping sync rounds and
//! credential reads run in turn. All other acquisition is nonblocking.
//! Only a sync reset removes the marker, after the database's sync state and
//! protected keys are gone. Live raw replacement is unsupported.
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use tokio::time::Instant;

/// Longer than any key store operation that stays local, shorter than one that
/// waits on the network.
const KEY_STORE_WAIT: Duration = Duration::from_secs(10);
const KEY_STORE_POLL_INTERVAL: Duration = Duration::from_millis(20);
const BUSY: &str = "error installation-busy";

pub struct InstallationGuard {
    _lock: File,
    marker: PathBuf,
    identity: PathBuf,
    exclusive: bool,
}

impl Drop for InstallationGuard {
    fn drop(&mut self) {
        // Closing alone leaves the lock held by descriptors inherited during
        // concurrent process creation, even when they are close-on-exec.
        let _ = self._lock.unlock();
    }
}

impl InstallationGuard {
    pub fn acquire(path: &Path) -> Result<Self> {
        Self::acquire_mode(path, true)
    }

    /// Exclusive, waiting up to [`KEY_STORE_WAIT`] while another holder
    /// finishes before failing with `installation-busy`.
    pub(crate) async fn acquire_waiting(path: &Path) -> Result<Self> {
        Self::acquire_waiting_for(path, KEY_STORE_WAIT).await
    }

    async fn acquire_waiting_for(path: &Path, wait: Duration) -> Result<Self> {
        let deadline = Instant::now() + wait;
        loop {
            if let Some(guard) = Self::try_acquire_mode(path, true)? {
                return Ok(guard);
            }
            let now = Instant::now();
            ensure!(now < deadline, BUSY);
            tokio::time::sleep(KEY_STORE_POLL_INTERVAL.min(deadline - now)).await;
        }
    }

    /// Ordinary operations share exclusion against source setup and replacement.
    pub(crate) fn acquire_plaintext(path: &Path) -> Result<Self> {
        let guard = Self::acquire_mode(path, false)?;
        guard.ensure_unbound()?;
        Ok(guard)
    }

    /// Backups share exclusion but may read an encrypted-sync installation.
    pub(crate) fn acquire_for_backup(path: &Path) -> Result<Self> {
        Self::acquire_mode(path, false)
    }

    fn acquire_mode(path: &Path, exclusive: bool) -> Result<Self> {
        Self::try_acquire_mode(path, exclusive)?.context(BUSY)
    }

    /// The guard, or `None` while a conflicting holder has the lock.
    fn try_acquire_mode(path: &Path, exclusive: bool) -> Result<Option<Self>> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let name = path
            .file_name()
            .context("error installation-path-invalid")?;
        let identity = parent.canonicalize()?.join(name);
        if let Ok(metadata) = fs::symlink_metadata(&identity) {
            ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "error installation-path-alias-unsupported"
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                ensure!(
                    metadata.nlink() == 1,
                    "error installation-path-alias-unsupported"
                );
            }
        }
        let sidecar = |suffix: &str| {
            let mut name = name.to_os_string();
            name.push(suffix);
            identity.with_file_name(name)
        };
        let lock_path = sidecar(".aven-installation.lock");
        if let Ok(metadata) = fs::symlink_metadata(&lock_path) {
            ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "error installation-lock-unsafe"
            );
        }
        let lock = crate::private_fs::open_lock_file(&lock_path)
            .context("error installation-lock-unsafe")?;
        let locked = if exclusive {
            lock.try_lock()
        } else {
            lock.try_lock_shared()
        };
        match locked {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Ok(None),
            Err(TryLockError::Error(error)) => return Err(error).context(BUSY),
        }
        Ok(Some(Self {
            _lock: lock,
            marker: sidecar(".aven-e2ee-bound"),
            identity,
            exclusive,
        }))
    }

    pub fn identity(&self) -> &Path {
        &self.identity
    }

    pub fn ensure_unbound(&self) -> Result<()> {
        match fs::symlink_metadata(&self.marker) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
            Ok(_) => anyhow::bail!(
                "error e2ee-installation-fenced encrypted-tail-and-replacement-unavailable"
            ),
        }
    }

    pub(crate) fn ensure_restore_target_unbound(&self) -> Result<()> {
        self.ensure_replacement_target_unbound(
            "This database takes part in sync, so restore is refused here. Restore the backup to a new database path.",
        )
    }

    fn ensure_import_target_unbound(&self) -> Result<()> {
        self.ensure_replacement_target_unbound(
            "This database takes part in sync, so import is refused here. Import into a new database path.",
        )
    }

    fn ensure_replacement_target_unbound(&self, message: &str) -> Result<()> {
        match fs::symlink_metadata(&self.marker) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
            Ok(_) => anyhow::bail!(message.to_string()),
        }
    }

    /// Lifts the fence of a database that no longer takes part in sync.
    pub fn unfence(&self) -> Result<()> {
        ensure!(self.exclusive, "error installation-exclusive-lock-required");
        match fs::remove_file(&self.marker) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            result => result?,
        }
        File::open(self.marker.parent().context("missing marker parent")?)?.sync_all()?;
        Ok(())
    }

    /// Permanently refuses replacement even if later source setup fails.
    pub fn fence(&self) -> Result<()> {
        ensure!(self.exclusive, "error installation-exclusive-lock-required");
        if fs::symlink_metadata(&self.marker).is_ok() {
            return Ok(());
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&self.marker)?;
        file.write_all(b"aven-e2ee-denial-only-v1\n")?;
        file.sync_all()?;
        File::open(self.marker.parent().context("missing marker parent")?)?.sync_all()?;
        Ok(())
    }
}

impl super::Database {
    pub(crate) fn plaintext_installation_guard(&self) -> Result<Option<InstallationGuard>> {
        self.file_identity()
            .map(InstallationGuard::acquire_plaintext)
            .transpose()
    }

    pub(crate) fn backup_installation_guard(&self) -> Result<Option<InstallationGuard>> {
        self.file_identity()
            .map(InstallationGuard::acquire_for_backup)
            .transpose()
    }

    pub(crate) fn import_installation_guard(&self) -> Result<Option<InstallationGuard>> {
        let guard = self
            .file_identity()
            .map(InstallationGuard::acquire_for_backup)
            .transpose()?;
        if let Some(guard) = &guard {
            guard.ensure_import_target_unbound()?;
        }
        Ok(guard)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn drop_releases_lock_while_an_inherited_descriptor_remains_open() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("db.sqlite");
        for exclusive in [true, false] {
            let guard = InstallationGuard::acquire_mode(&path, exclusive).unwrap();
            // A duplicate shares the open file description, just like a child
            // process's inherited descriptor before close-on-exec runs.
            let inherited = guard._lock.try_clone().unwrap();
            assert!(InstallationGuard::acquire(&path).is_err());
            drop(guard);
            let next = InstallationGuard::acquire(&path).unwrap();
            assert!(InstallationGuard::acquire(&path).is_err());
            drop(inherited);
            assert!(InstallationGuard::acquire(&path).is_err());
            drop(next);
            assert!(InstallationGuard::acquire(&path).is_ok());
        }
    }

    #[tokio::test]
    async fn waiting_acquisition_runs_after_the_holder_releases() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("db.sqlite");
        for exclusive in [true, false] {
            let holder = InstallationGuard::acquire_mode(&path, exclusive).unwrap();
            let waiting = tokio::spawn({
                let path = path.clone();
                async move { InstallationGuard::acquire_waiting(&path).await }
            });
            tokio::time::sleep(KEY_STORE_POLL_INTERVAL * 3).await;
            assert!(!waiting.is_finished());
            drop(holder);
            let guard = waiting.await.unwrap().unwrap();
            // A waiter holds the lock exclusively, and nonblocking callers
            // still fail at once instead of queueing behind it.
            assert!(
                InstallationGuard::acquire_for_backup(&path)
                    .err()
                    .unwrap()
                    .to_string()
                    .contains("installation-busy")
            );
            assert!(InstallationGuard::acquire(&path).is_err());
            drop(guard);
        }
    }

    #[tokio::test]
    async fn waiting_acquisition_gives_up_after_its_bound() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("db.sqlite");
        let holder = InstallationGuard::acquire(&path).unwrap();
        let started = Instant::now();
        let error = InstallationGuard::acquire_waiting_for(&path, Duration::from_millis(100))
            .await
            .err()
            .unwrap();
        assert!(started.elapsed() >= Duration::from_millis(100));
        assert_eq!(error.to_string(), BUSY);
        drop(holder);
        assert!(
            InstallationGuard::acquire_waiting_for(&path, Duration::ZERO)
                .await
                .is_ok()
        );
    }

    #[test]
    fn missing_target_retains_same_denial_identity_and_aliases_fail() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("db.sqlite");
        let guard = InstallationGuard::acquire(&path).unwrap();
        let identity = guard.identity().to_path_buf();
        guard.fence().unwrap();
        drop(guard);
        fs::write(&path, b"fixture").unwrap();
        let guard = InstallationGuard::acquire(&path).unwrap();
        assert_eq!(guard.identity(), identity);
        assert!(guard.ensure_unbound().is_err());
        drop(guard);
        #[cfg(unix)]
        {
            let alias = root.path().join("alias.sqlite");
            std::os::unix::fs::symlink(&path, &alias).unwrap();
            assert!(InstallationGuard::acquire(&alias).is_err());
            fs::remove_file(&alias).unwrap();
            fs::hard_link(&path, &alias).unwrap();
            assert!(InstallationGuard::acquire(&alias).is_err());
            assert!(InstallationGuard::acquire(&path).is_err());
        }
    }
}

#[cfg(test)]
mod concurrency_tests {
    use super::*;
    use crate::db::Database;
    use std::future::Future;
    use std::task::{Context as TaskContext, Waker};

    #[tokio::test]
    async fn ordinary_operations_overlap_across_clones_and_independent_pools() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("db.sqlite");
        let database = Database::open(&path).await.unwrap();
        let independent = Database::open(&path).await.unwrap();
        let clone = database.clone();
        let source = Database::open(&root.path().join("source.sqlite"))
            .await
            .unwrap();
        let capture = source
            .capture_local_shared_state_never_dispatched()
            .await
            .unwrap();
        let capture = capture.shared_state();
        let writer = database.acquire_writer().await.unwrap();
        let mut page = Box::pin(database.install_shared_state(capture));
        let mut context = TaskContext::from_waker(Waker::noop());
        // Polling stops at the held writer gate, after installation acquisition.
        assert!(page.as_mut().poll(&mut context).is_pending());
        crate::db::backup_database(&path, &root.path().join("backup.sqlite"))
            .await
            .unwrap();
        independent
            .create_backup_archive(root.path(), &root.path().join("backup.tar.zst"))
            .await
            .unwrap();
        let nested = InstallationGuard::acquire_plaintext(&path).unwrap();
        assert!(nested.fence().is_err());
        drop(nested);
        let mut second = Box::pin(clone.install_shared_state(capture));
        assert!(second.as_mut().poll(&mut context).is_pending());
        // Both ordinary calls coexist, but exclusive setup/replacement cannot.
        assert!(InstallationGuard::acquire(&path).is_err());
        drop(writer);
        page.await.unwrap();
        assert!(InstallationGuard::acquire(&path).is_err());
        // Only installation exclusion is under test, not the target's contents.
        let _ = second.await;
        let exclusive = InstallationGuard::acquire(&path).unwrap();
        assert!(
            independent
                .install_shared_state(capture)
                .await
                .unwrap_err()
                .to_string()
                .contains("installation")
        );
        exclusive.fence().unwrap();
        drop(exclusive);
        crate::db::backup_database(&path, &root.path().join("fenced.sqlite"))
            .await
            .unwrap();
        independent
            .create_backup_archive(root.path(), &root.path().join("fenced.tar.zst"))
            .await
            .unwrap();
    }
}
