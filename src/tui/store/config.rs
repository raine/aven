use anyhow::Result;

use crate::operations::{
    show_config as show_config_operation, show_config_paths as show_config_paths_operation,
};

use super::TuiStore;
use super::types::{SyncStatusCheck, TuiSyncStatus};

impl TuiStore {
    pub(crate) fn config_info_lines(&self) -> Result<Vec<String>> {
        let outcome = show_config_operation()?;
        let mut lines = vec![
            format!("config path: {}", outcome.path.display()),
            String::new(),
        ];
        lines.extend(outcome.text.lines().map(str::to_string));
        Ok(lines)
    }

    pub(crate) fn config_path_lines(&self) -> Result<Vec<String>> {
        Ok(show_config_paths_operation(Some(self.database.path()))?.lines)
    }

    pub(crate) fn init_config(&self, path: std::path::PathBuf) -> Result<String> {
        let outcome = crate::operations::init_config_at(path)?;
        Ok(format!("created config {}", outcome.path.display()))
    }

    pub(crate) async fn refresh_sync_status(&mut self) -> Result<()> {
        self.projection.sync_status =
            load_sync_status(&self.database, &self.app_config, &self.sync_status).await?;
        Ok(())
    }

    pub(crate) fn set_sync_invitation(
        &mut self,
        invitation: Option<crate::sync::encrypted::InvitationStatus>,
    ) {
        self.projection.sync_status.invitation = invitation;
    }
}

pub(super) async fn load_sync_status(
    database: &aven_core::db::Database,
    config: &crate::config::AppConfig,
    previous: &TuiSyncStatus,
) -> Result<TuiSyncStatus> {
    let persistence = database.sync_persistence_status().await?;
    let daemon_wake = match config.wake_addr() {
        Ok(addr) => SyncStatusCheck::new(true, addr.to_string()),
        Err(error) => SyncStatusCheck::new(false, format!("{error:#}")),
    };
    let phase = crate::sync::encrypted::local_phase(database).await?;
    // A sync operation holds the association while it runs; keep the last
    // snapshot until it finishes instead of waiting on or reporting it.
    let (association, protected_storage) =
        match crate::sync::encrypted::try_association_status(database, config).await {
            Ok(Some(association)) => (
                association,
                SyncStatusCheck::new(true, "available".to_string()),
            ),
            Ok(None) => (
                crate::sync::encrypted::AssociationStatus {
                    server: previous.server.clone(),
                    devices: previous.devices,
                    invitation: previous.invitation,
                },
                previous.protected_storage.clone(),
            ),
            Err(error)
                if error.chain().any(|source| {
                    source
                        .downcast_ref::<crate::protected_local_keys::ProtectedLocalKeyStoreError>()
                        .is_some()
                }) =>
            {
                let message = crate::sync::error_explanations::explain(
                    crate::sync::error_explanations::ErrorAction::General,
                    crate::sync::error_explanations::ErrorSurface::Tui,
                    &error,
                )
                .map(crate::sync::error_explanations::Explanation::combined)
                .unwrap_or_else(|| "Protected sync key storage is unavailable.".to_string());
                (
                    crate::sync::encrypted::AssociationStatus::default(),
                    SyncStatusCheck::new(false, message),
                )
            }
            Err(error) => return Err(error),
        };
    let access_refused_at = database
        .sync_access_refusal()
        .await?
        .map(|refusal| refusal.at);
    let vault_deleted_at = database
        .sync_vault_deletion()
        .await?
        .map(|deletion| deletion.at);
    Ok(TuiSyncStatus {
        enabled: config.sync.enabled,
        runtime_allowed: config.sync_is_allowed(),
        set_up: phase != crate::sync::encrypted::LocalPhase::NotSetUp,
        phase,
        interval_seconds: config.sync_interval_seconds(),
        daemon_wake,
        protected_storage,
        pending_changes: persistence.pending_changes,
        conflicts: persistence.conflicts,
        sync_cursor: persistence.sync_cursor,
        local_sequence: persistence.local_sequence,
        server: association.server,
        devices: association.devices,
        invitation: association.invitation,
        access_refused_at,
        vault_deleted_at,
    })
}
