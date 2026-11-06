mod common;

use std::fs::File;
use std::net::UdpSocket;
use std::path::Path;
use std::time::{Duration, Instant};

use common::{TestEnv, TestProcess, ok};

const STARTUP: Duration = Duration::from_secs(10);
/// Longer than the config poll, the lock retry and a slow test machine.
const PICKUP: Duration = Duration::from_secs(20);

fn write_config(env: &TestEnv, db: &Path, wake_addr: &str, extra: &str) {
    env.write_config(&format!(
        r#"
local:
  db_path: "{}"
daemon:
  wake_addr: "{wake_addr}"
{extra}"#,
        db.display()
    ));
}

fn daemon(env: &TestEnv) -> TestProcess {
    TestProcess::spawn_daemon(env, std::iter::empty::<(&str, &str)>())
}

fn host_lock(db: &Path) -> File {
    let mut name = db.file_name().unwrap().to_os_string();
    name.push(".host.lock");
    File::open(db.with_file_name(name)).expect("open host lock")
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
    let wake_addr = env.free_loopback_addr();
    write_config(&env, &db, &wake_addr, "");

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
    write_config(&env, &db, &wake_addr, "reminders:\n  enabled: true\n");
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
    let wake_addr = env.free_loopback_addr();
    write_config(&env, &db, &wake_addr, "");

    let first = daemon(&env);
    first.wait_for_log("daemon db=", STARTUP);
    let mut second = daemon(&env);
    second.wait_for_log("daemon-waiting reason=host-running", STARTUP);
    assert!(!second.output().contains("daemon db="));
    assert!(second.is_running());

    drop(first);
    second.wait_for_log("daemon db=", PICKUP);
}

#[test]
fn a_daemon_waits_while_an_old_daemon_owns_the_wake_address() {
    let env = TestEnv::new();
    let db = env.db("old.sqlite");
    let old_daemon = UdpSocket::bind("127.0.0.1:0").expect("bind wake address");
    let wake_addr = old_daemon.local_addr().unwrap().to_string();
    write_config(&env, &db, &wake_addr, "");

    let mut daemon = daemon(&env);
    daemon.wait_for_log("daemon-waiting reason=wake-address-busy", STARTUP);
    assert!(daemon.is_running());
    assert!(!daemon.output().contains("daemon db="));
    // It gave the lock up again, so it keeps nobody else from hosting.
    let lock = host_lock(&db);
    lock.try_lock().expect("the waiting daemon holds no lock");
}
