use std::env;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::ids::WorkspaceId;

mod custom_commands;
mod paths;
#[cfg(test)]
mod test_support;
mod tui;

pub(crate) use custom_commands::DEFAULT_CUSTOM_COMMAND_TIMEOUT_SECONDS;
pub use custom_commands::{
    CustomTuiCommandConfig, CustomTuiCommandExecution, CustomTuiCommandSuccess,
    CustomTuiCommandTarget,
};
pub(crate) use paths::expand_tilde_from;
pub use paths::{
    config_dir_path, config_file_path, debug_db_path_from_env, default_db_path, expand_tilde,
    resolve_blob_dir, resolve_db_path,
};
pub use tui::{SidebarView, TableColumn, TaskColumnConfig, TuiConfig};

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
    #[serde(default)]
    pub agent: AgentConfig,
    #[serde(default)]
    pub update: UpdateConfig,
    #[serde(default)]
    pub tui: TuiConfig,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct UpdateConfig {
    #[serde(default = "default_automatic_update_checks")]
    pub automatic_checks: bool,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            automatic_checks: default_automatic_update_checks(),
        }
    }
}

fn default_automatic_update_checks() -> bool {
    true
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LocalConfig {
    pub db_path: Option<PathBuf>,
    #[serde(default)]
    pub blob_dir: Option<PathBuf>,
    #[serde(default)]
    pub inline_images: InlineImagesConfig,
    #[serde(default)]
    pub image_optimization: ImageOptimizationConfig,
    #[serde(default)]
    pub attachment_lifecycle: AttachmentLifecycleConfig,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct AttachmentLifecycleConfig {
    #[serde(default = "default_attachment_grace_days")]
    pub grace_days: u64,
    #[serde(default = "default_server_attachment_grace_days", skip_serializing)]
    pub server_grace_days: u64,
    #[serde(default = "default_attachment_quota_bytes")]
    pub quota_bytes: i64,
    #[serde(default = "default_attachment_quota_bytes", skip_serializing)]
    pub server_workspace_quota_bytes: i64,
    #[serde(default = "default_preview_quota_bytes")]
    pub preview_quota_bytes: u64,
    #[serde(default = "default_attachment_maintenance_limit")]
    pub maintenance_limit: usize,
}

fn default_attachment_grace_days() -> u64 {
    7
}

fn default_server_attachment_grace_days() -> u64 {
    30
}

fn default_attachment_quota_bytes() -> i64 {
    crate::attachments::lifecycle::DEFAULT_ORIGINAL_QUOTA_BYTES
}

fn default_preview_quota_bytes() -> u64 {
    crate::attachments::lifecycle::DEFAULT_PREVIEW_QUOTA_BYTES
}

fn default_attachment_maintenance_limit() -> usize {
    crate::attachments::lifecycle::DEFAULT_MAINTENANCE_LIMIT
}

impl Default for AttachmentLifecycleConfig {
    fn default() -> Self {
        Self {
            grace_days: default_attachment_grace_days(),
            server_grace_days: default_server_attachment_grace_days(),
            quota_bytes: default_attachment_quota_bytes(),
            server_workspace_quota_bytes: default_attachment_quota_bytes(),
            preview_quota_bytes: default_preview_quota_bytes(),
            maintenance_limit: default_attachment_maintenance_limit(),
        }
    }
}

impl AttachmentLifecycleConfig {
    pub(crate) fn policy(self) -> crate::attachments::lifecycle::LifecyclePolicy {
        crate::attachments::lifecycle::LifecyclePolicy {
            grace: std::time::Duration::from_secs(self.grace_days.saturating_mul(24 * 60 * 60)),
            quota_bytes: self.quota_bytes,
            preview_quota_bytes: self.preview_quota_bytes,
            maintenance_limit: self.maintenance_limit,
        }
    }

    pub(crate) fn server_policy(self) -> crate::attachments::lifecycle::LifecyclePolicy {
        crate::attachments::lifecycle::LifecyclePolicy {
            grace: std::time::Duration::from_secs(
                self.server_grace_days.saturating_mul(24 * 60 * 60),
            ),
            quota_bytes: self.server_workspace_quota_bytes,
            preview_quota_bytes: self.preview_quota_bytes,
            maintenance_limit: self.maintenance_limit,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum InlineImagesConfig {
    Off,
    #[default]
    Auto,
    On,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ImageOptimizationConfig {
    #[default]
    Off,
    Paste,
    On,
}

impl ImageOptimizationConfig {
    pub(crate) fn optimizes_pasted_images(self) -> bool {
        matches!(self, Self::Paste | Self::On)
    }

    pub(crate) fn optimizes_file_attachments(self) -> bool {
        matches!(self, Self::On)
    }
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
    pub workspace_id: Option<WorkspaceId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    pub project: String,
    #[serde(default)]
    pub paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentConfig {
    #[serde(default)]
    pub task_intake: TaskIntakeConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskIntakeConfig {
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default = "default_task_intake_args")]
    pub args: Vec<String>,
    pub timeout_seconds: Option<u64>,
    #[serde(default)]
    pub system_prompt: Option<String>,
}

fn default_task_intake_args() -> Vec<String> {
    vec![
        "-p".to_string(),
        "--no-session-persistence".to_string(),
        "--bare".to_string(),
        "{prompt}".to_string(),
    ]
}

impl Default for TaskIntakeConfig {
    fn default() -> Self {
        Self {
            command: None,
            args: default_task_intake_args(),
            timeout_seconds: Some(45),
            system_prompt: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(skip)]
    pub(crate) disable_override: bool,
    pub interval_seconds: Option<u64>,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            disable_override: false,
            interval_seconds: Some(DEFAULT_SYNC_INTERVAL_SECONDS),
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
    pub fn project_key(&self) -> String {
        crate::projects::normalize_key(&self.project)
    }

    pub fn matches_workspace(
        &self,
        workspace_id: Option<&WorkspaceId>,
        workspace: Option<&str>,
    ) -> bool {
        match self.workspace_id.as_ref() {
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
        let mut config = if !path.exists() {
            Self::default()
        } else {
            let text = fs::read_to_string(path)
                .with_context(|| format!("could not read {}", path.display()))?;
            serde_yaml::from_str(&text)
                .with_context(|| format!("could not parse {}", path.display()))?
        };
        config.sync.disable_override |= sync_disabled_from_env();
        config.update.automatic_checks = automatic_update_checks_enabled(
            config.update.automatic_checks,
            env::var("AVEN_NO_UPDATE_CHECK").ok().as_deref(),
        );
        config
            .validate()
            .with_context(|| format!("invalid config {}", path.display()))?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        tui::validate(&self.tui)?;
        custom_commands::validate(&self.tui.commands)
    }

    pub fn has_project_override(
        &self,
        workspace_id: Option<&WorkspaceId>,
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

    pub(crate) fn sync_is_allowed(&self) -> bool {
        !self.sync.disable_override
    }

    pub(crate) fn automatic_sync_is_enabled(&self) -> bool {
        self.sync.enabled && self.sync_is_allowed()
    }

    pub(crate) fn ensure_sync_allowed(&self) -> Result<()> {
        if !self.sync_is_allowed() {
            bail!("error sync-disabled hint=\"sync is disabled in this environment\"");
        }
        Ok(())
    }

    pub(crate) fn ensure_automatic_sync_enabled(&self) -> Result<()> {
        self.ensure_sync_allowed()?;
        if !self.sync.enabled {
            bail!("error sync-disabled hint=\"set sync.enabled = true in config.yaml\"");
        }
        Ok(())
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

fn sync_disabled_from_env() -> bool {
    sync_disabled_value(env::var("AVEN_SYNC_DISABLED").ok().as_deref())
}

fn automatic_update_checks_enabled(configured: bool, disabled_env: Option<&str>) -> bool {
    configured && !disabled_env_value(disabled_env)
}

fn sync_disabled_value(value: Option<&str>) -> bool {
    matches!(value, Some("1") | Some("true") | Some("yes"))
}

fn disabled_env_value(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes"
        )
    })
}

pub fn write_config_text(path: &Path, text: String) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    let permissions = if path.exists() {
        Some(
            fs::metadata(path)
                .with_context(|| format!("could not inspect {}", path.display()))?
                .permissions(),
        )
    } else {
        None
    };
    let tmp_path = path.with_extension("yaml.tmp");
    fs::write(&tmp_path, text)
        .with_context(|| format!("could not write {}", tmp_path.display()))?;
    if let Some(permissions) = permissions {
        fs::set_permissions(&tmp_path, permissions)
            .with_context(|| format!("could not preserve permissions for {}", path.display()))?;
    }
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
    let config = AppConfig::default();
    config.validate()?;
    let text = serde_yaml::to_string(&config)?;
    write_config_text(path, text)
}

#[cfg(test)]
mod tests;
