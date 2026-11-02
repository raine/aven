pub(crate) mod check;
mod registry;
mod streaming;
mod transcription;
mod transport;

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use axum::Json;
use axum::extract::{State, rejection::JsonRejection};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
#[cfg(test)]
use base64::engine::general_purpose::STANDARD;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rig_core::completion::{CompletionRequest as RigRequest, FinishReason as RigFinishReason};
use rig_core::message::{
    AssistantContent, DocumentSourceKind, Image, ImageMediaType, Message as RigMessage,
    ProviderCallId, Reasoning, ToolCall, ToolCallId, ToolFunction, ToolResult, ToolResultContent,
    UserContent,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Semaphore;

use crate::server_config::{AiConfig, ServerConfig};
pub(crate) use registry::{
    AuthKind, ProviderInfo, TESTED, credentials_present, provider_display_name, provider_info,
};
use registry::{CompletionBackend, SchemaStrategy};

pub(crate) const MAX_REQUEST_BYTES: usize = 512 * 1024;
#[cfg(test)]
const MAX_MESSAGES: usize = 64;
const MAX_TEXT_BYTES: usize = 256 * 1024;
const MAX_IMAGES: usize = 4;
#[cfg(test)]
const MAX_IMAGE_BYTES: usize = 320 * 1024;
#[cfg(test)]
const MAX_TOOLS: usize = 16;
#[cfg(test)]
const MAX_TOOL_SCHEMA_BYTES: usize = 32 * 1024;
#[cfg(test)]
const MAX_SCHEMA_DEPTH: usize = 32;

pub(crate) use aven_ai_contract::*;

// Provider replay metadata is part of the correlation token, not client controls.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CallToken {
    id: ToolCallId,
    provider: Option<ProviderCallId>,
    signature: Option<String>,
    additional_params: Option<Value>,
}

fn encode_opaque(value: &impl Serialize) -> std::result::Result<String, ErrorCategory> {
    serde_json::to_vec(value)
        .map(|bytes| format!("v1.{}", URL_SAFE_NO_PAD.encode(bytes)))
        .map_err(|_| ErrorCategory::MalformedProviderResponse)
}

fn decode_opaque<T: serde::de::DeserializeOwned>(
    token: &str,
) -> std::result::Result<T, ErrorCategory> {
    let bytes = token
        .strip_prefix("v1.")
        .and_then(|value| URL_SAFE_NO_PAD.decode(value).ok())
        .ok_or(ErrorCategory::InvalidRequest)?;
    serde_json::from_slice(&bytes).map_err(|_| ErrorCategory::InvalidRequest)
}

pub(crate) struct HttpError(ErrorCategory);
impl From<ErrorCategory> for HttpError {
    fn from(category: ErrorCategory) -> Self {
        Self(category)
    }
}
impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        error_response(self.0)
    }
}
fn error_response(category: ErrorCategory) -> Response {
    (
        StatusCode::from_u16(category.status()).expect("contract HTTP status"),
        Json(ErrorEnvelope::from(category)),
    )
        .into_response()
}

fn classify_provider_error(error: &rig_core::completion::CompletionError) -> ErrorCategory {
    // HTTP status remains authoritative; successful SSE transports can instead
    // carry structured errors without any failure status.
    match error
        .provider_response_status()
        .map(|status| status.as_u16())
    {
        Some(401 | 403) => return ErrorCategory::AuthenticationFailure,
        Some(429) => return ErrorCategory::RateLimit,
        Some(400) => return ErrorCategory::InvalidRequest,
        Some(_) => return ErrorCategory::TemporaryProviderFailure,
        None => {}
    }
    if let Ok(Some(body)) = error.provider_response_json() {
        let detail = body
            .pointer("/response/error")
            .or_else(|| body.get("error"))
            .unwrap_or(&body);
        // Codes are more specific than types (e.g. a rate-limit code can
        // accompany a generic invalid_request_error type). Never match prose.
        for field in ["code", "type"] {
            let category = match detail.get(field).and_then(Value::as_str) {
                Some(
                    "invalid_request_error"
                    | "invalid_request"
                    | "invalid_function_parameters"
                    | "invalid_json_schema"
                    | "context_length_exceeded"
                    | "request_too_large"
                    | "input_too_long"
                    | "string_above_max_length"
                    | "max_tokens_exceeded",
                ) => ErrorCategory::InvalidRequest,
                Some("rate_limit_error" | "rate_limit_exceeded") => ErrorCategory::RateLimit,
                Some("overloaded_error" | "overloaded" | "server_error" | "api_error") => {
                    ErrorCategory::TemporaryProviderFailure
                }
                Some("authentication_error" | "invalid_api_key" | "permission_error") => {
                    ErrorCategory::AuthenticationFailure
                }
                _ => continue,
            };
            return category;
        }
    }
    ErrorCategory::TemporaryProviderFailure
}

