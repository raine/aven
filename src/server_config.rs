//! Settings for `aven server`, read from an optional `server.yaml` that lives
//! beside the server storage. The server never reads the client `config.yaml`.

mod ai;

use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

pub(crate) use ai::AiConfig;

use crate::attachments::lifecycle::{
    DEFAULT_MAINTENANCE_LIMIT, DEFAULT_ORIGINAL_QUOTA_BYTES, DEFAULT_PREVIEW_QUOTA_BYTES,
    LifecyclePolicy,
};

pub(crate) const SERVER_CONFIG_ENV: &str = "AVEN_SERVER_CONFIG";
const SERVER_CONFIG_FILE: &str = "server.yaml";
const DEFAULT_ATTACHMENT_GRACE_DAYS: u64 = 30;
const AI_PROVIDER_ENV: &str = "AVEN_AI_PROVIDER";
const AI_MODEL_ENV: &str = "AVEN_AI_MODEL";
const AI_ENDPOINT_ENV: &str = "AVEN_AI_ENDPOINT";

/// Where the server keeps its storage, configuration and credentials.
#[derive(Debug, Clone)]
pub(crate) struct ServerPaths {
    pub(crate) data: PathBuf,
    pub(crate) config_file: PathBuf,
}

impl ServerPaths {
    /// Resolves `--data` and `--config`, falling back to `AVEN_SERVER_CONFIG`
    /// and then `server.yaml` in the directory that holds the data file.
    pub(crate) fn resolve(data: Option<PathBuf>, config: Option<PathBuf>) -> Result<Self> {
        let data = match data {
            Some(path) => path,
            None => crate::config::default_server_data_path()?,
        };
        Ok(Self::from_parts(
            data,
            config,
            env::var_os(SERVER_CONFIG_ENV),
        ))
    }

    fn from_parts(data: PathBuf, config: Option<PathBuf>, env_config: Option<OsString>) -> Self {
        let config_file = config
            .or_else(|| {
                env_config
                    .filter(|value| !value.is_empty())
                    .map(PathBuf::from)
            })
            .unwrap_or_else(|| parent_dir(&data).join(SERVER_CONFIG_FILE));
        Self { data, config_file }
    }

    pub(crate) fn config_dir(&self) -> PathBuf {
        parent_dir(&self.config_file)
    }

    pub(crate) fn ai_credentials_dir(&self) -> PathBuf {
        self.config_dir().join("private").join("ai")
    }
}

