mod common;

use std::path::Path;

use common::{TestEnv, contains_all, contains_none, meta_value, ok};
use sqlx::ConnectOptions;
use sqlx::sqlite::SqliteConnectOptions;

#[test]
fn doctor_reports_default_database_health() {
    let env = TestEnv::new();
    let db = env.db("doctor.sqlite");

    let output = ok(env.aven(&db, ["doctor"]));

    contains_all(
        &output,
        &[
            "aven doctor",
            "Configuration",
            "Database",
            "Workspace",
            "Sync",
            "database source    --db",
            "ok sqlite",
            "ok client id",
            "active workspace",
            "tasks",
            "server             not configured",
            "daemon wake",
        ],
    );
}

#[test]
fn doctor_reports_configured_paths_and_sync_settings() {
    let env = TestEnv::new();
    let db = env.db("configured-doctor.sqlite");
    let wake_addr = env.free_loopback_addr();
    env.write_config(&format!(
        r#"
local:
  db_path: "{}"

sync:
  enabled: true
  server_url: "http://127.0.0.1:3000"
  interval_seconds: 45

daemon:
  wake_addr: "{}"
"#,
        db.display(),
        wake_addr
    ));

    let output = ok(env.aven_config(["doctor"]));

    contains_all(
        &output,
        &[
            "database source    config local.db_path",
            &db.display().to_string(),
            "enabled            yes",
            "server",
            "http://127.0.0.1:3000",
            "45 seconds",
            &wake_addr,
        ],
    );
}

#[test]
fn doctor_reports_disabled_sync_without_server_error() {
    let env = TestEnv::new();
    let db = env.db("disabled-sync.sqlite");

    let output = ok(env.aven(&db, ["doctor"]));

    contains_all(
        &output,
        &["enabled            no", "server             not configured"],
    );
    assert!(!output.contains("!! server"));
}

#[test]
fn doctor_reports_enabled_sync_without_server_as_failed_check() {
    let env = TestEnv::new();
    let db = env.db("enabled-sync.sqlite");
    env.write_config(&format!(
        r#"
local:
  db_path: "{}"

sync:
  enabled: true
"#,
        db.display()
    ));

    let output = ok(env.aven_config(["doctor"]));

    contains_all(
        &output,
        &[
            "enabled            yes",
            "!! server",
            "sync-server-required",
        ],
    );
}

#[test]
fn doctor_reports_invalid_daemon_wake_address() {
    let env = TestEnv::new();
    let db = env.db("invalid-wake.sqlite");
    env.write_config(&format!(
        r#"
local:
  db_path: "{}"

daemon:
  wake_addr: "not-an-address"
"#,
        db.display()
    ));

    let output = ok(env.aven_config(["doctor"]));

    contains_all(&output, &["!! daemon wake", "invalid daemon wake address"]);
}

#[test]
fn doctor_reports_invalid_sync_server_url() {
    let env = TestEnv::new();
    let db = env.db("invalid-server.sqlite");
    env.write_config(&format!(
        r#"
local:
  db_path: "{}"

sync:
  enabled: true
  server_url: "not-a-url"
"#,
        db.display()
    ));

    let output = ok(env.aven_config(["doctor"]));

    contains_all(&output, &["!! server", "not-a-url", "!! daemon server"]);
}

#[test]
fn doctor_rejects_sync_server_url_shapes_that_sync_cannot_use() {
    let env = TestEnv::new();
    let db = env.db("server-shapes.sqlite");
    for server_url in [
        "http://user@127.0.0.1:3000",
        "http://127.0.0.1:3000?x=y",
        "http://127.0.0.1:3000#frag",
    ] {
        env.write_config(&format!(
            r#"
local:
  db_path: "{}"

sync:
  enabled: true
  server_url: "{}"
"#,
            db.display(),
            server_url
        ));

        let output = ok(env.aven_config(["doctor"]));

        contains_all(&output, &["!! server", server_url]);
    }
}

#[test]
fn doctor_reports_daemon_server_separately_from_env_server() {
    let env = TestEnv::new();
    let db = env.db("env-server.sqlite");
    env.write_config(&format!(
        r#"
local:
  db_path: "{}"

sync:
  enabled: true
"#,
        db.display()
    ));

    let output = std::process::Command::new(common::bin())
        .env("AVEN_CONFIG_DIR", env.config_dir().join("aven"))
        .env("AVEN_SYNC_SERVER", "http://127.0.0.1:3000")
        .env_remove("AVEN_DB")
        .arg("doctor")
        .output()
        .expect("run aven doctor with env server");
    let output = ok(output);

    contains_all(
        &output,
        &[
            "ok server",
            "http://127.0.0.1:3000",
            "!! daemon server",
            "not configured",
        ],
    );
}

