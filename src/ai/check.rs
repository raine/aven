//! The synthetic exchange behind AI checks: a native tool call to an inert
//! tool, then a replay of the unchanged assistant message with the tool
//! result, ending in a final text answer. Requests and responses use the
//! completion wire format, so any executor (the in-process service or the
//! HTTP endpoint) can run it.

use std::future::Future;

use serde::Serialize;
use serde_json::{Value, json};

use super::ErrorCategory;

pub(crate) const TOOL_NAME: &str = "aven_check_lookup";
const TOOL_OUTPUT: &str = "{\"value\":\"aven-check-ok\"}";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Step {
    /// The model answers the first request with a call to the check tool.
    ToolCall,
    /// The model answers the replayed call and tool result with text.
    ToolResult,
}

impl Step {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ToolCall => "tool_call",
            Self::ToolResult => "tool_result",
        }
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::ToolCall => "model calls a tool",
            Self::ToolResult => "model answers from the tool result",
        }
    }
}

/// A completion that succeeded but did not do what the step requires.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Mismatch {
    NoToolCall,
    UnexpectedTool,
    NoFinalAnswer,
    MalformedResponse,
}

impl Mismatch {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::NoToolCall => "no_tool_call",
            Self::UnexpectedTool => "unexpected_tool",
            Self::NoFinalAnswer => "no_final_answer",
            Self::MalformedResponse => "malformed_response",
        }
    }

    pub(crate) fn hint(self) -> &'static str {
        match self {
            Self::NoToolCall => {
                "the model answered without calling the tool; set ai.model to a model with tool calling support, then restart the server"
            }
            Self::UnexpectedTool => {
                "the model called a tool it was not offered; set ai.model to a model with reliable tool calling, then restart the server"
            }
            Self::NoFinalAnswer => {
                "the model did not answer after the tool result; set ai.model to a model with tool calling support, then restart the server"
            }
            Self::MalformedResponse => {
                "the completion response does not match this Aven version; run the same Aven version on the server and this host"
            }
        }
    }
}

/// Hint for a completion error category reported during a check.
pub(crate) fn category_hint(category: ErrorCategory) -> &'static str {
    match category {
        ErrorCategory::InvalidRequest => {
            "the provider rejected the request; set ai.model to a model that supports tool calling, then restart the server"
        }
        ErrorCategory::UnsupportedCapability => {
            "this provider integration cannot carry tool calls; choose a provider with tools=true in `aven server ai providers`, then restart the server"
        }
        ErrorCategory::Unconfigured => {
            "set ai.provider and ai.model in server.yaml, then restart the server"
        }
        ErrorCategory::AuthenticationFailure => {
            "the provider rejected the credentials; run `aven server ai login <provider>` for OAuth providers or update the API key in the server environment, then restart the server"
        }
        ErrorCategory::RateLimit => {
            "the provider is rate limiting requests; wait, then run the check again"
        }
        ErrorCategory::Timeout => {
            "the provider did not answer within ai.timeout_seconds; check the endpoint or raise the timeout, then restart the server"
        }
        ErrorCategory::MalformedProviderResponse => {
            "the provider returned a response Aven cannot use; set ai.model to another model, then restart the server"
        }
        ErrorCategory::CanceledRequest => "the request was canceled; run the check again",
        ErrorCategory::TemporaryProviderFailure => {
            "the provider request failed; check the endpoint, network and provider status, then run the check again"
        }
    }
}

