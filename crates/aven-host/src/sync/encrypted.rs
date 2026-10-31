//! The desktop host for end-to-end encrypted sync.
//!
//! The engine lives in `aven_core::sync::client::engine`; this module supplies
//! the desktop host (configuration policy, protected storage, blob paths and
//! the computer name) and drives engine sessions over HTTP.
use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use aven_core::db::Database;
use aven_core::sync::client::ClientHost;
use aven_core::sync::client::engine;
pub use aven_core::sync::client::engine::{
    Admission, Amount, AssociationStatus, Cancellation, DaemonRound, InvitationStatus, LocalPhase,
    Outcome, PendingInvitation, Progress, SetupPreview, Stage, StatusReport, SyncState,
    local_phase, unix_now,
};
use aven_core::sync::client::keys::{ProtectedStorage, StoreResult};
use aven_core::sync::client::tail::ImageTransfer;

use crate::config::{self, AppConfig};
use crate::sync_http::HttpDriver;

mod devices;
#[cfg(any(test, feature = "test-support"))]
pub use aven_core::sync::client::engine::ROUND_LIMIT;
#[cfg(any(test, feature = "test-support"))]
pub use aven_core::sync::client::invitation::sample_invitations;
pub use aven_core::sync::client::server_origin;
pub use aven_core::sync::client::{DeviceInvitation, InvitationCheck, SetupInvitation};
pub use devices::{
    Device, DeviceListing, Removal, finish_removal, list as list_devices, load_devices,
    remove as remove_device, remove_other_device,
};

/// The desktop's answers to what the engine asks of its host.
pub struct DesktopHost {
    pub config: AppConfig,
    keychain_interaction: crate::protected_local_keys::KeychainInteraction,
    protected_storage: Mutex<Option<Arc<dyn ProtectedStorage>>>,
}

impl DesktopHost {
    pub fn foreground(config: &AppConfig) -> Self {
        let attended = std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
        Self::new(
            config,
            if attended {
                crate::protected_local_keys::KeychainInteraction::Allow
            } else {
                crate::protected_local_keys::KeychainInteraction::Deny
            },
        )
    }

    fn background(config: &AppConfig) -> Self {
        Self::new(
            config,
            crate::protected_local_keys::KeychainInteraction::Deny,
        )
    }

    fn new(
        config: &AppConfig,
        keychain_interaction: crate::protected_local_keys::KeychainInteraction,
    ) -> Self {
        Self {
            config: config.clone(),
            keychain_interaction,
            protected_storage: Mutex::new(None),
        }
    }
}

impl ClientHost for DesktopHost {
    fn check_reachability<'a>(
        &'a self,
        server: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move { driver()?.check_reachability(server).await })
    }

    fn ensure_sync_allowed(&self) -> Result<()> {
        self.config.ensure_sync_allowed()
    }

    fn protected_storage(&self) -> StoreResult<Arc<dyn ProtectedStorage>> {
        let mut storage = self.protected_storage.lock().map_err(|_| {
            crate::protected_local_keys::ProtectedLocalKeyStoreError::new(
                crate::protected_local_keys::ProtectedLocalKeyStoreErrorKind::Unavailable,
            )
        })?;
        if let Some(storage) = storage.as_ref() {
            return Ok(storage.clone());
        }
        let opened = crate::protected_local_keys::storage(self.keychain_interaction)?;
        *storage = Some(opened.clone());
        Ok(opened)
    }

    fn blob_dir(&self, database: &Database) -> Result<PathBuf> {
        config::resolve_blob_dir(database.path(), &self.config)
    }

    fn attachment_lifecycle_policy(&self) -> aven_core::attachments::LifecyclePolicy {
        self.config.local.attachment_lifecycle.policy()
    }

    fn device_label(&self) -> Option<String> {
        super::device_label::automatic_label()
    }
}

pub fn driver() -> Result<HttpDriver> {
    HttpDriver::new()
}

pub async fn setup_preview(database: &Database, config: &AppConfig) -> Result<SetupPreview> {
    engine::setup_preview(database, &DesktopHost::foreground(config)).await
}

pub async fn ensure_setup_available(database: &Database, config: &AppConfig) -> Result<()> {
    engine::ensure_setup_available(database, &DesktopHost::foreground(config)).await
}

