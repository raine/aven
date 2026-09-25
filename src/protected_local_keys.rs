mod adoption;
mod membership;
pub(crate) mod peer;
pub(crate) mod rotation;
mod seed;
pub use peer::EnrollmentReadiness;

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use aven_core::db::Database;
use aven_core::sync::{
    EncryptedLocalSharedStatePackage, LocalSharedStatePackageContext, LocalSharedStatePackageKey,
};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

const STORE_DIRECTORY: &str = "protected-keys";
const KEYRING_MAGIC: &[u8; 8] = b"AVENLKR1";
const MARKER_MAGIC: &[u8; 8] = b"AVENLKM1";
const KEYRING_BYTES: usize = 8 + 32 + 32 + 32 + 32;
const MARKER_BYTES: usize = 8 + 32 + 32;
const KEYCHAIN_SERVICE: &str = "fi.zendit.Aven.local-package-keyring";

/// Stable error classes for protected local key storage.
///
/// Messages intentionally contain neither key material nor database paths.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtectedLocalKeyStoreErrorKind {
    MissingAuthority,
    Unavailable,
    Corrupt,
    WriteFailed,
    WrongDatabase,
    UnsupportedPlatform,
}

#[derive(Debug)]
pub struct ProtectedLocalKeyStoreError {
    kind: ProtectedLocalKeyStoreErrorKind,
}

impl ProtectedLocalKeyStoreError {
    fn new(kind: ProtectedLocalKeyStoreErrorKind) -> Self {
        Self { kind }
    }

    pub fn kind(&self) -> ProtectedLocalKeyStoreErrorKind {
        self.kind
    }
}

impl fmt::Display for ProtectedLocalKeyStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self.kind {
            ProtectedLocalKeyStoreErrorKind::MissingAuthority => {
                "protected local key authority is missing"
            }
            ProtectedLocalKeyStoreErrorKind::Unavailable => {
                "protected local key storage is unavailable"
            }
            ProtectedLocalKeyStoreErrorKind::Corrupt => {
                "protected local key storage is corrupt or unsafe"
            }
            ProtectedLocalKeyStoreErrorKind::WriteFailed => {
                "protected local key storage could not be saved"
            }
            ProtectedLocalKeyStoreErrorKind::WrongDatabase => {
                "protected local key store belongs to a different database installation"
            }
            ProtectedLocalKeyStoreErrorKind::UnsupportedPlatform => {
                "protected local key storage is unsupported on this platform"
            }
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for ProtectedLocalKeyStoreError {}

type StoreResult<T> = Result<T, ProtectedLocalKeyStoreError>;

/// The one generation identity and secret needed by a local encrypted capture.
///
/// The secret has redacted debug output and is zeroized by the core key type.
pub struct ProtectedLocalPackageKey {
    context: LocalSharedStatePackageContext,
    key: LocalSharedStatePackageKey,
}

impl ProtectedLocalPackageKey {
    pub fn context(&self) -> LocalSharedStatePackageContext {
        self.context
    }

    pub fn package_key(&self) -> &LocalSharedStatePackageKey {
        &self.key
    }
}

impl fmt::Debug for ProtectedLocalPackageKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProtectedLocalPackageKey")
            .field("context", &self.context)
            .field("key", &"[REDACTED]")
            .finish()
    }
}

/// Host-owned protected storage scoped to one canonical database installation.
///
/// On macOS the keyring is a non-synchronizing login Keychain item. On Linux
/// it is an owner-only file below the application state directory. A
/// separate nonsecret marker detects loss of an established authority. Neither
/// location is part of Aven's database export, SQLite backup, or archive backup.
pub struct ProtectedLocalKeyStore {
    account: String,
    directory: PathBuf,
    backend: Backend,
}

impl ProtectedLocalKeyStore {
    pub fn for_database(database_path: &Path) -> StoreResult<Self> {
        #[cfg(not(test))]
        let directory = protected_store_directory()?;
        // Tests never reach the login Keychain; CLI test workers name isolated files.
        #[cfg(test)]
        let directory = std::env::var_os("AVEN_TEST_PROTECTED_KEYS")
            .map(PathBuf::from)
            .ok_or_else(|| error(ProtectedLocalKeyStoreErrorKind::Unavailable))?;
        Self::with_directory(database_path, directory)
    }

