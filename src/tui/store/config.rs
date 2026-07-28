use anyhow::Result;

use crate::config as app_config;
use crate::operations::{
    show_config as show_config_operation, show_config_paths as show_config_paths_operation,
};
use crate::sync::sync_server_url_is_valid;

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

    pub(super) async fn load_sync_status(&self) -> Result<TuiSyncStatus> {
        let config = match app_config::AppConfig::load() {
            Ok(config) => config,
            Err(error) => {
                return Ok(TuiSyncStatus {
                    config_error: Some(format!("{error:#}")),
                    ..TuiSyncStatus::default()
                });
            }
        };
        let persistence = self.database.sync_persistence_status().await?;
        let pinned_server = persistence.pinned_server.clone();
        let configured_server = configured_server_check(&config);
        let server_match = configured_server
            .as_ref()
            .filter(|server| server.ok)
            .and_then(|server| {
                pinned_server.as_deref().map(|pinned| {
                    let configured = server.value.trim_end_matches('/');
                    SyncStatusCheck::new(
                        pinned == configured,
                        if pinned == configured {
                            "yes".to_string()
                        } else {
                            format!("pinned={pinned} configured={configured}")
                        },
                    )
                })
            });
        let daemon_server = daemon_server_check(&config);
        let daemon_wake = match config.wake_addr() {
            Ok(addr) => SyncStatusCheck::new(true, addr.to_string()),
            Err(error) => SyncStatusCheck::new(false, format!("{error:#}")),
        };
        Ok(TuiSyncStatus {
            enabled: config.sync.enabled,
            config_error: None,
            configured_server,
            pinned_server,
            server_match,
            daemon_server,
            auth_token_configured: config.sync_auth_token().is_some(),
            interval_seconds: config.sync_interval_seconds(),
            daemon_wake,
            pending_changes: persistence.pending_changes,
            conflicts: persistence.conflicts,
            sync_cursor: persistence.sync_cursor,
            local_sequence: persistence.local_sequence,
            last_attempt: persistence.last_attempt,
            last_success: persistence.last_success,
            last_error: persistence.last_error,
            last_pushed: persistence.last_pushed,
            last_pulled: persistence.last_pulled,
            last_cursor: persistence.last_cursor,
        })
    }
}

fn configured_server_check(config: &app_config::AppConfig) -> Option<SyncStatusCheck> {
    match app_config::resolve_sync_server(None, config) {
        Ok(server) => Some(SyncStatusCheck::new(
            sync_server_url_is_valid(&server),
            server,
        )),
        Err(error) if config.sync.enabled => {
            Some(SyncStatusCheck::new(false, format!("{error:#}")))
        }
        Err(_) => None,
    }
}

fn daemon_server_check(config: &app_config::AppConfig) -> Option<SyncStatusCheck> {
    match config.sync.server_url.as_deref() {
        Some(server) => Some(SyncStatusCheck::new(
            sync_server_url_is_valid(server),
            server,
        )),
        None if config.sync.enabled => Some(SyncStatusCheck::new(false, "not configured")),
        None => None,
    }
}