#[derive(Debug)]
pub(crate) enum Failure<E> {
    /// The executor returned an error instead of a completion response.
    Completion(E),
    Mismatch(Mismatch),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Usage {
    pub(crate) input_tokens: u64,
    pub(crate) output_tokens: u64,
}

#[derive(Debug)]
pub(crate) struct Outcome<E> {
    /// Steps that passed, in order.
    pub(crate) passed: Vec<Step>,
    /// The step that failed, if any. No later step runs after a failure.
    pub(crate) failure: Option<(Step, Failure<E>)>,
    /// Tokens reported by every completion that returned a response.
    pub(crate) usage: Usage,
}

/// The first request: offers the inert tool and asks the model to call it.
pub(crate) fn tool_call_request() -> Value {
    json!({
        "request_id": "aven-check-1",
        "messages": [
            {"role": "system", "content": [{"type": "text", "text": "You are verifying that tool calling works. Use the provided tool when asked."}]},
            {"role": "user", "content": [{"type": "text", "text": format!("Call the {TOOL_NAME} tool with key \"status\". Then reply with one short sentence that includes the value it returns.")}]}
        ],
        "tools": [{
            "name": TOOL_NAME,
            "description": "Returns a fixed status value for a connectivity check. It has no side effects.",
            "parameters": {
                "type": "object",
                "properties": {"key": {"type": "string", "description": "The value to look up. Use \"status\"."}},
                "required": ["key"],
                "additionalProperties": false
            }
        }],
        "output": {"type": "text"}
    })
}

/// The assistant message from a verified tool-call response.
#[derive(Debug, Clone)]
pub(crate) struct ToolCalls {
    message: Value,
    call_ids: Vec<String>,
}

pub(crate) fn verify_tool_call(response: &Value) -> Result<ToolCalls, Mismatch> {
    let message = response.get("message").ok_or(Mismatch::MalformedResponse)?;
    let blocks = content_blocks(message)?;
    let mut call_ids = Vec::new();
    for block in blocks {
        if block.get("type").and_then(Value::as_str) != Some("tool_call") {
            continue;
        }
        if block.get("name").and_then(Value::as_str) != Some(TOOL_NAME) {
            return Err(Mismatch::UnexpectedTool);
        }
        let id = block
            .get("id")
            .and_then(Value::as_str)
            .ok_or(Mismatch::MalformedResponse)?;
        call_ids.push(id.to_string());
    }
    if call_ids.is_empty() {
        return Err(Mismatch::NoToolCall);
    }
    Ok(ToolCalls {
        message: message.clone(),
        call_ids,
    })
}

/// The second request: the first conversation, the assistant message exactly
/// as returned, and one tool result per call.
pub(crate) fn tool_result_request(calls: &ToolCalls) -> Value {
    let mut request = tool_call_request();
    request["request_id"] = json!("aven-check-2");
    let results = calls
        .call_ids
        .iter()
        .map(|id| json!({"type": "tool_result", "call_id": id, "name": TOOL_NAME, "output": TOOL_OUTPUT}))
        .collect::<Vec<_>>();
    if let Some(messages) = request["messages"].as_array_mut() {
        messages.push(calls.message.clone());
        messages.push(json!({"role": "tool", "content": results}));
    }
    request
}

pub(crate) fn verify_final_answer(response: &Value) -> Result<(), Mismatch> {
    let message = response.get("message").ok_or(Mismatch::MalformedResponse)?;
    let blocks = content_blocks(message)?;
    let has_text = blocks.iter().any(|block| {
        block.get("type").and_then(Value::as_str) == Some("text")
            && block
                .get("text")
                .and_then(Value::as_str)
                .is_some_and(|text| !text.trim().is_empty())
    });
    let calls_again = blocks
        .iter()
        .any(|block| block.get("type").and_then(Value::as_str) == Some("tool_call"));
    if has_text && !calls_again {
        Ok(())
    } else {
        Err(Mismatch::NoFinalAnswer)
    }
}

fn content_blocks(message: &Value) -> Result<&Vec<Value>, Mismatch> {
    if message.get("role").and_then(Value::as_str) != Some("assistant") {
        return Err(Mismatch::MalformedResponse);
    }
    message
        .get("content")
        .and_then(Value::as_array)
        .ok_or(Mismatch::MalformedResponse)
}

fn usage(response: &Value) -> Usage {
    let tokens = |field: &str| {
        response
            .pointer(&format!("/usage/{field}"))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    Usage {
        input_tokens: tokens("input_tokens"),
        output_tokens: tokens("output_tokens"),
    }
}

/// Runs both rounds with `complete`, which sends one wire-format completion
/// request and returns the wire-format response.
pub(crate) async fn run<E, F, Fut>(mut complete: F) -> Outcome<E>
where
    F: FnMut(Value) -> Fut,
    Fut: Future<Output = Result<Value, E>>,
{
    let mut outcome = Outcome {
        passed: Vec::new(),
        failure: None,
        usage: Usage::default(),
    };
    let mut round = async |step: Step, request: Value, outcome: &mut Outcome<E>| {
        let response = complete(request)
            .await
            .map_err(|error| (step, Failure::Completion(error)))?;
        let used = usage(&response);
        outcome.usage.input_tokens += used.input_tokens;
        outcome.usage.output_tokens += used.output_tokens;
        Ok::<_, (Step, Failure<E>)>(response)
    };
    let result = async {
        let response = round(Step::ToolCall, tool_call_request(), &mut outcome).await?;
        let calls = verify_tool_call(&response)
            .map_err(|mismatch| (Step::ToolCall, Failure::Mismatch(mismatch)))?;
        outcome.passed.push(Step::ToolCall);
        let response = round(Step::ToolResult, tool_result_request(&calls), &mut outcome).await?;
        verify_final_answer(&response)
            .map_err(|mismatch| (Step::ToolResult, Failure::Mismatch(mismatch)))?;
        outcome.passed.push(Step::ToolResult);
        Ok(())
    }
    .await;
    outcome.failure = result.err();
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(content: Value) -> Value {
        json!({
            "request_id": "aven-check",
            "message": {"role": "assistant", "content": content},
            "finish_reason": "stop",
            "usage": {"input_tokens": 10, "output_tokens": 3}
        })
    }

    fn call(name: &str) -> Value {
        json!({"type": "tool_call", "id": "v1.call", "name": name, "arguments": {"key": "status"}})
    }

    #[tokio::test]
    async fn replays_the_assistant_message_unchanged_and_requires_a_final_answer() {
        let assistant = json!([
            {"type": "reasoning", "opaque": "v1.reasoning"},
            call(TOOL_NAME)
        ]);
        let mut requests = Vec::new();
        let outcome = run(|request: Value| {
            requests.push(request);
            let reply = if requests.len() == 1 {
                response(assistant.clone())
            } else {
                response(json!([{"type": "text", "text": "Status is aven-check-ok."}]))
            };
            async move { Ok::<_, ErrorCategory>(reply) }
        })
        .await;
        assert!(outcome.failure.is_none(), "{outcome:?}");
        assert_eq!(outcome.passed, [Step::ToolCall, Step::ToolResult]);
        assert_eq!(
            outcome.usage,
            Usage {
                input_tokens: 20,
                output_tokens: 6
            }
        );
        let messages = requests[1]["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 4);
        assert_eq!(
            messages[2],
            json!({"role": "assistant", "content": assistant})
        );
        assert_eq!(messages[3]["content"][0]["call_id"], "v1.call");
        assert_eq!(messages[3]["content"][0]["name"], TOOL_NAME);
        assert_eq!(requests[1]["tools"], requests[0]["tools"]);
    }

    #[tokio::test]
    async fn reports_the_failing_step_and_stops() {
        let outcome = run(|_request: Value| async {
            Ok::<_, ErrorCategory>(response(json!([{"type": "text", "text": "No tools."}])))
        })
        .await;
        assert!(outcome.passed.is_empty());
        assert!(matches!(
            outcome.failure,
            Some((Step::ToolCall, Failure::Mismatch(Mismatch::NoToolCall)))
        ));

        let outcome =
            run(|_request: Value| async { Err::<Value, _>(ErrorCategory::UnsupportedCapability) })
                .await;
        assert!(matches!(
            outcome.failure,
            Some((
                Step::ToolCall,
                Failure::Completion(ErrorCategory::UnsupportedCapability)
            ))
        ));
    }

    #[test]
    fn verification_rejects_unoffered_tools_and_repeated_calls() {
        assert_eq!(
            verify_tool_call(&response(json!([call("delete_everything")]))).unwrap_err(),
            Mismatch::UnexpectedTool
        );
        assert_eq!(
            verify_final_answer(&response(json!([call(TOOL_NAME)]))).unwrap_err(),
            Mismatch::NoFinalAnswer
        );
        assert_eq!(
            verify_final_answer(&response(json!([{"type": "text", "text": "  "}]))).unwrap_err(),
            Mismatch::NoFinalAnswer
        );
        assert_eq!(
            verify_final_answer(&json!({"error": {}})).unwrap_err(),
            Mismatch::MalformedResponse
        );
    }
}
