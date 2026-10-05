//! Desktop persistence for the core protected local key store.
//!
//! On macOS one non-synchronizing login Keychain key encrypts the secret files
//! for each database. Linux stores those secrets directly in owner-only files.
//! Markers, evidence and the store lock are owner-only files below the
//! application state directory on both. Other platforms are unsupported.
pub use aven_core::sync::client::keys::{
    EnrollmentReadiness, ProtectedLocalKeyStore, ProtectedLocalKeyStoreError,
    ProtectedLocalKeyStoreErrorKind, peer, rotation,
};

#[cfg(test)]
use std::path::PathBuf;
use std::sync::Arc;

use aven_core::sync::client::keys::{FileProtectedStorage, ProtectedStorage, StoreResult};

#[cfg(not(test))]
const STORE_DIRECTORY: &str = "protected-keys";
#[cfg(target_os = "macos")]
#[cfg(not(test))]
const KEYCHAIN_SERVICE: &str = "fi.zendit.Aven.local-package-keyring";

/// Whether protected storage may ask an attended macOS user for Keychain access.
#[derive(Clone, Copy)]
pub enum KeychainInteraction {
    /// Allow Keychain authorization UI.
    Allow,
    /// Return an unavailable-storage error instead of displaying UI.
    Deny,
}

/// This installation's protected key storage.
pub fn storage(interaction: KeychainInteraction) -> StoreResult<Arc<dyn ProtectedStorage>> {
    #[cfg(not(test))]
    return production_storage(protected_store_directory()?, interaction);
    #[cfg(test)]
    let _ = interaction;
    // Tests never reach the login Keychain; CLI test workers name isolated files.
    #[cfg(test)]
    Ok(Arc::new(FileProtectedStorage::new(
        std::env::var_os("AVEN_TEST_PROTECTED_KEYS")
            .map(PathBuf::from)
            .ok_or_else(|| error(ProtectedLocalKeyStoreErrorKind::Unavailable))?,
    )))
}

fn error(kind: ProtectedLocalKeyStoreErrorKind) -> ProtectedLocalKeyStoreError {
    ProtectedLocalKeyStoreError::new(kind)
}

#[cfg(not(test))]
fn protected_store_directory() -> StoreResult<std::path::PathBuf> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| dirs::home_dir().map(|home| home.join(".local/state")))
        .ok_or_else(|| error(ProtectedLocalKeyStoreErrorKind::Unavailable))?;
    Ok(state.join("aven").join(STORE_DIRECTORY))
}

#[cfg(not(test))]
fn production_storage(
    directory: std::path::PathBuf,
    interaction: KeychainInteraction,
) -> StoreResult<Arc<dyn ProtectedStorage>> {
    #[cfg(target_os = "macos")]
    {
        Ok(Arc::new(keychain::KeychainStorage::new(
            KEYCHAIN_SERVICE.to_string(),
            FileProtectedStorage::new(directory),
            interaction,
        )))
    }
    #[cfg(target_os = "linux")]
    {
        let _ = interaction;
        Ok(Arc::new(FileProtectedStorage::new(directory)))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = directory;
        Err(error(ProtectedLocalKeyStoreErrorKind::UnsupportedPlatform))
    }
}

#[cfg(target_os = "macos")]
mod keychain {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use std::sync::Mutex;

    use aven_core::sync::client::keys::{ProtectedStorage, ProtectedStorageLock};
    use chacha20poly1305::aead::array::Array;
    use chacha20poly1305::aead::{Aead, KeyInit, Payload};
    use chacha20poly1305::{XChaCha20Poly1305, XNonce};
    use core_foundation::data::CFData;
    use security_framework::os::macos::keychain::SecKeychain;
    use zeroize::Zeroizing;