#[test]
fn doctor_reports_pinned_server_mismatch() {
    let env = TestEnv::new();
    let db = env.db("pinned-server.sqlite");
    env.write_config(&format!(
        r#"
local:
  db_path: "{}"

sync:
  enabled: true
  server_url: "http://127.0.0.1:3000"
"#,
        db.display()
    ));
    ok(env.aven_config(["doctor"]));
    assert_eq!(meta_value(&db, "sync_server_url"), None);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("create tokio runtime");
    runtime.block_on(async {
        let mut conn = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&db)
            .create_if_missing(false)
            .connect()
            .await
            .expect("open db");
        sqlx::query(
            "INSERT INTO meta(key, value) VALUES ('sync_server_url', 'http://127.0.0.1:4000')",
        )
        .execute(&mut conn)
        .await
        .expect("pin server");
    });

    let output = ok(env.aven_config(["doctor"]));

    contains_all(
        &output,
        &[
            "!! server match",
            "pinned=http://127.0.0.1:4000 configured=http://127.0.0.1:3000",
        ],
    );
}

#[test]
fn doctor_workspace_flag_affects_active_workspace_and_task_counts() {
    let env = TestEnv::new();
    let db = env.db("workspace-doctor.sqlite");

    ok(env.aven(&db, ["workspace", "create", "alpha"]));
    ok(env.aven(&db, ["workspace", "create", "beta"]));
    ok(env.aven(
        &db,
        [
            "--workspace",
            "alpha",
            "add",
            "alpha task",
            "--project",
            "app",
        ],
    ));
    ok(env.aven(
        &db,
        ["--workspace", "beta", "add", "beta one", "--project", "app"],
    ));
    ok(env.aven(
        &db,
        ["--workspace", "beta", "add", "beta two", "--project", "app"],
    ));

    let alpha = ok(env.aven(&db, ["--workspace", "alpha", "doctor"]));
    contains_all(
        &alpha,
        &[
            "active workspace   alpha (alpha)",
            "tasks              1 visible, 1 total",
        ],
    );

    let beta = ok(env.aven(&db, ["--workspace", "beta", "doctor"]));
    contains_all(
        &beta,
        &[
            "active workspace   beta (beta)",
            "tasks              2 visible, 2 total",
        ],
    );
}

#[test]
fn doctor_with_integrity_reports_passed_checks() {
    let env = TestEnv::new();
    let db = env.db("integrity-ok-doctor.sqlite");

    ok(env.aven(&db, ["add", "integrity task", "--project", "app"]));
    let output = ok(env.aven(&db, ["doctor", "--integrity"]));

    contains_all(
        &output,
        &[
            "Integrity",
            "quick check",
            "task projects",
            "meta local_seq",
        ],
    );
    contains_none(&output, &["!! result"]);
}

#[test]
fn doctor_with_integrity_reports_orphaned_task_data() {
    let env = TestEnv::new();
    let db = env.db("integrity-fail-doctor.sqlite");

    ok(env.aven(&db, ["add", "orphan check", "--project", "app"]));
    run_sql(
        &db,
        "PRAGMA foreign_keys = OFF; INSERT INTO notes (workspace_id, id, task_id, body, created_at, change_id) SELECT workspace_id, 'orphan-note', 'orphan-task-id', 'orphan', '1970-01-01T00:00:00Z', 'orphan-change' FROM tasks LIMIT 1",
    );

    let output = ok(env.aven(&db, ["doctor", "--integrity"]));
    contains_all(&output, &["Integrity", "!! notes", "!! result"]);
}

#[test]
fn doctor_json_reports_default_database_health() {
    let env = TestEnv::new();
    let db = env.db("doctor-json.sqlite");

    let output = ok(env.aven(&db, ["doctor", "--json"]));
    let report: serde_json::Value = serde_json::from_str(&output).unwrap();
    let sections = report["sections"].as_array().unwrap();
    assert!(
        sections
            .iter()
            .any(|section| section["title"] == "Database")
    );
    assert!(sections.iter().any(|section| {
        section["rows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["label"] == "sqlite" && row["status"] == "ok")
    }));
}

#[test]
fn doctor_json_with_integrity_reports_integrity_section() {
    let env = TestEnv::new();
    let db = env.db("integrity-json-doctor.sqlite");
    ok(env.aven(&db, ["add", "integrity task", "--project", "app"]));

    let output = ok(env.aven(&db, ["doctor", "--json", "--integrity"]));
    let report: serde_json::Value = serde_json::from_str(&output).unwrap();
    let sections = report["sections"].as_array().unwrap();
    assert!(
        sections
            .iter()
            .any(|section| section["title"] == "Integrity")
    );
}

fn run_sql(db: &Path, sql: &'static str) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("create runtime");
    runtime.block_on(async {
        let mut conn = SqliteConnectOptions::new()
            .filename(db)
            .create_if_missing(false)
            .foreign_keys(true)
            .connect()
            .await
            .expect("open test db");
        sqlx::query(sql).execute(&mut conn).await.expect("run sql");
    });
}
