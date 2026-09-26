//! Desktop persistence for the core protected local key store.
//!
//! On macOS key material lives in non-synchronizing login Keychain items; on
//! Linux in owner-only files below the application state directory. Markers,
//! evidence and the store lock are owner-only files in that directory on
//! both. Other platforms are unsupported.
pub use aven_core::sync::client::keys::{
    EnrollmentReadiness, ProtectedLocalKeyStore, ProtectedLocalKeyStoreError,
    ProtectedLocalKeyStoreErrorKind, peer, rotation,
};

#[cfg(test)]
use std::path::PathBuf;
use std::sync::Arc;

use aven_core::sync::client::keys::{FileProtectedStorage, ProtectedStorage, StoreResult};

#[cfg_attr(test, allow(dead_code))]
const STORE_DIRECTORY: &str = "protected-keys";
#[cfg(target_os = "macos")]
#[cfg_attr(test, allow(dead_code))]
const KEYCHAIN_SERVICE: &str = "fi.zendit.Aven.local-package-keyring";

/// This installation's protected key storage.
pub fn storage() -> StoreResult<Arc<dyn ProtectedStorage>> {
    #[cfg(not(test))]
    return production_storage(protected_store_directory()?);
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

#[cfg_attr(test, allow(dead_code))]
fn protected_store_directory() -> StoreResult<std::path::PathBuf> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| dirs::home_dir().map(|home| home.join(".local/state")))
        .ok_or_else(|| error(ProtectedLocalKeyStoreErrorKind::Unavailable))?;
    Ok(state.join("aven").join(STORE_DIRECTORY))
}