    static KEYCHAIN_INTERACTION: Mutex<()> = Mutex::new(());
    const MASTER_KEY_BYTES: usize = 32;
    const WRAPPED_MAGIC: &[u8; 8] = b"AVENKCS1";
    const NONCE_BYTES: usize = 24;
    const TAG_BYTES: usize = 16;
    const WRAPPED_OVERHEAD: usize = WRAPPED_MAGIC.len() + NONCE_BYTES + TAG_BYTES;
    const WRAPPED_PREFIX: &str = "wrapped-secret-";

    /// One generic password per database holds a wrapping key. Secret values are
    /// encrypted into owner-only files; nonsecret records stay in the same
    /// directory without the wrapped-secret prefix.
    pub(super) struct KeychainStorage {
        service: String,
        files: FileProtectedStorage,
        interaction: KeychainInteraction,
        cached_keys: Mutex<HashMap<String, Zeroizing<Vec<u8>>>>,
    }

    impl KeychainStorage {
        pub(super) fn new(
            service: String,
            files: FileProtectedStorage,
            interaction: KeychainInteraction,
        ) -> Self {
            Self {
                service,
                files,
                interaction,
                cached_keys: Mutex::new(HashMap::new()),
            }
        }

        fn options(&self, namespace: &str) -> security_framework::passwords::PasswordOptions {
            use security_framework::passwords::PasswordOptions;

            let mut options = PasswordOptions::new_generic_password(&self.service, namespace);
            options.set_access_synchronized(Some(false));
            options
        }

        /// Keychain ACL checks may display UI only for an attended foreground command.
        fn keychain_operation<T>(
            &self,
            operation: impl FnOnce() -> security_framework::base::Result<T>,
        ) -> StoreResult<security_framework::base::Result<T>> {
            let _process_guard = KEYCHAIN_INTERACTION
                .lock()
                .map_err(|_| error(ProtectedLocalKeyStoreErrorKind::Unavailable))?;
            match self.interaction {
                KeychainInteraction::Allow => Ok(operation()),
                KeychainInteraction::Deny => {
                    let _interaction_guard =
                        SecKeychain::disable_user_interaction().map_err(|source| {
                            tracing::warn!(
                                phase = "keychain_disable_ui",
                                os_status = source.code(),
                                "protected storage unavailable"
                            );
                            error(ProtectedLocalKeyStoreErrorKind::Unavailable)
                        })?;
                    Ok(operation())
                }
            }
        }

        fn load_keychain_key(&self, namespace: &str) -> StoreResult<Option<Zeroizing<Vec<u8>>>> {
            use security_framework::passwords::generic_password;
            use security_framework_sys::base::errSecItemNotFound;

            match self.keychain_operation(|| generic_password(self.options(namespace)))? {
                Ok(bytes) if bytes.len() == MASTER_KEY_BYTES => Ok(Some(Zeroizing::new(bytes))),
                Ok(_) => Err(error(ProtectedLocalKeyStoreErrorKind::Corrupt)),
                Err(source) if source.code() == errSecItemNotFound => Ok(None),
                Err(source) => {
                    tracing::warn!(
                        phase = "keychain_read",
                        os_status = source.code(),
                        "protected storage unavailable"
                    );
                    Err(error(ProtectedLocalKeyStoreErrorKind::Unavailable))
                }
            }
        }

        fn cached_key(&self, namespace: &str) -> StoreResult<Option<Zeroizing<Vec<u8>>>> {
            let cached = self
                .cached_keys
                .lock()
                .map_err(|_| error(ProtectedLocalKeyStoreErrorKind::Unavailable))?
                .get(namespace)
                .cloned();
            if cached.is_some() {
                return Ok(cached);
            }
            let Some(key) = self.load_keychain_key(namespace)? else {
                return Ok(None);
            };
            self.cache_key(namespace, key.clone())?;
            Ok(Some(key))
        }

        fn cache_key(&self, namespace: &str, key: Zeroizing<Vec<u8>>) -> StoreResult<()> {
            self.cached_keys
                .lock()
                .map_err(|_| error(ProtectedLocalKeyStoreErrorKind::Unavailable))?
                .insert(namespace.to_string(), key);
            Ok(())
        }