pub async fn run_setup(
    database: &Database,
    config: &AppConfig,
    invitation: &SetupInvitation,
    progress: &(dyn Fn(Progress) + Sync),
) -> Result<Outcome> {
    let host = DesktopHost::foreground(config);
    driver()?
        .run(|link| engine::run_setup(link, database, &host, invitation, progress))
        .await
}

pub async fn resume_setup(
    database: &Database,
    config: &AppConfig,
    progress: &(dyn Fn(Progress) + Sync),
) -> Result<(String, Outcome)> {
    let host = DesktopHost::foreground(config);
    driver()?
        .run(|link| engine::resume_setup(link, database, &host, progress))
        .await
}

pub async fn create_invitation(
    database: &Database,
    config: &AppConfig,
) -> Result<PendingInvitation> {
    let host = DesktopHost::foreground(config);
    driver()?
        .run(|link| engine::create_invitation(link, database, &host))
        .await
}

pub async fn association_status(
    database: &Database,
    config: &AppConfig,
) -> Result<AssociationStatus> {
    engine::association_status(database, &DesktopHost::background(config)).await
}

/// The association, or `None` while a sync operation holds this database.
pub async fn try_association_status(
    database: &Database,
    config: &AppConfig,
) -> Result<Option<AssociationStatus>> {
    engine::try_association_status(database, &DesktopHost::background(config)).await
}

pub async fn invitation_status(
    database: &Database,
    config: &AppConfig,
) -> Result<Option<InvitationStatus>> {
    Ok(association_status(database, config).await?.invitation)
}

pub async fn cancel_invitation(database: &Database, config: &AppConfig) -> Result<Cancellation> {
    let host = DesktopHost::foreground(config);
    driver()?
        .run(|link| engine::cancel_invitation(link, database, &host))
        .await
}

pub async fn await_admission(
    database: &Database,
    config: &AppConfig,
    invitation: &PendingInvitation,
) -> Result<Admission> {
    let host = DesktopHost::foreground(config);
    driver()?
        .run(|link| engine::await_admission(link, &host, database, invitation))
        .await
}

pub async fn ensure_join_available(database: &Database, config: &AppConfig) -> Result<()> {
    engine::ensure_join_available(database, &DesktopHost::foreground(config)).await
}

pub async fn run_join(
    database: &Database,
    config: &AppConfig,
    invitation: impl FnOnce() -> Result<Option<DeviceInvitation>> + Send,
    replace: bool,
    progress: &(dyn Fn(Progress) + Sync),
) -> Result<(String, Outcome)> {
    let host = DesktopHost::foreground(config);
    driver()?
        .run(|link| engine::run_join(link, database, &host, invitation, replace, progress))
        .await
}

pub async fn run_to_completion(database: &Database, config: &AppConfig) -> Result<Outcome> {
    let host = DesktopHost::foreground(config);
    let outcome = driver()?
        .run(|link| engine::run_to_completion(link, database, &host))
        .await?;
    if outcome.metadata_caught_up && outcome.images != ImageTransfer::Failed {
        crate::daemon::retry_after_foreground_sync(database, config).await;
    }
    Ok(outcome)
}

#[derive(Default)]
pub struct DaemonSync {
    cached: Option<DaemonResources>,
}

struct DaemonResources {
    key: DaemonResourceKey,
    host: DesktopHost,
    driver: HttpDriver,
    #[cfg(test)]
    generation: u64,
}

#[derive(PartialEq, Eq)]
struct DaemonResourceKey {
    blob_dir: Option<PathBuf>,
    automatic_sync_enabled: bool,
    association: Option<String>,
    setup_server: Option<String>,
}

impl DaemonSync {
    async fn resource_key(database: &Database, config: &AppConfig) -> Result<DaemonResourceKey> {
        Ok(DaemonResourceKey {
            blob_dir: config.local.blob_dir.clone(),
            automatic_sync_enabled: config.automatic_sync_is_enabled(),
            association: database.meta("e2ee_association").await?,
            setup_server: database.meta("sync_server_url").await?,
        })
    }