enum BackendState {
    Ready {
        backend: Box<dyn CompletionBackend>,
        schema: SchemaStrategy,
        tools: bool,
        transcriber: Option<Box<transcription::Transcriber>>,
    },
    Unavailable {
        status: AvailabilityStatus,
        hint: String,
    },
}

pub(crate) struct AiService {
    state: BackendState,
    provider: Option<String>,
    model: Option<String>,
    max_output_tokens: u64,
    timeout: Duration,
    semaphore: Arc<Semaphore>,
    requests_per_minute: u32,
    recent_requests: Mutex<VecDeque<Instant>>,
}

impl AiService {
    pub(crate) fn from_config(config: &ServerConfig) -> Self {
        let max_output_tokens = config.ai.max_output_tokens();
        let timeout = Duration::from_secs(config.ai.timeout_seconds());
        let semaphore = Arc::new(Semaphore::new(config.ai.max_concurrent_requests()));
        let requests_per_minute = config.ai.requests_per_minute();
        let state = Self::build_state(&config.ai, &config.paths.ai_credentials_dir());
        Self {
            state,
            provider: config.ai.provider.clone(),
            model: config.ai.model.clone(),
            max_output_tokens,
            timeout,
            semaphore,
            requests_per_minute,
            recent_requests: Mutex::new(VecDeque::new()),
        }
    }

    fn build_state(ai: &AiConfig, credentials_dir: &Path) -> BackendState {
        let (Some(provider), Some(model)) = (ai.provider.as_deref(), ai.model.as_deref()) else {
            return BackendState::Unavailable {
                status: AvailabilityStatus::Unconfigured,
                hint: "set ai.provider and ai.model in server.yaml, then restart the server"
                    .to_string(),
            };
        };
        let Some(info) = registry::provider_info(provider) else {
            return BackendState::Unavailable {
                status: AvailabilityStatus::Unconfigured,
                hint: "set ai.provider to a provider listed by `aven server ai providers`, then restart the server".to_string(),
            };
        };
        let private_dir = if info.auth == registry::AuthKind::OAuth {
            match ensure_private_dir(credentials_dir) {
                Ok(path) => Some(path),
                Err(_) => {
                    return BackendState::Unavailable {
                        status: AvailabilityStatus::TemporarilyUnavailable,
                        hint: "make private/ai beside server.yaml a writable directory, then restart the server".to_string(),
                    };
                }
            }
        } else {
            None
        };
        if !registry::credentials_present(*info, private_dir.as_deref()) {
            return BackendState::Unavailable {
                status: AvailabilityStatus::AuthenticationRequired,
                hint: match info.auth {
                    registry::AuthKind::OAuth => format!(
                        "run `aven server ai login {provider}` on the server host, then restart the server"
                    ),
                    registry::AuthKind::ApiKey(name) => {
                        format!("set {name} in the server's environment, then restart the server")
                    }
                    registry::AuthKind::None => {
                        "check the AI provider settings, then restart the server".to_string()
                    }
                },
            };
        }
        match registry::build_backend(*info, model, ai.endpoint.as_deref(), private_dir.as_deref())
        {
            Ok((backend, transcriber)) => BackendState::Ready {
                backend,
                schema: info.schema,
                tools: info.tools,
                transcriber,
            },
            Err(_) => BackendState::Unavailable {
                status: AvailabilityStatus::TemporarilyUnavailable,
                hint: "check the AI provider settings and credentials, then restart the server"
                    .to_string(),
            },
        }
    }

