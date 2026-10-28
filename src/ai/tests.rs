use super::*;

pub(super) struct PendingBackend;

impl CompletionBackend for PendingBackend {
    fn stream(&self, _request: RigRequest) -> registry::StreamingFuture<'_> {
        Box::pin(std::future::pending())
    }
}

pub(super) fn ready_service() -> AiService {
    AiService {
        state: BackendState::Ready {
            backend: Box::new(PendingBackend),
            schema: SchemaStrategy::NativeVerbatim,
            tools: true,
            transcriber: None,
        },
        provider: Some("test".to_string()),
        model: Some("test-model".to_string()),
        max_output_tokens: 128,
        timeout: Duration::from_millis(5),
        semaphore: Arc::new(Semaphore::new(1)),
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

fn image_request(role: &str, media_type: &str, data: &str) -> CompletionRequest {
    serde_json::from_value(serde_json::json!({
        "request_id": "image-1",
        "messages": [{"role": role, "content": [
            {"type": "text", "text": "What does this screenshot show?"},
            {"type": "image", "media_type": media_type, "data": data}
        ]}],
        "output": {"type": "text"}
    }))
    .unwrap()
}

#[test]
fn converts_user_images_to_typed_base64_content() {
    let data = STANDARD.encode(b"\xff\xd8\xff fixture");
    let converted = image_request("user", "image/jpeg", &data)
        .validate()
        .unwrap()
        .into_rig(SchemaStrategy::NativeVerbatim, 128)
        .unwrap();
    let Some(RigMessage::User { content }) = converted.chat_history.first() else {
        panic!("expected a user message");
    };
    let image = content
        .iter()
        .find_map(|item| match item {
            UserContent::Image(image) => Some(image),
            _ => None,
        })
        .unwrap();
    assert_eq!(image.data, DocumentSourceKind::Base64(data));
    assert_eq!(image.media_type, Some(ImageMediaType::JPEG));
}

#[test]
fn rejects_misplaced_unknown_malformed_and_oversized_images() {
    let data = STANDARD.encode(b"fixture");
    for request in [
        image_request("assistant", "image/jpeg", &data),
        image_request("system", "image/jpeg", &data),
        image_request("user", "image/heic", &data),
        image_request("user", "image/jpeg", "not base64!"),
        image_request(
            "user",
            "image/jpeg",
            &STANDARD.encode(vec![0; MAX_IMAGE_BYTES + 1]),
        ),
        image_request("user", "image/png", ""),
    ] {
        assert!(matches!(
            request.validate(),
            Err(ErrorCategory::InvalidRequest)
        ));
    }
    let mut request = image_request("user", "image/png", &data);
    let image = request.messages[0].content[1].clone();
    request.messages[0].content = vec![image; MAX_IMAGES + 1];
    assert!(matches!(
        request.validate(),
        Err(ErrorCategory::InvalidRequest)
    ));
}

#[tokio::test]
async fn responses_wire_sends_images_as_data_url_input_images() {
    use rig_core::client::CompletionClient;
    use rig_core::providers::chatgpt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (sent, received) = tokio::sync::oneshot::channel::<Value>();
    let sent = Arc::new(Mutex::new(Some(sent)));
    let app = axum::Router::new().route(
        "/responses",
        axum::routing::post(move |Json(body): Json<Value>| async move {
            if let Some(sent) = sent.lock().unwrap().take() {
                let _ = sent.send(body);
            }
            let event = serde_json::json!({"type":"error", "error":{"type":"overloaded_error", "message":"fixture"}});
            ([("content-type", "text/event-stream")], format!("data: {event}\n\n"))
        }),
    );
    let upstream = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = chatgpt::Client::builder()
        .api_key(chatgpt::ChatGPTAuth::AccessToken {
            access_token: "offline-fixture-token".into(),
            account_id: None,
        })
        .base_url(endpoint)
        .http_client(transport::BoundedHttpClient::default())
        .build()
        .unwrap();
    let mut service = ready_service();
    service.timeout = Duration::from_secs(5);
    service.state = BackendState::Ready {
        backend: Box::new(client.completion_model("fixture-model")),
        schema: SchemaStrategy::Prompted,
        tools: true,
        transcriber: None,
    };
    let data = STANDARD.encode(b"\xff\xd8\xff fixture");
    let _ = service
        .collect_completion(image_request("user", "image/jpeg", &data))
        .await;
    let body = received.await.unwrap();
    let parts = body["input"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|item| item["content"].as_array().cloned().unwrap_or_default())
        .collect::<Vec<_>>();
    assert!(parts.iter().any(|part| part["type"] == "input_image"
        && part["image_url"] == format!("data:image/jpeg;base64,{data}")));
    upstream.abort();
    let _ = upstream.await;
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
    let error = service.collect_completion(request()).await.unwrap_err();
    assert_eq!(error, ErrorCategory::Timeout);

    let permit = service.semaphore.try_acquire().unwrap();
    let error = service.collect_completion(request()).await.unwrap_err();
    assert_eq!(error, ErrorCategory::RateLimit);
    drop(permit);

    let error = service.collect_completion(request()).await.unwrap_err();
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

fn tool_request() -> CompletionRequest {
    serde_json::from_value(serde_json::json!({
        "request_id": "agent-1",
        "messages": [{"role": "user", "content": [{"type": "text", "text": "Find a task"}]}],
        "tools": [{"name": "search_tasks", "description": "Search tasks", "parameters": {"type": "object"}}],
        "output": {"type": "text"}
    })).unwrap()
}

fn usage() -> rig_core::completion::Usage {
    rig_core::completion::Usage {
        input_tokens: 10,
        output_tokens: 20,
        total_tokens: 30,
        ..Default::default()
    }
}

fn call(provider: Option<ProviderCallId>) -> ToolCall {
    ToolCall {
        id: ToolCallId::new("rig-correlation-id").unwrap(),
        provider,
        function: ToolFunction {
            name: "search_tasks".into(),
            arguments: serde_json::json!({"query": "dentist"}),
        },
        signature: Some("provider-signature".into()),
        additional_params: Some(serde_json::json!({"provider_extra": "preserve"})),
    }
}

#[test]
fn assistant_projection_replays_all_reasoning_and_call_metadata_in_order() {
    use rig_core::message::ReasoningContent;
    let reasoning = Reasoning {
        id: Some("rs_1".into()),
        content: vec![
            ReasoningContent::Encrypted("encrypted-payload".into()),
            ReasoningContent::Redacted {
                data: "redacted-payload".into(),
            },
            ReasoningContent::Text {
                text: "thinking".into(),
                signature: Some("signed-text".into()),
            },
            ReasoningContent::Summary("summary".into()),
        ],
    };
    for provider in [
        None,
        ProviderCallId::new("call_1"),
        ProviderCallId::new("call_2").map(|id| id.with_item_id("fc_2")),
    ] {
        let call = call(provider);
        let mut choice = vec![
            AssistantContent::Reasoning(reasoning.clone()),
            AssistantContent::Text("Searching".into()),
        ];
        // Parallel calls must not hit an artificial per-message eight-block limit.
        choice.extend((0..16).map(|_| AssistantContent::ToolCall(call.clone())));
        let response =
            rig_core::completion::CompletionResponse::new(choice.clone(), usage(), "fixture");
        let projected = project_response("turn-1".into(), response, 1024).unwrap();
        let json = serde_json::to_value(&projected).unwrap();
        assert_eq!(json["finish_reason"], "tool_calls");
        assert_eq!(json["message"]["content"][0]["type"], "reasoning");
        assert!(json["message"]["content"][0].get("text").is_none());
        let id = json["message"]["content"][2]["id"].as_str().unwrap();
        assert_ne!(id, call.id.as_str());
        let mut request = tool_request();
        request
            .messages
            .push(serde_json::from_value(json["message"].clone()).unwrap());
        request.messages.push(serde_json::from_value(serde_json::json!({
            "role": "tool", "content": [{"type": "tool_result", "call_id": id, "name": "search_tasks", "output": "{\"tasks\":[]}"}]
        })).unwrap());
        let rig = request
            .validate()
            .unwrap()
            .into_rig(SchemaStrategy::NativeVerbatim, 1024)
            .unwrap();
        assert_eq!(
            rig.chat_history[1],
            RigMessage::Assistant {
                id: None,
                content: choice
            }
        );
        assert_eq!(
            rig.chat_history[2],
            RigMessage::User {
                content: vec![UserContent::ToolResult(ToolResult {
                    call: call.id,
                    provider: call.provider,
                    name: "search_tasks".into(),
                    content: vec![ToolResultContent::Text("{\"tasks\":[]}".into())],
                })]
            }
        );
        assert_eq!(rig.tools[0].name, "search_tasks");
        assert_eq!(rig.tools[0].description, "Search tasks");
        assert_eq!(
            rig.tools[0].parameters,
            serde_json::json!({"type": "object"})
        );
        assert!(rig.tool_choice.is_none());
        assert!(rig.temperature.is_none());
        assert!(rig.model.is_none());
        assert!(rig.additional_params.is_none());
        assert!(!rig.record_telemetry_content);
        assert!(rig.output_schema.is_none());
    }
}

#[test]
fn parsing_rejects_wrong_roles_blocks_controls_and_opaque_tokens() {
    let token = encode_opaque(&CallToken {
        id: ToolCallId::new("id").unwrap(),
        provider: None,
        signature: None,
        additional_params: None,
    })
    .unwrap();
    let blocks = [
        serde_json::json!({"type": "text", "text": "hello"}),
        serde_json::json!({"type": "tool_call", "id": token, "name": "search", "arguments": {}}),
        serde_json::json!({"type": "reasoning", "opaque": encode_opaque(&Reasoning::new("thought")).unwrap()}),
        serde_json::json!({"type": "tool_result", "call_id": token, "name": "search", "output": ""}),
    ];
    for role in ["system", "user", "assistant", "tool"] {
        for (index, block) in blocks.iter().enumerate() {
            let mut request = tool_request();
            request.messages = vec![
                serde_json::from_value(serde_json::json!({"role": role, "content": [block]}))
                    .unwrap(),
            ];
            let allowed = match role {
                "system" | "user" => index == 0,
                "assistant" => index < 3,
                "tool" => index == 3,
                _ => unreachable!(),
            };
            assert_eq!(request.validate().is_ok(), allowed, "{role} block {index}");
        }
    }
    for role in [Role::System, Role::User, Role::Tool] {
        let mut request = tool_request();
        request.messages = vec![Message {
            role,
            content: vec![],
        }];
        assert!(request.validate().is_err());
    }
    for invalid in ["not-base64", "v2.e30", "v1.e30", "v1.!", "v1.bnVsbA"] {
        for block in [
            serde_json::json!({"type": "tool_call", "id": invalid, "name": "search", "arguments": {}}),
            serde_json::json!({"type": "reasoning", "opaque": invalid}),
        ] {
            let mut request = tool_request();
            request.messages = vec![
                serde_json::from_value(
                    serde_json::json!({"role": "assistant", "content": [block]}),
                )
                .unwrap(),
            ];
            assert!(request.validate().is_err());
        }
    }
    for field in [
        "provider",
        "model",
        "temperature",
        "tool_choice",
        "additional_params",
    ] {
        let mut json =
            serde_json::json!({"request_id": "x", "messages": [], "output": {"type": "text"}});
        json[field] = serde_json::json!("caller-selected");
        assert!(serde_json::from_value::<CompletionRequest>(json).is_err());
    }
    assert!(
        serde_json::from_value::<ToolDefinition>(
            serde_json::json!({"name":"x", "description":"d", "parameters":{}, "strict":true})
        )
        .is_err()
    );
    assert!(
        serde_json::from_value::<ContentBlock>(
            serde_json::json!({"type":"tool_result", "call_id":token, "output":"x"})
        )
        .is_err()
    );
}

#[test]
fn agent_request_limit_boundaries() {
    let mut request = tool_request();
    request.messages = vec![request.messages[0].clone(); MAX_MESSAGES];
    assert!(request.clone().validate().is_ok());
    request.messages.push(request.messages[0].clone());
    assert!(request.validate().is_err());

    let mut request = tool_request();
    request.messages[0].content = vec![ContentBlock::Text {
        text: "é".repeat(MAX_TEXT_BYTES / 2),
    }];
    assert!(request.clone().validate().is_ok());
    request.messages[0]
        .content
        .push(ContentBlock::Text { text: "x".into() });
    assert!(request.validate().is_err());

    let mut request = tool_request();
    let token = encode_opaque(&CallToken {
        id: ToolCallId::new("id").unwrap(),
        provider: None,
        signature: None,
        additional_params: None,
    })
    .unwrap();
    request.messages = vec![Message {
        role: Role::Tool,
        content: vec![ContentBlock::ToolResult {
            call_id: token.clone(),
            name: "search".into(),
            output: "x".repeat(MAX_TEXT_BYTES),
        }],
    }];
    assert!(request.clone().validate().is_ok());
    request.messages.push(Message {
        role: Role::Assistant,
        content: vec![ContentBlock::ToolCall {
            id: token,
            name: "search".into(),
            arguments: serde_json::json!({}),
        }],
    });
    assert!(request.validate().is_err());

    let mut request = tool_request();
    request.tools = (0..MAX_TOOLS)
        .map(|i| ToolDefinition {
            name: format!("tool_{i}"),
            description: String::new(),
            parameters: serde_json::json!({}),
        })
        .collect();
    assert!(request.clone().validate().is_ok());
    request.tools.push(ToolDefinition {
        name: "extra".into(),
        description: String::new(),
        parameters: serde_json::json!({}),
    });
    assert!(request.validate().is_err());

    let mut request = tool_request();
    let overhead = serde_json::to_vec(&serde_json::json!({"description":""}))
        .unwrap()
        .len();
    request.tools[0].parameters =
        serde_json::json!({"description": "x".repeat(MAX_TOOL_SCHEMA_BYTES - overhead - 2)});
    request.tools.push(ToolDefinition {
        name: "other".into(),
        description: String::new(),
        parameters: serde_json::json!({}),
    });
    assert!(request.clone().validate().is_ok());
    request.tools[1].parameters = serde_json::json!({"type":"object"});
    assert!(request.validate().is_err());

    let mut request = tool_request();
    request.tools.push(request.tools[0].clone());
    assert!(request.validate().is_err());
    for schema in [serde_json::json!([]), serde_json::json!(true)] {
        let mut request = tool_request();
        request.tools[0].parameters = schema;
        assert!(request.validate().is_err());
    }
    let mut request = tool_request();
    for _ in 0..MAX_SCHEMA_DEPTH {
        request.tools[0].parameters = serde_json::json!({"items": request.tools[0].parameters});
    }
    assert!(request.validate().is_err());
}

#[test]
fn text_and_schema_outputs_preserve_server_owned_strategy() {
    for strategy in [
        SchemaStrategy::NativeStrict,
        SchemaStrategy::NativeVerbatim,
        SchemaStrategy::Prompted,
    ] {
        let text = tool_request()
            .validate()
            .unwrap()
            .into_rig(strategy, 128)
            .unwrap();
        assert_eq!(text.chat_history.len(), 1);
        assert!(text.output_schema.is_none());
        let mut schema = tool_request();
        schema.output = request().output;
        let schema = schema.validate().unwrap().into_rig(strategy, 128).unwrap();
        assert_eq!(schema.tools.len(), 1);
        assert_eq!(
            schema.output_schema.is_none(),
            strategy == SchemaStrategy::Prompted
        );
        assert_eq!(
            schema.chat_history.len(),
            if strategy == SchemaStrategy::Prompted {
                2
            } else {
                1
            }
        );
    }
}

#[test]
fn finish_reasons_cover_tool_only_and_empty_terminal_outputs() {
    for (reason, expected) in [
        (None, "tool_calls"),
        (Some(RigFinishReason::Stop), "tool_calls"),
        (Some(RigFinishReason::ToolCalls), "tool_calls"),
        (Some(RigFinishReason::Length), "length"),
        (Some(RigFinishReason::ContentFilter), "error"),
    ] {
        let response = rig_core::completion::CompletionResponse::new(
            vec![AssistantContent::ToolCall(call(None))],
            usage(),
            "fixture",
        )
        .with_optional_finish_reason(reason);
        let projected = project_response("turn".into(), response, 128).unwrap();
        assert_eq!(
            serde_json::to_value(projected).unwrap()["finish_reason"],
            expected
        );
    }
    for (reason, expected) in [
        (RigFinishReason::Length, "length"),
        (RigFinishReason::ContentFilter, "error"),
        (RigFinishReason::Other("failed".into()), "error"),
    ] {
        let response = rig_core::completion::CompletionResponse::new(
            vec![],
            rig_core::completion::Usage {
                output_tokens: 0,
                ..usage()
            },
            "fixture",
        )
        .with_finish_reason(reason);
        let projected = project_response("turn".into(), response, 128).unwrap();
        let json = serde_json::to_value(projected).unwrap();
        assert_eq!(json["finish_reason"], expected);
        let mut request = tool_request();
        let follow_up = request.messages[0].clone();
        request
            .messages
            .push(serde_json::from_value(json["message"].clone()).unwrap());
        request.messages.push(follow_up);
        let rig = request
            .validate()
            .unwrap()
            .into_rig(SchemaStrategy::Prompted, 128)
            .unwrap();
        assert_eq!(rig.chat_history.len(), 2);
        assert_eq!(rig.chat_history[0], rig.chat_history[1]);
    }
    for (content, reported) in [
        (vec![], usage()),
        (
            vec![AssistantContent::Text("text".into())],
            Default::default(),
        ),
    ] {
        let response = rig_core::completion::CompletionResponse::new(content, reported, "fixture");
        assert!(project_response("turn".into(), response, 128).is_err());
    }
    let response = rig_core::completion::CompletionResponse::new(
        vec![AssistantContent::Text("text".into())],
        usage(),
        "fixture",
    );
    assert_eq!(
        serde_json::to_value(project_response("turn".into(), response, 128).unwrap()).unwrap()["finish_reason"],
        "stop"
    );
}

#[tokio::test]
async fn unsupported_tools_fail_before_rate_admission_and_are_advertised() {
    let mut service = ready_service();
    let capabilities = serde_json::to_value(service.capabilities()).unwrap();
    assert_eq!(capabilities["provider"], "test");
    assert_eq!(capabilities["model"], "test-model");
    assert!(service.capabilities().features.tools);
    assert!(service.capabilities().features.images);
    assert_eq!(service.capabilities().limits.max_images, MAX_IMAGES);
    if let BackendState::Ready { tools, .. } = &mut service.state {
        *tools = false;
    }
    assert!(!service.capabilities().features.tools);
    assert_eq!(
        service
            .collect_completion(tool_request())
            .await
            .unwrap_err(),
        ErrorCategory::UnsupportedCapability
    );
    assert!(service.recent_requests.lock().unwrap().is_empty());
    assert_eq!(
        service.collect_completion(request()).await.unwrap_err(),
        ErrorCategory::Timeout
    );
}

#[test]
fn responses_wire_replay_preserves_encrypted_reasoning_and_both_provider_ids() {
    use rig_core::message::ReasoningContent;
    let response = rig_core::completion::CompletionResponse::new(
        vec![
            AssistantContent::Reasoning(Reasoning {
                id: Some("rs_123".into()),
                content: vec![ReasoningContent::Encrypted("ciphertext".into())],
            }),
            AssistantContent::ToolCall(call(
                ProviderCallId::new("call_123").map(|id| id.with_item_id("fc_123")),
            )),
        ],
        usage(),
        "fixture",
    );
    let projected = project_response("turn".into(), response, 128).unwrap();
    let json = serde_json::to_value(projected).unwrap();
    let mut request = tool_request();
    request
        .messages
        .push(serde_json::from_value(json["message"].clone()).unwrap());
    request.messages.push(serde_json::from_value(serde_json::json!({"role":"tool", "content":[{"type":"tool_result", "call_id":json["message"]["content"][1]["id"], "name":"search_tasks", "output":"[]"}]})).unwrap());
    let rig = request
        .validate()
        .unwrap()
        .into_rig(SchemaStrategy::Prompted, 128)
        .unwrap();
    let wire = rig_core::providers::openai::responses_api::CompletionRequest::try_from((
        "fixture-model".into(),
        rig,
    ))
    .unwrap();
    let wire = serde_json::to_value(wire).unwrap();
    let input = wire["input"].as_array().unwrap();
    let reasoning = input
        .iter()
        .find(|item| item["type"] == "reasoning")
        .unwrap();
    assert_eq!(reasoning["id"], "rs_123");
    assert_eq!(reasoning["encrypted_content"], "ciphertext");
    let call = input
        .iter()
        .find(|item| item["type"] == "function_call")
        .unwrap();
    assert_eq!(call["id"], "fc_123");
    assert_eq!(call["call_id"], "call_123");
    let result = input
        .iter()
        .find(|item| item["type"] == "function_call_output")
        .unwrap();
    assert_eq!(result["call_id"], "call_123");
    assert_eq!(result["output"], "[]");
}

#[test]
fn structured_provider_errors_use_codes_and_types_not_message_text() {
    use rig_core::completion::CompletionError;
    for (detail, expected) in [
        (
            serde_json::json!({"type":"invalid_request_error", "code":"invalid_function_parameters"}),
            ErrorCategory::InvalidRequest,
        ),
        (
            serde_json::json!({"type":"invalid_request_error"}),
            ErrorCategory::InvalidRequest,
        ),
        (
            serde_json::json!({"code":"context_length_exceeded"}),
            ErrorCategory::InvalidRequest,
        ),
        (
            serde_json::json!({"code":"request_too_large"}),
            ErrorCategory::InvalidRequest,
        ),
        (
            serde_json::json!({"code":"input_too_long"}),
            ErrorCategory::InvalidRequest,
        ),
        (
            serde_json::json!({"code":"max_tokens_exceeded"}),
            ErrorCategory::InvalidRequest,
        ),
        (
            serde_json::json!({"type":"invalid_request_error", "code":"rate_limit_exceeded"}),
            ErrorCategory::RateLimit,
        ),
        (
            serde_json::json!({"type":"rate_limit_error"}),
            ErrorCategory::RateLimit,
        ),
        (
            serde_json::json!({"type":"overloaded_error"}),
            ErrorCategory::TemporaryProviderFailure,
        ),
        (
            serde_json::json!({"code":"server_error"}),
            ErrorCategory::TemporaryProviderFailure,
        ),
        (
            serde_json::json!({"type":"authentication_error"}),
            ErrorCategory::AuthenticationFailure,
        ),
        (
            serde_json::json!({"type":"unknown", "message":"invalid_request_error context_length_exceeded"}),
            ErrorCategory::TemporaryProviderFailure,
        ),
    ] {
        for body in [
            serde_json::json!({"type":"error", "error":detail}),
            serde_json::json!({"type":"response.failed", "sequence_number":1, "response":{"id":"resp_test","object":"response","created_at":0,"status":"failed","model":"fixture-model","output":[],"tools":[],"error":detail}}),
            detail,
        ] {
            let error = CompletionError::from_provider_body(body.to_string());
            assert_eq!(classify_provider_error(&error), expected);
        }
    }
    for body in ["not JSON", "null", "{}"] {
        let error = CompletionError::from_provider_body(body);
        assert_eq!(
            classify_provider_error(&error),
            ErrorCategory::TemporaryProviderFailure
        );
    }
    for (status, expected) in [
        (401, ErrorCategory::AuthenticationFailure),
        (429, ErrorCategory::RateLimit),
        (400, ErrorCategory::InvalidRequest),
        (503, ErrorCategory::TemporaryProviderFailure),
    ] {
        let error = CompletionError::from_http_response(
            StatusCode::from_u16(status).unwrap(),
            r#"{"error":{"type":"invalid_request_error"}}"#,
        );
        assert_eq!(classify_provider_error(&error), expected);
    }
    let error = CompletionError::ProviderError("invalid_request_error".into());
    assert_eq!(
        classify_provider_error(&error),
        ErrorCategory::TemporaryProviderFailure
    );
}

#[tokio::test]
async fn chatgpt_sse_errors_project_safe_permanent_and_transient_envelopes() {
    use rig_core::{client::CompletionClient, providers::chatgpt};
    for (event, category, status, retryable) in [
        (
            serde_json::json!({"type":"error", "error":{"message":"Invalid schema for function 'list_tasks': null is not of type array; private-schema-marker", "type":"invalid_request_error", "param":"tools", "code":"invalid_function_parameters"}}),
            "invalid_request",
            StatusCode::BAD_REQUEST,
            false,
        ),
        (
            serde_json::json!({"type":"response.failed", "sequence_number":1, "response":{"id":"resp_test","object":"response","created_at":0,"status":"failed","model":"fixture-model","output":[],"tools":[],"error":{"code":"context_length_exceeded", "message":"private-schema-marker"}}}),
            "invalid_request",
            StatusCode::BAD_REQUEST,
            false,
        ),
        (
            serde_json::json!({"type":"error", "error":{"type":"rate_limit_error", "message":"private-schema-marker"}}),
            "rate_limit",
            StatusCode::TOO_MANY_REQUESTS,
            true,
        ),
        (
            serde_json::json!({"type":"error", "error":{"type":"overloaded_error", "message":"private-schema-marker"}}),
            "temporary_provider_failure",
            StatusCode::SERVICE_UNAVAILABLE,
            true,
        ),
        (
            serde_json::json!({"type":"response.failed", "sequence_number":1, "response":{"id":"resp_test","object":"response","created_at":0,"status":"failed","model":"fixture-model","output":[],"tools":[],"error":{"code":"server_error", "message":"private-schema-marker"}}}),
            "temporary_provider_failure",
            StatusCode::SERVICE_UNAVAILABLE,
            true,
        ),
    ] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let body = format!("data: {event}\n\n");
        let app = axum::Router::new().route(
            "/responses",
            axum::routing::post(
                move || async move { ([("content-type", "text/event-stream")], body) },
            ),
        );
        let upstream = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = chatgpt::Client::builder()
            .api_key(chatgpt::ChatGPTAuth::AccessToken {
                access_token: "offline-fixture-token".into(),
                account_id: None,
            })
            .base_url(endpoint)
            .http_client(transport::BoundedHttpClient::default())
            .build()
            .unwrap();
        let mut service = ready_service();
        service.timeout = Duration::from_secs(5);
        service.state = BackendState::Ready {
            backend: Box::new(client.completion_model("fixture-model")),
            schema: SchemaStrategy::Prompted,
            tools: true,
            transcriber: None,
        };
        let mut request = tool_request();
        request.tools[0].parameters = serde_json::json!({"type":"object", "properties":{"status":{"type":"string", "enum":null}}});
        let response = error_response(service.collect_completion(request).await.unwrap_err());
        assert_eq!(response.status(), status);
        let bytes = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        let actual: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            actual,
            serde_json::json!({"error":{"category":category, "retryable":retryable}})
        );
        upstream.abort();
        let _ = upstream.await;
    }
}
