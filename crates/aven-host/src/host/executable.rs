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
            .filter(|argv0| argv0.is_absolute() && crate::paths::same_file(argv0, &current))
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

    /// Whether the executable was replaced or removed.
    pub fn changed(&self) -> Result<bool> {
        match Self::of(&self.launch_path) {
            Ok(now) => Ok(now != *self),
            Err(_) if self.missing() => Ok(true),
            Err(error) => Err(error),
        }
    }

    /// Whether nothing exists at the path the process was started through,
    /// as when the app bundle was moved to the Trash.
    pub fn missing(&self) -> bool {
        matches!(self.launch_path.try_exists(), Ok(false))
    }

    /// Whether the executable now at the path can start without Gatekeeper
    /// stopping it to ask the user: it exists, and carries no quarantine or
    /// one the user approved.
    pub fn replacement_ready(&self) -> bool {
        !self.missing() && !quarantine_unapproved(&self.launch_path)
    }
}

/// How long a host that stopped for its executable waits for
/// [`Fingerprint::replacement_ready`] before it exits anyway. Homebrew moves
/// the new app into place unapproved and copies the old app's approval onto
/// it a moment later, and a service manager starts the new executable as
/// soon as the old process exits.
pub const REPLACEMENT_WAIT: std::time::Duration = std::time::Duration::from_secs(20);

/// Whether `path` carries a quarantine the user has not approved.
#[cfg(target_os = "macos")]
fn quarantine_unapproved(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;

    const USER_APPROVED: u32 = 0x40;
    let Ok(path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    let mut value = [0u8; 256];
    // SAFETY: both strings are NUL-terminated and `value` outlives the call,
    // which writes at most `value.len()` bytes.
    let len = unsafe {
        libc::getxattr(
            path.as_ptr(),
            c"com.apple.quarantine".as_ptr(),
            value.as_mut_ptr().cast(),
            value.len(),
            0,
            0,
        )
    };
    let Ok(len) = usize::try_from(len) else {
        return false;
    };
    quarantine_flags(&value[..len]).is_some_and(|flags| flags & USER_APPROVED == 0)
}

#[cfg(not(target_os = "macos"))]
fn quarantine_unapproved(_path: &Path) -> bool {
    false
}

/// The flags field of a `com.apple.quarantine` value such as
/// `0181;6aee0191;Homebrew Cask;…`.
#[cfg(any(target_os = "macos", test))]
fn quarantine_flags(value: &[u8]) -> Option<u32> {
    let value = std::str::from_utf8(value).ok()?;
    u32::from_str_radix(value.split(';').next()?, 16).ok()
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

    #[test]
    fn quarantine_flags_are_read_from_the_first_field() {
        assert_eq!(
            quarantine_flags(b"0181;6aee0191;Homebrew Cask;84D75741"),
            Some(0x181)
        );
        assert_eq!(quarantine_flags(b"01c1;6aee0191;;"), Some(0x1c1));
        assert_eq!(quarantine_flags(b"garbage"), None);
    }

    #[test]
    fn a_present_unquarantined_replacement_is_ready() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("aven");
        std::fs::write(&path, "one").unwrap();
        let initial = Fingerprint::of(&path).unwrap();
        assert!(initial.replacement_ready());
        std::fs::remove_file(&path).unwrap();
        assert!(!initial.replacement_ready());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn an_unapproved_quarantine_is_not_ready_until_approved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("aven");
        std::fs::write(&path, "one").unwrap();
        let initial = Fingerprint::of(&path).unwrap();
        let set = |value: &str| {
            let status = std::process::Command::new("/usr/bin/xattr")
                .args(["-w", "com.apple.quarantine", value])
                .arg(&path)
                .status()
                .unwrap();
            assert!(status.success());
        };
        set("0181;6aee0191;Homebrew Cask;");
        assert!(!initial.replacement_ready());
        set("01c1;6aee0191;Homebrew Cask;");
        assert!(initial.replacement_ready());
    }

    #[test]
    fn a_removed_executable_counts_as_changed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("aven");
        std::fs::write(&path, "one").unwrap();
        let initial = Fingerprint::of(&path).unwrap();
        assert!(!initial.missing());
        std::fs::remove_file(&path).unwrap();
        assert!(initial.changed().unwrap());
        assert!(initial.missing());
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
