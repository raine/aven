use super::*;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::{Stream, StreamExt};
use rig_core::streaming::{StreamedAssistantContent, StreamingCompletionResponse};
use std::convert::Infallible;
use tokio::sync::OwnedSemaphorePermit;

struct StreamState {
    provider: Option<StreamingCompletionResponse>,
    _permit: OwnedSemaphorePermit,
    deadline: tokio::time::Instant,
    request_id: String,
    max_output_tokens: u64,
    text_bytes: usize,
}

impl AiService {
    async fn start_stream(&self, request: CompletionRequest) -> Result<StreamState, ErrorCategory> {
        let validated = request.validate()?;
        let BackendState::Ready {
            backend,
            schema,
            tools,
            ..
        } = &self.state
        else {
            return Err(self.unavailable_error());
        };
        if !tools && validated.requires_tools() {
            return Err(ErrorCategory::UnsupportedCapability);
        }
        let request_id = validated.0.request_id.clone();
        let request = validated.into_rig(*schema, self.max_output_tokens)?;
        self.admit()?;
        let permit = self
            .semaphore
            .clone()
            .try_acquire_owned()
            .map_err(|_| ErrorCategory::RateLimit)?;
        let deadline = tokio::time::Instant::now() + self.timeout;
        let provider = tokio::time::timeout_at(deadline, backend.stream(request))
            .await
            .map_err(|_| ErrorCategory::Timeout)?
            .map_err(|error| classify_provider_error(&error))?;
        Ok(StreamState {
            provider: Some(provider),
            _permit: permit,
            deadline,
            request_id,
            max_output_tokens: self.max_output_tokens,
            text_bytes: 0,
        })
    }
}

fn events(state: StreamState) -> impl Stream<Item = Result<Event, Infallible>> + Send {
    futures::stream::unfold(state, |mut state| async move {
        let provider = state.provider.as_mut()?;
        loop {
            let next = tokio::time::timeout_at(state.deadline, provider.next()).await;
            let error = match next {
                Err(_) => ErrorCategory::Timeout,
                Ok(Some(Err(error))) => classify_provider_error(&error),
                Ok(Some(Ok(StreamedAssistantContent::Text(text)))) => {
                    state.text_bytes = state.text_bytes.saturating_add(text.text.len());
                    let limit = state
                        .max_output_tokens
                        .saturating_mul(8)
                        .min(MAX_TEXT_BYTES as u64);
                    if state.text_bytes as u64 > limit {
                        ErrorCategory::MalformedProviderResponse
                    } else {
                        let event = Event::default()
                            .event("text_delta")
                            .json_data(serde_json::json!({
                                "request_id":state.request_id, "text":text.text
                            }))
                            .expect("text event serializes");
                        return Some((Ok(event), state));
                    }
                }
                Ok(Some(Ok(_))) => continue,
                Ok(None) => {
                    let provider = state.provider.take().expect("active stream");
                    let result = if provider.response.is_none() {
                        Err(ErrorCategory::MalformedProviderResponse)
                    } else {
                        project_response(
                            state.request_id.clone(),
                            provider.into(),
                            state.max_output_tokens,
                        )
                    };
                    match result {
                        Ok(response) => {
                            let event = Event::default()
                                .event("completion")
                                .json_data(response)
                                .expect("completion serializes");
                            return Some((Ok(event), state));
                        }
                        Err(error) => error,
                    }
                }
            };
            // Provisional text never authorizes replay or tool execution. Dropping
            // the provider stream cancels its request, including on client disconnect.
            state.provider = None;
            let event = Event::default()
                .event("error")
                .json_data(serde_json::json!({
                    "request_id":state.request_id, "error":{"category":error}
                }))
                .expect("error event serializes");
            return Some((Ok(event), state));
        }
    })
}