        fn create_key(&self, namespace: &str) -> StoreResult<Zeroizing<Vec<u8>>> {
            use security_framework::item::{ItemAddOptions, ItemAddValue, ItemClass};
            use security_framework_sys::base::errSecDuplicateItem;

            if let Some(key) = self.cached_key(namespace)? {
                return Ok(key);
            }
            if !self.files.list_records(namespace)?.is_empty() {
                return Err(error(ProtectedLocalKeyStoreErrorKind::MissingAuthority));
            }
            let mut key = Zeroizing::new(vec![0_u8; MASTER_KEY_BYTES]);
            getrandom::fill(&mut key)
                .map_err(|_| error(ProtectedLocalKeyStoreErrorKind::Unavailable))?;
            let mut options = ItemAddOptions::new(ItemAddValue::Data {
                class: ItemClass::generic_password(),
                data: CFData::from_buffer(&key),
            });
            options
                .set_service(&self.service)
                .set_account_name(namespace);
            match self.keychain_operation(|| options.add())? {
                Ok(()) => {}
                Err(source) if source.code() == errSecDuplicateItem => {}
                Err(source) => {
                    tracing::warn!(
                        phase = "keychain_add",
                        os_status = source.code(),
                        "protected storage write failed"
                    );
                    return Err(error(ProtectedLocalKeyStoreErrorKind::WriteFailed));
                }
            }
            let saved = self.load_keychain_key(namespace)?.ok_or_else(|| {
                tracing::warn!(
                    phase = "keychain_add_readback_missing",
                    "protected storage write failed"
                );
                error(ProtectedLocalKeyStoreErrorKind::WriteFailed)
            })?;
            self.cache_key(namespace, saved.clone())?;
            Ok(saved)
        }

        fn delete_key(&self, namespace: &str) -> StoreResult<()> {
            use security_framework::passwords::delete_generic_password_options;
            use security_framework_sys::base::errSecItemNotFound;

            match self
                .keychain_operation(|| delete_generic_password_options(self.options(namespace)))?
            {
                Ok(()) => {}
                Err(source) if source.code() == errSecItemNotFound => {}
                Err(_) => return Err(error(ProtectedLocalKeyStoreErrorKind::WriteFailed)),
            }
            self.cached_keys
                .lock()
                .map_err(|_| error(ProtectedLocalKeyStoreErrorKind::Unavailable))?
                .remove(namespace);
            Ok(())
        }

        fn wrapped_name(item: &str) -> String {
            format!("{WRAPPED_PREFIX}{}", hex::encode(item))
        }

        fn wrapped_item(name: &str) -> StoreResult<Option<String>> {
            let Some(encoded) = name.strip_prefix(WRAPPED_PREFIX) else {
                return Ok(None);
            };
            let bytes = hex::decode(encoded)
                .map_err(|_| error(ProtectedLocalKeyStoreErrorKind::Corrupt))?;
            String::from_utf8(bytes)
                .map(Some)
                .map_err(|_| error(ProtectedLocalKeyStoreErrorKind::Corrupt))
        }

        fn aad(namespace: &str, item: &str) -> Vec<u8> {
            let mut aad = b"aven-macos-keychain-secret-v1\0".to_vec();
            aad.extend_from_slice(namespace.as_bytes());
            aad.push(0);
            aad.extend_from_slice(item.as_bytes());
            aad
        }

        fn encrypt(
            &self,
            namespace: &str,
            item: &str,
            key: &[u8],
            bytes: &[u8],
        ) -> StoreResult<Zeroizing<Vec<u8>>> {
            let mut nonce = [0_u8; NONCE_BYTES];
            getrandom::fill(&mut nonce)
                .map_err(|_| error(ProtectedLocalKeyStoreErrorKind::Unavailable))?;
            let key: &[u8; MASTER_KEY_BYTES] = key
                .try_into()
                .map_err(|_| error(ProtectedLocalKeyStoreErrorKind::Corrupt))?;
            let cipher = XChaCha20Poly1305::new(&Array(*key));
            let ciphertext = cipher
                .encrypt(
                    &XNonce::from(nonce),
                    Payload {
                        msg: bytes,
                        aad: &Self::aad(namespace, item),
                    },
                )
                .map_err(|_| error(ProtectedLocalKeyStoreErrorKind::WriteFailed))?;
            let mut wrapped = Zeroizing::new(Vec::with_capacity(WRAPPED_OVERHEAD + bytes.len()));
            wrapped.extend_from_slice(WRAPPED_MAGIC);
            wrapped.extend_from_slice(&nonce);
            wrapped.extend_from_slice(&ciphertext);
            Ok(wrapped)
        }

