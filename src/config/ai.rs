use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

pub(crate) const DEFAULT_AI_TIMEOUT_SECONDS: u64 = 45;
pub(crate) const DEFAULT_AI_MAX_OUTPUT_TOKENS: u64 = 2048;
pub(crate) const DEFAULT_AI_MAX_CONCURRENT_REQUESTS: usize = 2;
pub(crate) const DEFAULT_AI_REQUESTS_PER_MINUTE: u32 = 60;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent_requests: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requests_per_minute: Option<u32>,
}

impl AiConfig {
    pub fn is_empty(&self) -> bool {
        self.provider.is_none()
            && self.model.is_none()
            && self.endpoint.is_none()
            && self.timeout_seconds.is_none()
            && self.max_output_tokens.is_none()
            && self.max_concurrent_requests.is_none()
            && self.requests_per_minute.is_none()
    }

    pub fn timeout_seconds(&self) -> u64 {
        self.timeout_seconds.unwrap_or(DEFAULT_AI_TIMEOUT_SECONDS)
    }

    pub fn max_output_tokens(&self) -> u64 {
        self.max_output_tokens
            .unwrap_or(DEFAULT_AI_MAX_OUTPUT_TOKENS)
    }

    pub fn max_concurrent_requests(&self) -> usize {
        self.max_concurrent_requests
            .unwrap_or(DEFAULT_AI_MAX_CONCURRENT_REQUESTS)
    }

    pub fn requests_per_minute(&self) -> u32 {
        self.requests_per_minute
            .unwrap_or(DEFAULT_AI_REQUESTS_PER_MINUTE)
    }
}

pub(super) fn validate(config: &AiConfig) -> Result<()> {
    if config
        .provider
        .as_deref()
        .is_some_and(|value| value.trim().is_empty())
    {
        bail!("ai.provider must not be blank");
    }
    if config
        .model
        .as_deref()
        .is_some_and(|value| value.trim().is_empty())
    {
        bail!("ai.model must not be blank");
    }
    if config.provider.is_some() != config.model.is_some() {
        bail!("ai.provider and ai.model must be configured together");
    }
    if !(1..=300).contains(&config.timeout_seconds()) {
        bail!("ai.timeout_seconds must be between 1 and 300");
    }
    if !(1..=16_384).contains(&config.max_output_tokens()) {
        bail!("ai.max_output_tokens must be between 1 and 16384");
    }
    if !(1..=32).contains(&config.max_concurrent_requests()) {
        bail!("ai.max_concurrent_requests must be between 1 and 32");
    }
    if !(1..=600).contains(&config.requests_per_minute()) {
        bail!("ai.requests_per_minute must be between 1 and 600");
    }
    if let Some(endpoint) = config.endpoint.as_deref() {
        let url = url::Url::parse(endpoint).context("ai.endpoint must be an absolute URL")?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            bail!("ai.endpoint must be an absolute http or https URL");
        }
        if !matches!(
            config.provider.as_deref(),
            Some("llamafile" | "ollama" | "openai")
        ) {
            bail!("ai.endpoint is supported only for llamafile, ollama, and openai");
        }
    }
    Ok(())
}
