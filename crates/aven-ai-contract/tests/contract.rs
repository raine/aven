use aven_ai_contract::*;
use serde_json::{Value, json};

fn request(name: &str) -> CompletionRequest {
    serde_json::from_str(match name {
        "text" => include_str!("../fixtures/text.json"),
        "schema" => include_str!("../fixtures/schema.json"),
        "image" => include_str!("../fixtures/image.json"),
        _ => unreachable!(),
    })
    .unwrap()
}

#[test]
fn golden_requests_and_two_round_exchange_round_trip() {
    for name in ["text", "schema", "image"] {
        let request = request(name);
        validate(&request, &ValidationPolicy::default()).unwrap();
        let value = serde_json::to_value(&request).unwrap();
        let decoded: CompletionRequest = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), value);
    }
    let exchange: Value =
        serde_json::from_str(include_str!("../fixtures/tool-exchange.json")).unwrap();
    for value in exchange["requests"].as_array().unwrap() {
        let request: CompletionRequest = serde_json::from_value(value.clone()).unwrap();
        validate(&request, &ValidationPolicy::default()).unwrap();
        assert_eq!(serde_json::to_value(request).unwrap(), *value);
    }
    let completion: CompletionResponse =
        serde_json::from_value(exchange["completion"].clone()).unwrap();
    assert_eq!(
        serde_json::to_value(completion).unwrap(),
        exchange["completion"]
    );
    let final_completion: CompletionResponse =
        serde_json::from_value(exchange["final_completion"].clone()).unwrap();
    assert_eq!(
        final_completion.request_id,
        exchange["requests"][1]["request_id"]
    );
    assert_eq!(
        serde_json::to_value(final_completion).unwrap(),
        exchange["final_completion"]
    );
    assert_eq!(
        exchange["requests"][1]["messages"][1],
        exchange["completion"]["message"]
    );
    assert_eq!(
        exchange["requests"][1]["messages"][2]["content"][0]["call_id"],
        exchange["completion"]["message"]["content"][1]["id"]
    );
}

#[test]
fn runtime_features_reject_only_disabled_capabilities() {
    for allow_images in [false, true] {
        for allow_json_schema in [false, true] {
            let policy = ValidationPolicy {
                allow_images,
                allow_json_schema,
                ..Default::default()
            };
            assert_eq!(validate(&request("text"), &policy), Ok(()));
            for (name, allowed) in [("image", allow_images), ("schema", allow_json_schema)] {
                assert_eq!(
                    validate(&request(name), &policy),
                    if allowed {
                        Ok(())
                    } else {
                        Err(ErrorCategory::UnsupportedCapability)
                    }
                );
            }
        }
    }
}

#[test]
fn runtime_limits_are_inclusive_and_measure_bytes() {
    let mut policy = ValidationPolicy::default();
    let mut request = request("text");
    request.messages[0].content = vec![ContentBlock::Text { text: "é".into() }];
    policy.limits.max_text_bytes = 2;
    assert_eq!(validate(&request, &policy), Ok(()));
    policy.limits.max_text_bytes = 1;
    assert_eq!(
        validate(&request, &policy),
        Err(ErrorCategory::InvalidRequest)
    );
    policy = ValidationPolicy::default();
    policy.limits.max_request_bytes = serde_json::to_vec(&request).unwrap().len();
    assert_eq!(validate(&request, &policy), Ok(()));
    policy.limits.max_request_bytes -= 1;
    assert_eq!(
        validate(&request, &policy),
        Err(ErrorCategory::InvalidRequest)
    );
    policy = ValidationPolicy::default();
    policy.limits.max_images = 0;
    assert_eq!(
        validate(&self::request("image"), &policy),
        Err(ErrorCategory::InvalidRequest)
    );
    policy.limits.max_images = 1;
    policy.limits.max_image_bytes = 8;
    assert_eq!(validate(&self::request("image"), &policy), Ok(()));
    policy.limits.max_image_bytes = 7;
    assert_eq!(
        validate(&self::request("image"), &policy),
        Err(ErrorCategory::InvalidRequest)
    );
}

#[test]
fn replay_tokens_are_opaque_but_nonempty_and_bounded() {
    let mut request = request("text");
    let mut policy = ValidationPolicy::default();
    policy.limits.max_opaque_bytes = 3;
    for token in ["", "abcd", "abc"] {
        for block in [
            ContentBlock::Reasoning {
                opaque: token.into(),
            },
            ContentBlock::ToolCall {
                id: token.into(),
                name: "tool".into(),
                arguments: json!({}),
            },
            ContentBlock::ToolResult {
                call_id: token.into(),
                name: "tool".into(),
                output: "ok".into(),
            },
        ] {
            let role = if matches!(block, ContentBlock::ToolResult { .. }) {
                Role::Tool
            } else {
                Role::Assistant
            };
            request.messages = vec![Message {
                role,
                content: vec![block],
            }];
            assert_eq!(validate(&request, &policy).is_ok(), token == "abc");
        }
    }
}