        fn decrypt(
            &self,
            namespace: &str,
            item: &str,
            key: &[u8],
            wrapped: &[u8],
        ) -> StoreResult<Zeroizing<Vec<u8>>> {
            if wrapped.len() < WRAPPED_OVERHEAD || &wrapped[..WRAPPED_MAGIC.len()] != WRAPPED_MAGIC
            {
                return Err(error(ProtectedLocalKeyStoreErrorKind::Corrupt));
            }
            let key: &[u8; MASTER_KEY_BYTES] = key
                .try_into()
                .map_err(|_| error(ProtectedLocalKeyStoreErrorKind::Corrupt))?;
            let nonce_offset = WRAPPED_MAGIC.len();
            let nonce: [u8; NONCE_BYTES] = wrapped[nonce_offset..nonce_offset + NONCE_BYTES]
                .try_into()
                .map_err(|_| error(ProtectedLocalKeyStoreErrorKind::Corrupt))?;
            let cipher = XChaCha20Poly1305::new(&Array(*key));
            cipher
                .decrypt(
                    &XNonce::from(nonce),
                    Payload {
                        msg: &wrapped[nonce_offset + NONCE_BYTES..],
                        aad: &Self::aad(namespace, item),
                    },
                )
                .map(Zeroizing::new)
                .map_err(|_| error(ProtectedLocalKeyStoreErrorKind::Corrupt))
        }

        fn wrapped_items(&self, namespace: &str) -> StoreResult<HashSet<String>> {
            self.files
                .list_secrets(namespace)?
                .into_iter()
                .filter_map(|name| Self::wrapped_item(&name).transpose())
                .collect()
        }

        #[cfg(test)]
        fn keychain_services(&self, namespace: &str) -> StoreResult<HashSet<String>> {
            use security_framework::item::{CloudSync, ItemClass, ItemSearchOptions, Limit};
            use security_framework_sys::base::errSecItemNotFound;

            let results = match self.keychain_operation(|| {
                ItemSearchOptions::new()
                    .class(ItemClass::generic_password())
                    .account(namespace)
                    .cloud_sync(CloudSync::MatchSyncNo)
                    .load_attributes(true)
                    .limit(Limit::All)
                    .search()
            })? {
                Ok(results) => results,
                Err(source) if source.code() == errSecItemNotFound => {
                    return Ok(HashSet::new());
                }
                Err(_) => return Err(error(ProtectedLocalKeyStoreErrorKind::Unavailable)),
            };
            results
                .into_iter()
                .filter_map(|result| {
                    result
                        .simplify_dict()
                        .and_then(|attributes| attributes.get("svce").cloned())
                        .filter(|service| service == &self.service)
                        .map(Ok)
                })
                .collect()
        }
    }

    impl ProtectedStorage for KeychainStorage {
        fn prepare(&self, namespace: &str) -> StoreResult<()> {
            self.files.prepare(namespace)
        }

        fn lock(&self, namespace: &str) -> StoreResult<ProtectedStorageLock> {
            self.files.lock(namespace)
        }

