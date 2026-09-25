//! Owner-only creation of files and directories that hold local plaintext.
//!
//! Modes are applied at creation so no umask-derived window exists. New names
//! are created exclusively, so an existing entry or planted symlink is never
//! followed. Existing user-managed files keep their modes.
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io;
use std::path::Path;

use tempfile::{NamedTempFile, TempDir};

#[cfg(unix)]
const PRIVATE_DIR_MODE: u32 = 0o700;
#[cfg(unix)]
const PRIVATE_FILE_MODE: u32 = 0o600;

/// Options that create a new owner-only file and refuse any existing entry.
pub fn create_new_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(PRIVATE_FILE_MODE);
    }
    options
}

/// Opens or creates a lock file, owner-only when created, without following a
/// final-component symlink and refusing anything but a regular file.
pub fn open_lock_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(PRIVATE_FILE_MODE)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(not(unix))]
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(io::Error::other("lock path is a symlink"));
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("lock path is not a regular file"));
    }
    Ok(file)
}

/// Creates a new empty owner-only file. Fails if anything exists at `path`.
pub fn create_new_file(path: &Path) -> io::Result<File> {
    create_new_options().open(path)
}

/// Writes `bytes` to a new owner-only file. Fails if anything exists at `path`.
pub fn write_new_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    io::Write::write_all(&mut create_new_file(path)?, bytes)
}

/// Creates missing directories owner-only, leaving existing ones unchanged.
pub fn create_dir_all(path: &Path) -> io::Result<()> {
    let mut builder = DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(PRIVATE_DIR_MODE);
    }
    builder.create(path)
}

/// Creates a new owner-only directory. Fails if anything exists at `path`.
pub fn create_dir(path: &Path) -> io::Result<()> {
    let mut builder = DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(PRIVATE_DIR_MODE);
    }
    builder.create(path)
}

/// Creates an unpredictably named owner-only directory in the system temp dir.
pub fn tempdir() -> io::Result<TempDir> {
    tempdir_in(&std::env::temp_dir(), ".aven-")
}

/// Creates an unpredictably named owner-only directory inside `parent`.
pub fn tempdir_in(parent: &Path, prefix: &str) -> io::Result<TempDir> {
    let mut builder = tempfile::Builder::new();
    builder.prefix(prefix);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(fs::Permissions::from_mode(PRIVATE_DIR_MODE));
    }
    builder.tempdir_in(parent)
}

/// Creates an unpredictably named, exclusively created owner-only file beside
/// `target`, so persisting it is a same-filesystem atomic rename.
pub fn sibling_tempfile(target: &Path) -> io::Result<NamedTempFile> {
    let parent = target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut prefix = std::ffi::OsString::from(".");
    prefix.push(target.file_name().unwrap_or_default());
    prefix.push(".");
    let mut builder = tempfile::Builder::new();
    builder.prefix(&prefix).suffix(".tmp");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(fs::Permissions::from_mode(PRIVATE_FILE_MODE));
    }
    builder.tempfile_in(parent)
}

/// Copies `source` into a new owner-only file at `target`.
pub fn copy_to_new_file(source: &Path, target: &Path) -> io::Result<()> {
    let mut input = File::open(source)?;
    let mut output = create_new_file(target)?;
    io::copy(&mut input, &mut output)?;
    Ok(())
}

#[cfg(all(test, unix))]
pub(crate) mod test_umask {
    use std::sync::{Mutex, MutexGuard};

    static UMASK: Mutex<()> = Mutex::new(());

    /// Holds a process-wide umask for the guard's lifetime.
    pub(crate) struct Umask {
        previous: libc::mode_t,
        _lock: MutexGuard<'static, ()>,
    }

    impl Umask {
        pub(crate) fn set(mask: libc::mode_t) -> Self {
            let lock = UMASK.lock().unwrap_or_else(|error| error.into_inner());
            // SAFETY: umask has no memory-safety preconditions.
            let previous = unsafe { libc::umask(mask) };
            Self {
                previous,
                _lock: lock,
            }
        }
    }

    impl Drop for Umask {
        fn drop(&mut self) {
            // SAFETY: umask has no memory-safety preconditions.
            unsafe { libc::umask(self.previous) };
        }
    }

    pub(crate) fn mode(path: &std::path::Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::symlink_metadata(path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777
    }
}
