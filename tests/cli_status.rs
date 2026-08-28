mod common;

use std::path::Path;
use std::time::Duration;

use common::{TestEnv, extract_ref, ok, png_bytes};
use serde_json::Value;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode};
use sqlx::{Connection, SqliteConnection};

fn configured(env: &TestEnv, db: &Path, token: Option<&str>) {
    let auth = token
        .map(|token| format!("  auth_token: \"{token}\"\n"))
        .unwrap_or_default();
    env.write_config(&format!(
        "local:\n  db_path: \"{}\"\nsync:\n  enabled: true\n  server_url: \"https://sync.example.test/v1\"\n{auth}",
        db.display()
    ));
}

fn execute(db: &Path, statements: &[&'static str]) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let options = SqliteConnectOptions::new()
            .filename(db)
            .create_if_missing(false)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));
        let mut conn = SqliteConnection::connect_with(&options).await.unwrap();
        for statement in statements {
            sqlx::query(*statement).execute(&mut conn).await.unwrap();
        }
    });
}

fn json_status(env: &TestEnv) -> Value {
    serde_json::from_str(&ok(env.aven_config(["sync", "status", "--json"]))).unwrap()
}

#[test]
fn sync_status_distinguishes_disabled_unconfigured_pending_and_healthy() {
    let env = TestEnv::new();
    let db = env.db("status.sqlite");

    ok(env.aven(&db, ["list"]));
    env.write_config(&format!("local:\n  db_path: \"{}\"\n", db.display()));
    assert_eq!(json_status(&env)["state"], "disabled");

    env.write_config(&format!(
        "local:\n  db_path: \"{}\"\nsync:\n  enabled: true\n",
        db.display()
    ));
    assert_eq!(json_status(&env)["state"], "unconfigured");

    configured(&env, &db, None);
    let task_ref = extract_ref(&ok(env.aven_config(["add", "Pending status test"]))).to_string();
    let image = env.path("pending.png");
    std::fs::write(&image, png_bytes(2, 2)).unwrap();
    ok(env.aven_config(["attachment", "add", &task_ref, image.to_str().unwrap()]));
    let pending = json_status(&env);
    assert_eq!(pending["state"], "degraded");
    assert!(pending["pending"]["changes"].as_i64().unwrap() > 0);
    assert_eq!(pending["pending"]["attachment_uploads"], 1);
    assert!(
        pending["pending"]["attachment_upload_bytes"]
            .as_i64()
            .unwrap()
            > 0
    );

    execute(
        &db,
        &[
            "UPDATE changes SET server_seq = local_seq WHERE server_seq IS NULL",
            "INSERT INTO meta(key, value) VALUES ('sync_server_url', 'https://sync.example.test/v1') ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            "INSERT INTO meta(key, value) VALUES ('sync_last_attempt_at', '2026-01-01T00:00:00Z') ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            "INSERT INTO meta(key, value) VALUES ('sync_last_success_at', '2026-01-01T00:00:00Z') ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            "INSERT INTO meta(key, value) VALUES ('sync_last_error', '') ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        ],
    );
    assert_eq!(json_status(&env)["state"], "healthy");
}

#[test]
fn sync_status_reports_failures_and_conflicts_without_exposing_secrets() {
    let env = TestEnv::new();
    let db = env.db("failed.sqlite");
    let token = "top-secret-status-token";
    configured(&env, &db, Some(token));
    ok(env.aven_config(["add", "Conflict status test"]));
    execute(
        &db,
        &[
            "INSERT INTO meta(key, value) VALUES ('sync_last_attempt_at', '2026-01-02T00:00:00Z') ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            "INSERT INTO meta(key, value) VALUES ('sync_last_success_at', '2026-01-01T00:00:00Z') ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            "INSERT INTO meta(key, value) VALUES ('sync_last_error', 'unsafe response top-secret-status-token task body') ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        ],
    );

    let failed_json = ok(env.aven_config(["sync", "status", "--json"]));
    let failed_human = ok(env.aven_config(["sync", "status"]));
    assert!(!failed_json.contains(token));
    assert!(!failed_human.contains(token));
    assert_eq!(
        serde_json::from_str::<Value>(&failed_json).unwrap()["state"],
        "failed"
    );
    assert!(failed_human.contains("Last error: sync failed (details withheld)"));

    execute(
        &db,
        &[
            "INSERT INTO conflicts(workspace_id, task_id, field, local_value, remote_value, remote_change_id, variant_a, variant_b, created_at) SELECT workspace_id, id, 'title', 'local', 'remote', 'REMOTECHANGE0000', 'a', 'b', 't' FROM tasks LIMIT 1",
        ],
    );
    let blocked = json_status(&env);
    assert_eq!(blocked["state"], "blocked");
    assert_eq!(blocked["unresolved_conflicts"], 1);
}

#[test]
fn daemon_status_json_is_typed_and_never_contains_authentication_secrets() {
    let env = TestEnv::new();
    let db = env.db("daemon-status.sqlite");
    let token = "daemon-secret-token";
    configured(&env, &db, Some(token));

    let human = ok(env.aven_config(["daemon", "status"]));
    let json = ok(env.aven_config(["daemon", "status", "--json"]));
    assert!(!human.contains(token));
    assert!(!json.contains(token));
    let report: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(report["version"], 1);
    assert!(report["platform_supported"].is_boolean());
    assert!(report["installed"].is_boolean());
    assert!(report["paths"].is_object());
}
