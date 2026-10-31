use anyhow::Result;
use std::fs;

use super::AppConfig;

pub(in crate::config) fn load_config(text: &str) -> Result<AppConfig> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config.yaml");
    fs::write(&path, text)?;
    AppConfig::load_from_path(&path)
}
