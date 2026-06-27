mod common;

use std::net::UdpSocket;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use serde_json::json;

use common::{
    TestEnv, TestProcess, TestServer, contains_all, contains_none, eventually, extract_ref, fail,
    ok, scalar_i64,
};

const MAX_PUSH_BATCH: usize = 256;
const DAEMON_SYNC_PAGE_BUDGET: usize = 8;
const DEFAULT_WORKSPACE_ID: &str = "0000000000000000";
const APP_PROJECT_ID: &str = "APP0000000000000";

fn exec_sql(db: &Path, sql: &str) {
    let output = Command::new("sqlite3")
        .arg(db)
        .arg(sql)
        .output()
        .expect("run sqlite");
    assert!(
        output.status.success(),
        "sqlite failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn seed_budgeted_local_backlog(db: &Path, count: usize) {
    exec_sql(
        db,
        &format!(
            "INSERT OR IGNORE INTO projects(id, workspace_id, key, name, prefix, created_at, updated_at)
             VALUES ('{APP_PROJECT_ID}', '{DEFAULT_WORKSPACE_ID}', 'app', 'app', 'APP',
                     '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"
        ),
    );

    for start in (1..=count).step_by(128) {
        let end = (start + 127).min(count);
        let task_values = (start..=end)
            .map(|seq| {
                format!(
                    "('{DEFAULT_WORKSPACE_ID}', 'TSK{seq:013}', 'budgeted daemon task {seq}', '', '{APP_PROJECT_ID}', 'inbox', 'none', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let change_values = (start..=end)
            .map(|seq| {
                let payload = json!({
                    "workspace_id": DEFAULT_WORKSPACE_ID,
                    "workspace_key": "default",
                    "title": format!("budgeted daemon task {seq}"),
                    "description": "",
                    "project_id": APP_PROJECT_ID,
                    "project_key": "app",
                    "project_name": "app",
                    "project_prefix": "APP",
                    "status": "inbox",
                    "priority": "none",
                    "created_at": "2026-01-01T00:00:00Z"
                })
                .to_string()
                .replace('\'', "''");
                format!(
                    "('CHG{seq:013}', 'client-a', {seq}, 'task', 'TSK{seq:013}', NULL, 'create_task', '{payload}', NULL, '2026-01-01T00:00:00Z', NULL)"
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        exec_sql(
            db,
            &format!(
                "BEGIN;
                 INSERT INTO tasks(workspace_id, id, title, description, project_id, status, priority, created_at, updated_at, queue_activity_at) VALUES {task_values};
                 INSERT INTO changes(change_id, client_id, local_seq, entity_type, entity_id, field, op_type, payload, base_version, created_at, server_seq) VALUES {change_values};
                 COMMIT;"
            ),
        );
    }
}

#[test]
fn daemon_reports_startup_configuration_errors() {
    let env = TestEnv::new();
    env.write_config(
        r#"
sync:
  enabled: false
  server_url: "http://127.0.0.1:9"
"#,
    );
    contains_all(&fail(env.aven_config(["daemon"])), &["error sync-disabled"]);

    let env = TestEnv::new();
    env.write_config(
        r#"
sync:
  enabled: true
"#,
    );
    contains_all(
        &fail(env.aven_config(["daemon"])),
        &["error sync-server-required"],
    );

    let env = TestEnv::new();
    env.write_config(
        r#"
sync:
  enabled: true
  server_url: "http://127.0.0.1:9"

daemon:
  wake_addr: "not-an-address"
"#,
    );
    contains_all(
        &fail(env.aven_config(["daemon"])),
        &["invalid daemon wake address"],
    );

    let env = TestEnv::new();
    env.write_config(
        r#"
sync:
  enabled: true
  server_url: "http://127.0.0.1:9"

daemon:
  wake_addr: "0.0.0.0:47631"
"#,
    );
    contains_all(
        &fail(env.aven_config(["daemon"])),
        &["error daemon-wake-requires-loopback"],
    );
}

#[test]
fn daemon_refuses_wake_port_that_is_already_bound() {
    let env = TestEnv::new();
    let db = env.db("daemon.sqlite");
    let wake_addr = env.free_loopback_addr();
    let _socket = UdpSocket::bind(&wake_addr).expect("bind wake addr");
    env.write_config(&format!(
        r#"
local:
  db_path: "{}"

sync:
  enabled: true
  server_url: "http://127.0.0.1:9"

daemon:
  wake_addr: "{}"
"#,
        db.display(),
        wake_addr
    ));

    let error = fail(env.aven_config(["daemon"]));
    contains_all(
        &error,
        &[
            "could not bind daemon wake address",
            "is another daemon running",
        ],
    );
}

#[test]
fn daemon_wake_syncs_representative_mutations() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let client_a = env.db("client-a.sqlite");
    let client_b = env.db("client-b.sqlite");
    let wake_addr = env.free_loopback_addr();
    env.write_daemon_config(&client_a, &server, &wake_addr, 3600);

    let daemon = TestProcess::start_daemon(&env);
    daemon.wait_for_log("daemon-synced", Duration::from_secs(5));

    let mark = daemon.log_mark();
    ok(env.aven_config(["label", "create", "sync"]));
    let task_ref = extract_ref(&ok(env.aven_config([
        "add",
        "wake synced task",
        "--project",
        "app",
        "--label",
        "sync",
    ])));
    daemon.wait_for_log_after(mark, "daemon-synced", Duration::from_secs(5));

    let mark = daemon.log_mark();
    ok(env.aven_config(["update", &task_ref, "--status", "active"]));
    ok(env.aven_config_stdin(["note", &task_ref, "--stdin"], "wake note\n"));
    ok(env.aven_config(["delete", &task_ref]));
    ok(env.aven_config(["restore", &task_ref]));
    daemon.wait_for_log_after(mark, "daemon-synced", Duration::from_secs(5));

    ok(env.aven(&client_b, ["sync", "--server", &server.url]));
    let shown = ok(env.aven(&client_b, ["show", &task_ref, "--full"]));
    contains_all(
        &shown,
        &[&task_ref, "wake synced task", "status=active", "wake note"],
    );
    contains_none(&shown, &["deleted=yes"]);
}

#[test]
fn daemon_periodic_syncs_without_wake() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let client_a = env.db("client-a.sqlite");
    let client_b = env.db("client-b.sqlite");
    let wake_addr = env.free_loopback_addr();
    env.write_daemon_config(&client_a, &server, &wake_addr, 1);

    let daemon = TestProcess::start_daemon(&env);
    daemon.wait_for_log("daemon-synced", Duration::from_secs(5));
    let mark = daemon.log_mark();
    let task_ref = extract_ref(&ok(env.aven(
        &client_a,
        ["add", "periodic synced task", "--project", "app"],
    )));

    daemon.wait_for_log_after(mark, "daemon-synced", Duration::from_secs(5));
    ok(env.aven(&client_b, ["sync", "--server", &server.url]));
    let list = ok(env.aven(&client_b, ["list", "--all"]));
    contains_all(&list, &[&task_ref, "periodic synced task"]);
}

#[test]
fn two_daemons_converge_bidirectionally() {
    let root = TestEnv::new();
    let env_a = TestEnv::new();
    let env_b = TestEnv::new();
    let server = TestServer::start(&root);
    let client_a = env_a.db("client-a.sqlite");
    let client_b = env_b.db("client-b.sqlite");
    env_a.write_daemon_config(&client_a, &server, &env_a.free_loopback_addr(), 1);
    env_b.write_daemon_config(&client_b, &server, &env_b.free_loopback_addr(), 1);

    let daemon_a = TestProcess::start_daemon(&env_a);
    let daemon_b = TestProcess::start_daemon(&env_b);
    daemon_a.wait_for_log("daemon-synced", Duration::from_secs(5));
    daemon_b.wait_for_log("daemon-synced", Duration::from_secs(5));

    let a_ref = extract_ref(&ok(env_a.aven_config([
        "add",
        "task from daemon a",
        "--project",
        "app",
    ])));
    let b_ref = extract_ref(&ok(env_b.aven_config([
        "add",
        "task from daemon b",
        "--project",
        "app",
    ])));

    eventually(Duration::from_secs(10), || {
        let list_a = ok(env_a.aven_config(["list", "--all"]));
        let list_b = ok(env_b.aven_config(["list", "--all"]));
        list_a.contains(&a_ref)
            && list_a.contains(&b_ref)
            && list_b.contains(&a_ref)
            && list_b.contains(&b_ref)
    });
}

#[test]
fn sync_auth_daemon_sends_token() {
    let server_env = TestEnv::new();
    server_env.write_config(
        r#"
sync:
  auth_token: "secret"
"#,
    );
    let server = TestServer::start_configured(&server_env, "server.sqlite");

    let env = TestEnv::new();
    let client_a = env.db("client-a.sqlite");
    let client_b = env.db("client-b.sqlite");
    let wake_addr = env.free_loopback_addr();

    env.write_daemon_config_with_auth(&client_a, &server, &wake_addr, 3600, Some("secret"));

    let daemon = TestProcess::start_daemon(&env);
    daemon.wait_for_log("daemon-synced", Duration::from_secs(5));

    let mark = daemon.log_mark();
    let task_ref = extract_ref(&ok(env.aven_config([
        "add",
        "daemon auth task",
        "--project",
        "app",
    ])));
    daemon.wait_for_log_after(mark, "daemon-synced", Duration::from_secs(5));

    ok(env.aven_config([
        "--db",
        client_b.to_str().expect("utf8 db path"),
        "sync",
        "--server",
        &server.url,
    ]));
    contains_all(
        &ok(env.aven(&client_b, ["show", &task_ref])),
        &[&task_ref, "daemon auth task"],
    );
}

#[test]
fn daemon_syncs_large_backlog_across_budgeted_rounds() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let client_a = env.db("client-a.sqlite");
    let client_b = env.db("client-b.sqlite");
    let wake_addr = env.free_loopback_addr();
    let daemon_log = env.path("daemon-client-reuse.log");
    env.write_daemon_config(&client_a, &server, &wake_addr, 3600);

    ok(env.aven_config(["list", "--all"]));
    let task_count = MAX_PUSH_BATCH * DAEMON_SYNC_PAGE_BUDGET + 1;
    seed_budgeted_local_backlog(&client_a, task_count);

    let daemon = TestProcess::start_daemon_with_env(
        &env,
        [
            ("AVEN_LOG", "aven=debug"),
            ("AVEN_LOG_FILE", daemon_log.to_str().unwrap()),
        ],
    );
    let first_pushed = MAX_PUSH_BATCH * DAEMON_SYNC_PAGE_BUDGET;
    let incomplete = format!(
        "daemon-synced pushed={first_pushed} pulled=0 cursor={first_pushed} complete=false pages={DAEMON_SYNC_PAGE_BUDGET}"
    );
    let complete =
        format!("daemon-synced pushed=1 pulled=0 cursor={task_count} complete=true pages=1");

    daemon.wait_for_log(&incomplete, Duration::from_secs(10));
    daemon.wait_for_log(&complete, Duration::from_secs(10));
    let output = daemon.output();
    assert!(
        output
            .find(&incomplete)
            .expect("incomplete daemon sync marker")
            < output.find(&complete).expect("complete daemon sync marker"),
        "daemon output should report incomplete work before completion\n{output}"
    );
    assert_eq!(
        scalar_i64(
            &client_a,
            "SELECT count(*) FROM changes WHERE server_seq IS NULL"
        ),
        0
    );

    ok(env.aven(&client_b, ["sync", "--server", &server.url]));
    assert_eq!(
        scalar_i64(
            &client_b,
            "SELECT count(*) FROM tasks WHERE title LIKE 'budgeted daemon task %'",
        ),
        task_count as i64
    );

    // Assert the daemon reused the same HTTP client across wake rounds
    let logs = std::fs::read_to_string(&daemon_log).expect("read daemon log for client reuse");
    let client_ids: Vec<&str> = logs
        .lines()
        .filter(|line| line.contains("sync client starting") && line.contains("http_client_id="))
        .filter_map(|line| line.split("http_client_id=").nth(1))
        .filter_map(|value| {
            value
                .split_whitespace()
                .next()
                .or_else(|| value.split(',').next())
        })
        .collect();
    assert!(
        client_ids.len() >= 2,
        "expected at least 2 sync client starting events, got {}",
        client_ids.len()
    );
    let first = client_ids[0];
    for (i, id) in client_ids.iter().enumerate() {
        assert_eq!(
            *id, first,
            "http_client_id mismatch at index {i}: expected {first}, got {id}"
        );
    }
}
