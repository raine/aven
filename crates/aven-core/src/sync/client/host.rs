//! What engine operations need from their host besides HTTP.
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;

use super::keys::{ProtectedLocalKeyStore, ProtectedStorage, StoreResult};
use crate::db::Database;

pub trait ClientHost: Send + Sync {
    /// Refuses sync while the host's policy disables it.
    fn ensure_sync_allowed(&self) -> Result<()>;

    /// Storage for protected keys. Called only when an operation opens the
    /// key store, so databases that don't take part in sync never need it.
    fn protected_storage(&self) -> StoreResult<Arc<dyn ProtectedStorage>>;

    /// The local image blob root of `database`.
    fn blob_dir(&self, database: &Database) -> Result<PathBuf>;

    /// A name other devices show for this one, such as the computer name.
    /// Asked for only while this device has no published label; control
    /// characters are dropped and the length is bounded.
    fn device_label(&self) -> Option<String>;
}

pub(crate) async fn key_store(
    host: &dyn ClientHost,
    database: &Database,
) -> Result<ProtectedLocalKeyStore> {
    Ok(ProtectedLocalKeyStore::open(database, host.protected_storage()?).await?)
}
