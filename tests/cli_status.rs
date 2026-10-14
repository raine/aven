mod common;

use std::path::Path;

use common::{TestEnv, ok};
use serde_json::Value;

fn configured(env: &TestEnv, db: &Path) {
    env.write_config(&format!(
        "local:\n  db_path: \"{}\"\nsync:\n  enabled: true\n",
        db.display()
    ));
}

#[test]
fn sync_status_reports_a_local_database_without_failing() {
    let env = TestEnv::new();
    let db = env.db("status.sqlite");
    configured(&env, &db);
    ok(env.aven_config(["add", "Local task"]));

    let report: Value =
        serde_json::from_str(&ok(env.aven_config(["sync", "status", "--json"]))).unwrap();
    assert_eq!(report["version"], 1);
    assert_eq!(report["state"], "not-set-up");
    assert!(report["server"].is_null());
    let human = ok(env.aven_config(["sync", "status"]));
    assert!(human.contains("Sync: not set up"), "{human}");
    assert!(human.contains("aven sync setup"), "{human}");

    // The database stays usable locally.
    assert!(ok(env.aven_config(["list"])).contains("Local task"));
}

#[test]
fn daemon_status_json_is_typed() {
    let env = TestEnv::new();
    let db = env.db("daemon-status.sqlite");
    configured(&env, &db);

    ok(env.aven_config(["daemon", "status"]));
    let json = ok(env.aven_config(["daemon", "status", "--json"]));
    let report: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(report["version"], 1);
    assert!(report["platform_supported"].is_boolean());
    assert!(report["installed"].is_boolean());
    assert!(report["sync_enabled"].is_boolean());
    assert!(report.get("server_configured").is_none());
    assert!(report["paths"].is_object());
}

#[test]
fn daemon_status_and_doctor_report_persisted_background_failure() {
    let env = TestEnv::new();
    let db = env.db("daemon-health.sqlite");
    configured(&env, &db);
    ok(env.aven_config(["add", "Local task"]));
    common::execute_sql(
        &db,
        r#"INSERT INTO meta(key, value) VALUES ('sync_daemon_health',
        '{"last_success_at":100,"error":{"code":"protected-key-storage-unavailable","message":"Choose Sync now, then Always Allow.","since":110},"next_retry_at":410}')"#,
    );
    let human = ok(env.aven_config(["daemon", "status"]));
    assert!(
        human.contains("Last daemon sync: 1970-01-01T00:01:40"),
        "{human}"
    );
    assert!(
        human.contains("Daemon error: protected-key-storage-unavailable since 1970-01-01T00:01:50"),
        "{human}"
    );
    let json: Value =
        serde_json::from_str(&ok(env.aven_config(["daemon", "status", "--json"]))).unwrap();
    assert_eq!(json["sync"]["last_success_at"], 100);
    assert_eq!(
        json["sync"]["error"]["code"],
        "protected-key-storage-unavailable"
    );
    let output = env.aven_config(["doctor", "--json"]);
    assert!(output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let section = report["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|section| section["title"] == "Daemon")
        .unwrap();
    let rows = section["rows"].as_array().unwrap();
    assert!(rows.iter().any(|row| {
        row["label"] == "last daemon sync"
            && row["value"]
                .as_str()
                .is_some_and(|message| message.contains("1970-01-01T00:01:40"))
    }));
    assert!(
        rows.iter()
            .any(|row| row["label"] == "daemon error" && row["status"] == "error")
    );
}
