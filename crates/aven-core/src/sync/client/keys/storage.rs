//! Host persistence for protected local keys.
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use zeroize::Zeroizing;

use super::{ProtectedLocalKeyStoreErrorKind, StoreResult, error};

/// Names the package keyring among secret items. Every other secret item is
/// named by its kind, such as `seed` or `peer-identity`.
pub const KEYRING_ITEM: &str = "keyring";

/// Holds a namespace's exclusive lock until dropped.
pub struct ProtectedStorageLock {
    _guard: Box<dyn Send + Sync>,
}

impl ProtectedStorageLock {
    pub fn new(guard: impl Send + Sync + 'static) -> Self {
        Self {
            _guard: Box::new(guard),
        }
    }
}

/// Host-owned storage for protected local keys.
///
/// Every call names a namespace: a lowercase hex digest of the database's
/// location and random `client_id`, so each database has its own items.
/// Secret items hold key material and must stay out of database exports,
/// backups and synchronizing credential stores. Records are nonsecret
/// markers and public evidence. Both are opaque byte strings.
///
/// Creating an item or record never replaces an existing one. Absence is
/// `Ok(None)`, never an error. Failures map to the stable error classes:
/// `Unavailable` while storage can't be reached (for example, locked),
/// `Corrupt` for unsafe or malformed contents and `WriteFailed` when a write
/// or delete fails. Messages never contain key material or paths.
pub trait ProtectedStorage: Send + Sync {
    /// Stable bytes naming where the database lives. With its `client_id`
    /// they name the namespace, so they must not change while the database
    /// stays in place. The default is the canonical path, which suits hosts
    /// whose paths are stable across launches and updates.
    fn database_location(&self, database_path: &Path) -> StoreResult<Vec<u8>> {
        canonical_location(database_path)
    }

    /// Makes the namespace ready before first use.
    fn prepare(&self, namespace: &str) -> StoreResult<()>;

    /// Excludes every other process and store using this namespace until the
    /// returned lock is dropped.
    fn lock(&self, namespace: &str) -> StoreResult<ProtectedStorageLock>;

    /// The secret item, which callers expect to be `expected_len` bytes.
    fn load_secret(
        &self,
        namespace: &str,
        item: &str,
        expected_len: usize,
    ) -> StoreResult<Option<Zeroizing<Vec<u8>>>>;

    /// Durably saves a secret item that the caller observed absent.
    fn create_secret(&self, namespace: &str, item: &str, bytes: &[u8]) -> StoreResult<()>;

    /// Deletes a secret item; an absent item is already deleted.
    fn delete_secret(&self, namespace: &str, item: &str) -> StoreResult<()>;

    /// The record, which callers expect to be `expected_len` bytes.
    fn read_record(
        &self,
        namespace: &str,
        name: &str,
        expected_len: usize,
    ) -> StoreResult<Option<Vec<u8>>>;

    /// Durably saves a new record; an existing record is a failure.
    fn create_record(&self, namespace: &str, name: &str, bytes: &[u8]) -> StoreResult<()>;

    /// Removes a record; an absent record is already removed.
    fn remove_record(&self, namespace: &str, name: &str) -> StoreResult<()>;

    /// Names of every secret item in the namespace. Extra names are allowed
    /// and only cost a load; a missing name makes its item read as absent.
    /// Called under the namespace lock, once per lock.
    fn list_secrets(&self, namespace: &str) -> StoreResult<HashSet<String>>;

    /// Names of every record in the namespace, with the same contract as
    /// [`Self::list_secrets`].
    fn list_records(&self, namespace: &str) -> StoreResult<HashSet<String>>;
}

/// The canonical database path as bytes.
pub fn canonical_location(database_path: &Path) -> StoreResult<Vec<u8>> {
    let canonical = database_path
        .canonicalize()
        .map_err(|_| error(ProtectedLocalKeyStoreErrorKind::Unavailable))?;
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(canonical.as_os_str().as_bytes().to_vec())
    }
    #[cfg(not(unix))]
    {
        Ok(canonical.to_string_lossy().as_bytes().to_vec())
    }
}

