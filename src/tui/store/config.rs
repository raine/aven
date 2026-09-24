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

    pub(super) async fn load_sync_status(&self) -> Result<TuiSyncStatus> {
        let config = self.config();
        let persistence = self.database.sync_persistence_status().await?;
        let daemon_wake = match config.wake_addr() {
            Ok(addr) => SyncStatusCheck::new(true, addr.to_string()),
            Err(error) => SyncStatusCheck::new(false, format!("{error:#}")),
        };
        Ok(TuiSyncStatus {
            enabled: config.sync.enabled,
            runtime_allowed: config.sync_is_allowed(),
            set_up: crate::sync::encrypted::is_set_up(&self.database).await?,
            interval_seconds: config.sync_interval_seconds(),
            daemon_wake,
            pending_changes: persistence.pending_changes,
            conflicts: persistence.conflicts,
            sync_cursor: persistence.sync_cursor,
            local_sequence: persistence.local_sequence,
        })
    }
}
