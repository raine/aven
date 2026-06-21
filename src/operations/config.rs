use std::path::PathBuf;

use anyhow::Result;

use crate::config as app_config;

pub(crate) struct ConfigShowOutcome {
    pub(crate) path: PathBuf,
    pub(crate) text: String,
}

pub(crate) struct ConfigInitOutcome {
    pub(crate) path: PathBuf,
}

pub(crate) struct ConfigStatusOutcome {
    pub(crate) lines: Vec<String>,
}

pub(crate) struct ConfigPathsOutcome {
    pub(crate) lines: Vec<String>,
}
pub(crate) fn show_config() -> Result<ConfigShowOutcome> {
    let path = app_config::config_file_path()?;
    let config = app_config::AppConfig::load()?;
    let text = serde_yaml::to_string(&config)?;
    Ok(ConfigShowOutcome { path, text })
}

pub(crate) fn show_config_status() -> Result<ConfigStatusOutcome> {
    let config = app_config::AppConfig::load()?;
    let sync_server = app_config::resolve_sync_server(None, &config)
        .unwrap_or_else(|error| format!("unavailable ({error:#})"));
    let wake_addr = config.wake_addr().map_or_else(
        |error| format!("invalid ({error:#})"),
        |addr| addr.to_string(),
    );
    Ok(ConfigStatusOutcome {
        lines: vec![
            format!("sync enabled: {}", config.sync.enabled),
            format!("sync server: {sync_server}"),
            format!("sync interval seconds: {}", config.sync_interval_seconds()),
            format!("daemon wake address: {wake_addr}"),
            "daemon state: not checked from TUI".to_string(),
            "sync state: not checked from TUI".to_string(),
        ],
    })
}

pub(crate) fn show_config_paths() -> Result<ConfigPathsOutcome> {
    let config = app_config::AppConfig::load()?;
    let config_dir = app_config::config_dir_path()?;
    let config_file = app_config::config_file_path()?;
    let default_db = app_config::default_db_path()?;
    let effective_db = app_config::resolve_db_path(None, &config)?;
    let db_source = if std::env::var_os("AVEN_DB").is_some() {
        "AVEN_DB"
    } else if config.local.db_path.is_some() {
        "config local.db_path"
    } else {
        "default"
    };
    Ok(ConfigPathsOutcome {
        lines: vec![
            format!("config directory: {}", config_dir.display()),
            format!("config file: {}", config_file.display()),
            format!("default database: {}", default_db.display()),
            format!("effective database: {}", effective_db.display()),
            format!("database source: {db_source}"),
        ],
    })
}

pub(crate) fn init_config() -> Result<ConfigInitOutcome> {
    let path = app_config::config_file_path()?;
    app_config::write_default_config(&path)?;
    Ok(ConfigInitOutcome { path })
}
