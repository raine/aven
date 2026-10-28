# Aven AI contract

Provider-neutral completion DTOs, capability/error types, structural validation,
and SSE payloads. Runtime dependencies are only `serde`, `serde_json`, and
`base64`. This crate does not execute requests or authenticate replay tokens.

## Public API

- Requests: `CompletionRequest`, `Message`, `Role`, `ContentBlock`,
  `ToolDefinition`, `OutputRequest`.
- Responses: `CompletionResponse`, `AssistantMessage`, `FinishReason`, `Usage`.
- Capabilities: `CapabilitiesResponse`, `AvailabilityStatus`, `Features`,
  `Limits`, `TranscriptionCapability` (the advertisement only).
- Validation: `validate(&CompletionRequest, &ValidationPolicy)` returns
  `Result<(), ErrorCategory>`. `ValidationPolicy` has `allow_images`,
  `allow_json_schema`, and `limits: ValidationLimits`. Defaults permit both
  features and use self-hosted structural limits. Managed services should set
  their feature policy and bounds explicitly.
- Errors: `ErrorCategory::{as_str, status, retryable}`; `status()` returns a
  numeric HTTP status (`u16`). `ErrorEnvelope::from(category)` constructs the
  HTTP JSON body containing `ErrorBody`.
- SSE: `StreamEvent::TextDelta { request_id, text }`,
  `StreamEvent::Completion(CompletionResponse)`, and
  `StreamEvent::Error { request_id, error: StreamError { category } }`.
  `name()` returns the SSE event name; `to_json()` returns the JSON data payload.
  Serde serialization produces the same payload, without an extra enum tag.
  Stream errors intentionally omit the HTTP envelope's `retryable` field.

All DTOs support serialization and deserialization. `features.images` and
`limits.max_images` default to false/zero when decoding older capability JSON.
`Limits` is the advertised subset; `ValidationLimits` contains the structural
bounds, including message, text, image, tool/schema, request ID and opaque-token
limits. Output token enforcement belongs to the provider adapter.

Validation measures the compact serialized request against `max_request_bytes`;
HTTP handlers must also bound the original incoming body before deserialization
(to include whitespace and escape overhead). Opaque tool-call and reasoning
values are checked only for nonempty content and `max_opaque_bytes`. Each server
must authenticate or decode its own tokens before provider dispatch. Tool-call
correlation and provider-specific schema validity also belong to the adapter.

## Conformance fixtures

`fixtures/` contains JSON requests and SSE transcripts as dependency-free data.
Consumers need not depend on `aven-conformance`, which depends on `aven-core`.

- `text.json`, `schema.json`, `image.json`: golden requests. The image has only
  synthetic bytes; validation checks base64, media type and size, not decoding.
- `tool-exchange.json`: two requests and both completions. The
  second request replays the assistant message and correlates its tool result.
- `text.sse`: provisional deltas followed by exactly one completion.
- `eof.sse`, `overflow.sse`, `timeout.sse`: terminal errors, never completion.
- `tools.sse`: replay metadata appears only in the final completion.

Compare SSE event names and parsed JSON, not object-key ordering. Replay tokens
in these fixtures are synthetic opaque strings: substitute tokens produced by
the adapter under test for adapter-level replay tests. Contract tests consume
all fixtures; application streaming tests compare real engine output to the
text/failure transcripts and separately verify provider-specific token replay.
