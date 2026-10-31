use anyhow::Result;

use crate::tui::app::App;
use crate::tui::config_overlay::{
    config_info_overlay, config_init_overlay, config_paths_overlay, database_stats_overlay,
};

impl App {
    pub(super) fn show_config_write_refusal(&mut self, title: &str, error: &anyhow::Error) -> bool {
        let Some(message) = crate::config::config_write_refusal_message(error) else {
            return false;
        };
        self.overlay = Some(crate::tui::overlay::OverlayState::ConfigWriteRefusal {
            title: title.to_string(),
            message,
            reason: error
                .downcast_ref::<crate::config::ConfigWriteRefusal>()
                .expect("refusal message requires a refusal")
                .reason,
        });
        true
    }

    pub(super) fn show_config_info(&mut self) -> Result<()> {
        self.pending_shortcut.clear();
        self.overlay = Some(config_info_overlay(&self.store)?);
        Ok(())
    }

    pub(super) fn show_config_paths(&mut self) -> Result<()> {
        self.pending_shortcut.clear();
        self.overlay = Some(config_paths_overlay(&self.store)?);
        Ok(())
    }

    pub(super) async fn show_database_stats(&mut self) -> Result<()> {
        self.pending_shortcut.clear();
        self.store.load_database_stats().await?;
        self.overlay = Some(database_stats_overlay(&self.store)?);
        Ok(())
    }

    pub(super) fn begin_config_init(&mut self) -> Result<()> {
        self.pending_shortcut.clear();
        self.overlay = Some(config_init_overlay()?);
        Ok(())
    }

    pub(super) fn submit_config_init(&mut self, path: std::path::PathBuf) -> Result<()> {
        let message = match self.store.init_config(path) {
            Ok(message) => message,
            Err(error) if self.show_config_write_refusal("Could not initialize config", &error) => {
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        self.set_success(message);
        Ok(())
    }
}