#[cfg(not(test))]
fn production_storage(directory: std::path::PathBuf) -> StoreResult<Arc<dyn ProtectedStorage>> {
    #[cfg(target_os = "macos")]
    {
        Ok(Arc::new(keychain::KeychainStorage::new(
            KEYCHAIN_SERVICE.to_string(),
            FileProtectedStorage::new(directory),
        )))
    }
    #[cfg(target_os = "linux")]
    {
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
    use std::collections::HashSet;

    use aven_core::sync::client::keys::{KEYRING_ITEM, ProtectedStorage, ProtectedStorageLock};
    use zeroize::Zeroizing;

    /// Secret items are generic passwords named by service and namespace:
    /// the keyring uses `service`, every other item `service.item`.
    /// Everything nonsecret stays in `files`.
    pub(super) struct KeychainStorage {
        service: String,
        files: FileProtectedStorage,
    }

    impl KeychainStorage {
        pub(super) fn new(service: String, files: FileProtectedStorage) -> Self {
            Self { service, files }
        }

        fn options(
            &self,
            namespace: &str,
            item: &str,
        ) -> security_framework::passwords::PasswordOptions {
            use security_framework::passwords::PasswordOptions;

            let service = if item == KEYRING_ITEM {
                self.service.clone()
            } else {
                format!("{}.{item}", self.service)
            };
            let mut options = PasswordOptions::new_generic_password(&service, namespace);
            options.set_access_synchronized(Some(false));
            options
        }

        fn load(&self, namespace: &str, item: &str) -> StoreResult<Option<Zeroizing<Vec<u8>>>> {
            use security_framework::passwords::generic_password;
            use security_framework_sys::base::errSecItemNotFound;

            match generic_password(self.options(namespace, item)) {
                Ok(bytes) => Ok(Some(Zeroizing::new(bytes))),
                Err(source) if source.code() == errSecItemNotFound => Ok(None),
                Err(_) => Err(error(ProtectedLocalKeyStoreErrorKind::Unavailable)),
            }
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
            let bytes = self.load(namespace, item)?;
            if bytes
                .as_ref()
                .is_some_and(|bytes| bytes.len() != expected_len)
            {
                return Err(error(ProtectedLocalKeyStoreErrorKind::Corrupt));
            }
            Ok(bytes)
        }

        fn create_secret(&self, namespace: &str, item: &str, bytes: &[u8]) -> StoreResult<()> {
            use security_framework::passwords::set_generic_password_options;

            // The caller has already observed absence. A process guard around this
            // operation serializes cooperating Aven hosts. Re-read before success.
            match set_generic_password_options(bytes, self.options(namespace, item)) {
                Ok(()) => match self.load(namespace, item)? {
                    Some(saved) if saved.as_slice() == bytes => Ok(()),
                    _ => Err(error(ProtectedLocalKeyStoreErrorKind::WriteFailed)),
                },
                Err(_) => Err(error(ProtectedLocalKeyStoreErrorKind::WriteFailed)),
            }
        }

        fn delete_secret(&self, namespace: &str, item: &str) -> StoreResult<()> {
            use security_framework::passwords::delete_generic_password_options;
            use security_framework_sys::base::errSecItemNotFound;

            match delete_generic_password_options(self.options(namespace, item)) {
                Ok(()) => Ok(()),
                Err(source) if source.code() == errSecItemNotFound => Ok(()),
                Err(_) => Err(error(ProtectedLocalKeyStoreErrorKind::WriteFailed)),
            }
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

        /// Items of this service under `namespace`. The query scope matches
        /// `options()` except for the service, which is filtered by prefix.
        fn list_secrets(&self, namespace: &str) -> StoreResult<HashSet<String>> {
            use security_framework::item::{CloudSync, ItemClass, ItemSearchOptions, Limit};
            use security_framework_sys::base::errSecItemNotFound;

            let results = match ItemSearchOptions::new()
                .class(ItemClass::generic_password())
                .account(namespace)
                .cloud_sync(CloudSync::MatchSyncNo)
                .load_attributes(true)
                .limit(Limit::All)
                .search()
            {
                Ok(results) => results,
                Err(source) if source.code() == errSecItemNotFound => {
                    return Ok(HashSet::new());
                }
                Err(_) => return Err(error(ProtectedLocalKeyStoreErrorKind::Unavailable)),
            };
            let prefix = format!("{}.", self.service);
            let mut items = HashSet::new();
            for result in results {
                let attributes = result
                    .simplify_dict()
                    .ok_or_else(|| error(ProtectedLocalKeyStoreErrorKind::Unavailable))?;
                match attributes.get("svce") {
                    Some(service) if *service == self.service => {
                        items.insert(KEYRING_ITEM.to_string());
                    }
                    Some(service) => {
                        if let Some(item) = service.strip_prefix(&prefix) {
                            items.insert(item.to_string());
                        }
                    }
                    None => {}
                }
            }
            Ok(items)
        }

        fn list_records(&self, namespace: &str) -> StoreResult<HashSet<String>> {
            self.files.list_records(namespace)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use aven_core::db::Database;

        struct Cleanup(KeychainStorage, String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = self.0.delete_secret(&self.1, KEYRING_ITEM);
                let _ = self.0.delete_secret(&self.1, "seed");
            }
        }

        async fn isolated(database: &Database, root: &std::path::Path) -> ProtectedLocalKeyStore {
            let id = database.meta("client_id").await.unwrap().unwrap();
            let storage = Arc::new(KeychainStorage::new(
                format!("fi.zendit.Aven.tests.{id}"),
                FileProtectedStorage::new(root.join("markers")),
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
            let other = format!("{namespace}-other");
            storage.create_secret(&other, "listed-other", b"x").unwrap();
            let listed = storage.list_secrets(namespace);
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
        }

        #[tokio::test]
        #[ignore = "uses two unique test-only macOS Keychain items and temporary markers"]
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
                ),
                store.account().to_string(),
            );
            let first = store.prepare_seed_claim(&db, [9; 32]).await.unwrap();
            let reopened = store.prepare_seed_claim(&db, [9; 32]).await.unwrap();
            assert_eq!(
                first.protected_storage_bytes(),
                reopened.protected_storage_bytes()
            );
            cleanup.0.delete_secret(&cleanup.1, "seed").unwrap();
            assert!(store.prepare_seed_claim(&db, [9; 32]).await.is_err());
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    pub(crate) use aven_core::sync::client::keys::test_support::{BACKEND_LOADS, isolated_store};
}