pub(crate) async fn streaming_handler(
    State(service): State<Arc<AiService>>,
    request: Result<Json<CompletionRequest>, JsonRejection>,
) -> Result<Response, ErrorCategory> {
    let Json(request) = request.map_err(|_| ErrorCategory::InvalidRequest)?;
    let state = service.start_stream(request).await?;
    let mut response = Sse::new(events(state))
        .keep_alive(KeepAlive::default())
        .into_response();
    response
        .headers_mut()
        .insert("x-accel-buffering", "no".parse().unwrap());
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rig_core::streaming::{RawStreamingChoice, StreamFinal};

    async fn state(items: Vec<RawStreamingChoice>) -> StreamState {
        let semaphore = Arc::new(Semaphore::new(1));
        StreamState {
            provider: Some(StreamingCompletionResponse::stream(
                "test",
                Box::pin(futures::stream::iter(items.into_iter().map(Ok))),
            )),
            _permit: semaphore.acquire_owned().await.unwrap(),
            deadline: tokio::time::Instant::now() + Duration::from_secs(1),
            request_id: "test".into(),
            max_output_tokens: 128,
            text_bytes: 0,
        }
    }

    fn terminal() -> RawStreamingChoice {
        let mut usage = rig_core::completion::Usage::new();
        usage.input_tokens = 4;
        usage.output_tokens = 2;
        usage.total_tokens = 6;
        RawStreamingChoice::FinalResponse(
            StreamFinal::new("test", usage).with_finish_reason(RigFinishReason::Stop),
        )
    }

    async fn wire(state: StreamState) -> String {
        let response = Sse::new(events(state)).into_response();
        String::from_utf8(
            axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn deltas_precede_authoritative_completion_without_duplication() {
        let output = wire(
            state(vec![
                RawStreamingChoice::Message("Hello ".into()),
                RawStreamingChoice::Message("world".into()),
                terminal(),
            ])
            .await,
        )
        .await;
        assert_eq!(output.matches("event: text_delta").count(), 2);
        assert_eq!(output.matches("event: completion").count(), 1);
        assert!(output.find("text_delta").unwrap() < output.find("completion").unwrap());
        assert!(output.contains("Hello world"));
        assert!(output.contains("\"output_tokens\":2"));
        assert!(!output.contains("event: error"));
    }

    #[tokio::test]
    async fn eof_and_output_overflow_never_emit_completion() {
        for items in [
            vec![RawStreamingChoice::Message("partial".into())],
            vec![RawStreamingChoice::Message("x".repeat(1025)), terminal()],
        ] {
            let output = wire(state(items).await).await;
            assert!(output.contains("event: error"));
            assert!(!output.contains("event: completion"));
        }
    }

    #[tokio::test]
    async fn text_is_delivered_before_provider_finishes_and_drop_releases_permit() {
        let mut state = state(vec![]).await;
        let semaphore = Arc::new(Semaphore::new(1));
        state._permit = semaphore.clone().acquire_owned().await.unwrap();
        let source = futures::stream::iter([Ok(RawStreamingChoice::Message("early".into()))])
            .chain(futures::stream::pending());
        state.provider = Some(StreamingCompletionResponse::stream(
            "test",
            Box::pin(source),
        ));
        let mut events = Box::pin(events(state));
        assert!(
            tokio::time::timeout(Duration::from_millis(50), events.next())
                .await
                .unwrap()
                .is_some()
        );
        assert_eq!(semaphore.available_permits(), 0);
        drop(events);
        assert_eq!(semaphore.available_permits(), 1);
    }

    #[tokio::test]
    async fn tool_replay_metadata_only_appears_in_validated_completion() {
        use rig_core::streaming::{RawStreamingToolCall, StreamPartId};
        let tool = RawStreamingToolCall::new(
            StreamPartId::wire("tool-1"),
            "count_tasks".into(),
            serde_json::json!({}),
        )
        .with_call_id("provider-call".into())
        .with_signature(Some("private-signature".into()));
        let output =
            wire(state(vec![RawStreamingChoice::ToolCall(tool.clone()), terminal()]).await).await;
        assert!(!output.contains("event: text_delta"));
        assert!(!output.contains("private-signature"));
        let payload: Value = serde_json::from_str(
            output
                .lines()
                .find_map(|line| line.strip_prefix("data: "))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(payload["finish_reason"], "tool_calls");
        let token: CallToken =
            decode_opaque(payload["message"]["content"][0]["id"].as_str().unwrap()).unwrap();
        assert_eq!(token.signature.as_deref(), Some("private-signature"));
        let output = wire(state(vec![RawStreamingChoice::ToolCall(tool)]).await).await;
        assert!(output.contains("event: error"));
        assert!(!output.contains("count_tasks"));
    }

    #[tokio::test]
    async fn stalled_provider_emits_terminal_timeout() {
        let mut state = state(vec![]).await;
        state.deadline = tokio::time::Instant::now();
        state.provider = Some(StreamingCompletionResponse::stream(
            "test",
            Box::pin(futures::stream::pending()),
        ));
        let output = wire(state).await;
        assert!(output.contains("timeout"));
        assert!(!output.contains("event: completion"));
    }
}
