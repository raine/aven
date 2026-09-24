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