/// Basenames longer than this may not fit filesystems such as eCryptfs,
/// whose encrypted filenames cap plaintext names at 143 bytes.
const COMPACT_NAME_THRESHOLD: usize = 143;

/// Owner-only files in one directory. Secret item `kind` of namespace `ns`
/// is `ns.kind`, the keyring is `ns.keyring`, record `name` is `ns.name` and
/// the lock is `ns.lock`.
///
/// When `ns.name` is longer than [`COMPACT_NAME_THRESHOLD`] bytes and the
/// filesystem rejects it as too long, the item is created as `alias.name`,
/// where `alias` is the unpadded base64url encoding of the namespace digest.
/// Reads, listings and deletes consider both names; equal copies are one
/// item and differing copies are corrupt.
pub struct FileProtectedStorage {
    directory: PathBuf,
}

impl FileProtectedStorage {
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn path(&self, namespace: &str, name: &str) -> PathBuf {
        self.directory.join(format!("{namespace}.{name}"))
    }

    /// The compact name for `name`, when its usual name is long enough to
    /// need one.
    fn alias_path(&self, namespace: &str, name: &str) -> Option<PathBuf> {
        if namespace.len() + 1 + name.len() <= COMPACT_NAME_THRESHOLD {
            return None;
        }
        let prefix = alias_prefix(namespace)?;
        Some(self.directory.join(format!("{prefix}.{name}")))
    }

    fn read(
        &self,
        namespace: &str,
        name: &str,
        expected_len: usize,
    ) -> StoreResult<Option<Zeroizing<Vec<u8>>>> {
        let alternate = self.alias_path(namespace, name);
        let legacy = read_restricted_file(
            &self.path(namespace, name),
            expected_len,
            alternate.is_some(),
        )?;
        let Some(alternate) = alternate else {
            return Ok(legacy);
        };
        match (
            legacy,
            read_restricted_file(&alternate, expected_len, false)?,
        ) {
            (Some(a), Some(b)) if a != b => Err(error(ProtectedLocalKeyStoreErrorKind::Corrupt)),
            (a, b) => Ok(a.or(b)),
        }
    }

    /// Removes both names, syncing the directory after each removal.
    fn remove(&self, namespace: &str, name: &str) -> StoreResult<()> {
        let alternate = self.alias_path(namespace, name);
        let legacy = remove_restricted_file(&self.path(namespace, name), alternate.is_some());
        let alternate = alternate.map_or(Ok(()), |path| remove_restricted_file(&path, false));
        legacy.and(alternate)
    }
}

/// The unpadded base64url encoding of a lowercase hex namespace digest.
fn alias_prefix(namespace: &str) -> Option<String> {
    let bytes: [u8; 32] = hex::decode(namespace).ok()?.try_into().ok()?;
    (hex::encode(bytes) == namespace).then(|| URL_SAFE_NO_PAD.encode(bytes))
}

#[cfg(unix)]
fn name_too_long(source: &std::io::Error) -> bool {
    source.raw_os_error() == Some(libc::ENAMETOOLONG)
}

#[cfg(not(unix))]
fn name_too_long(_: &std::io::Error) -> bool {
    false
}

impl ProtectedStorage for FileProtectedStorage {
    fn prepare(&self, _namespace: &str) -> StoreResult<()> {
        prepare_directory(&self.directory)
    }

    fn lock(&self, namespace: &str) -> StoreResult<ProtectedStorageLock> {
        let path = self.path(namespace, "lock");
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(path)
            .map_err(|_| error(ProtectedLocalKeyStoreErrorKind::Unavailable))?;
        file.lock()
            .map_err(|_| error(ProtectedLocalKeyStoreErrorKind::Unavailable))?;
        Ok(ProtectedStorageLock::new(file))
    }

    fn load_secret(
        &self,
        namespace: &str,
        item: &str,
        expected_len: usize,
    ) -> StoreResult<Option<Zeroizing<Vec<u8>>>> {
        self.read(namespace, item, expected_len)
    }

    fn create_secret(&self, namespace: &str, item: &str, bytes: &[u8]) -> StoreResult<()> {
        write_restricted_new(
            &self.path(namespace, item),
            self.alias_path(namespace, item).as_deref(),
            bytes,
        )
    }

