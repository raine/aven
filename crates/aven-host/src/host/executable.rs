//! Noticing that the running executable was replaced, so a host started from
//! it can stop and let its service manager start the new one.
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use anyhow::{Context, Result};

/// Short, because an update waits for a host to notice its replaced
/// executable and exit before the new version opens the database.
pub const CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fingerprint {
    /// Path the process was started through, resolved again on every check so
    /// a package manager repointing a symlink counts as a change.
    launch_path: PathBuf,
    pub path: PathBuf,
    len: u64,
    modified_ns: Option<u128>,
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
}

impl Fingerprint {
    /// The executable this process runs, as started.
    pub fn current() -> Result<Self> {
        let current = std::env::current_exe().context("resolve current executable")?;
        let launch_path = std::env::args_os()
            .next()
            .map(PathBuf::from)
            .filter(|argv0| argv0.is_absolute() && same_file(argv0, &current))
            .unwrap_or(current);
        Self::of(&launch_path)
    }

    pub fn of(launch_path: &Path) -> Result<Self> {
        let path = launch_path
            .canonicalize()
            .unwrap_or_else(|_| launch_path.to_path_buf());
        let metadata = std::fs::metadata(&path)
            .with_context(|| format!("read executable metadata {}", path.display()))?;
        let modified_ns = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos());
        Ok(Self {
            launch_path: launch_path.to_path_buf(),
            path,
            len: metadata.len(),
            modified_ns,
            #[cfg(unix)]
            dev: metadata.dev(),
            #[cfg(unix)]
            ino: metadata.ino(),
        })
    }

    pub fn changed(&self) -> Result<bool> {
        Ok(Self::of(&self.launch_path)? != *self)
    }
}

fn same_file(left: &Path, right: &Path) -> bool {
    matches!((left.canonicalize(), right.canonicalize()), (Ok(left), Ok(right)) if left == right)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn fingerprint_changes_when_file_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("aven");
        std::fs::write(&path, "one").unwrap();
        let initial = Fingerprint::of(&path).unwrap();
        std::thread::sleep(Duration::from_millis(5));
        std::fs::write(&path, "two-two").unwrap();
        assert!(initial.changed().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn fingerprint_changes_when_launch_symlink_is_repointed() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("0.1.0");
        let new = dir.path().join("0.2.0");
        std::fs::write(&old, "same").unwrap();
        std::fs::write(&new, "same").unwrap();
        let link = dir.path().join("aven");
        std::os::unix::fs::symlink(&old, &link).unwrap();
        let initial = Fingerprint::of(&link).unwrap();
        assert!(!initial.changed().unwrap());

        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&new, &link).unwrap();
        assert!(old.exists());
        assert!(initial.changed().unwrap());
    }
}