    fn with_directory(database_path: &Path, directory: PathBuf) -> StoreResult<Self> {
        let canonical = database_path
            .canonicalize()
            .map_err(|_| error(ProtectedLocalKeyStoreErrorKind::Unavailable))?;
        let account = database_account(&canonical);
        #[cfg(not(test))]
        let backend = Backend::production(&directory, &account)?;
        #[cfg(test)]
        let backend = Backend::File(FileBackend {
            path: directory.join(format!("{account}.keyring")),
        });
        Ok(Self {
            account,
            directory,
            backend,
        })
    }

    /// Loads the established keyring, or creates and durably marks a new one.
    ///
    /// An existing marker with missing key material is an error. Existing or
    /// corrupt material is never replaced. If key persistence succeeds but the
    /// marker write fails, retry loads the same key and completes the marker.
    pub fn load_or_create(&self) -> StoreResult<ProtectedLocalPackageKey> {
        prepare_directory(&self.directory)?;
        let _guard = self.lock()?;
        let marker = read_marker(&self.marker_path())?;
        match self.backend.load()? {
            Some(bytes) => {
                let key = decode_keyring(bytes)?;
                match marker {
                    Some(expected) if expected != key.context => {
                        Err(error(ProtectedLocalKeyStoreErrorKind::Corrupt))
                    }
                    Some(_) => Ok(key),
                    None => {
                        write_marker(&self.marker_path(), key.context)?;
                        Ok(key)
                    }
                }
            }
            None if marker.is_some() => {
                Err(error(ProtectedLocalKeyStoreErrorKind::MissingAuthority))
            }
            None => {
                if self.seed_authority_exists()? || self.peer_authority_exists()? {
                    return Err(error(ProtectedLocalKeyStoreErrorKind::MissingAuthority));
                }
                let key = generate_keyring()?;
                let encoded = encode_keyring(&key);
                self.backend.create(&encoded)?;
                write_marker(&self.marker_path(), key.context)?;
                Ok(key)
            }
        }
    }

    /// Loads an established authority without generating key material.
    ///
    /// If key persistence completed before its nonsecret marker, this completes
    /// the marker using the existing key identity.
    pub fn load_required(&self) -> StoreResult<ProtectedLocalPackageKey> {
        prepare_directory(&self.directory)?;
        let _guard = self.lock()?;
        self.load_required_locked()
    }

    fn load_required_locked(&self) -> StoreResult<ProtectedLocalPackageKey> {
        let marker = read_marker(&self.marker_path())?;
        let bytes = self
            .backend
            .load()?
            .ok_or_else(|| error(ProtectedLocalKeyStoreErrorKind::MissingAuthority))?;
        let key = decode_keyring(bytes)?;
        match marker {
            Some(expected) if expected != key.context => {
                Err(error(ProtectedLocalKeyStoreErrorKind::Corrupt))
            }
            Some(_) => Ok(key),
            None => {
                write_marker(&self.marker_path(), key.context)?;
                Ok(key)
            }
        }
    }

    /// Creates or reuses protected key material before freezing package bytes.
    /// The predecessor is caller-supplied context, not membership authorization.
    /// An existing freeze requires its established authority even if bytes are missing.
    pub async fn package_local_capture(
        &self,
        database: &Database,
        blob_dir: &Path,
        membership_predecessor: [u8; 32],
    ) -> Result<EncryptedLocalSharedStatePackage, anyhow::Error> {
        self.validate_database(database)?;
        let protected = if database
            .has_local_shared_state_package_never_dispatched()
            .await?
            || database.local_seed_genesis_commitment().await?.is_some()
        {
            self.load_required()?
        } else {
            self.load_or_create()?
        };
        database
            .package_local_shared_state_never_dispatched(
                blob_dir,
                protected.context(),
                protected.package_key(),
                membership_predecessor,
            )
            .await
    }

    fn validate_database(&self, database: &Database) -> StoreResult<()> {
        let canonical = database
            .path()
            .canonicalize()
            .map_err(|_| error(ProtectedLocalKeyStoreErrorKind::Unavailable))?;
        if database_account(&canonical) != self.account {
            return Err(error(ProtectedLocalKeyStoreErrorKind::WrongDatabase));
        }
        Ok(())
    }

    fn marker_path(&self) -> PathBuf {
        self.directory.join(format!("{}.authority", self.account))
    }