        fn load_secret(
            &self,
            namespace: &str,
            item: &str,
            expected_len: usize,
        ) -> StoreResult<Option<Zeroizing<Vec<u8>>>> {
            let Some(wrapped) = self.files.load_secret(
                namespace,
                &Self::wrapped_name(item),
                expected_len + WRAPPED_OVERHEAD,
            )?
            else {
                return Ok(None);
            };
            let key = self
                .cached_key(namespace)?
                .ok_or_else(|| error(ProtectedLocalKeyStoreErrorKind::MissingAuthority))?;
            let bytes = self.decrypt(namespace, item, &key, &wrapped)?;
            if bytes.len() != expected_len {
                return Err(error(ProtectedLocalKeyStoreErrorKind::Corrupt));
            }
            Ok(Some(bytes))
        }

        fn create_secret(&self, namespace: &str, item: &str, bytes: &[u8]) -> StoreResult<()> {
            let key = self.create_key(namespace)?;
            let wrapped = self.encrypt(namespace, item, &key, bytes)?;
            self.files
                .create_secret(namespace, &Self::wrapped_name(item), &wrapped)?;
            match self.load_secret(namespace, item, bytes.len())? {
                Some(saved) if saved.as_slice() == bytes => Ok(()),
                _ => {
                    tracing::warn!(
                        phase = "wrapped_secret_readback_mismatch",
                        "protected storage write failed"
                    );
                    Err(error(ProtectedLocalKeyStoreErrorKind::WriteFailed))
                }
            }
        }

        fn delete_secret(&self, namespace: &str, item: &str) -> StoreResult<()> {
            let wrapped_items = self.wrapped_items(namespace)?;
            if wrapped_items.len() == 1 && wrapped_items.contains(item) {
                self.delete_key(namespace)?;
            }
            self.files
                .delete_secret(namespace, &Self::wrapped_name(item))
        }

        fn read_record(
            &self,
            namespace: &str,
            name: &str,
            expected_len: usize,
        ) -> StoreResult<Option<Vec<u8>>> {
            self.files.read_record(namespace, name, expected_len)
        }

        fn create_record(&self, namespace: &str, name: &str, bytes: &[u8]) -> StoreResult<()> {
            self.files.create_record(namespace, name, bytes)
        }

        fn remove_record(&self, namespace: &str, name: &str) -> StoreResult<()> {
            self.files.remove_record(namespace, name)
        }

        fn list_secrets(&self, namespace: &str) -> StoreResult<HashSet<String>> {
            self.wrapped_items(namespace)
        }

        fn list_records(&self, namespace: &str) -> StoreResult<HashSet<String>> {
            Ok(self
                .files
                .list_records(namespace)?
                .into_iter()
                .filter(|name| !name.starts_with(WRAPPED_PREFIX))
                .collect())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use aven_core::db::Database;

        struct Cleanup(KeychainStorage, String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                if let Ok(items) = self.0.list_secrets(&self.1) {
                    for item in items {
                        let _ = self.0.delete_secret(&self.1, &item);
                    }
                }
            }
        }

        async fn isolated(database: &Database, root: &std::path::Path) -> ProtectedLocalKeyStore {
            let id = database.meta("client_id").await.unwrap().unwrap();
            let storage = Arc::new(KeychainStorage::new(
                format!("fi.zendit.Aven.tests.{id}"),
                FileProtectedStorage::new(root.join("markers")),
                KeychainInteraction::Allow,
            ));
            ProtectedLocalKeyStore::open(database, storage)
                .await
                .unwrap()
        }