    pub(crate) fn capabilities(&self) -> CapabilitiesResponse {
        let (status, setup_hint) = match &self.state {
            BackendState::Ready { .. } => (AvailabilityStatus::Ready, None),
            BackendState::Unavailable { status, hint } => (*status, Some(hint.clone())),
        };
        CapabilitiesResponse {
            status,
            completion_contract: 1,
            features: Features {
                text: true,
                images: true,
                json_schema: true,
                tools: matches!(self.state, BackendState::Ready { tools: true, .. }),
            },
            limits: Limits {
                max_request_bytes: MAX_REQUEST_BYTES,
                max_images: MAX_IMAGES,
                max_output_tokens: self.max_output_tokens,
            },
            provider: self.provider.clone(),
            model: self.model.clone(),
            setup_hint,
            transcription: transcription::capability(self.transcription_status()),
        }
    }

    fn unavailable_error(&self) -> ErrorCategory {
        match &self.state {
            BackendState::Unavailable {
                status: AvailabilityStatus::AuthenticationRequired,
                ..
            } => ErrorCategory::AuthenticationFailure,
            BackendState::Unavailable {
                status: AvailabilityStatus::TemporarilyUnavailable,
                ..
            } => ErrorCategory::TemporaryProviderFailure,
            _ => ErrorCategory::Unconfigured,
        }
    }

    fn admit(&self) -> std::result::Result<(), ErrorCategory> {
        let now = Instant::now();
        let mut requests = self
            .recent_requests
            .lock()
            .map_err(|_| ErrorCategory::TemporaryProviderFailure)?;
        while requests
            .front()
            .is_some_and(|instant| now.duration_since(*instant) >= Duration::from_secs(60))
        {
            requests.pop_front();
        }
        if requests.len() >= self.requests_per_minute as usize {
            return Err(ErrorCategory::RateLimit);
        }
        requests.push_back(now);
        Ok(())
    }
}

trait ValidateRequest {
    fn validate(self) -> std::result::Result<ValidatedRequest, ErrorCategory>;
}
impl ValidateRequest for CompletionRequest {
    fn validate(self) -> std::result::Result<ValidatedRequest, ErrorCategory> {
        // HTTP admission bounds the original body, not its reserialized representation.
        let mut policy = ValidationPolicy::default();
        policy.limits.max_request_bytes = usize::MAX;
        policy.limits.max_opaque_bytes = usize::MAX;
        aven_ai_contract::validate(&self, &policy)?;
        for message in &self.messages {
            for block in &message.content {
                match block {
                    ContentBlock::ToolCall { id, .. }
                    | ContentBlock::ToolResult { call_id: id, .. } => {
                        let _: CallToken = decode_opaque(id)?;
                    }
                    ContentBlock::Reasoning { opaque } => {
                        let _: Reasoning = decode_opaque(opaque)?;
                    }
                    _ => {}
                }
            }
        }
        Ok(ValidatedRequest(self))
    }
}

fn image_media_type(value: &str) -> std::result::Result<ImageMediaType, ErrorCategory> {
    match value {
        "image/jpeg" => Ok(ImageMediaType::JPEG),
        "image/png" => Ok(ImageMediaType::PNG),
        _ => Err(ErrorCategory::InvalidRequest),
    }
}

fn json_size(value: &Value) -> std::result::Result<usize, ErrorCategory> {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .map_err(|_| ErrorCategory::InvalidRequest)
}

struct ValidatedRequest(CompletionRequest);

impl ValidatedRequest {
    fn requires_tools(&self) -> bool {
        !self.0.tools.is_empty()
            || self.0.messages.iter().any(|message| {
                message.content.iter().any(|block| {
                    matches!(
                        block,
                        ContentBlock::ToolCall { .. } | ContentBlock::ToolResult { .. }
                    )
                })
            })
    }

