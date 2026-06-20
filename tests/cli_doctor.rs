mod common;

use common::{TestEnv, contains_all, meta_value, ok};
use sqlx::ConnectOptions;

#[test]
fn doctor_reports_default_database_health() {
    let env = TestEnv::new();
    let db = env.db("doctor.sqlite");

    let output = ok(env.atm(&db, ["doctor"]));

    contains_all(
        &output,
        &[
            "atm doctor",
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
[local]
db_path = "{}"

[sync]
enabled = true
server_url = "http://127.0.0.1:3000"
interval_seconds = 45

[daemon]
wake_addr = "{}"
"#,
        db.display(),
        wake_addr
    ));

    let output = ok(env.atm_config(["doctor"]));

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

    let output = ok(env.atm(&db, ["doctor"]));

    contains_all(
        &output,
        &[
            "enabled            no",
            "server             not configured",
        ],
    );
    assert!(!output.contains("!! server"));
}

#[test]
fn doctor_reports_enabled_sync_without_server_as_failed_check() {
    let env = TestEnv::new();
    let db = env.db("enabled-sync.sqlite");
    env.write_config(&format!(
        r#"
[local]
db_path = "{}"

[sync]
enabled = true
"#,
        db.display()
    ));

    let output = ok(env.atm_config(["doctor"]));

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
[local]
db_path = "{}"

[daemon]
wake_addr = "not-an-address"
"#,
        db.display()
    ));

    let output = ok(env.atm_config(["doctor"]));

    contains_all(
        &output,
        &[
            "!! daemon wake",
            "invalid daemon wake address",
        ],
    );
}

#[test]
fn doctor_reports_invalid_sync_server_url() {
    let env = TestEnv::new();
    let db = env.db("invalid-server.sqlite");
    env.write_config(&format!(
        r#"
[local]
db_path = "{}"

[sync]
enabled = true
server_url = "not-a-url"
"#,
        db.display()
    ));

    let output = ok(env.atm_config(["doctor"]));

    contains_all(
        &output,
        &[
            "!! server",
            "not-a-url",
            "!! daemon server",
        ],
    );
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
[local]
db_path = "{}"

[sync]
enabled = true
server_url = "{}"
"#,
            db.display(),
            server_url
        ));

        let output = ok(env.atm_config(["doctor"]));

        contains_all(&output, &["!! server", server_url]);
    }
}

#[test]
fn doctor_reports_daemon_server_separately_from_env_server() {
    let env = TestEnv::new();
    let db = env.db("env-server.sqlite");
    env.write_config(&format!(
        r#"
[local]
db_path = "{}"

[sync]
enabled = true
"#,
        db.display()
    ));

    let output = std::process::Command::new(common::bin())
        .env(
            "ATM_CONFIG_DIR",
            env.config_dir().join("agentic-task-manager"),
        )
        .env("ATM_SYNC_SERVER", "http://127.0.0.1:3000")
        .env_remove("ATM_DB")
        .arg("doctor")
        .output()
        .expect("run atm doctor with env server");
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
[local]
db_path = "{}"

[sync]
enabled = true
server_url = "http://127.0.0.1:3000"
"#,
        db.display()
    ));
    ok(env.atm_config(["doctor"]));
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
        sqlx::query("INSERT INTO meta(key, value) VALUES ('sync_server_url', 'http://127.0.0.1:4000')")
            .execute(&mut conn)
            .await
            .expect("pin server");
    });

    let output = ok(env.atm_config(["doctor"]));

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

    ok(env.atm(&db, ["workspace", "create", "alpha"]));
    ok(env.atm(&db, ["workspace", "create", "beta"]));
    ok(env.atm(
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
    ok(env.atm(
        &db,
        [
            "--workspace",
            "beta",
            "add",
            "beta one",
            "--project",
            "app",
        ],
    ));
    ok(env.atm(
        &db,
        [
            "--workspace",
            "beta",
            "add",
            "beta two",
            "--project",
            "app",
        ],
    ));

    let alpha = ok(env.atm(&db, ["--workspace", "alpha", "doctor"]));
    contains_all(
        &alpha,
        &[
            "active workspace   alpha (alpha)",
            "tasks              1 visible, 1 total",
        ],
    );

    let beta = ok(env.atm(&db, ["--workspace", "beta", "doctor"]));
    contains_all(
        &beta,
        &[
            "active workspace   beta (beta)",
            "tasks              2 visible, 2 total",
        ],
    );
}
