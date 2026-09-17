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
use rig_core::completion::{CompletionRequest as RigRequest, FinishReason as RigFinishReason};
use rig_core::message::{AssistantContent, Message as RigMessage, UserContent};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Semaphore;
use tracing::{info, warn};

use crate::config::{self, AiConfig};
use registry::{CompletionBackend, SchemaStrategy};

pub(crate) const MAX_REQUEST_BYTES: usize = 65_536;
const MAX_MESSAGES: usize = 8;
const MAX_BLOCKS_PER_MESSAGE: usize = 8;
const MAX_TEXT_BYTES: usize = 48 * 1024;
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
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Message {
    role: Role,
    content: Vec<TextBlock>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Role {
    System,
    User,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum TextBlock {
    Text { text: String },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum OutputRequest {
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
    content: Vec<AssistantTextBlock>,
}

#[derive(Debug, Clone, Serialize)]
struct AssistantTextBlock {
    #[serde(rename = "type")]
    kind: &'static str,
    text: String,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
enum FinishReason {
    Stop,
    Length,
    Error,
    Aborted,
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

enum BackendState {
    Ready {
        backend: Box<dyn CompletionBackend>,
        schema: SchemaStrategy,
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
    pub(crate) fn from_config(config: &config::AppConfig) -> Self {
        let max_output_tokens = config.ai.max_output_tokens();
        let timeout = Duration::from_secs(config.ai.timeout_seconds());
        let semaphore = Semaphore::new(config.ai.max_concurrent_requests());
        let requests_per_minute = config.ai.requests_per_minute();
        let state = Self::build_state(&config.ai);
        Self {
            state,
            max_output_tokens,
            timeout,
            semaphore,
            requests_per_minute,
            recent_requests: Mutex::new(VecDeque::new()),
        }
    }

    fn build_state(ai: &AiConfig) -> BackendState {
        let (Some(provider), Some(model)) = (ai.provider.as_deref(), ai.model.as_deref()) else {
            return BackendState::Unavailable {
                status: AvailabilityStatus::Unconfigured,
                hint: "configure ai.provider and ai.model".to_string(),
            };
        };
        let Some(info) = registry::provider_info(provider) else {
            return BackendState::Unavailable {
                status: AvailabilityStatus::Unconfigured,
                hint: "select a provider listed by aven ai providers".to_string(),
            };
        };
        let private_dir = if info.auth == registry::AuthKind::OAuth {
            match ensure_private_dir() {
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
                    registry::AuthKind::OAuth => format!("run aven ai login {provider}"),
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
                tools: false,
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
            provider,
        } = &self.state
        else {
            return Err(self.unavailable_error());
        };
        self.admit()?;
        let permit = self
            .semaphore
            .try_acquire()
            .map_err(|_| ErrorCategory::RateLimit)?;
        let request_id = validated.request_id.clone();
        let rig_request = validated.into_rig(*schema, self.max_output_tokens)?;
        let started = Instant::now();
        let result = tokio::time::timeout(self.timeout, backend.complete(rig_request)).await;
        drop(permit);
        let response = match result {
            Err(_) => return Err(ErrorCategory::Timeout),
            Ok(Err(error)) => {
                let category = match error
                    .provider_response_status()
                    .map(|status| status.as_u16())
                {
                    Some(401 | 403) => ErrorCategory::AuthenticationFailure,
                    Some(429) => ErrorCategory::RateLimit,
                    Some(400) => ErrorCategory::InvalidRequest,
                    Some(500..=599) | None => ErrorCategory::TemporaryProviderFailure,
                    Some(_) => ErrorCategory::TemporaryProviderFailure,
                };
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
            if message.content.is_empty() || message.content.len() > MAX_BLOCKS_PER_MESSAGE {
                return Err(ErrorCategory::InvalidRequest);
            }
            for block in &message.content {
                match block {
                    TextBlock::Text { text } => {
                        if text.is_empty() {
                            return Err(ErrorCategory::InvalidRequest);
                        }
                        text_bytes = text_bytes.saturating_add(text.len());
                    }
                }
            }
        }
        if text_bytes > MAX_TEXT_BYTES {
            return Err(ErrorCategory::InvalidRequest);
        }
        let OutputRequest::JsonSchema { schema } = &self.output;
        let schema_bytes = serde_json::to_vec(schema).map_err(|_| ErrorCategory::InvalidRequest)?;
        if schema_bytes.len() > MAX_SCHEMA_BYTES || json_depth(schema) > MAX_SCHEMA_DEPTH {
            return Err(ErrorCategory::InvalidRequest);
        }
        if !schema.is_object() {
            return Err(ErrorCategory::InvalidRequest);
        }
        Ok(ValidatedRequest {
            request_id: self.request_id,
            messages: self.messages,
            schema: schema.clone(),
        })
    }
}

struct ValidatedRequest {
    request_id: String,
    messages: Vec<Message>,
    schema: Value,
}

impl ValidatedRequest {
    fn into_rig(
        mut self,
        strategy: SchemaStrategy,
        max_output_tokens: u64,
    ) -> std::result::Result<RigRequest, ErrorCategory> {
        if strategy == SchemaStrategy::Prompted {
            let schema =
                serde_json::to_string(&self.schema).map_err(|_| ErrorCategory::InvalidRequest)?;
            self.messages.insert(
                0,
                Message {
                    role: Role::System,
                    content: vec![TextBlock::Text {
                        text: format!(
                            "Return only JSON matching this caller-supplied schema. Do not add prose: {schema}"
                        ),
                    }],
                },
            );
        }
        let chat_history = self
            .messages
            .into_iter()
            .map(|message| {
                let text = message
                    .content
                    .into_iter()
                    .map(|block| match block {
                        TextBlock::Text { text } => text,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                match message.role {
                    Role::System => RigMessage::System { content: text },
                    Role::User => RigMessage::User {
                        content: vec![UserContent::text(text)],
                    },
                }
            })
            .collect();
        let output_schema = if strategy == SchemaStrategy::Prompted {
            None
        } else {
            Some(serde_json::from_value(self.schema).map_err(|_| ErrorCategory::InvalidRequest)?)
        };
        Ok(RigRequest {
            model: None,
            preamble: None,
            chat_history,
            documents: Vec::new(),
            tools: Vec::new(),
            temperature: None,
            max_tokens: Some(max_output_tokens),
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
    if !response.usage.has_values()
        || response.usage.output_tokens == 0
        || response.usage.output_tokens > max_output_tokens
    {
        return Err(ErrorCategory::MalformedProviderResponse);
    }
    let finish_reason = response.finish_reason();
    let mut text = String::new();
    for item in response.choice {
        match item {
            AssistantContent::Text(part) => text.push_str(&part.text),
            AssistantContent::Reasoning(_) => {}
            AssistantContent::ToolCall(_) | AssistantContent::Image(_) => {
                return Err(ErrorCategory::UnsupportedCapability);
            }
        }
    }
    let max_output_bytes = usize::try_from(max_output_tokens)
        .unwrap_or(usize::MAX)
        .saturating_mul(8)
        .min(256 * 1024);
    if text.is_empty() || text.len() > max_output_bytes {
        return Err(ErrorCategory::MalformedProviderResponse);
    }
    let finish_reason = match finish_reason {
        Some(RigFinishReason::Stop) | None => FinishReason::Stop,
        Some(RigFinishReason::Length) => FinishReason::Length,
        Some(
            RigFinishReason::ToolCalls | RigFinishReason::ContentFilter | RigFinishReason::Other(_),
        ) => FinishReason::Error,
    };
    Ok(CompletionResponse {
        request_id,
        message: AssistantMessage {
            role: "assistant",
            content: vec![AssistantTextBlock { kind: "text", text }],
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

pub(crate) fn ensure_private_dir() -> Result<PathBuf> {
    let path = config::ai_private_dir_path()?;
    fs::create_dir_all(&path)
        .with_context(|| format!("create AI credential directory {}", path.display()))?;
    secure_private_dir(&path)?;
    Ok(path)
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

pub(crate) async fn login(provider: &str) -> Result<()> {
    let info = registry::provider_info(provider)
        .with_context(|| format!("unsupported AI provider {provider}"))?;
    if info.auth != registry::AuthKind::OAuth {
        bail!(
            "provider {provider} uses {} instead of interactive login",
            info.auth.description()
        );
    }
    let dir = ensure_private_dir()?;
    registry::login(provider, &dir).await
}

pub(crate) async fn check(config: &config::AppConfig) -> Result<CompletionResponse> {
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
mod tests {
    use super::*;

    struct PendingBackend;

    impl CompletionBackend for PendingBackend {
        fn complete(&self, _request: RigRequest) -> registry::CompletionFuture<'_> {
            Box::pin(std::future::pending())
        }
    }

    fn ready_service() -> AiService {
        AiService {
            state: BackendState::Ready {
                backend: Box::new(PendingBackend),
                schema: SchemaStrategy::NativeVerbatim,
                provider: "test".to_string(),
            },
            max_output_tokens: 128,
            timeout: Duration::from_millis(5),
            semaphore: Semaphore::new(1),
            requests_per_minute: 2,
            recent_requests: Mutex::new(VecDeque::new()),
        }
    }

    fn request() -> CompletionRequest {
        serde_json::from_value(serde_json::json!({
            "request_id": "test-1",
            "messages": [{"role": "user", "content": [{"type": "text", "text": "Return a color"}]}],
            "output": {"type": "json_schema", "schema": {"type": "object", "properties": {"color": {"type": "string"}}}}
        }))
        .unwrap()
    }

    #[test]
    fn validates_and_converts_neutral_request() {
        let validated = request().validate().unwrap();
        let converted = validated
            .into_rig(SchemaStrategy::NativeVerbatim, 128)
            .unwrap();
        assert_eq!(converted.max_tokens, Some(128));
        assert!(converted.output_schema.is_some());
        assert!(converted.tools.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn private_credential_directory_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("ai");
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        secure_private_dir(&path).unwrap();
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[test]
    fn rejects_unknown_request_fields() {
        let result = serde_json::from_value::<CompletionRequest>(serde_json::json!({
            "request_id": "test-1",
            "messages": [],
            "output": {"type": "json_schema", "schema": {}},
            "model": "caller-choice"
        }));
        assert!(result.is_err());
    }

    #[test]
    fn rejects_deep_schema() {
        let mut schema = serde_json::json!({"type": "string"});
        for _ in 0..MAX_SCHEMA_DEPTH {
            schema = serde_json::json!({"items": schema});
        }
        let mut request = request();
        request.output = OutputRequest::JsonSchema { schema };
        assert!(request.validate().is_err());
    }

    #[tokio::test]
    async fn service_enforces_timeout_concurrency_and_rate_limits() {
        let service = ready_service();
        let error = service.complete(request()).await.unwrap_err();
        assert_eq!(error, ErrorCategory::Timeout);

        let permit = service.semaphore.try_acquire().unwrap();
        let error = service.complete(request()).await.unwrap_err();
        assert_eq!(error, ErrorCategory::RateLimit);
        drop(permit);

        let error = service.complete(request()).await.unwrap_err();
        assert_eq!(error, ErrorCategory::RateLimit);
    }

    #[test]
    fn response_projection_enforces_reported_output_limit() {
        let usage = rig_core::completion::Usage {
            input_tokens: 3,
            output_tokens: 5,
            total_tokens: 8,
            cached_input_tokens: 0,
            cache_creation_input_tokens: 0,
            tool_use_prompt_tokens: 0,
            reasoning_tokens: 0,
        };
        let response = rig_core::completion::CompletionResponse::new(
            vec![AssistantContent::Text(rig_core::message::Text::from(
                "{\"ok\":true}",
            ))],
            usage,
            "test",
        )
        .with_raw(serde_json::json!({"secret_provider_field": "must-not-leak"}));

        let projected = project_response("request-1".to_string(), response, 5).unwrap();
        let json = serde_json::to_string(&projected).unwrap();
        assert!(json.contains("{\\\"ok\\\":true}"));
        assert!(!json.contains("secret_provider_field"));

        let usage = rig_core::completion::Usage {
            output_tokens: 6,
            ..usage
        };
        let response = rig_core::completion::CompletionResponse::new(
            vec![AssistantContent::Text(rig_core::message::Text::from("{}"))],
            usage,
            "test",
        );
        assert!(project_response("request-2".to_string(), response, 5).is_err());
    }
}
