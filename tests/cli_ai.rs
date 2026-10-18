mod common;

use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

use common::{TestEnv, TestServer};

#[test]
fn ai_administrator_inspection_does_not_require_a_database() {
    let env = TestEnv::new();
    env.write_config("");

    let providers = env.aven_config(["ai", "providers"]);
    assert!(providers.status.success());
    assert!(String::from_utf8_lossy(&providers.stdout).contains("chatgpt auth=oauth"));

    let status = env.aven_config(["ai", "status", "--json"]);
    assert!(status.status.success());
    assert!(String::from_utf8_lossy(&status.stdout).contains("\"status\": \"unconfigured\""));
}

#[tokio::test]
async fn unconfigured_ai_is_authenticated_bounded_and_does_not_block_sync_server() {
    let env = TestEnv::new();
    env.write_config("sync:\n  auth_token: test-secret\n");
    let server = TestServer::start_configured_with_env(
        &env,
        "server.sqlite",
        std::iter::empty::<(&str, &str)>(),
    );
    let url = server.url.clone();
    let client = reqwest::Client::new();

    let unauthenticated = client
        .get(format!("{url}/ai/capabilities"))
        .send()
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), reqwest::StatusCode::UNAUTHORIZED);

    let capabilities = client
        .get(format!("{url}/ai/capabilities"))
        .bearer_auth("test-secret")
        .send()
        .await
        .unwrap();
    assert_eq!(capabilities.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = capabilities.json().await.unwrap();
    assert_eq!(body["status"], "unconfigured");
    assert_eq!(body["completion_contract"], 1);
    assert_eq!(body["limits"]["max_request_bytes"], 524_288);

    let completion = client
        .post(format!("{url}/ai/completions"))
        .bearer_auth("test-secret")
        .json(&serde_json::json!({
            "request_id": "test-1",
            "messages": [{"role": "user", "content": [{"type": "text", "text": "hello"}]}],
            "output": {"type": "json_schema", "schema": {"type": "object"}}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        completion.status(),
        reqwest::StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        completion.json::<serde_json::Value>().await.unwrap()["error"]["category"],
        "unconfigured"
    );

    let valid = serde_json::json!({
        "request_id": "boundary", "messages": [{"role": "user", "content": [{"type":"text", "text":"hi"}]}], "output":{"type":"text"}
    }).to_string();
    for (size, expected) in [(524_288, "unconfigured"), (524_289, "invalid_request")] {
        let padded = format!("{}{}", valid, " ".repeat(size - valid.len()));
        let response = client
            .post(format!("{url}/ai/completions"))
            .bearer_auth("test-secret")
            .header("content-type", "application/json")
            .body(padded)
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.json::<serde_json::Value>().await.unwrap()["error"]["category"],
            expected
        );
    }

    drop(server);
}

#[tokio::test]
async fn synthetic_json_schema_completion_uses_the_generic_service() {
    let env = TestEnv::new();
    let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
    let upstream_addr = upstream.local_addr().unwrap();
    let stub = thread::spawn(move || {
        let (mut stream, _) = upstream.accept().unwrap();
        let request = read_http_request(&mut stream);
        let request_text = String::from_utf8_lossy(&request);
        assert!(request_text.starts_with("POST /api/chat "));
        assert!(request_text.contains("generic color"));
        assert!(!request_text.contains("task"));
        let body = serde_json::json!({
            "model": "stub-model",
            "created_at": "2026-09-17T00:00:00Z",
            "message": {"role": "assistant", "content": "{\"color\":\"blue\"}"},
            "done": true,
            "done_reason": "stop",
            "prompt_eval_count": 9,
            "eval_count": 5
        })
        .to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
    });

    env.write_config(&format!(
        "sync:\n  auth_token: test-secret\nai:\n  provider: ollama\n  model: stub-model\n  endpoint: http://{upstream_addr}\n"
    ));
    fs::write(env.config_dir().join("aven/private"), "not a directory").unwrap();
    let server = TestServer::start_configured_with_env(
        &env,
        "server.sqlite",
        std::iter::empty::<(&str, &str)>(),
    );
    let response = reqwest::Client::new()
        .post(format!("{}/ai/completions", server.url))
        .bearer_auth("test-secret")
        .json(&serde_json::json!({
            "request_id": "synthetic-1",
            "messages": [
                {"role": "system", "content": [{"type": "text", "text": "Return a generic color object."}]},
                {"role": "user", "content": [{"type": "text", "text": "Choose blue."}]}
            ],
            "output": {"type": "json_schema", "schema": {"type": "object", "properties": {"color": {"type": "string"}}, "required": ["color"]}}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["request_id"], "synthetic-1");
    assert_eq!(
        body["message"]["content"][0]["text"],
        "{\"color\":\"blue\"}"
    );
    assert_eq!(body["usage"]["output_tokens"], 5);

    stub.join().unwrap();
    drop(server);
}

#[tokio::test]
async fn ai_requires_a_configured_token_even_on_loopback() {
    let env = TestEnv::new();
    env.write_config("");
    let server = TestServer::start_configured_with_env(
        &env,
        "server.sqlite",
        std::iter::empty::<(&str, &str)>(),
    );

    let response = reqwest::get(format!("{}/ai/capabilities", server.url))
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);

    drop(server);
}

fn read_http_request(stream: &mut std::net::TcpStream) -> Vec<u8> {
    let mut request = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let read = stream.read(&mut buffer).unwrap();
        assert!(read > 0, "upstream closed before completing request");
        request.extend_from_slice(&buffer[..read]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&request);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .and_then(|value| value.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            let header_end = request
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .unwrap()
                + 4;
            while request.len() < header_end + content_length {
                let read = stream.read(&mut buffer).unwrap();
                assert!(read > 0, "upstream closed before completing request");
                request.extend_from_slice(&buffer[..read]);
            }
            break;
        }
    }
    request
}

#[tokio::test]
async fn agent_turn_replays_native_calls_and_named_results_without_logging_content() {
    let env = TestEnv::new();
    let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
    let upstream_addr = upstream.local_addr().unwrap();
    let stub = thread::spawn(move || {
        for round in 0..2 {
            let (mut stream, _) = upstream.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                .unwrap();
            let request = read_http_request(&mut stream);
            let split = request.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
            let request: serde_json::Value = serde_json::from_slice(&request[split..]).unwrap();
            assert_eq!(request["model"], "fixture-model");
            assert_eq!(request["tools"][0]["function"]["name"], "search_tasks");
            if round == 1 {
                let messages = request["messages"].as_array().unwrap();
                assert_eq!(
                    messages[messages.len() - 2]["tool_calls"][0]["function"]["arguments"]["query"],
                    "argument-marker"
                );
                assert_eq!(messages.last().unwrap()["role"], "tool");
                assert_eq!(messages.last().unwrap()["tool_name"], "search_tasks");
                assert_eq!(messages.last().unwrap()["content"], "tool-result-marker");
            }
            let message = if round == 0 {
                serde_json::json!({"role":"assistant", "content":"", "tool_calls":[{"function":{"name":"search_tasks", "arguments":{"query":"argument-marker"}}}]})
            } else {
                serde_json::json!({"role":"assistant", "content":"generated-marker"})
            };
            let body = serde_json::json!({"model":"fixture-model", "created_at":"2026-10-18T00:00:00Z", "message":message, "done":true, "prompt_eval_count":10, "eval_count":5}).to_string();
            write!(stream, "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}", body.len(), body).unwrap();
        }
    });
    env.write_config(&format!("sync:\n  auth_token: test-secret\nai:\n  provider: ollama\n  model: fixture-model\n  endpoint: http://{upstream_addr}\n"));
    let server =
        TestServer::start_configured_with_env(&env, "server.sqlite", [("RUST_LOG", "debug")]);
    let client = reqwest::Client::new();
    let capabilities: serde_json::Value = client
        .get(format!("{}/ai/capabilities", server.url))
        .bearer_auth("test-secret")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(capabilities["features"]["tools"], true);
    let mut request = serde_json::json!({
        "request_id":"turn-1", "messages":[{"role":"user", "content":[{"type":"text", "text":"prompt-marker"}]}],
        "tools":[{"name":"search_tasks", "description":"schema-marker", "parameters":{"type":"object", "properties":{"query":{"type":"string"}}}}],
        "output":{"type":"text"}
    });
    let first = client
        .post(format!("{}/ai/completions", server.url))
        .bearer_auth("test-secret")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), reqwest::StatusCode::OK);
    let first: serde_json::Value = first.json().await.unwrap();
    assert_eq!(first["finish_reason"], "tool_calls");
    let id = first["message"]["content"][0]["id"].as_str().unwrap();
    request["messages"].as_array_mut().unwrap().extend([
        first["message"].clone(),
        serde_json::json!({"role":"tool", "content":[{"type":"tool_result", "call_id":id, "name":"search_tasks", "output":"tool-result-marker"}]}),
    ]);
    request["request_id"] = serde_json::json!("turn-2");
    let second = client
        .post(format!("{}/ai/completions", server.url))
        .bearer_auth("test-secret")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), reqwest::StatusCode::OK);
    let second: serde_json::Value = second.json().await.unwrap();
    assert_eq!(second["finish_reason"], "stop");
    assert_eq!(second["message"]["content"][0]["text"], "generated-marker");
    stub.join().unwrap();
    for secret in [
        "prompt-marker",
        "schema-marker",
        "argument-marker",
        "tool-result-marker",
        "generated-marker",
    ] {
        assert!(!server.output().contains(secret));
    }
}