    fn into_rig(
        self,
        strategy: SchemaStrategy,
        max_output_tokens: u64,
    ) -> std::result::Result<RigRequest, ErrorCategory> {
        let mut request = self.0;
        let output_schema = match request.output {
            OutputRequest::Text => None,
            OutputRequest::JsonSchema { schema } if strategy == SchemaStrategy::Prompted => {
                request.messages.insert(0, Message {
                    role: Role::System,
                    content: vec![ContentBlock::Text { text: format!(
                        "Return only JSON matching this caller-supplied schema. Do not add prose: {schema}"
                    ) }],
                });
                None
            }
            OutputRequest::JsonSchema { schema } => {
                Some(serde_json::from_value(schema).map_err(|_| ErrorCategory::InvalidRequest)?)
            }
        };
        let chat_history = request
            .messages
            .into_iter()
            // A truncated or filtered completion can have no replayable content.
            .filter(|message| !message.content.is_empty())
            .map(|message| {
                let mut user = Vec::new();
                let mut assistant = Vec::new();
                let mut system = Vec::new();
                for block in message.content {
                    match block {
                        ContentBlock::Text { text } => match message.role {
                            Role::System => system.push(text),
                            Role::User => user.push(UserContent::text(text)),
                            Role::Assistant => assistant.push(AssistantContent::Text(text.into())),
                            Role::Tool => return Err(ErrorCategory::InvalidRequest),
                        },
                        ContentBlock::ToolCall {
                            id,
                            name,
                            arguments,
                        } => {
                            let token: CallToken = decode_opaque(&id)?;
                            assistant.push(AssistantContent::ToolCall(ToolCall {
                                id: token.id,
                                provider: token.provider,
                                function: ToolFunction { name, arguments },
                                signature: token.signature,
                                additional_params: token.additional_params,
                            }));
                        }
                        ContentBlock::Reasoning { opaque } => {
                            assistant.push(AssistantContent::Reasoning(decode_opaque(&opaque)?))
                        }
                        ContentBlock::Image { media_type, data } => {
                            user.push(UserContent::Image(Image {
                                data: DocumentSourceKind::Base64(data),
                                media_type: Some(image_media_type(&media_type)?),
                                detail: None,
                                additional_params: None,
                            }))
                        }
                        ContentBlock::ToolResult {
                            call_id,
                            name,
                            output,
                        } => {
                            let token: CallToken = decode_opaque(&call_id)?;
                            user.push(UserContent::ToolResult(ToolResult {
                                call: token.id,
                                provider: token.provider,
                                name,
                                content: vec![ToolResultContent::Text(output.into())],
                            }));
                        }
                    }
                }
                Ok(match message.role {
                    Role::System => RigMessage::System {
                        content: system.join("\n"),
                    },
                    Role::User | Role::Tool => RigMessage::User { content: user },
                    Role::Assistant => RigMessage::Assistant {
                        id: None,
                        content: assistant,
                    },
                })
            })
            .collect::<std::result::Result<Vec<_>, ErrorCategory>>()?;
        Ok(RigRequest {
            model: None,
            preamble: None,
            chat_history,
            documents: Vec::new(),
            tools: request
                .tools
                .into_iter()
                .map(|tool| rig_core::completion::ToolDefinition {
                    name: tool.name,
                    description: tool.description,
                    parameters: tool.parameters,
                })
                .collect(),
            temperature: None,
            max_tokens: Some(max_output_tokens),
            // None leaves choice automatic, including integrations that reject explicit Auto.
            tool_choice: None,
            additional_params: None,
            output_schema,
            record_telemetry_content: false,
        })
    }
}

fn project_response(
    request_id: String,
    response: rig_core::completion::CompletionResponse,
    max_output_tokens: u64,
) -> std::result::Result<CompletionResponse, ErrorCategory> {
    let finish_reason = response.finish_reason();
    let terminal_empty = matches!(
        finish_reason,
        Some(RigFinishReason::Length | RigFinishReason::ContentFilter | RigFinishReason::Other(_))
    );
    if !response.usage.has_values()
        || (response.usage.output_tokens == 0 && !terminal_empty)
        || response.usage.output_tokens > max_output_tokens
    {
        return Err(ErrorCategory::MalformedProviderResponse);
    }
    let mut content = Vec::new();
    let mut text_bytes = 0usize;
    let mut has_tools = false;
    for item in response.choice {
        match item {
            AssistantContent::Text(part) => {
                text_bytes += part.text.len();
                if !part.text.is_empty() {
                    content.push(ContentBlock::Text { text: part.text });
                }
            }
            AssistantContent::Reasoning(reasoning) => content.push(ContentBlock::Reasoning {
                opaque: encode_opaque(&reasoning)?,
            }),
            AssistantContent::ToolCall(call) => {
                has_tools = true;
                text_bytes += json_size(&call.function.arguments)
                    .map_err(|_| ErrorCategory::MalformedProviderResponse)?;
                let id = encode_opaque(&CallToken {
                    id: call.id,
                    provider: call.provider,
                    signature: call.signature,
                    additional_params: call.additional_params,
                })?;
                content.push(ContentBlock::ToolCall {
                    id,
                    name: call.function.name,
                    arguments: call.function.arguments,
                });
            }
            AssistantContent::Image(_) => return Err(ErrorCategory::UnsupportedCapability),
        }
    }
    let max_output_bytes = usize::try_from(max_output_tokens)
        .unwrap_or(usize::MAX)
        .saturating_mul(8)
        .min(MAX_TEXT_BYTES);
    if (content.is_empty() && !terminal_empty)
        || text_bytes > max_output_bytes
        || serde_json::to_vec(&content)
            .map_err(|_| ErrorCategory::MalformedProviderResponse)?
            .len()
            > MAX_REQUEST_BYTES / 2
    {
        return Err(ErrorCategory::MalformedProviderResponse);
    }
    let finish_reason = match finish_reason {
        Some(RigFinishReason::Stop) | None if has_tools => FinishReason::ToolCalls,
        Some(RigFinishReason::Stop) | None => FinishReason::Stop,
        Some(RigFinishReason::Length) => FinishReason::Length,
        Some(RigFinishReason::ToolCalls) => FinishReason::ToolCalls,
        Some(RigFinishReason::ContentFilter | RigFinishReason::Other(_)) => FinishReason::Error,
    };
    Ok(CompletionResponse {
        request_id,
        message: AssistantMessage {
            role: "assistant".into(),
            content,
        },
        finish_reason,
        usage: Usage {
            input_tokens: response.usage.input_tokens,
            output_tokens: response.usage.output_tokens,
        },
    })
}

