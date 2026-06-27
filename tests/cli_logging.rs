mod common;

use std::time::Duration;

use common::{TestEnv, TestProcess, TestServer, contains_all, contains_none, ok};

#[test]
fn logging_writes_to_default_state_file_without_affecting_output() {
    let env = TestEnv::new();
    let db = env.db("tasks.sqlite");
    let state_home = env.path("state");
    let output = common::command_with_db(&db)
        .env_remove("AVEN_LOG")
        .env_remove("AVEN_LOG_FILE")
        .env("XDG_STATE_HOME", &state_home)
        .args([
            "add",
            "default log secret title",
            "--description",
            "default log secret body",
            "--project",
            "app",
        ])
        .output()
        .expect("run aven");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    contains_all(&stdout, &["created", "default log secret title"]);
    assert_eq!(stderr, "");

    let logs = std::fs::read_to_string(state_home.join("aven").join("aven.log"))
        .expect("read default logs");
    contains_all(&logs, &["task created", "task_id"]);
    contains_none(
        &logs,
        &["default log secret title", "default log secret body"],
    );
}

#[test]
fn file_logging_records_local_action_without_user_content() {
    let env = TestEnv::new();
    let db = env.db("tasks.sqlite");
    let log = env.path("aven.log");
    let mut command = common::command_with_db(&db);
    command
        .env("AVEN_LOG", "aven=debug")
        .env("AVEN_LOG_FILE", &log)
        .args([
            "add",
            "secret task title",
            "--description",
            "secret body",
            "--project",
            "app",
        ]);
    let output = command.output().expect("run logged aven");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");

    let logs = std::fs::read_to_string(log).expect("read logs");
    contains_all(&logs, &["task created", "task_id", "project_key"]);
    contains_none(&logs, &["secret task title", "secret body"]);
}

#[test]
fn sync_logging_does_not_print_auth_token() {
    let server_env = TestEnv::new();
    let server_log = server_env.path("server.log");
    server_env.write_config(
        r#"
sync:
  auth_token: "super-secret-token"
"#,
    );
    let server = TestServer::start_configured_with_env(
        &server_env,
        "server.sqlite",
        [
            ("AVEN_LOG", "aven=debug"),
            ("AVEN_LOG_FILE", server_log.to_str().unwrap()),
        ],
    );

    let client_env = TestEnv::new();
    let client_log = client_env.path("client.log");
    let db = client_env.db("client.sqlite");
    client_env.write_config(&format!(
        r#"
local:
  db_path: "{}"

sync:
  server_url: "{}"
  auth_token: "super-secret-token"
"#,
        db.display(),
        server.url
    ));
    ok(client_env.aven_config([
        "add",
        "auth log redaction title",
        "--description",
        "auth log redaction body",
        "--project",
        "app",
    ]));

    let mut command = common::command();
    command
        .env("AVEN_CONFIG_DIR", client_env.config_dir().join("aven"))
        .env_remove("AVEN_DB")
        .env_remove("AVEN_SYNC_SERVER")
        .env("AVEN_LOG", "aven=debug")
        .env("AVEN_LOG_FILE", &client_log)
        .args(["sync"]);
    let output = command.output().expect("run logged sync");
    assert!(output.status.success());

    let logs = format!(
        "{}\n{}",
        std::fs::read_to_string(server_log).expect("read server logs"),
        std::fs::read_to_string(client_log).expect("read client logs"),
    );
    contains_all(&logs, &["auth_enabled=true", "sync request", "sync client"]);
    contains_none(
        &logs,
        &[
            "super-secret-token",
            "auth log redaction title",
            "auth log redaction body",
        ],
    );
}

#[test]
fn daemon_sync_logging_redacts_task_content() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let db = env.db("client.sqlite");
    let wake_addr = env.free_loopback_addr();
    let log = env.path("daemon.log");
    env.write_daemon_config(&db, &server, &wake_addr, 3600);

    let daemon = TestProcess::start_daemon_with_env(
        &env,
        [
            ("AVEN_LOG", "aven=debug"),
            ("AVEN_LOG_FILE", log.to_str().unwrap()),
        ],
    );
    daemon.wait_for_log("daemon-synced", Duration::from_secs(5));

    let mark = daemon.log_mark();
    ok(env.aven_config([
        "add",
        "daemon log secret title",
        "--description",
        "daemon log secret body",
        "--project",
        "app",
    ]));
    daemon.wait_for_log_after(mark, "daemon-synced", Duration::from_secs(5));

    let logs = std::fs::read_to_string(log).expect("read daemon logs");
    contains_all(
        &logs,
        &[
            "daemon starting",
            "daemon sync completed",
            "pushed",
            "cursor",
            "pulled",
            "complete",
            "pages",
        ],
    );
    contains_none(
        &logs,
        &[
            "daemon log secret title",
            "daemon log secret body",
            "super-secret-token",
        ],
    );
}