    async fn prepare(&mut self, database: &Database, config: &AppConfig) -> Result<()> {
        let key = Self::resource_key(database, config).await?;
        if self.cached.as_ref().is_some_and(|cached| cached.key == key) {
            return Ok(());
        }
        self.cached = Some(DaemonResources {
            key,
            host: DesktopHost::background(config),
            driver: driver()?,
            #[cfg(test)]
            generation: next_daemon_resource_generation(),
        });
        Ok(())
    }

    pub async fn round(
        &mut self,
        database: &Database,
        config: &AppConfig,
        round_limit: usize,
    ) -> Result<DaemonRound> {
        self.prepare(database, config).await?;
        let cached = self.cached.as_ref().expect("daemon resources prepared");
        let result = cached
            .driver
            .run(|link| engine::daemon_round(link, database, &cached.host, round_limit))
            .await;
        if let Err(error) = &result {
            self.discard_after_error(error);
        }
        result
    }

    fn discard_after_error(&mut self, error: &anyhow::Error) {
        if should_rebuild_daemon_resources(error) {
            self.cached = None;
        }
    }
}

fn should_rebuild_daemon_resources(error: &anyhow::Error) -> bool {
    use aven_core::sync::client::errors::has_code;

    error
        .chain()
        .any(|cause| cause.is::<crate::protected_local_keys::ProtectedLocalKeyStoreError>())
        || [
            "sync-transport",
            "encrypted-tail-network",
            "encrypted-tail-tls",
            "enrollment-network",
            "enrollment-tls",
        ]
        .into_iter()
        .any(|code| has_code(error, code))
}

#[cfg(test)]
fn next_daemon_resource_generation() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

pub async fn status_report(database: &Database, config: &AppConfig) -> Result<StatusReport> {
    engine::status_report(database, &DesktopHost::background(config)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn daemon_resources_persist_until_config_server_or_fatal_error_changes() {
        use crate::protected_local_keys::{
            ProtectedLocalKeyStoreError, ProtectedLocalKeyStoreErrorKind,
        };

        let temp = tempfile::tempdir().unwrap();
        let database = aven_core::db::Database::open(&temp.path().join("daemon.sqlite"))
            .await
            .unwrap();
        let mut config = crate::config::AppConfig::default();
        let mut sync = DaemonSync::default();

        sync.prepare(&database, &config).await.unwrap();
        let original = sync.cached.as_ref().unwrap().generation;
        sync.prepare(&database, &config).await.unwrap();
        assert_eq!(sync.cached.as_ref().unwrap().generation, original);

        config.local.blob_dir = Some(temp.path().join("other-blobs"));
        sync.prepare(&database, &config).await.unwrap();
        let reconfigured = sync.cached.as_ref().unwrap().generation;
        assert_ne!(reconfigured, original);

        sqlx::query("INSERT INTO meta(key, value) VALUES('sync_server_url', ?)")
            .bind("https://sync.example.test")
            .execute(&mut *aven_core::test_support::acquire(&database).await.unwrap())
            .await
            .unwrap();
        sync.prepare(&database, &config).await.unwrap();
        let rebound = sync.cached.as_ref().unwrap().generation;
        assert_ne!(rebound, reconfigured);

        sqlx::query("INSERT INTO meta(key, value) VALUES('e2ee_association', 'new-binding')")
            .execute(&mut *aven_core::test_support::acquire(&database).await.unwrap())
            .await
            .unwrap();
        sync.prepare(&database, &config).await.unwrap();
        let reassociated = sync.cached.as_ref().unwrap().generation;
        assert_ne!(reassociated, rebound);

        let storage = anyhow::Error::new(ProtectedLocalKeyStoreError::new(
            ProtectedLocalKeyStoreErrorKind::Unavailable,
        ));
        sync.discard_after_error(&storage);
        assert!(sync.cached.is_none());

        sync.prepare(&database, &config).await.unwrap();
        let recovered = sync.cached.as_ref().unwrap().generation;
        sync.discard_after_error(&anyhow::anyhow!(
            "error encrypted-tail-network outcome-unknown"
        ));
        assert!(sync.cached.is_none());
        sync.prepare(&database, &config).await.unwrap();
        assert_ne!(sync.cached.as_ref().unwrap().generation, recovered);
    }
}
