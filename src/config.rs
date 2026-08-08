use std::env;
use std::fs;
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::ids::WorkspaceId;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuiConfig {
    #[serde(default = "default_task_columns")]
    pub columns: Vec<TaskColumnConfig>,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            columns: default_task_columns(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskColumnConfig {
    pub name: String,
    pub statuses: Vec<String>,
}

impl TaskColumnConfig {
    fn new(name: &str, statuses: &[&str]) -> Self {
        Self {
            name: name.to_string(),
            statuses: statuses.iter().map(|status| status.to_string()).collect(),
        }
    }
}

fn default_task_columns() -> Vec<TaskColumnConfig> {
    vec![
        TaskColumnConfig::new("Inbox", &["inbox"]),
        TaskColumnConfig::new("Backlog", &["backlog"]),
        TaskColumnConfig::new("Todo", &["todo"]),
        TaskColumnConfig::new("Active", &["active"]),
        TaskColumnConfig::new("Done", &["done", "canceled"]),
    ]
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
    pub server_url: Option<String>,
    pub interval_seconds: Option<u64>,
    pub auth_token: Option<String>,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            disable_override: false,
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
        use std::collections::BTreeSet;

        if self.tui.columns.is_empty() {
            bail!("column view requires at least one column");
        }
        let valid = crate::choices::STATUSES
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        let mut assigned = BTreeSet::new();
        for column in &self.tui.columns {
            if column.name.trim().is_empty() {
                bail!("column name must not be blank");
            }
            if column.statuses.is_empty() {
                bail!("column {} must include at least one status", column.name);
            }
            for status in &column.statuses {
                if !valid.contains(status.as_str()) {
                    bail!("unknown column status {status}");
                }
                if !assigned.insert(status.as_str()) {
                    bail!("duplicate column status {status}");
                }
            }
        }
        let missing = valid.difference(&assigned).copied().collect::<Vec<_>>();
        if !missing.is_empty() {
            bail!("missing column statuses {}", missing.join(","));
        }
        Ok(())
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

    pub fn sync_auth_token(&self) -> Option<&str> {
        self.sync
            .auth_token
            .as_deref()
            .map(str::trim)
            .filter(|token| !token.is_empty())
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

pub fn expand_tilde(path: &Path) -> Result<PathBuf> {
    let mut components = path.components();
    if !matches!(components.next(), Some(Component::Normal(component)) if component == "~") {
        return Ok(path.to_path_buf());
    }
    let home = dirs::home_dir().context("could not find home directory")?;
    Ok(home.join(components.as_path()))
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
    resolve_db_path_from(
        flag,
        env::var_os("AVEN_DB").map(PathBuf::from),
        debug_db_path_from_env(),
        config,
        cfg!(debug_assertions),
    )
}

fn resolve_db_path_from(
    flag: Option<PathBuf>,
    env_db: Option<PathBuf>,
    dev_db: Option<PathBuf>,
    config: &AppConfig,
    debug_build: bool,
) -> Result<PathBuf> {
    if let Some(path) = flag {
        return Ok(path);
    }
    if debug_build && let Some(path) = dev_db {
        return Ok(path);
    }
    if let Some(path) = env_db {
        return Ok(path);
    }
    if let Some(path) = &config.local.db_path {
        return expand_tilde(path);
    }
    if debug_build {
        bail!("error debug-database-required hint=\"set AVEN_DEV_DB, set AVEN_DB, or pass --db\"");
    }
    default_db_path()
}

pub fn debug_db_path_from_env() -> Option<PathBuf> {
    if cfg!(debug_assertions) {
        env::var_os("AVEN_DEV_DB").map(PathBuf::from)
    } else {
        None
    }
}

#[allow(dead_code)]
pub fn resolve_blob_dir(db_path: &Path, config: &AppConfig) -> Result<PathBuf> {
    let base = db_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    match &config.local.blob_dir {
        Some(path) if path.is_absolute() => Ok(path.clone()),
        Some(path) => Ok(base.join(path)),
        None => {
            let mut blob_dir = db_path.as_os_str().to_os_string();
            blob_dir.push(".blobs");
            Ok(PathBuf::from(blob_dir))
        }
    }
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
    let mut config = AppConfig::default();
    config.sync.auth_token = Some(String::new());
    config.validate()?;
    let text = serde_yaml::to_string(&config)?;
    write_config_text(path, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load_config(text: &str) -> Result<AppConfig> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.yaml");
        fs::write(&path, text)?;
        AppConfig::load_from_path(&path)
    }

    #[test]
    fn tilde_paths_expand_from_home() {
        let home = dirs::home_dir().expect("home directory");

        assert_eq!(
            expand_tilde(Path::new("~/work")).unwrap(),
            home.join("work")
        );
        assert_eq!(
            expand_tilde(Path::new("~someone/work")).unwrap(),
            PathBuf::from("~someone/work")
        );
        assert_eq!(
            expand_tilde(Path::new("relative/work")).unwrap(),
            PathBuf::from("relative/work")
        );
    }

    #[test]
    fn default_columns_cover_every_status_once() {
        let config = AppConfig::default();

        config.validate().unwrap();
        assert_eq!(
            config
                .tui
                .columns
                .iter()
                .map(|column| column.name.as_str())
                .collect::<Vec<_>>(),
            ["Inbox", "Backlog", "Todo", "Active", "Done"]
        );
    }

    #[test]
    fn custom_columns_load_in_configured_order() {
        let config = load_config(
            "tui:\n  columns:\n    - name: Work\n      statuses: [active, todo]\n    - name: Later\n      statuses: [inbox, backlog]\n    - name: Closed\n      statuses: [done, canceled]\n",
        )
        .unwrap();

        assert_eq!(config.tui.columns[0].name, "Work");
        assert_eq!(config.tui.columns[0].statuses, ["active", "todo"]);
    }

    #[test]
    fn empty_config_uses_default_columns() {
        assert_eq!(load_config("{}\n").unwrap().tui.columns.len(), 5);
    }

    #[test]
    fn column_config_rejects_missing_statuses() {
        let error =
            load_config("tui:\n  columns:\n    - name: Current\n      statuses: [active, todo]\n")
                .unwrap_err();

        assert!(format!("{error:#}").contains("missing column statuses"));
    }

    #[test]
    fn column_config_rejects_duplicate_statuses() {
        let error = load_config(
            "tui:\n  columns:\n    - name: One\n      statuses: [inbox, backlog, todo, active, done, canceled]\n    - name: Two\n      statuses: [active]\n",
        )
        .unwrap_err();

        assert!(format!("{error:#}").contains("duplicate column status active"));
    }

    #[test]
    fn column_config_rejects_unknown_statuses() {
        let error = load_config(
            "tui:\n  columns:\n    - name: One\n      statuses: [inbox, backlog, todo, active, done, canceled, parked]\n",
        )
        .unwrap_err();

        assert!(format!("{error:#}").contains("unknown column status parked"));
    }

    #[test]
    fn column_config_rejects_empty_columns_and_names() {
        let empty = load_config("tui:\n  columns: []\n").unwrap_err();
        assert!(format!("{empty:#}").contains("requires at least one column"));

        let unnamed = load_config(
            "tui:\n  columns:\n    - name: '  '\n      statuses: [inbox, backlog, todo, active, done, canceled]\n",
        )
        .unwrap_err();
        assert!(format!("{unnamed:#}").contains("column name must not be blank"));

        let no_statuses =
            load_config("tui:\n  columns:\n    - name: Empty\n      statuses: []\n").unwrap_err();
        assert!(format!("{no_statuses:#}").contains("must include at least one status"));
    }

    #[test]
    fn project_override_workspace_ids_are_validated() {
        let valid = load_config(
            "project:\n  overrides:\n    - workspace_id: 0123456789ABCDEF\n      project: app\n      paths: []\n",
        )
        .unwrap();
        assert_eq!(
            valid.project.overrides[0]
                .workspace_id
                .as_ref()
                .unwrap()
                .as_str(),
            "0123456789ABCDEF"
        );

        let invalid = load_config(
            "project:\n  overrides:\n    - workspace_id: invalid\n      project: app\n      paths: []\n",
        )
        .unwrap_err();
        assert!(format!("{invalid:#}").contains("workspace ID must be"));
    }

    #[test]
    fn default_columns_round_trip() {
        let config = AppConfig::default();
        let yaml = serde_yaml::to_string(&config).unwrap();
        let loaded: AppConfig = serde_yaml::from_str(&yaml).unwrap();

        loaded.validate().unwrap();
        assert_eq!(loaded.tui.columns, config.tui.columns);
    }

    #[test]
    fn debug_database_resolution_requires_an_explicit_path() {
        let config = AppConfig::default();
        let error = resolve_db_path_from(None, None, None, &config, true).unwrap_err();

        assert!(format!("{error:#}").contains("debug-database-required"));
    }

    #[test]
    fn debug_database_resolution_uses_dev_environment_path() {
        let config = AppConfig::default();
        let dev_db = PathBuf::from("/tmp/aven-dev.sqlite");

        assert_eq!(
            resolve_db_path_from(None, None, Some(dev_db.clone()), &config, true).unwrap(),
            dev_db
        );
    }

    #[test]
    fn debug_database_resolution_prefers_dev_environment_path() {
        let config = AppConfig::default();
        let dev_db = PathBuf::from("/tmp/aven-dev.sqlite");
        let env_db = PathBuf::from("/tmp/aven-env.sqlite");

        assert_eq!(
            resolve_db_path_from(None, Some(env_db), Some(dev_db.clone()), &config, true,).unwrap(),
            dev_db
        );
    }

    #[test]
    fn database_flag_overrides_debug_environment_path() {
        let config = AppConfig::default();
        let dev_db = Some(PathBuf::from("/tmp/aven-dev.sqlite"));
        let flag_db = PathBuf::from("/tmp/aven-flag.sqlite");

        assert_eq!(
            resolve_db_path_from(Some(flag_db.clone()), None, dev_db, &config, true).unwrap(),
            flag_db
        );
    }

    #[test]
    fn release_database_resolution_ignores_dev_environment_path() {
        let mut config = AppConfig::default();
        config.local.db_path = Some(PathBuf::from("/tmp/configured.sqlite"));

        assert_eq!(
            resolve_db_path_from(
                None,
                None,
                Some(PathBuf::from("/tmp/aven-dev.sqlite")),
                &config,
                false,
            )
            .unwrap(),
            PathBuf::from("/tmp/configured.sqlite")
        );
    }

    #[test]
    fn update_automatic_checks_parse_from_config() {
        let disabled: AppConfig =
            serde_yaml::from_str("update:\n  automatic_checks: false\n").unwrap();
        let default: AppConfig = serde_yaml::from_str("{}\n").unwrap();

        assert!(!disabled.update.automatic_checks);
        assert!(default.update.automatic_checks);
    }

    #[test]
    fn update_check_environment_disable_takes_precedence() {
        for value in [Some("1"), Some("true"), Some("YES")] {
            assert!(!automatic_update_checks_enabled(true, value));
        }
        assert!(!automatic_update_checks_enabled(false, None));
        assert!(automatic_update_checks_enabled(true, Some("false")));
    }

    #[test]
    fn sync_disabled_flag_recognizes_true_values() {
        for value in [Some("1"), Some("true"), Some("yes")] {
            assert!(sync_disabled_value(value));
        }
    }

    #[test]
    fn sync_disabled_flag_ignores_other_values() {
        for value in [None, Some("0"), Some("false"), Some("")] {
            assert!(!sync_disabled_value(value));
        }
    }

    #[test]
    fn generated_config_includes_sync_opt_in_without_inactive_override() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");

        write_default_config(&path).unwrap();

        let text = fs::read_to_string(path).unwrap();
        assert!(text.contains("sync:\n  enabled: false\n"));
        assert!(!text.contains("disabled:"));
    }

    #[test]
    fn runtime_disable_override_is_not_serialized() {
        let mut config = AppConfig::default();
        config.sync.enabled = true;
        config.sync.disable_override = true;

        let text = serde_yaml::to_string(&config).unwrap();

        assert!(text.contains("enabled: true"));
        assert!(!text.contains("disabled:"));
    }

    #[test]
    fn effective_sync_predicates_distinguish_opt_in_and_override() {
        let mut disabled = AppConfig::default();
        disabled.sync.enabled = true;
        disabled.sync.disable_override = true;
        let mut enabled = AppConfig::default();
        enabled.sync.enabled = true;
        let unconfigured = AppConfig::default();

        assert!(!disabled.automatic_sync_is_enabled());
        assert!(enabled.automatic_sync_is_enabled());
        assert!(!unconfigured.automatic_sync_is_enabled());
        assert!(unconfigured.sync_is_allowed());
    }

    #[test]
    fn disabled_sync_reports_actionable_error() {
        let mut config = AppConfig::default();
        config.sync.disable_override = true;
        let error = config.ensure_sync_allowed().unwrap_err();

        assert!(format!("{error:#}").contains("sync-disabled"));
        assert!(format!("{error:#}").contains("sync is disabled"));
    }

    #[test]
    fn sync_without_disable_override_is_allowed() {
        AppConfig::default().ensure_sync_allowed().unwrap();
    }

    #[test]
    fn resolves_blob_dir_from_db_path_and_config() {
        let db_path = PathBuf::from("/tmp/aven/db.sqlite");
        let config = AppConfig::default();
        assert_eq!(
            resolve_blob_dir(&db_path, &config).unwrap(),
            PathBuf::from("/tmp/aven/db.sqlite.blobs")
        );

        let mut config = AppConfig::default();
        config.local.blob_dir = Some(PathBuf::from("blobs"));
        assert_eq!(
            resolve_blob_dir(&db_path, &config).unwrap(),
            PathBuf::from("/tmp/aven/blobs")
        );

        config.local.blob_dir = Some(PathBuf::from("/var/aven/blobs"));
        assert_eq!(
            resolve_blob_dir(&db_path, &config).unwrap(),
            PathBuf::from("/var/aven/blobs")
        );
    }

    #[test]
    fn generated_client_config_omits_server_attachment_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");

        write_default_config(&path).unwrap();

        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains("server_grace_days"));
        assert!(!text.contains("server_workspace_quota_bytes"));
        let loaded = AppConfig::load_from_path(&path).unwrap();
        assert_eq!(
            loaded.local.attachment_lifecycle.server_grace_days,
            default_server_attachment_grace_days()
        );
        assert_eq!(
            loaded
                .local
                .attachment_lifecycle
                .server_workspace_quota_bytes,
            default_attachment_quota_bytes()
        );
    }

    #[test]
    fn server_attachment_settings_remain_loadable_and_configurable() {
        let config = load_config(
            "local:\n  attachment_lifecycle:\n    server_grace_days: 12\n    server_workspace_quota_bytes: 345\n",
        )
        .unwrap();

        assert_eq!(config.local.attachment_lifecycle.server_grace_days, 12);
        assert_eq!(
            config
                .local
                .attachment_lifecycle
                .server_workspace_quota_bytes,
            345
        );
        let policy = config.local.attachment_lifecycle.server_policy();
        assert_eq!(
            policy.grace,
            std::time::Duration::from_secs(12 * 24 * 60 * 60)
        );
        assert_eq!(policy.quota_bytes, 345);
    }

    #[test]
    fn local_inline_images_defaults_to_auto() {
        let config = AppConfig::default();

        assert_eq!(config.local.inline_images, InlineImagesConfig::Auto);
    }

    #[test]
    fn local_image_optimization_defaults_to_off() {
        let config = AppConfig::default();

        assert_eq!(
            config.local.image_optimization,
            ImageOptimizationConfig::Off
        );
        assert!(!config.local.image_optimization.optimizes_pasted_images());
        assert!(!config.local.image_optimization.optimizes_file_attachments());
        let yaml = serde_yaml::to_string(&config).unwrap();
        assert!(yaml.contains("image_optimization: off"));

        for (value, expected) in [
            ("paste", ImageOptimizationConfig::Paste),
            ("on", ImageOptimizationConfig::On),
        ] {
            let parsed: AppConfig =
                serde_yaml::from_str(&format!("local:\n  image_optimization: {value}\n")).unwrap();
            assert_eq!(parsed.local.image_optimization, expected);
        }
    }
}
