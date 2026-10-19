mod common;

use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

use common::{TestEnv, TestServer};

fn write_server_config(env: &TestEnv, text: &str) -> std::path::PathBuf {
    let path = env.path("server/server.yaml");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, text).unwrap();
    path
}

#[test]
fn ai_administrator_inspection_does_not_require_a_database() {
    let env = TestEnv::new();
    // The server ignores the client config entirely, even when it is invalid.
    env.write_config("tui: {unclosed\n");

    let providers = env.aven_config(["server", "ai", "providers"]);
    assert!(providers.status.success());
    assert!(String::from_utf8_lossy(&providers.stdout).contains("chatgpt auth=oauth"));

    let status = env.aven_config(["server", "ai", "status", "--json"]);
    assert!(status.status.success());
    assert!(String::from_utf8_lossy(&status.stdout).contains("\"status\": \"unconfigured\""));
}

#[test]
fn server_config_beside_the_data_is_overridden_by_environment() {
    let env = TestEnv::new();
    let config = write_server_config(&env, "ai:\n  provider: ollama\n  model: file-model\n");
    let data = config.with_file_name("sync-server.sqlite");

    let status = env.aven_config_env(
        [
            "server".as_ref(),
            "ai".as_ref(),
            "status".as_ref(),
            "--data".as_ref(),
            data.as_os_str(),
        ],
        [("AVEN_AI_MODEL", "env-model")],
    );
    let stdout = String::from_utf8_lossy(&status.stdout);
    assert!(status.status.success(), "{stdout}");
    assert!(
        stdout.contains("provider=ollama provider_source=file"),
        "{stdout}"
    );
    assert!(
        stdout.contains("model=env-model model_source=env"),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!("config={}", config.display())),
        "{stdout}"
    );

    let other = env.path("elsewhere.yaml");
    let status = env.aven_config_env(
        [
            "server".as_ref(),
            "ai".as_ref(),
            "status".as_ref(),
            "--data".as_ref(),
            data.as_os_str(),
        ],
        [("AVEN_SERVER_CONFIG", other.as_os_str())],
    );
    assert!(String::from_utf8_lossy(&status.stdout).contains("provider=none"));
}

#[test]
fn invalid_server_config_fails_startup_naming_the_key() {
    let env = TestEnv::new();
    let config = write_server_config(&env, "attachments:\n  grace_dayz: 3\n");
    let output = env.aven_config([
        "server".as_ref(),
        "--data".as_ref(),
        env.path("missing.sqlite").as_os_str(),
        "--config".as_ref(),
        config.as_os_str(),
    ]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("grace_dayz"), "{stderr}");
    assert!(stderr.contains("server.yaml"), "{stderr}");
}

#[test]
fn server_setup_writes_a_commented_template_once() {
    let env = TestEnv::new();
    let data = env.path("server/sync-server.sqlite");
    let setup = || {
        env.aven_config([
            "server".as_ref(),
            "setup".as_ref(),
            "--url".as_ref(),
            "http://127.0.0.1:3746".as_ref(),
            "--data".as_ref(),
            data.as_os_str(),
        ])
    };
    let first = setup();
    assert!(first.status.success());
    let template = env.path("server/server.yaml");
    let text = fs::read_to_string(&template).unwrap();
    assert!(text.contains("# ai:"));
    assert!(String::from_utf8_lossy(&first.stdout).contains("(commented template)"));

    fs::write(&template, "ai:\n  provider: ollama\n  model: kept\n").unwrap();
    assert!(setup().status.success());
    assert!(fs::read_to_string(&template).unwrap().contains("kept"));
}

