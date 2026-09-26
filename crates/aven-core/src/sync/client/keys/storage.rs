//! Host persistence for protected local keys.
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use zeroize::Zeroizing;

use super::{ProtectedLocalKeyStoreErrorKind, StoreResult, error};

/// Names the package keyring among secret items. Every other secret item is
/// named by its kind, such as `seed` or `peer-identity`.
pub const KEYRING_ITEM: &str = "keyring";

/// Holds a namespace's exclusive lock until dropped.
pub struct ProtectedStorageLock(#[allow(dead_code)] Box<dyn Send + Sync>);

impl ProtectedStorageLock {
    pub fn new(guard: impl Send + Sync + 'static) -> Self {
        Self(Box::new(guard))
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

/// Owner-only files in one directory. Secret item `kind` of namespace `ns`
/// is `ns.kind`, the keyring is `ns.keyring`, record `name` is `ns.name` and
/// the lock is `ns.lock`.
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
        Ok(read_restricted_file(&self.path(namespace, item), expected_len)?.map(Zeroizing::new))
    }

    fn create_secret(&self, namespace: &str, item: &str, bytes: &[u8]) -> StoreResult<()> {
        write_restricted_new(&self.path(namespace, item), bytes)
    }

    fn delete_secret(&self, namespace: &str, item: &str) -> StoreResult<()> {
        remove_restricted_file(&self.path(namespace, item))
    }

    fn read_record(
        &self,
        namespace: &str,
        name: &str,
        expected_len: usize,
    ) -> StoreResult<Option<Vec<u8>>> {
        read_restricted_file(&self.path(namespace, name), expected_len)
    }

    fn create_record(&self, namespace: &str, name: &str, bytes: &[u8]) -> StoreResult<()> {
        write_restricted_new(&self.path(namespace, name), bytes)
    }

    fn remove_record(&self, namespace: &str, name: &str) -> StoreResult<()> {
        remove_restricted_file(&self.path(namespace, name))
    }

    /// Secret items and records share the directory, so both listings name
    /// every file of the namespace.
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
            if let Some(name) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.strip_prefix(&prefix))
            {
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

fn read_restricted_file(path: &Path, expected_len: usize) -> StoreResult<Option<Vec<u8>>> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
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
    let mut bytes = vec![0_u8; expected_len];
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

fn remove_restricted_file(path: &Path) -> StoreResult<()> {
    match fs::remove_file(path) {
        Ok(()) => sync_parent(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(super::error(ProtectedLocalKeyStoreErrorKind::WriteFailed)),
    }
}

fn write_restricted_new(path: &Path, bytes: &[u8]) -> StoreResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| error(ProtectedLocalKeyStoreErrorKind::WriteFailed))?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)
        .map_err(|_| error(ProtectedLocalKeyStoreErrorKind::WriteFailed))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        staged
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|_| error(ProtectedLocalKeyStoreErrorKind::WriteFailed))?;
    }
    staged
        .as_file_mut()
        .write_all(bytes)
        .and_then(|()| staged.as_file().sync_all())
        .map_err(|_| error(ProtectedLocalKeyStoreErrorKind::WriteFailed))?;
    staged
        .persist_noclobber(path)
        .map_err(|_| error(ProtectedLocalKeyStoreErrorKind::WriteFailed))?;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| error(ProtectedLocalKeyStoreErrorKind::WriteFailed))
}