    fn lock(&self) -> StoreResult<File> {
        let path = self.directory.join(format!("{}.lock", self.account));
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
        Ok(file)
    }
}

fn error(kind: ProtectedLocalKeyStoreErrorKind) -> ProtectedLocalKeyStoreError {
    ProtectedLocalKeyStoreError::new(kind)
}

#[cfg_attr(test, allow(dead_code))]
fn protected_store_directory() -> StoreResult<PathBuf> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| dirs::home_dir().map(|home| home.join(".local/state")))
        .ok_or_else(|| error(ProtectedLocalKeyStoreErrorKind::Unavailable))?;
    Ok(state.join("aven").join(STORE_DIRECTORY))
}

fn database_account(canonical_path: &Path) -> String {
    #[cfg(unix)]
    let bytes = {
        use std::os::unix::ffi::OsStrExt;
        canonical_path.as_os_str().as_bytes()
    };
    #[cfg(not(unix))]
    let bytes = canonical_path.to_string_lossy().as_bytes();
    hex::encode(Sha256::digest(bytes))
}

fn generate_keyring() -> StoreResult<ProtectedLocalPackageKey> {
    let mut bytes = Zeroizing::new([0_u8; 96]);
    getrandom::fill(bytes.as_mut())
        .map_err(|_| error(ProtectedLocalKeyStoreErrorKind::Unavailable))?;
    let mut vault_id = [0_u8; 32];
    let mut generation_id = [0_u8; 32];
    let mut key = [0_u8; 32];
    vault_id.copy_from_slice(&bytes[..32]);
    generation_id.copy_from_slice(&bytes[32..64]);
    key.copy_from_slice(&bytes[64..]);
    Ok(ProtectedLocalPackageKey {
        context: LocalSharedStatePackageContext {
            vault_id,
            generation_id,
        },
        key: LocalSharedStatePackageKey::new(key),
    })
}

fn encode_keyring(key: &ProtectedLocalPackageKey) -> Zeroizing<Vec<u8>> {
    let mut encoded = Zeroizing::new(Vec::with_capacity(KEYRING_BYTES));
    encoded.extend_from_slice(KEYRING_MAGIC);
    encoded.extend_from_slice(&key.context.vault_id);
    encoded.extend_from_slice(&key.context.generation_id);
    // LocalSharedStatePackageKey intentionally does not expose bytes to host
    // callers. Encoding is performed through its bounded persistence bridge.
    encoded.extend_from_slice(key.key.protected_storage_bytes());
    let checksum = Sha256::digest(encoded.as_slice());
    encoded.extend_from_slice(&checksum);
    encoded
}

fn decode_keyring(mut encoded: Zeroizing<Vec<u8>>) -> StoreResult<ProtectedLocalPackageKey> {
    if encoded.len() != KEYRING_BYTES || &encoded[..8] != KEYRING_MAGIC {
        return Err(error(ProtectedLocalKeyStoreErrorKind::Corrupt));
    }
    let checksum_offset = KEYRING_BYTES - 32;
    let expected = Sha256::digest(&encoded[..checksum_offset]);
    if expected.as_slice() != &encoded[checksum_offset..] {
        return Err(error(ProtectedLocalKeyStoreErrorKind::Corrupt));
    }
    let mut vault_id = [0_u8; 32];
    let mut generation_id = [0_u8; 32];
    let mut key = [0_u8; 32];
    vault_id.copy_from_slice(&encoded[8..40]);
    generation_id.copy_from_slice(&encoded[40..72]);
    key.copy_from_slice(&encoded[72..104]);
    encoded.zeroize();
    Ok(ProtectedLocalPackageKey {
        context: LocalSharedStatePackageContext {
            vault_id,
            generation_id,
        },
        key: LocalSharedStatePackageKey::new(key),
    })
}

fn marker_bytes(context: LocalSharedStatePackageContext) -> [u8; MARKER_BYTES] {
    let mut bytes = [0_u8; MARKER_BYTES];
    bytes[..8].copy_from_slice(MARKER_MAGIC);
    bytes[8..40].copy_from_slice(&context.vault_id);
    bytes[40..].copy_from_slice(&context.generation_id);
    bytes
}