    fn delete_secret(&self, namespace: &str, item: &str) -> StoreResult<()> {
        self.remove(namespace, item)
    }

    fn read_record(
        &self,
        namespace: &str,
        name: &str,
        expected_len: usize,
    ) -> StoreResult<Option<Vec<u8>>> {
        Ok(self
            .read(namespace, name, expected_len)?
            .map(|mut bytes| std::mem::take(&mut *bytes)))
    }

    fn create_record(&self, namespace: &str, name: &str, bytes: &[u8]) -> StoreResult<()> {
        write_restricted_new(
            &self.path(namespace, name),
            self.alias_path(namespace, name).as_deref(),
            bytes,
        )
    }

    fn remove_record(&self, namespace: &str, name: &str) -> StoreResult<()> {
        self.remove(namespace, name)
    }

    /// Secret items and records share the directory, so both listings name
    /// every file of the namespace except its lock.
    fn list_secrets(&self, namespace: &str) -> StoreResult<HashSet<String>> {
        self.list(namespace)
    }

    fn list_records(&self, namespace: &str) -> StoreResult<HashSet<String>> {
        self.list(namespace)
    }
}

impl FileProtectedStorage {
    fn list(&self, namespace: &str) -> StoreResult<HashSet<String>> {
        let prefix = format!("{namespace}.");
        let alias = alias_prefix(namespace).map(|alias| format!("{alias}."));
        let entries = match fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                return Ok(HashSet::new());
            }
            Err(_) => return Err(error(ProtectedLocalKeyStoreErrorKind::Unavailable)),
        };
        let mut names = HashSet::new();
        for entry in entries {
            let entry = entry.map_err(|_| error(ProtectedLocalKeyStoreErrorKind::Unavailable))?;
            let file_name = entry.file_name();
            let Some(file_name) = file_name.to_str() else {
                continue;
            };
            let name = match file_name.strip_prefix(&prefix) {
                Some(name) => Some(name).filter(|name| *name != "lock"),
                None => alias
                    .as_deref()
                    .and_then(|alias| file_name.strip_prefix(alias))
                    .filter(|name| prefix.len() + name.len() > COMPACT_NAME_THRESHOLD),
            };
            if let Some(name) = name {
                names.insert(name.to_string());
            }
        }
        Ok(names)
    }
}

fn prepare_directory(path: &Path) -> StoreResult<()> {
    fs::create_dir_all(path).map_err(|_| error(ProtectedLocalKeyStoreErrorKind::Unavailable))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|_| error(ProtectedLocalKeyStoreErrorKind::Unavailable))?;
    }
    Ok(())
}

/// `may_be_too_long` reads a name the filesystem rejects as absent.
fn read_restricted_file(
    path: &Path,
    expected_len: usize,
    may_be_too_long: bool,
) -> StoreResult<Option<Zeroizing<Vec<u8>>>> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(source)
            if source.kind() == std::io::ErrorKind::NotFound
                || (may_be_too_long && name_too_long(&source)) =>
        {
            return Ok(None);
        }
        Err(_) => return Err(error(ProtectedLocalKeyStoreErrorKind::Unavailable)),
    };
    let metadata = file
        .metadata()
        .map_err(|_| error(ProtectedLocalKeyStoreErrorKind::Unavailable))?;
    if !metadata.is_file() || metadata.len() != expected_len as u64 {
        return Err(error(ProtectedLocalKeyStoreErrorKind::Corrupt));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(error(ProtectedLocalKeyStoreErrorKind::Corrupt));
        }
    }
    let mut bytes = Zeroizing::new(vec![0_u8; expected_len]);
    file.read_exact(&mut bytes)
        .map_err(|_| error(ProtectedLocalKeyStoreErrorKind::Corrupt))?;
    Ok(Some(bytes))
}

fn sync_parent(path: &Path) -> StoreResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| error(ProtectedLocalKeyStoreErrorKind::WriteFailed))?;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| error(ProtectedLocalKeyStoreErrorKind::WriteFailed))
}

