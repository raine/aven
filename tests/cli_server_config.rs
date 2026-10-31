mod common;

use std::fs;

use common::TestEnv;

fn write_server_config(env: &TestEnv, text: &str) -> std::path::PathBuf {
    let path = env.path("server/server.yaml");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, text).unwrap();
    path
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
    assert!(text.contains("# attachments:"));
    assert!(!text.contains("ai:"));
    assert!(String::from_utf8_lossy(&first.stdout).contains("(commented template)"));

    fs::write(&template, "attachments:\n  grace_days: 17\n").unwrap();
    assert!(setup().status.success());
    assert!(
        fs::read_to_string(&template)
            .unwrap()
            .contains("grace_days: 17")
    );
}

#[test]
fn ai_commands_are_not_exposed() {
    let env = TestEnv::new();
    for args in [vec!["ai", "--help"], vec!["server", "ai", "--help"]] {
        let output = env.aven_config(args);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("unrecognized subcommand"));
    }
}

#[tokio::test]
async fn server_ignores_client_config_and_has_no_ai_routes() {
    let env = TestEnv::new();
    let client_dir = env.config_dir().join("aven");
    fs::create_dir_all(&client_dir).unwrap();
    fs::write(client_dir.join("config.yaml"), "invalid: [yaml").unwrap();
    write_server_config(&env, "attachments:\n  grace_days: 17\n");
    let server = common::TestServer::start_configured_with_env(
        &env,
        "server/sync-server.sqlite",
        std::iter::empty::<(&str, &str)>(),
    );
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap();
    for (method, path) in [
        (reqwest::Method::GET, "/ai/capabilities"),
        (reqwest::Method::POST, "/ai/completions/stream"),
        (reqwest::Method::POST, "/ai/transcriptions"),
    ] {
        let response = client
            .request(method, format!("{}{path}", server.url))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND, "{path}");
    }
}