#[tokio::test]
async fn unconfigured_ai_is_authenticated_bounded_and_does_not_block_sync_server() {
    let env = TestEnv::new();
    env.write_config("");
    let vault = aven_core::test_support::PublishedVault::new().await;
    let credential = hex::encode(vault.seed.bearer().expose());
    let server = TestServer::start_prepared_with_env(
        &env,
        vault.db.path(),
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
        .bearer_auth(&credential)
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
        .bearer_auth(&credential)
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
            .bearer_auth(&credential)
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

    let config = write_server_config(
        &env,
        &format!(
            "ai:\n  provider: ollama\n  model: stub-model\n  endpoint: http://{upstream_addr}\n"
        ),
    );
    fs::write(env.path("server/private"), "not a directory").unwrap();
    let vault = aven_core::test_support::PublishedVault::new().await;
    let credential = hex::encode(vault.seed.bearer().expose());
    let server = TestServer::start_prepared_with_env(
        &env,
        vault.db.path(),
        [("AVEN_SERVER_CONFIG", &config)],
    );
    let response = reqwest::Client::new()
        .post(format!("{}/ai/completions", server.url))
        .bearer_auth(&credential)
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
async fn ai_requires_a_device_credential_even_on_loopback() {
    let env = TestEnv::new();
    env.write_config("");
    let vault = aven_core::test_support::PublishedVault::new().await;
    let credential = hex::encode(vault.seed.bearer().expose());
    let server = TestServer::start_prepared_with_env(
        &env,
        vault.db.path(),
        std::iter::empty::<(&str, &str)>(),
    );

    let response = reqwest::get(format!("{}/ai/capabilities", server.url))
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
    let authorized = reqwest::Client::new()
        .get(format!("{}/ai/capabilities", server.url))
        .bearer_auth(&credential)
        .send()
        .await
        .unwrap();
    assert_eq!(authorized.status(), reqwest::StatusCode::OK);

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
    let config = write_server_config(
        &env,
        &format!(
            "ai:\n  provider: ollama\n  model: fixture-model\n  endpoint: http://{upstream_addr}\n"
        ),
    );
    let vault = aven_core::test_support::PublishedVault::new().await;
    let credential = hex::encode(vault.seed.bearer().expose());
    let server = TestServer::start_prepared_with_env(
        &env,
        vault.db.path(),
        [
            ("RUST_LOG", std::ffi::OsStr::new("debug")),
            ("AVEN_SERVER_CONFIG", config.as_os_str()),
        ],
    );
    let client = reqwest::Client::new();
    let capabilities: serde_json::Value = client
        .get(format!("{}/ai/capabilities", server.url))
        .bearer_auth(&credential)
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
        .bearer_auth(&credential)
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
        .bearer_auth(&credential)
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

#[tokio::test]
async fn ai_accepts_only_current_device_credentials_and_hides_revocation() {
    use aven_core::sync::seed_claim::{
        membership::{Device, Invitation, Joiner, Membership},
        peer::Authentication,
    };
    let env = TestEnv::new();
    let old_token = "99".repeat(32);
    env.write_config(&format!("sync:\n  auth_token: {old_token:?}\n"));
    let vault = aven_core::test_support::PublishedVault::new().await;
    let initial = Membership::from_publication(
        vault.seed.genesis(),
        &vault.package.descriptor,
        vault.publication.record(),
    )
    .unwrap();
    let seed_device = vault.seed.genesis().device_id();
    let seed_auth = Authentication {
        vault: initial.genesis().context().vault_id,
        genesis: initial.genesis().commitment(),
        device: seed_device,
        head: initial.head(),
        bearer: vault.seed.bearer(),
    };
    let (invitation, declaration) = Device::seed(&vault.seed)
        .prepare_invitation(&initial, 3700)
        .unwrap();
    let peer = Joiner::generate(
        Invitation::from_protected_storage(&invitation.protected_storage_bytes()).unwrap(),
    )
    .unwrap();
    let record = Device::seed(&vault.seed)
        .prepare_admission(
            &initial,
            &declaration,
            &invitation,
            peer.request(),
            &initial.verify_initial_key(&vault.key).unwrap(),
        )
        .unwrap();
    vault
        .db
        .register_membership_invitation_at(&seed_auth, declaration.record(), 100)
        .await
        .unwrap();
    vault
        .db
        .post_membership_request_at(seed_auth.vault, declaration.handle(), peer.request(), 100)
        .await
        .unwrap();
    vault
        .db
        .admit_membership_device_at(&seed_auth, declaration.handle(), &record, 101)
        .await
        .unwrap();
    let membership = initial
        .append(declaration.record(), peer.request(), &record)
        .unwrap();
    let credential = hex::encode(vault.seed.bearer().expose());
    let peer_credential = hex::encode(peer.bearer().expose());
    let server =
        TestServer::start_prepared_with_env(&env, vault.db.path(), [("RUST_LOG", "debug")]);
    let client = reqwest::Client::new();
    let request = |path: &str| {
        if path == "/ai/capabilities" {
            client.get(format!("{}{path}", server.url))
        } else {
            client.post(format!("{}{path}", server.url)).json(&serde_json::json!({
                "request_id":"auth-test", "messages":[{"role":"user", "content":[{"type":"text", "text":"hello"}]}], "output":{"type":"text"}
            }))
        }
    };
    for path in ["/ai/capabilities", "/ai/completions"] {
        for bearer in [&credential, &peer_credential] {
            let response = request(path).bearer_auth(bearer).send().await.unwrap();
            if path == "/ai/capabilities" {
                assert_eq!(response.status(), reqwest::StatusCode::OK);
            } else {
                assert_eq!(response.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
                assert_eq!(
                    response.json::<serde_json::Value>().await.unwrap()["error"]["category"],
                    "unconfigured"
                );
            }
        }
        for header in [
            None,
            Some(format!("Bearer {old_token}")),
            Some(format!("Bearer {}", "55".repeat(32))),
            Some("Bearer bad".into()),
            Some(format!("Basic {credential}")),
            Some(format!("Bearer {}", "AB".repeat(32))),
            Some(format!("Bearer {credential}extra")),
        ] {
            let mut request = request(path);
            if let Some(header) = header {
                request = request.header("authorization", header);
            }
            let response = request.send().await.unwrap();
            assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
            assert!(response.bytes().await.unwrap().is_empty());
        }
        let duplicate = request(path)
            .header("authorization", format!("Bearer {credential}"))
            .header("authorization", format!("Bearer {peer_credential}"))
            .send()
            .await
            .unwrap();
        assert_eq!(duplicate.status(), reqwest::StatusCode::UNAUTHORIZED);
    }
    let peer_auth = Authentication {
        device: peer.device(),
        head: membership.head(),
        bearer: peer.bearer(),
        ..seed_auth
    };
    let revoke = peer
        .authority()
        .prepare_revoke(&membership, &[seed_device])
        .unwrap();
    vault
        .db
        .apply_membership_management(&peer_auth, &revoke)
        .await
        .unwrap();
    for path in ["/ai/capabilities", "/ai/completions"] {
        let revoked = request(path).bearer_auth(&credential).send().await.unwrap();
        assert_eq!(revoked.status(), reqwest::StatusCode::UNAUTHORIZED);
        assert!(revoked.bytes().await.unwrap().is_empty());
        let surviving = request(path)
            .bearer_auth(&peer_credential)
            .send()
            .await
            .unwrap();
        assert_ne!(surviving.status(), reqwest::StatusCode::UNAUTHORIZED);
    }
    for secret in [&credential, &peer_credential, &old_token] {
        assert!(!server.output().contains(secret));
    }
}
