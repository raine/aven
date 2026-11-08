mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use common::{TestEnv, TestProcess, ok};

const STARTUP: Duration = Duration::from_secs(10);
/// Longer than the config poll, the lock retry and a slow test machine.
const PICKUP: Duration = Duration::from_secs(20);

fn write_config(env: &TestEnv, db: &Path, extra: &str) {
    env.write_config(&format!(
        r#"
local:
  db_path: "{}"
{extra}"#,
        db.display()
    ));
}

fn daemon(env: &TestEnv) -> TestProcess {
    TestProcess::spawn_daemon(env, std::iter::empty::<(&str, &str)>())
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + PICKUP;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn a_daemon_without_work_idles_and_takes_up_config_changes() {
    let env = TestEnv::new();
    let db = env.db("idle.sqlite");
    write_config(&env, &db, "");

    let mut daemon = daemon(&env);
    daemon.wait_for_log("daemon db=", STARTUP);
    daemon.wait_for_log("daemon-idle", STARTUP);
    std::thread::sleep(Duration::from_millis(500));
    assert!(daemon.is_running(), "an idle daemon keeps running");

    let status: serde_json::Value =
        serde_json::from_str(&ok(env.aven_config(["daemon", "status", "--json"]))).unwrap();
    assert_eq!(status["host"]["state"], "running");
    assert_eq!(status["host"]["status"]["role"], "daemon");
    assert_eq!(
        status["host"]["status"]["capabilities"],
        serde_json::json!([])
    );

    let mark = daemon.log_mark();
    write_config(&env, &db, "reminders:\n  enabled: true\n");
    daemon.wait_for_log_after(
        mark,
        "daemon-config-reloaded sync=false reminders=true",
        PICKUP,
    );
    wait_until("the host to record reminders", || {
        let status: serde_json::Value =
            serde_json::from_str(&ok(env.aven_config(["daemon", "status", "--json"]))).unwrap();
        status["host"]["status"]["capabilities"] == serde_json::json!(["reminders"])
    });
    let human = ok(env.aven_config(["daemon", "status"]));
    assert!(
        human.contains("Background host: running (aven daemon, pid"),
        "{human}"
    );
    assert!(daemon.is_running());
}

#[test]
fn a_second_daemon_waits_for_the_first_and_takes_over_when_it_stops() {
    let env = TestEnv::new();
    let db = env.db("two.sqlite");
    write_config(&env, &db, "");

    let first = daemon(&env);
    first.wait_for_log("daemon db=", STARTUP);
    let mut second = daemon(&env);
    second.wait_for_log("daemon-waiting reason=host-running", STARTUP);
    assert!(!second.output().contains("daemon db="));
    assert!(second.is_running());

    drop(first);
    second.wait_for_log("daemon db=", PICKUP);
}

/// Two users on one machine: each has their own home, config and database,
/// and both configs still name the wake address every Aven once shared.
/// Each user's daemon hosts its own database on its own wake socket.
#[test]
fn daemons_of_two_users_both_host_with_the_old_shared_wake_address() {
    let users = [TestEnv::new(), TestEnv::new()];
    let mut daemons = Vec::new();
    for env in &users {
        let db = env.db("aven.sqlite");
        write_config(
            env,
            &db,
            "daemon:\n  wake_addr: \"127.0.0.1:47631\"\nreminders:\n  enabled: true\n",
        );
        let daemon = daemon(env);
        daemon.wait_for_log("daemon db=", STARTUP);
        let socket = aven_host::host::wake::paths(&db)[0].clone();
        assert!(
            daemon
                .output()
                .contains(&format!("wake={}", socket.display())),
            "{}",
            daemon.output()
        );
        daemons.push(daemon);
    }
    for (env, daemon) in users.iter().zip(&mut daemons) {
        assert!(daemon.is_running());
        assert!(!daemon.output().contains("daemon-waiting"));
        let status: serde_json::Value =
            serde_json::from_str(&ok(env.aven_config(["daemon", "status", "--json"]))).unwrap();
        assert_eq!(status["host"]["state"], "running", "{status}");
        assert_eq!(status["host"]["status"]["role"], "daemon");
    }
}

#[test]
fn daemon_status_says_a_turned_off_daemon_is_off_and_how_to_turn_it_on() {
    let env = TestEnv::new();
    let db = env.db("off.sqlite");
    // What joining from Aven Desktop with the background offer off leaves.
    write_config(&env, &db, "background: on\nsync:\n  enabled: false\n");
    ok(env.aven_config(["add", "Local task"]));

    // A home of its own, so no installed agent of this machine's user shows.
    let home = env.path("home");
    let status = |args: &[&str]| ok(env.aven_config_env(args, [("HOME", &home)]));
    let human = status(&["daemon", "status"]);
    if !human.contains("platform unsupported") {
        assert!(human.contains("Daemon: off"), "{human}");
        assert!(human.contains("Keep syncing after Aven quits"), "{human}");
        assert!(
            human.contains("aven config set sync.enabled true"),
            "{human}"
        );
        assert!(!human.contains("restart"), "{human}");
        assert!(!human.contains("invalid"), "{human}");
        assert!(
            human.contains("Configuration: sync off, reminders off"),
            "{human}"
        );
        let json: serde_json::Value =
            serde_json::from_str(&status(&["daemon", "status", "--json"])).unwrap();
        assert_eq!(json["state"], "off");
    }
}
