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
    assert_eq!(body["limits"]["max_request_bytes"], 65_536);

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

    let oversized = client
        .post(format!("{url}/ai/completions"))
        .bearer_auth("test-secret")
        .header("content-type", "application/json")
        .body(format!("{{\"padding\":\"{}\"}}", "x".repeat(65_536)))
        .send()
        .await
        .unwrap();
    assert_eq!(oversized.status(), reqwest::StatusCode::BAD_REQUEST);
    assert_eq!(
        oversized.json::<serde_json::Value>().await.unwrap()["error"]["category"],
        "invalid_request"
    );

    drop(server);
}

#[tokio::test]
async fn synthetic_json_schema_completion_uses_the_generic_service() {
    let env = TestEnv::new();
    let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
    let upstream_addr = upstream.local_addr().unwrap();
    let stub = thread::spawn(move || {
        let (mut stream, _) = upstream.accept().unwrap();
        let mut request = Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            let read = stream.read(&mut buffer).unwrap();
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
                    request.extend_from_slice(&buffer[..read]);
                }
                break;
            }
        }
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