/// `may_be_too_long` treats a name the filesystem rejects as absent.
fn remove_restricted_file(path: &Path, may_be_too_long: bool) -> StoreResult<()> {
    match fs::remove_file(path) {
        Ok(()) => sync_parent(path),
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound
                || (may_be_too_long && name_too_long(&error)) =>
        {
            Ok(())
        }
        Err(_) => Err(super::error(ProtectedLocalKeyStoreErrorKind::WriteFailed)),
    }
}

fn write_failure(
    phase: &'static str,
    source: std::io::Error,
) -> super::ProtectedLocalKeyStoreError {
    tracing::warn!(
        phase,
        errno = source.raw_os_error(),
        "protected storage write failed"
    );
    error(ProtectedLocalKeyStoreErrorKind::WriteFailed)
}

/// Creates `path`, or `alternate` when the filesystem rejects `path` as too
/// long. Neither name may already exist.
fn write_restricted_new(path: &Path, alternate: Option<&Path>, bytes: &[u8]) -> StoreResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| error(ProtectedLocalKeyStoreErrorKind::WriteFailed))?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)
        .map_err(|source| write_failure("stage_create", source))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        staged
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|source| write_failure("stage_permissions", source))?;
    }
    staged
        .as_file_mut()
        .write_all(bytes)
        .map_err(|source| write_failure("stage_write", source))?;
    staged
        .as_file()
        .sync_all()
        .map_err(|source| write_failure("stage_sync", source))?;
    publish_new(staged, path, alternate)
        .map_err(|source| write_failure("stage_persist", source))?;
    File::open(parent)
        .map_err(|source| write_failure("parent_open", source))?
        .sync_all()
        .map_err(|source| write_failure("parent_sync", source))
}

