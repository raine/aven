use anyhow::Result;

use crate::tui::overlay::{OverlayRoute, OverlayState, TextPanelState};
use crate::tui::store::TuiStore;

pub(crate) const CONFIG_STATUS_TITLE: &str = "Sync status";
pub(crate) const CONFIG_INFO_TITLE: &str = "Configuration";
pub(crate) const CONFIG_PATHS_TITLE: &str = "Config paths";
pub(crate) const CONFIG_INIT_TITLE: &str = "Initialize configuration";

pub(crate) fn config_status_overlay(store: &TuiStore) -> Result<OverlayState> {
    Ok(OverlayState::TextPanel(TextPanelState::new(
        CONFIG_STATUS_TITLE,
        store.config_status_lines()?,
    )))
}

pub(crate) fn config_info_overlay(store: &TuiStore) -> Result<OverlayState> {
    Ok(OverlayState::TextPanel(TextPanelState::new(
        CONFIG_INFO_TITLE,
        store.config_info_lines()?,
    )))
}

pub(crate) fn config_paths_overlay(store: &TuiStore) -> Result<OverlayState> {
    Ok(OverlayState::TextPanel(TextPanelState::new(
        CONFIG_PATHS_TITLE,
        store.config_path_lines()?,
    )))
}

pub(crate) fn config_init_overlay() -> Result<OverlayState> {
    let path = crate::config::config_file_path()?;
    Ok(OverlayState::confirm(
        OverlayRoute::ConfigInit,
        CONFIG_INIT_TITLE,
        format!("Create default config at {}?", path.display()),
    ))
}
