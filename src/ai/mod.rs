mod registry;
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
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rig_core::completion::{CompletionRequest as RigRequest, FinishReason as RigFinishReason};
use rig_core::message::{
    AssistantContent, Message as RigMessage, ProviderCallId, Reasoning, ToolCall, ToolCallId,
    ToolFunction, ToolResult, ToolResultContent, UserContent,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Semaphore;
use tracing::{info, warn};

use crate::server_config::{AiConfig, ServerConfig};
use registry::{CompletionBackend, SchemaStrategy};

pub(crate) const MAX_REQUEST_BYTES: usize = 512 * 1024;
const MAX_MESSAGES: usize = 64;
const MAX_TEXT_BYTES: usize = 256 * 1024;
const MAX_TOOLS: usize = 16;
const MAX_TOOL_SCHEMA_BYTES: usize = 32 * 1024;
const MAX_SCHEMA_BYTES: usize = 24 * 1024;
const MAX_SCHEMA_DEPTH: usize = 32;
const MAX_REQUEST_ID_BYTES: usize = 128;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct CapabilitiesResponse {
    pub(crate) status: AvailabilityStatus,
    pub(crate) completion_contract: u32,
    pub(crate) features: Features,
    pub(crate) limits: Limits,
    pub(crate) setup_hint: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AvailabilityStatus {
    Ready,
    Unconfigured,
    AuthenticationRequired,
    TemporarilyUnavailable,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Features {
    text: bool,
    json_schema: bool,
    tools: bool,
    streaming: bool,
    images: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Limits {
    max_request_bytes: usize,
    max_output_tokens: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CompletionRequest {
    request_id: String,
    messages: Vec<Message>,
    output: OutputRequest,
    #[serde(default)]
    tools: Vec<ToolDefinition>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Message {
    role: Role,
    content: Vec<ContentBlock>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum ContentBlock {
    Text {
        text: String,
    },
    ToolCall {
        id: String,
        name: String,
        arguments: Value,
    },
    Reasoning {
        opaque: String,
    },
    ToolResult {
        call_id: String,
        name: String,
        output: String,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolDefinition {
    name: String,
    description: String,
    parameters: Value,
}

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

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum OutputRequest {
    Text,
    JsonSchema { schema: Value },
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct CompletionResponse {
    request_id: String,
    message: AssistantMessage,
    finish_reason: FinishReason,
    usage: Usage,
}

#[derive(Debug, Clone, Serialize)]
struct AssistantMessage {
    role: &'static str,
    content: Vec<ContentBlock>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum FinishReason {
    Stop,
    Length,
    Error,
    ToolCalls,
}

#[derive(Debug, Clone, Serialize)]
struct Usage {
    input_tokens: u64,
    output_tokens: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub(crate) enum ErrorCategory {
    InvalidRequest,
    UnsupportedCapability,
    Unconfigured,
    AuthenticationFailure,
    RateLimit,
    Timeout,
    MalformedProviderResponse,
    CanceledRequest,
    TemporaryProviderFailure,
}

#[derive(Debug, Clone, Serialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

#[derive(Debug, Clone, Serialize)]
struct ErrorBody {
    category: ErrorCategory,
    retryable: bool,
}

impl ErrorCategory {
    fn status(self) -> StatusCode {
        match self {
            Self::InvalidRequest | Self::UnsupportedCapability => StatusCode::BAD_REQUEST,
            Self::AuthenticationFailure => StatusCode::SERVICE_UNAVAILABLE,
            Self::RateLimit => StatusCode::TOO_MANY_REQUESTS,
            Self::Timeout => StatusCode::GATEWAY_TIMEOUT,
            Self::MalformedProviderResponse => StatusCode::BAD_GATEWAY,
            Self::CanceledRequest => StatusCode::REQUEST_TIMEOUT,
            Self::Unconfigured | Self::TemporaryProviderFailure => StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    fn retryable(self) -> bool {
        matches!(
            self,
            Self::RateLimit
                | Self::Timeout
                | Self::CanceledRequest
                | Self::TemporaryProviderFailure
        )
    }
}

impl IntoResponse for ErrorCategory {
    fn into_response(self) -> Response {
        let envelope = ErrorEnvelope {
            error: ErrorBody {
                category: self,
                retryable: self.retryable(),
            },
        };
        (self.status(), Json(envelope)).into_response()
    }
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
        provider: String,
    },
    Unavailable {
        status: AvailabilityStatus,
        hint: String,
    },
}

pub(crate) struct AiService {
    state: BackendState,
    max_output_tokens: u64,
    timeout: Duration,
    semaphore: Semaphore,
    requests_per_minute: u32,
    recent_requests: Mutex<VecDeque<Instant>>,
}

impl AiService {
    pub(crate) fn from_config(config: &ServerConfig) -> Self {
        let max_output_tokens = config.ai.max_output_tokens();
        let timeout = Duration::from_secs(config.ai.timeout_seconds());
        let semaphore = Semaphore::new(config.ai.max_concurrent_requests());
        let requests_per_minute = config.ai.requests_per_minute();
        let state = Self::build_state(&config.ai, &config.paths.ai_credentials_dir());
        Self {
            state,
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
                hint: "configure ai.provider and ai.model in server.yaml".to_string(),
            };
        };
        let Some(info) = registry::provider_info(provider) else {
            return BackendState::Unavailable {
                status: AvailabilityStatus::Unconfigured,
                hint: "select a provider listed by aven server ai providers".to_string(),
            };
        };
        let private_dir = if info.auth == registry::AuthKind::OAuth {
            match ensure_private_dir(credentials_dir) {
                Ok(path) => Some(path),
                Err(_) => {
                    return BackendState::Unavailable {
                        status: AvailabilityStatus::TemporarilyUnavailable,
                        hint: "AI credential storage is unavailable".to_string(),
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
                    registry::AuthKind::OAuth => format!("run aven server ai login {provider}"),
                    registry::AuthKind::ApiKey(name) => {
                        format!("set {name} for the server process")
                    }
                    registry::AuthKind::None => "check provider configuration".to_string(),
                },
            };
        }
        match registry::build_backend(*info, model, ai.endpoint.as_deref(), private_dir.as_deref())
        {
            Ok(backend) => BackendState::Ready {
                backend,
                schema: info.schema,
                tools: info.tools,
                provider: provider.to_string(),
            },
            Err(_) => BackendState::Unavailable {
                status: AvailabilityStatus::TemporarilyUnavailable,
                hint: "check the configured AI provider and credentials".to_string(),
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
                json_schema: true,
                tools: matches!(self.state, BackendState::Ready { tools: true, .. }),
                streaming: false,
                images: false,
            },
            limits: Limits {
                max_request_bytes: MAX_REQUEST_BYTES,
                max_output_tokens: self.max_output_tokens,
            },
            setup_hint,
        }
    }

    pub(crate) async fn complete(
        &self,
        request: CompletionRequest,
    ) -> std::result::Result<CompletionResponse, ErrorCategory> {
        let validated = request.validate()?;
        let BackendState::Ready {
            backend,
            schema,
            tools,
            provider,
        } = &self.state
        else {
            return Err(self.unavailable_error());
        };
        if !tools && validated.requires_tools() {
            return Err(ErrorCategory::UnsupportedCapability);
        }
        let request_id = validated.0.request_id.clone();
        let rig_request = validated.into_rig(*schema, self.max_output_tokens)?;
        self.admit()?;
        let permit = self
            .semaphore
            .try_acquire()
            .map_err(|_| ErrorCategory::RateLimit)?;
        let started = Instant::now();
        let result = tokio::time::timeout(self.timeout, backend.complete(rig_request)).await;
        drop(permit);
        let response = match result {
            Err(_) => return Err(ErrorCategory::Timeout),
            Ok(Err(error)) => {
                let category = classify_provider_error(&error);
                warn!(provider, outcome = ?category, duration_ms = started.elapsed().as_millis(), "AI completion failed");
                return Err(category);
            }
            Ok(Ok(response)) => response,
        };
        let response = project_response(request_id, response, self.max_output_tokens)?;
        info!(
            provider,
            outcome = "success",
            duration_ms = started.elapsed().as_millis(),
            input_tokens = response.usage.input_tokens,
            output_tokens = response.usage.output_tokens,
            "AI completion finished"
        );
        Ok(response)
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

impl CompletionRequest {
    fn validate(self) -> std::result::Result<ValidatedRequest, ErrorCategory> {
        if self.request_id.is_empty()
            || self.request_id.len() > MAX_REQUEST_ID_BYTES
            || !self
                .request_id
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        {
            return Err(ErrorCategory::InvalidRequest);
        }
        if self.messages.is_empty() || self.messages.len() > MAX_MESSAGES {
            return Err(ErrorCategory::InvalidRequest);
        }
        let mut text_bytes = 0usize;
        for message in &self.messages {
            if message.content.is_empty() && !matches!(message.role, Role::Assistant) {
                return Err(ErrorCategory::InvalidRequest);
            }
            for block in &message.content {
                match (&message.role, block) {
                    (Role::System | Role::User | Role::Assistant, ContentBlock::Text { text }) => {
                        if text.is_empty() {
                            return Err(ErrorCategory::InvalidRequest);
                        }
                        text_bytes += text.len();
                    }
                    (
                        Role::Assistant,
                        ContentBlock::ToolCall {
                            id,
                            name,
                            arguments,
                        },
                    ) => {
                        let _: CallToken = decode_opaque(id)?;
                        if name.is_empty() {
                            return Err(ErrorCategory::InvalidRequest);
                        }
                        text_bytes += json_size(arguments)?;
                    }
                    (Role::Assistant, ContentBlock::Reasoning { opaque }) => {
                        let _: Reasoning = decode_opaque(opaque)?;
                    }
                    (
                        Role::Tool,
                        ContentBlock::ToolResult {
                            call_id,
                            name,
                            output,
                        },
                    ) => {
                        let _: CallToken = decode_opaque(call_id)?;
                        if name.is_empty() {
                            return Err(ErrorCategory::InvalidRequest);
                        }
                        text_bytes += output.len();
                    }
                    _ => return Err(ErrorCategory::InvalidRequest),
                }
            }
        }
        if text_bytes > MAX_TEXT_BYTES || self.tools.len() > MAX_TOOLS {
            return Err(ErrorCategory::InvalidRequest);
        }
        let mut schema_bytes = 0;
        let mut names = std::collections::HashSet::new();
        for tool in &self.tools {
            if tool.name.is_empty() || !names.insert(&tool.name) {
                return Err(ErrorCategory::InvalidRequest);
            }
            schema_bytes += validate_schema(&tool.parameters, MAX_TOOL_SCHEMA_BYTES)?;
        }
        if schema_bytes > MAX_TOOL_SCHEMA_BYTES {
            return Err(ErrorCategory::InvalidRequest);
        }
        if let OutputRequest::JsonSchema { schema } = &self.output {
            validate_schema(schema, MAX_SCHEMA_BYTES)?;
        }
        Ok(ValidatedRequest(self))
    }
}

fn json_size(value: &Value) -> std::result::Result<usize, ErrorCategory> {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .map_err(|_| ErrorCategory::InvalidRequest)
}

fn validate_schema(schema: &Value, max_bytes: usize) -> std::result::Result<usize, ErrorCategory> {
    if !schema.is_object() || json_depth(schema) > MAX_SCHEMA_DEPTH {
        return Err(ErrorCategory::InvalidRequest);
    }
    let bytes = json_size(schema)?;
    if bytes > max_bytes {
        return Err(ErrorCategory::InvalidRequest);
    }
    Ok(bytes)
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
            role: "assistant",
            content,
        },
        finish_reason,
        usage: Usage {
            input_tokens: response.usage.input_tokens,
            output_tokens: response.usage.output_tokens,
        },
    })
}

fn json_depth(value: &Value) -> usize {
    match value {
        Value::Array(values) => 1 + values.iter().map(json_depth).max().unwrap_or(0),
        Value::Object(values) => 1 + values.values().map(json_depth).max().unwrap_or(0),
        _ => 1,
    }
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

pub(crate) async fn completion_handler(
    State(service): State<Arc<AiService>>,
    request: std::result::Result<Json<CompletionRequest>, JsonRejection>,
) -> std::result::Result<Json<CompletionResponse>, ErrorCategory> {
    let Json(request) = request.map_err(|_| ErrorCategory::InvalidRequest)?;
    service.complete(request).await.map(Json)
}

pub(crate) fn providers() -> &'static [registry::ProviderInfo] {
    registry::PROVIDERS
}

pub(crate) fn exclusions() -> &'static [(&'static str, &'static str)] {
    registry::EXCLUSIONS
}

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

pub(crate) async fn check(config: &ServerConfig) -> Result<CompletionResponse> {
    let request: CompletionRequest = serde_json::from_value(serde_json::json!({
        "request_id": "aven-ai-check",
        "messages": [
            {"role": "system", "content": [{"type": "text", "text": "Return only the requested JSON."}]},
            {"role": "user", "content": [{"type": "text", "text": "Return a JSON object with ok set to true."}]}
        ],
        "output": {
            "type": "json_schema",
            "schema": {
                "type": "object",
                "properties": {"ok": {"type": "boolean"}},
                "required": ["ok"],
                "additionalProperties": false
            }
        }
    }))?;
    AiService::from_config(config)
        .complete(request)
        .await
        .map_err(|error| anyhow::anyhow!("AI check failed: {:?}", error))
}

#[cfg(test)]
mod tests;