fn parent_dir(path: &Path) -> PathBuf {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ServerConfigFile {
    #[serde(default)]
    ai: AiConfig,
    #[serde(default)]
    attachments: AttachmentsConfig,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AttachmentsConfig {
    #[serde(default = "default_attachment_grace_days")]
    pub(crate) grace_days: u64,
    #[serde(default = "default_workspace_quota_bytes")]
    pub(crate) workspace_quota_bytes: u64,
}

fn default_attachment_grace_days() -> u64 {
    DEFAULT_ATTACHMENT_GRACE_DAYS
}

fn default_workspace_quota_bytes() -> u64 {
    DEFAULT_ORIGINAL_QUOTA_BYTES.unsigned_abs()
}

impl Default for AttachmentsConfig {
    fn default() -> Self {
        Self {
            grace_days: default_attachment_grace_days(),
            workspace_quota_bytes: default_workspace_quota_bytes(),
        }
    }
}

impl AttachmentsConfig {
    fn validate(&self) -> Result<()> {
        if i64::try_from(self.workspace_quota_bytes).is_err() {
            bail!(
                "attachments.workspace_quota_bytes must be at most {}",
                i64::MAX
            );
        }
        Ok(())
    }

    pub(crate) fn policy(self) -> LifecyclePolicy {
        LifecyclePolicy {
            grace: Duration::from_secs(self.grace_days.saturating_mul(24 * 60 * 60)),
            quota_bytes: i64::try_from(self.workspace_quota_bytes).unwrap_or(i64::MAX),
            preview_quota_bytes: DEFAULT_PREVIEW_QUOTA_BYTES,
            maintenance_limit: DEFAULT_MAINTENANCE_LIMIT,
        }
    }
}

/// Where an AI setting's effective value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SettingSource {
    Unset,
    File,
    Env,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct AiSources {
    pub(crate) provider: SettingSource,
    pub(crate) model: SettingSource,
    pub(crate) endpoint: SettingSource,
}

/// Effective server settings: `server.yaml` with `AVEN_AI_*` overrides.
#[derive(Debug, Clone)]
pub(crate) struct ServerConfig {
    pub(crate) paths: ServerPaths,
    pub(crate) ai: AiConfig,
    pub(crate) ai_sources: AiSources,
    pub(crate) attachments: AttachmentsConfig,
}

impl ServerConfig {
    pub(crate) fn load(paths: ServerPaths) -> Result<Self> {
        Self::load_with_env(paths, |name| env::var(name).ok())
    }

    fn load_with_env(paths: ServerPaths, var: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let path = &paths.config_file;
        let file = match fs::read_to_string(path) {
            Ok(text) => parse(&text)
                .map_err(|error| anyhow!("could not parse {}: {error:#}", path.display()))?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => ServerConfigFile::default(),
            Err(error) => {
                bail!("could not read {}: {error}", path.display());
            }
        };
        file.attachments
            .validate()
            .map_err(|error| anyhow!("invalid server config {}: {error:#}", path.display()))?;
        let mut ai = file.ai;
        let mut overridden = Vec::new();
        let mut apply = |slot: &mut Option<String>, name: &'static str| {
            let file_source = if slot.is_some() {
                SettingSource::File
            } else {
                SettingSource::Unset
            };
            match var(name).filter(|value| !value.is_empty()) {
                Some(value) => {
                    *slot = Some(value);
                    overridden.push(name);
                    SettingSource::Env
                }
                None => file_source,
            }
        };
        let ai_sources = AiSources {
            provider: apply(&mut ai.provider, AI_PROVIDER_ENV),
            model: apply(&mut ai.model, AI_MODEL_ENV),
            endpoint: apply(&mut ai.endpoint, AI_ENDPOINT_ENV),
        };
        ai::validate(&ai).map_err(|error| {
            let overrides = if overridden.is_empty() {
                String::new()
            } else {
                format!(" with {} set", overridden.join(", "))
            };
            anyhow!(
                "invalid server config {}{overrides}: {error:#}",
                path.display()
            )
        })?;
        Ok(Self {
            paths,
            ai,
            ai_sources,
            attachments: file.attachments,
        })
    }
}

fn parse(text: &str) -> Result<ServerConfigFile> {
    // A file of only comments, such as the setup template, is an empty document.
    if text
        .lines()
        .all(|line| line.trim().is_empty() || line.trim_start().starts_with('#'))
    {
        return Ok(ServerConfigFile::default());
    }
    Ok(serde_saphyr::from_str(text)?)
}

fn template() -> String {
    format!(
        r#"# Aven server configuration. Changes apply after the server restarts.
# Every setting is optional. Uncomment a line to change it.
# AVEN_AI_PROVIDER, AVEN_AI_MODEL and AVEN_AI_ENDPOINT override the ai values.

# ai:
#   provider: ollama              # see `aven server ai providers`
#   model: llama3.2
#   endpoint: http://127.0.0.1:11434  # llamafile, ollama and openai only
#   timeout_seconds: {timeout}
#   max_output_tokens: {max_output_tokens}
#   max_concurrent_requests: {max_concurrent}
#   requests_per_minute: {requests_per_minute}

# attachments:
#   grace_days: {grace_days}                # minimum age before unused images are pruned
#   workspace_quota_bytes: {quota}  # image storage limit per workspace
"#,
        timeout = ai::DEFAULT_AI_TIMEOUT_SECONDS,
        max_output_tokens = ai::DEFAULT_AI_MAX_OUTPUT_TOKENS,
        max_concurrent = ai::DEFAULT_AI_MAX_CONCURRENT_REQUESTS,
        requests_per_minute = ai::DEFAULT_AI_REQUESTS_PER_MINUTE,
        grace_days = DEFAULT_ATTACHMENT_GRACE_DAYS,
        quota = default_workspace_quota_bytes(),
    )
}

/// Writes the commented template unless a server config already exists.
/// Returns whether a file was written.
pub(crate) fn write_template_if_missing(path: &Path) -> Result<bool> {
    fs::create_dir_all(parent_dir(path))
        .with_context(|| format!("create server config directory for {}", path.display()))?;
    let mut file = match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => return Ok(false),
        Err(error) => {
            return Err(error).with_context(|| format!("could not create {}", path.display()));
        }
    };
    file.write_all(template().as_bytes())
        .with_context(|| format!("could not write {}", path.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests;