pub(crate) fn ensure_private_dir(path: &Path) -> Result<PathBuf> {
    fs::create_dir_all(path)
        .with_context(|| format!("create AI credential directory {}", path.display()))?;
    secure_private_dir(path)?;
    Ok(path.to_path_buf())
}

#[cfg(unix)]
fn secure_private_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        bail!("AI credential path must be a directory");
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn secure_private_dir(path: &Path) -> Result<()> {
    if !fs::symlink_metadata(path)?.file_type().is_dir() {
        bail!("AI credential path must be a directory");
    }
    Ok(())
}

pub(crate) async fn capabilities_handler(
    State(service): State<Arc<AiService>>,
) -> Json<CapabilitiesResponse> {
    Json(service.capabilities())
}

pub(crate) use streaming::streaming_handler;
pub(crate) use transcription::transcription_handler;

pub(crate) fn providers() -> &'static [registry::ProviderInfo] {
    registry::PROVIDERS
}

pub(crate) fn exclusions() -> &'static [(&'static str, &'static str)] {
    registry::EXCLUSIONS
}

/// Runs the interactive OAuth device flow for `provider`.
pub(crate) async fn login(provider: &str, credentials_dir: &Path) -> Result<()> {
    let info = registry::provider_info(provider)
        .with_context(|| format!("unsupported AI provider {provider}"))?;
    if info.auth != registry::AuthKind::OAuth {
        bail!(
            "provider {provider} uses {} instead of interactive login",
            info.auth.description()
        );
    }
    let dir = ensure_private_dir(credentials_dir)?;
    registry::login(provider, &dir).await
}

/// Removes stored OAuth credentials for `provider` and returns the removed
/// path, or `None` when nothing was stored.
pub(crate) fn logout(provider: &str, credentials_dir: &Path) -> Result<Option<PathBuf>> {
    let info = registry::provider_info(provider)
        .with_context(|| format!("unsupported AI provider {provider}"))?;
    if info.auth != registry::AuthKind::OAuth {
        bail!(
            "provider {provider} uses {} instead of stored login credentials",
            info.auth.description()
        );
    }
    let path = credentials_dir.join(provider);
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("inspect {}", path.display()));
        }
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(&path),
        Ok(_) => fs::remove_file(&path),
    }
    .with_context(|| format!("remove {}", path.display()))?;
    Ok(Some(path))
}

/// Runs the synthetic tool-call exchange against this process's service.
pub(crate) async fn check(service: &AiService) -> check::Outcome<ErrorCategory> {
    check::run(|request| async move {
        let request: CompletionRequest =
            serde_json::from_value(request).map_err(|_| ErrorCategory::InvalidRequest)?;
        let response = service.collect_completion(request).await?;
        serde_json::to_value(response).map_err(|_| ErrorCategory::MalformedProviderResponse)
    })
    .await
}

#[cfg(test)]
mod tests;
