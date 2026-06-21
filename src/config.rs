use std::env;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

const APP_DIR: &str = "aven";
const DEFAULT_WAKE_ADDR: &str = "127.0.0.1:47631";
const DEFAULT_SYNC_INTERVAL_SECONDS: u64 = 30;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub local: LocalConfig,
    #[serde(default)]
    pub sync: SyncConfig,
    #[serde(default)]
    pub daemon: DaemonConfig,
    #[serde(default)]
    pub workspace: WorkspaceConfig,
    #[serde(default)]
    pub project: ProjectConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LocalConfig {
    pub db_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkspaceConfig {
    pub default: Option<String>,
    #[serde(default)]
    pub routes: Vec<WorkspaceRouteConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkspaceRouteConfig {
    pub workspace: String,
    #[serde(default)]
    pub paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectConfig {
    #[serde(default)]
    pub overrides: Vec<ProjectOverrideConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectOverrideConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    pub project: String,
    #[serde(default)]
    pub paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncConfig {
    #[serde(default)]
    pub enabled: bool,
    pub server_url: Option<String>,
    pub interval_seconds: Option<u64>,
    pub auth_token: Option<String>,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            server_url: None,
            interval_seconds: Some(DEFAULT_SYNC_INTERVAL_SECONDS),
            auth_token: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    pub wake_addr: Option<String>,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            wake_addr: Some(DEFAULT_WAKE_ADDR.to_string()),
        }
    }
}

impl ProjectOverrideConfig {
    pub(crate) fn project_key(&self) -> String {
        crate::projects::normalize_key(&self.project)
    }

    pub(crate) fn matches_workspace(
        &self,
        workspace_id: Option<&str>,
        workspace: Option<&str>,
    ) -> bool {
        match self.workspace_id.as_deref() {
            Some(id) => Some(id) == workspace_id,
            None => self
                .workspace
                .as_deref()
                .is_none_or(|key| Some(key) == workspace),
        }
    }
}

impl AppConfig {
    pub fn load() -> Result<Self> {
        let path = config_file_path()?;
        Self::load_from_path(&path)
    }

    pub fn load_from_path(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = fs::read_to_string(path)
            .with_context(|| format!("could not read {}", path.display()))?;
        serde_yaml::from_str(&text).with_context(|| format!("could not parse {}", path.display()))
    }

    pub fn has_project_override(
        &self,
        workspace_id: Option<&str>,
        workspace: Option<&str>,
        project_key: &str,
    ) -> bool {
        self.project.overrides.iter().any(|project_override| {
            project_override.matches_workspace(workspace_id, workspace)
                && project_override.project_key() == project_key
        })
    }

    pub fn sync_interval_seconds(&self) -> u64 {
        self.sync
            .interval_seconds
            .unwrap_or(DEFAULT_SYNC_INTERVAL_SECONDS)
            .max(1)
    }

    pub fn sync_auth_token(&self) -> Option<&str> {
        self.sync
            .auth_token
            .as_deref()
            .map(str::trim)
            .filter(|token| !token.is_empty())
    }

    pub fn wake_addr(&self) -> Result<SocketAddr> {
        let value = self
            .daemon
            .wake_addr
            .as_deref()
            .unwrap_or(DEFAULT_WAKE_ADDR);
        let addr = SocketAddr::from_str(value)
            .with_context(|| format!("invalid daemon wake address {value}"))?;
        if !addr.ip().is_loopback() {
            bail!("error daemon-wake-requires-loopback addr={addr}");
        }
        Ok(addr)
    }
}

pub fn config_dir_path() -> Result<PathBuf> {
    if let Ok(path) = env::var("AVEN_CONFIG_DIR") {
        return Ok(PathBuf::from(path));
    }
    let home = dirs::home_dir().context("could not find home directory")?;
    Ok(home.join(".config").join(APP_DIR))
}

pub fn config_file_path() -> Result<PathBuf> {
    let mut path = config_dir_path()?;
    path.push("config.yaml");
    Ok(path)
}

pub fn default_db_path() -> Result<PathBuf> {
    let mut dir = env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| dirs::home_dir().map(|home| home.join(".local/state")))
        .context("could not find state directory")?;
    dir.push("aven");
    dir.push("db.sqlite");
    Ok(dir)
}

pub fn resolve_db_path(flag: Option<PathBuf>, config: &AppConfig) -> Result<PathBuf> {
    if let Some(path) = flag {
        return Ok(path);
    }
    if let Ok(path) = env::var("AVEN_DB") {
        return Ok(PathBuf::from(path));
    }
    if let Some(path) = &config.local.db_path {
        return Ok(path.clone());
    }
    default_db_path()
}

pub fn resolve_sync_server(flag: Option<&str>, config: &AppConfig) -> Result<String> {
    if let Some(server) = flag {
        return Ok(server.to_string());
    }
    if let Ok(server) = env::var("AVEN_SYNC_SERVER") {
        return Ok(server);
    }
    if let Some(server) = &config.sync.server_url {
        return Ok(server.clone());
    }
    bail!("error sync-server-required hint=\"pass --server or configure sync.server_url\"")
}

pub fn write_config(path: &Path, config: &AppConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    let text = serde_yaml::to_string(config)?;
    let tmp_path = path.with_extension("yaml.tmp");
    fs::write(&tmp_path, text)
        .with_context(|| format!("could not write {}", tmp_path.display()))?;
    fs::rename(&tmp_path, path).with_context(|| {
        format!(
            "could not replace {} with {}",
            path.display(),
            tmp_path.display()
        )
    })?;
    Ok(())
}

pub fn write_default_config(path: &Path) -> Result<()> {
    if path.exists() {
        bail!("error config-exists path={}", path.display());
    }
    let mut config = AppConfig::default();
    config.sync.auth_token = Some(String::new());
    write_config(path, &config)
}
