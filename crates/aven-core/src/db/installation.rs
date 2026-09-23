//! Denial-only exclusion for explicitly selected E2EE installations.
//!
//! A marker survives SQLite replacement. It never authorizes initialization or
//! adoption. Locks serialize supported replacement with source preparation, not
//! arbitrary database users. Marker removal and live raw replacement are unsupported.
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};

pub struct InstallationGuard {
    _lock: File,
    marker: PathBuf,
    identity: PathBuf,
}

impl InstallationGuard {
    pub fn acquire(path: &Path) -> Result<Self> {
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
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)?;
        lock.try_lock().context("error installation-busy")?;
        Ok(Self {
            _lock: lock,
            marker: sidecar(".aven-e2ee-bound"),
            identity,
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

    /// Permanently refuses replacement even if later source setup fails.
    pub fn fence(&self) -> Result<()> {
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
            .map(|path| {
                let guard = InstallationGuard::acquire(path)?;
                guard.ensure_unbound()?;
                Ok(guard)
            })
            .transpose()
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
