//! Denial-only exclusion for explicitly selected E2EE installations.
//!
//! A marker survives SQLite replacement. It never authorizes initialization or
//! adoption. Locks serialize supported replacement with source preparation, not
//! arbitrary database users. Ordinary operations share the lock; source setup
//! and replacement acquire it exclusively. All acquisition is nonblocking.
//! Marker removal and live raw replacement are unsupported.
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};

pub struct InstallationGuard {
    _lock: File,
    marker: PathBuf,
    identity: PathBuf,
    exclusive: bool,
}

impl InstallationGuard {
    pub fn acquire(path: &Path) -> Result<Self> {
        Self::acquire_mode(path, true)
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
        if exclusive {
            lock.try_lock()
        } else {
            lock.try_lock_shared()
        }
        .context("error installation-busy")?;
        Ok(Self {
            _lock: lock,
            marker: sidecar(".aven-e2ee-bound"),
            identity,
            exclusive,
        })
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