        #[tokio::test]
        #[ignore = "uses isolated macOS Keychain items"]
        async fn isolated_macos_keychain_smoke_test() {
            let temp = tempfile::tempdir().unwrap();
            let database = Database::open(&temp.path().join("database.sqlite"))
                .await
                .unwrap();
            let store = isolated(&database, temp.path()).await;
            let id = database.meta("client_id").await.unwrap().unwrap();
            let _cleanup = Cleanup(
                KeychainStorage::new(
                    format!("fi.zendit.Aven.tests.{id}"),
                    FileProtectedStorage::new(temp.path().join("markers")),
                    KeychainInteraction::Allow,
                ),
                store.account().to_string(),
            );
            let first = store.load_or_create().unwrap().context();
            assert_eq!(store.load_required().unwrap().context(), first);

            let storage = &_cleanup.0;
            let namespace = store.account();
            for item in ["listed-a", "listed-b"] {
                storage.create_secret(namespace, item, b"x").unwrap();
            }
            let wrapped = std::fs::read(
                storage
                    .files
                    .path(namespace, &KeychainStorage::wrapped_name("listed-a")),
            )
            .unwrap();
            assert_eq!(wrapped.len(), WRAPPED_OVERHEAD + 1);
            assert_eq!(&wrapped[..WRAPPED_MAGIC.len()], WRAPPED_MAGIC);
            assert_ne!(wrapped.as_slice(), b"x");
            assert_eq!(
                storage.keychain_services(namespace).unwrap(),
                HashSet::from([storage.service.clone()])
            );

            let other = format!("{namespace}-other");
            storage.create_secret(&other, "listed-other", b"x").unwrap();
            assert_eq!(
                storage.keychain_services(&other).unwrap(),
                HashSet::from([storage.service.clone()])
            );
            let listed = storage.list_secrets(namespace);
            let wrapped_path = storage
                .files
                .path(namespace, &KeychainStorage::wrapped_name("listed-a"));
            let mut tampered = std::fs::read(&wrapped_path).unwrap();
            *tampered.last_mut().unwrap() ^= 1;
            std::fs::write(&wrapped_path, tampered).unwrap();
            assert_eq!(
                storage
                    .load_secret(namespace, "listed-a", 1)
                    .unwrap_err()
                    .kind(),
                ProtectedLocalKeyStoreErrorKind::Corrupt
            );
            storage.delete_secret(namespace, "listed-a").unwrap();
            let after_delete = storage.list_secrets(namespace);
            storage.delete_secret(namespace, "listed-b").unwrap();
            storage.delete_secret(&other, "listed-other").unwrap();
            assert_eq!(
                listed.unwrap(),
                HashSet::from(["keyring", "listed-a", "listed-b"].map(String::from))
            );
            assert_eq!(
                after_delete.unwrap(),
                HashSet::from(["keyring", "listed-b"].map(String::from))
            );

            storage.delete_key(namespace).unwrap();
            assert_eq!(
                storage
                    .create_secret(namespace, "replacement", b"x")
                    .unwrap_err()
                    .kind(),
                ProtectedLocalKeyStoreErrorKind::MissingAuthority
            );
            assert!(storage.keychain_services(namespace).unwrap().is_empty());
            let reopened = isolated(&database, temp.path()).await;
            let error = reopened.load_required().unwrap_err();
            assert_eq!(
                error.kind(),
                ProtectedLocalKeyStoreErrorKind::MissingAuthority
            );
        }

        #[tokio::test]
        #[ignore = "uses one unique test-only macOS Keychain item and temporary files"]
        async fn isolated_seed_keychain_reopen() {
            let temp = tempfile::tempdir().unwrap();
            let db = Database::open(&temp.path().join("db.sqlite"))
                .await
                .unwrap();
            let store = isolated(&db, temp.path()).await;
            let id = db.meta("client_id").await.unwrap().unwrap();
            let cleanup = Cleanup(
                KeychainStorage::new(
                    format!("fi.zendit.Aven.tests.{id}"),
                    FileProtectedStorage::new(temp.path().join("markers")),
                    KeychainInteraction::Allow,
                ),
                store.account().to_string(),
            );
            let first = store.prepare_seed_claim(&db, [9; 32]).await.unwrap();
            let reopened = store.prepare_seed_claim(&db, [9; 32]).await.unwrap();
            assert_eq!(
                first.protected_storage_bytes(),
                reopened.protected_storage_bytes()
            );
            store.prepare_seed_source(&db).await.unwrap();
            cleanup.0.delete_secret(&cleanup.1, "seed").unwrap();
            assert!(store.prepare_seed_claim(&db, [9; 32]).await.is_err());
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    pub(crate) use aven_core::sync::client::keys::test_support::{BACKEND_LOADS, isolated_store};
}