fn read_marker(path: &Path) -> StoreResult<Option<LocalSharedStatePackageContext>> {
    let Some(bytes) = read_restricted_file(path, MARKER_BYTES)? else {
        return Ok(None);
    };
    if &bytes[..8] != MARKER_MAGIC {
        return Err(error(ProtectedLocalKeyStoreErrorKind::Corrupt));
    }
    let mut vault_id = [0_u8; 32];
    let mut generation_id = [0_u8; 32];
    vault_id.copy_from_slice(&bytes[8..40]);
    generation_id.copy_from_slice(&bytes[40..]);
    Ok(Some(LocalSharedStatePackageContext {
        vault_id,
        generation_id,
    }))
}

fn write_marker(path: &Path, context: LocalSharedStatePackageContext) -> StoreResult<()> {
    write_restricted_new(path, &marker_bytes(context))
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

enum Backend {
    #[cfg(target_os = "macos")]
    Keychain(KeychainBackend),
    #[cfg(any(target_os = "linux", test))]
    File(FileBackend),
    #[cfg(test)]
    FailWrite,
    #[cfg(test)]
    Unavailable,
}

impl Backend {
    #[cfg_attr(test, allow(dead_code))]
    fn production(directory: &Path, account: &str) -> StoreResult<Self> {
        #[cfg(target_os = "macos")]
        {
            let _ = directory;
            Ok(Self::Keychain(KeychainBackend {
                service: KEYCHAIN_SERVICE.to_string(),
                account: account.to_string(),
            }))
        }
        #[cfg(target_os = "linux")]
        {
            Ok(Self::File(FileBackend {
                path: directory.join(format!("{account}.keyring")),
            }))
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            let _ = (directory, account);
            Err(error(ProtectedLocalKeyStoreErrorKind::UnsupportedPlatform))
        }
    }

    fn load(&self) -> StoreResult<Option<Zeroizing<Vec<u8>>>> {
        self.load_bounded(KEYRING_BYTES)
    }

    fn load_bounded(&self, expected_len: usize) -> StoreResult<Option<Zeroizing<Vec<u8>>>> {
        #[cfg(test)]
        tests::BACKEND_LOADS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        match self {
            #[cfg(target_os = "macos")]
            Self::Keychain(backend) => {
                let bytes = backend.load()?;
                if bytes
                    .as_ref()
                    .is_some_and(|bytes| bytes.len() != expected_len)
                {
                    return Err(error(ProtectedLocalKeyStoreErrorKind::Corrupt));
                }
                Ok(bytes)
            }
            #[cfg(any(target_os = "linux", test))]
            Self::File(backend) => backend.load(expected_len),
            #[cfg(test)]
            Self::FailWrite => Ok(None),
            #[cfg(test)]
            Self::Unavailable => Err(error(ProtectedLocalKeyStoreErrorKind::Unavailable)),
        }
    }

    fn create(&self, bytes: &[u8]) -> StoreResult<()> {
        match self {
            #[cfg(target_os = "macos")]
            Self::Keychain(backend) => backend.create(bytes),
            #[cfg(any(target_os = "linux", test))]
            Self::File(backend) => backend.create(bytes),
            #[cfg(test)]
            Self::FailWrite => Err(error(ProtectedLocalKeyStoreErrorKind::WriteFailed)),
            #[cfg(test)]
            Self::Unavailable => Err(error(ProtectedLocalKeyStoreErrorKind::Unavailable)),
        }
    }
}

#[cfg(any(target_os = "linux", test))]
struct FileBackend {
    path: PathBuf,
}

#[cfg(any(target_os = "linux", test))]
impl FileBackend {
    fn load(&self, expected_len: usize) -> StoreResult<Option<Zeroizing<Vec<u8>>>> {
        Ok(read_restricted_file(&self.path, expected_len)?.map(Zeroizing::new))
    }

    fn create(&self, bytes: &[u8]) -> StoreResult<()> {
        write_restricted_new(&self.path, bytes)
    }
}

#[cfg(target_os = "macos")]
struct KeychainBackend {
    service: String,
    account: String,
}

#[cfg(target_os = "macos")]
impl KeychainBackend {
    fn options(&self) -> security_framework::passwords::PasswordOptions {
        use security_framework::passwords::PasswordOptions;

        let mut options = PasswordOptions::new_generic_password(&self.service, &self.account);
        options.set_access_synchronized(Some(false));
        options
    }

    fn load(&self) -> StoreResult<Option<Zeroizing<Vec<u8>>>> {
        use security_framework::passwords::generic_password;
        use security_framework_sys::base::errSecItemNotFound;

        match generic_password(self.options()) {
            Ok(bytes) => Ok(Some(Zeroizing::new(bytes))),
            Err(source) if source.code() == errSecItemNotFound => Ok(None),
            Err(_) => Err(error(ProtectedLocalKeyStoreErrorKind::Unavailable)),
        }
    }

    fn create(&self, bytes: &[u8]) -> StoreResult<()> {
        use security_framework::passwords::set_generic_password_options;

        // The caller has already observed absence. A process guard around this
        // operation serializes cooperating Aven hosts. Re-read before success.
        match set_generic_password_options(bytes, self.options()) {
            Ok(()) => match self.load()? {
                Some(saved) if saved.as_slice() == bytes => Ok(()),
                _ => Err(error(ProtectedLocalKeyStoreErrorKind::WriteFailed)),
            },
            Err(_) => Err(error(ProtectedLocalKeyStoreErrorKind::WriteFailed)),
        }
    }

    #[cfg(test)]
    fn delete(&self) {
        use security_framework::passwords::delete_generic_password_options;
        let _ = delete_generic_password_options(self.options());
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    tokio::task_local! {
        static SCOPED_BACKEND_LOADS: std::sync::atomic::AtomicU64;
    }

    pub(crate) struct BackendLoads(std::sync::atomic::AtomicU64);
    impl BackendLoads {
        pub(crate) fn fetch_add(&self, value: u64, ordering: std::sync::atomic::Ordering) -> u64 {
            let previous = self.0.fetch_add(value, ordering);
            let _ = SCOPED_BACKEND_LOADS.try_with(|loads| {
                loads.fetch_add(value, ordering);
            });
            previous
        }

        pub(crate) fn load(&self, ordering: std::sync::atomic::Ordering) -> u64 {
            self.0.load(ordering)
        }

        pub(crate) async fn measure<F: std::future::Future>(&self, future: F) -> (F::Output, u64) {
            SCOPED_BACKEND_LOADS
                .scope(std::sync::atomic::AtomicU64::new(0), async {
                    let output = future.await;
                    let loads = SCOPED_BACKEND_LOADS
                        .with(|count| count.load(std::sync::atomic::Ordering::Relaxed));
                    (output, loads)
                })
                .await
        }
    }

    /// Protected backend loads in this process: each is one Keychain lookup on macOS.
    pub(crate) static BACKEND_LOADS: BackendLoads =
        BackendLoads(std::sync::atomic::AtomicU64::new(0));
    use aven_core::api::{CreateTask, Store};
    use aven_core::choices::{TaskPriority, TaskStatus};

    pub(super) async fn captured_database(root: &Path) -> Database {
        let path = root.join("source.sqlite");
        let store = Store::open(&path).await.unwrap();
        let workspace = store.list_workspaces().await.unwrap().remove(0);
        store
            .create_task(
                &workspace.id,
                CreateTask {
                    title: "protected package".into(),
                    description: String::new(),
                    project: "app".into(),
                    status: TaskStatus::Todo,
                    priority: TaskPriority::None,
                    metadata: vec![],
                    available_at: None,
                    due_on: None,
                },
            )
            .await
            .unwrap();
        drop(store);
        let database = Database::open(&path).await.unwrap();
        database
            .capture_local_shared_state_never_dispatched()
            .await
            .unwrap();
        database
    }

    pub(crate) fn isolated_store(database_path: &Path, root: &Path) -> ProtectedLocalKeyStore {
        let canonical = database_path.canonicalize().unwrap();
        let account = database_account(&canonical);
        ProtectedLocalKeyStore {
            backend: Backend::File(FileBackend {
                path: root.join(format!("{account}.keyring")),
            }),
            account,
            directory: root.to_path_buf(),
        }
    }

    #[tokio::test]
    async fn protected_store_reopens_and_reuses_the_package_key() {
        let temp = tempfile::tempdir().unwrap();
        let database = captured_database(temp.path()).await;
        let key_root = temp.path().join("authority");
        let store = isolated_store(database.path(), &key_root);
        let first = store
            .package_local_capture(&database, temp.path(), [0x64; 32])
            .await
            .unwrap();
        drop(store);
        drop(database);

        let database = Database::open(&temp.path().join("source.sqlite"))
            .await
            .unwrap();
        let reopened = isolated_store(database.path(), &key_root);
        fs::remove_file(reopened.marker_path()).unwrap();
        let retry = reopened
            .package_local_capture(&database, temp.path(), [0x64; 32])
            .await
            .unwrap();
        assert_eq!(first, retry);
        assert!(reopened.marker_path().exists());
        let key_path = match &reopened.backend {
            Backend::File(backend) => backend.path.clone(),
            _ => unreachable!(),
        };
        let encoded = Zeroizing::new(fs::read(key_path).unwrap());
        let raw_key = &encoded[72..104];
        for path in [
            database.path().to_path_buf(),
            PathBuf::from(format!("{}-wal", database.path().display())),
        ] {
            match fs::read(path) {
                Ok(bytes) => assert!(!bytes.windows(raw_key.len()).any(|window| window == raw_key)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => panic!("cannot inspect database bytes: {error}"),
            }
        }
        let exported = serde_json::to_vec(
            &database
                .export_data("2026-09-22T00:00:00Z".into())
                .await
                .unwrap(),
        )
        .unwrap();
        assert!(
            !exported
                .windows(raw_key.len())
                .any(|window| window == raw_key)
        );

        let backup = temp.path().join("backup.tar.zst");
        database
            .create_backup_archive(&temp.path().join("blobs"), &backup)
            .await
            .unwrap();

        let target_dir = tempfile::tempdir().unwrap();
        let target = Database::open(&target_dir.path().join("target.sqlite"))
            .await
            .unwrap();
        let protected = reopened.load_required().unwrap();
        target
            .decrypt_and_install_local_shared_state_package(&retry, protected.package_key())
            .await
            .unwrap();
        let installed = target
            .export_data("2026-09-22T00:00:00Z".into())
            .await
            .unwrap();
        assert_eq!(installed.tables.tasks[0].title, "protected package");

        database
            .cancel_local_shared_state_never_dispatched(first.candidate_id())
            .await
            .unwrap();
        database
            .create_backup_archive(&temp.path().join("blobs"), &backup)
            .await
            .unwrap();
        let decoder = zstd::Decoder::new(File::open(backup).unwrap()).unwrap();
        let mut archive = tar::Archive::new(decoder);
        for entry in archive.entries().unwrap() {
            let mut entry = entry.unwrap();
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            assert!(!bytes.windows(raw_key.len()).any(|window| window == raw_key));
        }
    }

    #[tokio::test]
    async fn existing_package_never_regenerates_destroyed_authority() {
        let temp = tempfile::tempdir().unwrap();
        let database = captured_database(temp.path()).await;
        let key_root = temp.path().join("authority");
        let store = isolated_store(database.path(), &key_root);
        store
            .package_local_capture(&database, temp.path(), [0x64; 32])
            .await
            .unwrap();
        let key_path = match &store.backend {
            Backend::File(backend) => backend.path.clone(),
            _ => unreachable!(),
        };
        fs::remove_file(&key_path).unwrap();
        fs::remove_file(store.marker_path()).unwrap();

        let error = store
            .package_local_capture(&database, temp.path(), [0x64; 32])
            .await
            .unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<ProtectedLocalKeyStoreError>()
                .unwrap()
                .kind(),
            ProtectedLocalKeyStoreErrorKind::MissingAuthority
        );
        assert!(!key_path.exists());
        assert!(!store.marker_path().exists());
    }

    #[tokio::test]
    async fn package_store_rejects_a_different_database_before_storage_effects() {
        let first_root = tempfile::tempdir().unwrap();
        let first = Database::open(&first_root.path().join("first.sqlite"))
            .await
            .unwrap();
        let second_root = tempfile::tempdir().unwrap();
        let second = captured_database(second_root.path()).await;
        let key_root = first_root.path().join("authority");
        let store = isolated_store(first.path(), &key_root);

        let error = store
            .package_local_capture(&second, second_root.path(), [0x64; 32])
            .await
            .unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<ProtectedLocalKeyStoreError>()
                .unwrap()
                .kind(),
            ProtectedLocalKeyStoreErrorKind::WrongDatabase
        );
        assert!(!key_root.exists());
    }

    #[test]
    fn missing_and_corrupt_authority_never_regenerate() {
        let temp = tempfile::tempdir().unwrap();
        let database_path = temp.path().join("database.sqlite");
        File::create(&database_path).unwrap();
        let root = temp.path().join("authority");
        let store = isolated_store(&database_path, &root);
        let original = store.load_or_create().unwrap().context();
        let key_path = match &store.backend {
            Backend::File(backend) => backend.path.clone(),
            _ => unreachable!(),
        };
        fs::remove_file(&key_path).unwrap();

        let error = store.load_or_create().unwrap_err();
        assert_eq!(
            error.kind(),
            ProtectedLocalKeyStoreErrorKind::MissingAuthority
        );
        assert!(!key_path.exists());

        fs::remove_file(store.marker_path()).unwrap();
        fs::write(&key_path, vec![0_u8; KEYRING_BYTES]).unwrap();
        let mut permissions = fs::metadata(&key_path).unwrap().permissions();
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o600);
        fs::set_permissions(&key_path, permissions).unwrap();
        let error = store.load_or_create().unwrap_err();
        assert_eq!(error.kind(), ProtectedLocalKeyStoreErrorKind::Corrupt);
        assert_ne!(
            store.load_or_create().unwrap_err().kind(),
            ProtectedLocalKeyStoreErrorKind::WriteFailed
        );
        let _ = original;
    }

    #[test]
    fn unavailable_and_failed_writes_do_not_create_authority() {
        let temp = tempfile::tempdir().unwrap();
        let database_path = temp.path().join("database.sqlite");
        File::create(&database_path).unwrap();
        for (backend, expected) in [
            (
                Backend::Unavailable,
                ProtectedLocalKeyStoreErrorKind::Unavailable,
            ),
            (
                Backend::FailWrite,
                ProtectedLocalKeyStoreErrorKind::WriteFailed,
            ),
        ] {
            let root = temp.path().join(format!("authority-{expected:?}"));
            let store = ProtectedLocalKeyStore {
                account: database_account(&database_path.canonicalize().unwrap()),
                directory: root,
                backend,
            };
            let error = store.load_or_create().unwrap_err();
            assert_eq!(error.kind(), expected);
            assert!(!store.marker_path().exists());
        }
    }

    #[test]
    fn incomplete_marker_write_recovers_existing_key_without_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let database_path = temp.path().join("database.sqlite");
        File::create(&database_path).unwrap();
        let root = temp.path().join("authority");
        let store = isolated_store(&database_path, &root);
        prepare_directory(&root).unwrap();
        let key = generate_keyring().unwrap();
        let encoded = encode_keyring(&key);
        store.backend.create(&encoded).unwrap();

        let recovered = store.load_or_create().unwrap();
        assert_eq!(recovered.context(), key.context());
        assert!(store.marker_path().exists());
    }

    #[test]
    fn unsafe_file_permissions_fail_closed_without_secret_diagnostics() {
        let temp = tempfile::tempdir().unwrap();
        let database_path = temp.path().join("database.sqlite");
        File::create(&database_path).unwrap();
        let root = temp.path().join("authority");
        let store = isolated_store(&database_path, &root);
        let protected = store.load_or_create().unwrap();
        let key_path = match &store.backend {
            Backend::File(backend) => backend.path.clone(),
            _ => unreachable!(),
        };
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&key_path, fs::Permissions::from_mode(0o644)).unwrap();

        let error = store.load_required().unwrap_err();
        assert_eq!(error.kind(), ProtectedLocalKeyStoreErrorKind::Corrupt);
        let diagnostic = format!("{error:?} {error}");
        assert!(!diagnostic.contains(&hex::encode(protected.context().vault_id)));
        assert!(!diagnostic.contains(database_path.to_string_lossy().as_ref()));
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "uses an isolated macOS Keychain item"]
    fn isolated_macos_keychain_smoke_test() {
        let temp = tempfile::tempdir().unwrap();
        let database_path = temp.path().join("database.sqlite");
        File::create(&database_path).unwrap();
        let directory = temp.path().join("markers");
        let account = format!("aven-test-{}", std::process::id());
        let backend = KeychainBackend {
            service: format!("fi.zendit.Aven.tests.{}", account),
            account: account.clone(),
        };
        backend.delete();
        let store = ProtectedLocalKeyStore {
            account,
            directory,
            backend: Backend::Keychain(backend),
        };
        let first = store.load_or_create().unwrap().context();
        assert_eq!(store.load_required().unwrap().context(), first);
        let Backend::Keychain(backend) = &store.backend else {
            unreachable!();
        };
        backend.delete();
    }
}