#[test]
fn schema_limits_and_structural_validation() {
    let mut policy = ValidationPolicy::default();
    let mut request = request("schema");
    policy.limits.max_schema_depth = 1;
    assert_eq!(
        validate(&request, &policy),
        Err(ErrorCategory::InvalidRequest)
    );
    policy = ValidationPolicy::default();
    policy.limits.max_schema_bytes = 1;
    assert_eq!(
        validate(&request, &policy),
        Err(ErrorCategory::InvalidRequest)
    );
    policy = ValidationPolicy::default();
    request.messages[0].role = Role::Tool;
    assert_eq!(
        validate(&request, &policy),
        Err(ErrorCategory::InvalidRequest)
    );
    let mut value = serde_json::to_value(self::request("text")).unwrap();
    value["model"] = json!("client-selected");
    assert!(serde_json::from_value::<CompletionRequest>(value).is_err());
}

#[test]
fn sse_goldens_have_exact_payloads_and_one_terminal_event() {
    for fixture in [
        include_str!("../fixtures/text.sse"),
        include_str!("../fixtures/eof.sse"),
        include_str!("../fixtures/overflow.sse"),
        include_str!("../fixtures/timeout.sse"),
        include_str!("../fixtures/tools.sse"),
    ] {
        let mut terminal = false;
        for frame in fixture.trim().split("\n\n") {
            assert!(!terminal, "event after terminal");
            let (name, data) = frame.split_once('\n').unwrap();
            let name = name.strip_prefix("event: ").unwrap();
            let value: Value = serde_json::from_str(data.strip_prefix("data: ").unwrap()).unwrap();
            let event: StreamEvent = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(event.name(), name);
            assert_eq!(
                serde_json::from_str::<Value>(&event.to_json().unwrap()).unwrap(),
                value
            );
            terminal = name != "text_delta";
            if !terminal {
                assert!(value.get("message").is_none());
                assert!(value.get("error").is_none());
            }
        }
        assert!(terminal);
    }
}

#[test]
fn errors_keep_http_mapping_and_retryability() {
    for (category, status, retryable) in [
        (ErrorCategory::InvalidRequest, 400, false),
        (ErrorCategory::UnsupportedCapability, 400, false),
        (ErrorCategory::Unconfigured, 503, false),
        (ErrorCategory::AuthenticationFailure, 503, false),
        (ErrorCategory::RateLimit, 429, true),
        (ErrorCategory::Timeout, 504, true),
        (ErrorCategory::MalformedProviderResponse, 502, false),
        (ErrorCategory::CanceledRequest, 408, true),
        (ErrorCategory::TemporaryProviderFailure, 503, true),
    ] {
        assert_eq!(category.status(), status);
        assert_eq!(category.retryable(), retryable);
        assert_eq!(
            serde_json::to_value(ErrorEnvelope::from(category)).unwrap(),
            json!({"error":{"category":category.as_str(),"retryable":retryable}})
        );
    }
}

#[test]
fn message_tool_and_id_bounds_use_runtime_policy() {
    let mut request = request("text");
    let mut policy = ValidationPolicy::default();
    policy.limits.max_messages = 1;
    assert_eq!(validate(&request, &policy), Ok(()));
    request.messages.push(request.messages[0].clone());
    assert_eq!(
        validate(&request, &policy),
        Err(ErrorCategory::InvalidRequest)
    );
    request.messages.pop();
    policy.limits.max_request_id_bytes = request.request_id.len();
    assert_eq!(validate(&request, &policy), Ok(()));
    policy.limits.max_request_id_bytes -= 1;
    assert_eq!(
        validate(&request, &policy),
        Err(ErrorCategory::InvalidRequest)
    );
    policy = ValidationPolicy::default();
    request.tools = vec![
        ToolDefinition {
            name: "a".into(),
            description: String::new(),
            parameters: json!({}),
        },
        ToolDefinition {
            name: "b".into(),
            description: String::new(),
            parameters: json!({}),
        },
    ];
    policy.limits.max_tools = 2;
    policy.limits.max_tool_schema_bytes = 4;
    assert_eq!(validate(&request, &policy), Ok(()));
    policy.limits.max_tool_schema_bytes = 3;
    assert_eq!(
        validate(&request, &policy),
        Err(ErrorCategory::InvalidRequest)
    );
    policy.limits.max_tool_schema_bytes = 4;
    policy.limits.max_tools = 1;
    assert_eq!(
        validate(&request, &policy),
        Err(ErrorCategory::InvalidRequest)
    );
    policy.limits.max_tools = 2;
    request.tools[1].name = "a".into();
    assert_eq!(
        validate(&request, &policy),
        Err(ErrorCategory::InvalidRequest)
    );
}

#[test]
fn additive_capabilities_preserve_older_payloads() {
    let value = json!({
        "status":"ready", "completion_contract":1,
        "features":{"text":true,"tools":true,"json_schema":true},
        "limits":{"max_request_bytes":524288,"max_output_tokens":128},
        "provider":null,"model":null,"setup_hint":null,
        "transcription":{"contract":1,"status":"unconfigured","audio_format":"audio/mp4","max_audio_bytes":4194304,"max_duration_seconds":300}
    });
    let mut capabilities: CapabilitiesResponse = serde_json::from_value(value.clone()).unwrap();
    assert!(!capabilities.features.images);
    assert_eq!(capabilities.limits.max_images, 0);
    capabilities.features.images = true;
    capabilities.limits.max_images = 4;
    let wire = serde_json::to_value(capabilities).unwrap();
    assert_eq!(wire["features"]["images"], true);
    assert_eq!(wire["limits"]["max_images"], 4);
    assert_eq!(wire["transcription"], value["transcription"]);
}
