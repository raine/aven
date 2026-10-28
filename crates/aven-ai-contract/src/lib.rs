//! Provider-neutral AI completion wire contract. Replay tokens are opaque to this crate.
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilitiesResponse {
    pub status: AvailabilityStatus,
    pub completion_contract: u32,
    pub features: Features,
    pub limits: Limits,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub setup_hint: Option<String>,
    pub transcription: TranscriptionCapability,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AvailabilityStatus {
    Ready,
    Unconfigured,
    AuthenticationRequired,
    TemporarilyUnavailable,
}

impl AvailabilityStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Unconfigured => "unconfigured",
            Self::AuthenticationRequired => "authentication_required",
            Self::TemporarilyUnavailable => "temporarily_unavailable",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Features {
    pub text: bool,
    pub json_schema: bool,
    pub tools: bool,
    #[serde(default)]
    pub images: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Limits {
    pub max_request_bytes: usize,
    pub max_output_tokens: u64,
    #[serde(default)]
    pub max_images: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompletionRequest {
    pub request_id: String,
    pub messages: Vec<Message>,
    pub output: OutputRequest,
    #[serde(default)]
    pub tools: Vec<ToolDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContentBlock {
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
    /// Base64 (standard alphabet) image bytes, only in user messages.
    Image {
        media_type: String,
        data: String,
    },
    ToolResult {
        call_id: String,
        name: String,
        output: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum OutputRequest {
    Text,
    JsonSchema { schema: Value },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionResponse {
    pub request_id: String,
    pub message: AssistantMessage,
    pub finish_reason: FinishReason,
    pub usage: Usage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantMessage {
    pub role: String,
    pub content: Vec<ContentBlock>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    Length,
    Error,
    ToolCalls,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCategory {
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorEnvelope {
    pub error: ErrorBody,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorBody {
    pub category: ErrorCategory,
    pub retryable: bool,
}

impl ErrorCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::UnsupportedCapability => "unsupported_capability",
            Self::Unconfigured => "unconfigured",
            Self::AuthenticationFailure => "authentication_failure",
            Self::RateLimit => "rate_limit",
            Self::Timeout => "timeout",
            Self::MalformedProviderResponse => "malformed_provider_response",
            Self::CanceledRequest => "canceled_request",
            Self::TemporaryProviderFailure => "temporary_provider_failure",
        }
    }

    /// HTTP status for a pre-stream failure; no transport dependency is required.
    pub fn status(self) -> u16 {
        match self {
            Self::InvalidRequest | Self::UnsupportedCapability => 400,
            Self::AuthenticationFailure => 503,
            Self::RateLimit => 429,
            Self::Timeout => 504,
            Self::MalformedProviderResponse => 502,
            Self::CanceledRequest => 408,
            Self::Unconfigured | Self::TemporaryProviderFailure => 503,
        }
    }

    pub fn retryable(self) -> bool {
        matches!(
            self,
            Self::RateLimit
                | Self::Timeout
                | Self::CanceledRequest
                | Self::TemporaryProviderFailure
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionCapability {
    pub contract: u32,
    pub status: AvailabilityStatus,
    pub audio_format: String,
    pub max_audio_bytes: usize,
    pub max_duration_seconds: u64,
}

/// Structural bounds, independent of provider execution and HTTP body limits.
#[derive(Debug, Clone)]
pub struct ValidationLimits {
    /// Compact serialized DTO size. Also bound the original HTTP body at ingress.
    pub max_request_bytes: usize,
    pub max_messages: usize,
    pub max_text_bytes: usize,
    pub max_images: usize,
    pub max_image_bytes: usize,
    pub max_tools: usize,
    pub max_tool_schema_bytes: usize,
    pub max_schema_bytes: usize,
    pub max_schema_depth: usize,
    pub max_request_id_bytes: usize,
    pub max_opaque_bytes: usize,
}
impl Default for ValidationLimits {
    fn default() -> Self {
        Self {
            max_request_bytes: 512 * 1024,
            max_messages: 64,
            max_text_bytes: 256 * 1024,
            max_images: 4,
            max_image_bytes: 320 * 1024,
            max_tools: 16,
            max_tool_schema_bytes: 32 * 1024,
            max_schema_bytes: 24 * 1024,
            max_schema_depth: 32,
            max_request_id_bytes: 128,
            max_opaque_bytes: 512 * 1024,
        }
    }
}
/// Provider feature policy and runtime structural bounds.
#[derive(Debug, Clone)]
pub struct ValidationPolicy {
    pub allow_images: bool,
    pub allow_json_schema: bool,
    pub limits: ValidationLimits,
}
impl Default for ValidationPolicy {
    fn default() -> Self {
        Self {
            allow_images: true,
            allow_json_schema: true,
            limits: ValidationLimits::default(),
        }
    }
}
fn validate_token(token: &str, policy: &ValidationPolicy) -> Result<(), ErrorCategory> {
    if token.is_empty() || token.len() > policy.limits.max_opaque_bytes {
        Err(ErrorCategory::InvalidRequest)
    } else {
        Ok(())
    }
}
/// Validate structure and enabled features without interpreting provider replay tokens.
/// Provider-specific schema and token checks remain the caller's responsibility.
pub fn validate(
    request: &CompletionRequest,
    policy: &ValidationPolicy,
) -> Result<(), ErrorCategory> {
    if serde_json::to_vec(request)
        .map_err(|_| ErrorCategory::InvalidRequest)?
        .len()
        > policy.limits.max_request_bytes
    {
        return Err(ErrorCategory::InvalidRequest);
    }
    if request.request_id.is_empty()
        || request.request_id.len() > policy.limits.max_request_id_bytes
        || !request
            .request_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        return Err(ErrorCategory::InvalidRequest);
    }
    if request.messages.is_empty() || request.messages.len() > policy.limits.max_messages {
        return Err(ErrorCategory::InvalidRequest);
    }
    let mut text_bytes = 0usize;
    let mut images = 0usize;
    for message in &request.messages {
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
                    validate_token(id, policy)?;
                    if name.is_empty() {
                        return Err(ErrorCategory::InvalidRequest);
                    }
                    text_bytes += json_size(arguments)?;
                }
                (Role::Assistant, ContentBlock::Reasoning { opaque }) => {
                    validate_token(opaque, policy)?;
                }
                (Role::User, ContentBlock::Image { media_type, data }) => {
                    if !policy.allow_images {
                        return Err(ErrorCategory::UnsupportedCapability);
                    }
                    if !matches!(media_type.as_str(), "image/jpeg" | "image/png") {
                        return Err(ErrorCategory::InvalidRequest);
                    }
                    let decoded = STANDARD
                        .decode(data)
                        .map_err(|_| ErrorCategory::InvalidRequest)?;
                    images += 1;
                    if decoded.is_empty() || decoded.len() > policy.limits.max_image_bytes {
                        return Err(ErrorCategory::InvalidRequest);
                    }
                }
                (
                    Role::Tool,
                    ContentBlock::ToolResult {
                        call_id,
                        name,
                        output,
                    },
                ) => {
                    validate_token(call_id, policy)?;
                    if name.is_empty() {
                        return Err(ErrorCategory::InvalidRequest);
                    }
                    text_bytes += output.len();
                }
                _ => return Err(ErrorCategory::InvalidRequest),
            }
        }
    }
    if text_bytes > policy.limits.max_text_bytes
        || images > policy.limits.max_images
        || request.tools.len() > policy.limits.max_tools
    {
        return Err(ErrorCategory::InvalidRequest);
    }
    let mut schema_bytes = 0;
    let mut names = std::collections::HashSet::new();
    for tool in &request.tools {
        if tool.name.is_empty() || !names.insert(&tool.name) {
            return Err(ErrorCategory::InvalidRequest);
        }
        schema_bytes += validate_schema(
            &tool.parameters,
            policy.limits.max_tool_schema_bytes,
            policy.limits.max_schema_depth,
        )?;
    }
    if schema_bytes > policy.limits.max_tool_schema_bytes {
        return Err(ErrorCategory::InvalidRequest);
    }
    if let OutputRequest::JsonSchema { schema } = &request.output {
        if !policy.allow_json_schema {
            return Err(ErrorCategory::UnsupportedCapability);
        }
        validate_schema(
            schema,
            policy.limits.max_schema_bytes,
            policy.limits.max_schema_depth,
        )?;
    }
    Ok(())
}

fn json_size(value: &Value) -> std::result::Result<usize, ErrorCategory> {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .map_err(|_| ErrorCategory::InvalidRequest)
}

fn validate_schema(
    schema: &Value,
    max_bytes: usize,
    max_depth: usize,
) -> std::result::Result<usize, ErrorCategory> {
    if !schema.is_object() || json_depth(schema) > max_depth {
        return Err(ErrorCategory::InvalidRequest);
    }
    let bytes = json_size(schema)?;
    if bytes > max_bytes {
        return Err(ErrorCategory::InvalidRequest);
    }
    Ok(bytes)
}

fn json_depth(value: &Value) -> usize {
    match value {
        Value::Array(values) => 1 + values.iter().map(json_depth).max().unwrap_or(0),
        Value::Object(values) => 1 + values.values().map(json_depth).max().unwrap_or(0),
        _ => 1,
    }
}

/// SSE payloads; replay metadata is carried only by the authoritative completion.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StreamEvent {
    TextDelta {
        request_id: String,
        text: String,
    },
    Completion(CompletionResponse),
    Error {
        request_id: String,
        error: StreamError,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamError {
    pub category: ErrorCategory,
}
impl StreamEvent {
    pub fn name(&self) -> &'static str {
        match self {
            Self::TextDelta { .. } => "text_delta",
            Self::Completion(_) => "completion",
            Self::Error { .. } => "error",
        }
    }
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}
impl From<ErrorCategory> for ErrorEnvelope {
    fn from(category: ErrorCategory) -> Self {
        Self {
            error: ErrorBody {
                category,
                retryable: category.retryable(),
            },
        }
    }
}
