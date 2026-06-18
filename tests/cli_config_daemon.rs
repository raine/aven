mod common;

use std::thread;
use std::time::{Duration, Instant};

use common::{TestEnv, TestProcess, TestServer, contains_all, extract_ref, ok};

#[test]
fn config_db_path_and_sync_server_are_used() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let db = env.db("configured.sqlite");
    env.write_config(&format!(
        r#"
[local]
db_path = "{}"

[sync]
enabled = true
server_url = "{}"
interval_seconds = 30
"#,
        db.display(),
        server.url
    ));

    let task_ref = extract_ref(&ok(env.atm_config([
        "add",
        "configured task",
        "--project",
        "app",
    ])));
    let sync = ok(env.atm_config(["sync"]));
    contains_all(&sync, &["synced", "cursor="]);

    let shown = ok(env.atm_config(["show", &task_ref]));
    contains_all(&shown, &[&task_ref, "configured task"]);
}

#[test]
fn daemon_auto_syncs_configured_database() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let client_a = env.db("client-a.sqlite");
    let client_b = env.db("client-b.sqlite");
    let wake_addr = env.free_loopback_addr();
    env.write_config(&format!(
        r#"
[local]
db_path = "{}"

[sync]
enabled = true
server_url = "{}"
interval_seconds = 60

[daemon]
wake_addr = "{}"
"#,
        client_a.display(),
        server.url,
        wake_addr
    ));

    let _daemon = TestProcess::start_daemon(&env);
    let task_ref = extract_ref(&ok(env.atm_config([
        "add",
        "daemon synced task",
        "--project",
        "app",
    ])));

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        ok(env.atm(&client_b, ["sync", "--server", &server.url]));
        let list_b = ok(env.atm(&client_b, ["list", "--all"]));
        if list_b.contains(&task_ref) && list_b.contains("daemon synced task") {
            break;
        }
        assert!(Instant::now() < deadline, "daemon did not sync task");
        thread::sleep(Duration::from_millis(100));
    }
}
