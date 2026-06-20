mod common;

use std::net::UdpSocket;
use std::time::Duration;

use common::{
    TestEnv, TestProcess, TestServer, contains_all, contains_none, eventually, extract_ref, fail,
    ok,
};

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
    contains_all(
        &fail(env.aven_config(["daemon", "run"])),
        &["error sync-disabled"],
    );

    let env = TestEnv::new();
    env.write_config(
        r#"
sync:
  enabled: true
"#,
    );
    contains_all(
        &fail(env.aven_config(["daemon", "run"])),
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
        &fail(env.aven_config(["daemon", "run"])),
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
        &fail(env.aven_config(["daemon", "run"])),
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

    let error = fail(env.aven_config(["daemon", "run"]));
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