fn publish_new(
    staged: tempfile::NamedTempFile,
    legacy: &Path,
    alternate: Option<&Path>,
) -> std::io::Result<File> {
    if let Some(path) = alternate {
        match fs::symlink_metadata(path) {
            Ok(_) => return Err(std::io::ErrorKind::AlreadyExists.into()),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => return Err(source),
        }
    }
    match staged.persist_noclobber(legacy) {
        Ok(file) => Ok(file),
        Err(failure) => match alternate {
            Some(path) if name_too_long(&failure.error) => failure
                .file
                .persist_noclobber(path)
                .map_err(|failure| failure.error),
            _ => Err(failure.error),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAMESPACE: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    fn storage() -> (tempfile::TempDir, FileProtectedStorage) {
        let directory = tempfile::tempdir().unwrap();
        let storage = FileProtectedStorage::new(directory.path().to_path_buf());
        (directory, storage)
    }

    fn kind<T: std::fmt::Debug>(result: StoreResult<T>) -> ProtectedLocalKeyStoreErrorKind {
        result.unwrap_err().kind()
    }

    #[test]
    fn failed_create_preserves_existing_bytes_and_error_contract() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("record");
        write_restricted_new(&path, None, b"first").unwrap();
        assert_eq!(
            write_restricted_new(&path, None, b"other")
                .unwrap_err()
                .kind(),
            ProtectedLocalKeyStoreErrorKind::WriteFailed
        );
        assert_eq!(
            *read_restricted_file(&path, 5, false).unwrap().unwrap(),
            b"first"
        );
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn failed_staging_keeps_stable_private_error() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("missing/record");
        let failure = write_restricted_new(&path, None, b"private").unwrap_err();
        assert_eq!(failure.kind(), ProtectedLocalKeyStoreErrorKind::WriteFailed);
        assert!(!failure.to_string().contains("missing"));
        assert!(!failure.to_string().contains("private"));
    }

    #[test]
    fn compact_names_fit_143_bytes_and_apply_only_to_long_names() {
        let (_directory, storage) = storage();
        let alias = alias_prefix(NAMESPACE).unwrap();
        assert_eq!(
            URL_SAFE_NO_PAD.decode(&alias).unwrap(),
            hex::decode(NAMESPACE).unwrap()
        );
        // The longest enrollment names: membership evidence and invitation
        // authority markers.
        for name in [
            format!("membership-evidence-{}", "cd".repeat(32)),
            format!("invite-{}-candidate-7-authority", "ab".repeat(32)),
            format!("invite-{}-withdrawing-authority", "ab".repeat(32)),
        ] {
            let path = storage.alias_path(NAMESPACE, &name).unwrap();
            assert!(path.file_name().unwrap().len() <= COMPACT_NAME_THRESHOLD);
        }
        assert!(storage.alias_path(NAMESPACE, &"x".repeat(78)).is_none());
        assert!(storage.alias_path(NAMESPACE, &"x".repeat(79)).is_some());
        for namespace in [NAMESPACE.to_uppercase(), NAMESPACE[..62].to_string()] {
            assert!(storage.alias_path(&namespace, &"x".repeat(200)).is_none());
        }
    }

    /// The host rejects the 265-byte usual name, as eCryptfs rejects names
    /// over 143 bytes.
    #[cfg(unix)]
    #[test]
    fn names_the_filesystem_rejects_fall_back_to_compact_names() {
        let (_directory, storage) = storage();
        let name = "n".repeat(200);
        assert_eq!(storage.read_record(NAMESPACE, &name, 4).unwrap(), None);
        storage.create_record(NAMESPACE, &name, b"long").unwrap();
        let alias = storage.alias_path(NAMESPACE, &name).unwrap();
        assert_eq!(fs::read(&alias).unwrap(), b"long");
        assert_eq!(
            storage.read_record(NAMESPACE, &name, 4).unwrap().unwrap(),
            b"long"
        );
        assert_eq!(
            storage.list_records(NAMESPACE).unwrap(),
            HashSet::from([name.clone()])
        );
        assert_eq!(
            kind(storage.create_record(NAMESPACE, &name, b"next")),
            ProtectedLocalKeyStoreErrorKind::WriteFailed
        );
        storage.remove_record(NAMESPACE, &name).unwrap();
        assert!(!alias.exists());
    }

    #[test]
    fn compact_copies_are_found_where_usual_names_fit() {
        let (directory, storage) = storage();
        let name = "n".repeat(100);
        let alias = storage.alias_path(NAMESPACE, &name).unwrap();
        write_restricted_new(&alias, None, b"seen").unwrap();
        let prefix = alias_prefix(NAMESPACE).unwrap();
        for ignored in ["lock".to_string(), "n".repeat(78)] {
            fs::write(directory.path().join(format!("{prefix}.{ignored}")), b"").unwrap();
        }
        assert_eq!(
            kind(storage.create_record(NAMESPACE, &name, b"next")),
            ProtectedLocalKeyStoreErrorKind::WriteFailed
        );
        assert!(!storage.path(NAMESPACE, &name).exists());
        assert_eq!(
            *storage.load_secret(NAMESPACE, &name, 4).unwrap().unwrap(),
            b"seen"
        );
        assert_eq!(
            storage.list_secrets(NAMESPACE).unwrap(),
            HashSet::from([name.clone()])
        );
        storage.delete_secret(NAMESPACE, &name).unwrap();
        assert!(!alias.exists());
    }

    #[test]
    fn both_copies_must_be_valid_and_agree() {
        let (_directory, storage) = storage();
        let name = "n".repeat(100);
        let (legacy, alias) = (
            storage.path(NAMESPACE, &name),
            storage.alias_path(NAMESPACE, &name).unwrap(),
        );
        // Names that fit are created under the usual name.
        storage.create_record(NAMESPACE, &name, b"same").unwrap();
        assert!(legacy.exists() && !alias.exists());
        for (legacy_bytes, alias_bytes, valid) in [
            (&b"same"[..], &b"same"[..], true),
            (b"same", b"diff", false),
            (b"same", b"long!", false),
            (b"long!", b"same", false),
        ] {
            for (path, bytes) in [(&legacy, legacy_bytes), (&alias, alias_bytes)] {
                let _ = fs::remove_file(path);
                write_restricted_new(path, None, bytes).unwrap();
            }
            let read = storage.read_record(NAMESPACE, &name, 4);
            if valid {
                assert_eq!(read.unwrap().unwrap(), b"same");
            } else {
                assert_eq!(kind(read), ProtectedLocalKeyStoreErrorKind::Corrupt);
            }
        }
        storage.remove_record(NAMESPACE, &name).unwrap();
        assert!(!legacy.exists() && !alias.exists());
    }
}
