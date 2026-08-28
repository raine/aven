mod common;

use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use common::{
    TestEnv, command_with_db, contains_all, contains_none, extract_ref, meta_value, ok, png_bytes,
};
use sqlx::ConnectOptions;
use sqlx::sqlite::SqliteConnectOptions;

#[test]
fn doctor_reports_default_database_health() {
    let env = TestEnv::new();
    let db = env.db("doctor.sqlite");
    initialize_db(&env, &db);

    let output = ok(env.aven(&db, ["doctor"]));

    contains_all(
        &output,
        &[
            "aven doctor",
            "Configuration",
            "Database",
            "Workspace",
            "Sync",
            "Daemon",
            "Attachment lifecycle",
            "database source    --db",
            "ok sqlite",
            "ok client id",
            "active workspace",
            "tasks",
            "server             not configured",
            "daemon wake",
            "referenced",
        ],
    );
    contains_none(&output, &["!! lifecycle"]);
}

#[test]
fn doctor_reports_workspace_resolution_failures() {
    let env = TestEnv::new();
    let db = env.db("missing-workspace-doctor.sqlite");
    initialize_db(&env, &db);

    let output = ok(env.aven(&db, ["--workspace", "missing", "doctor"]));

    contains_all(
        &output,
        &[
            "!! active workspace",
            "error unknown-workspace input=missing",
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
fn doctor_distinguishes_enabled_sync_from_runtime_disable_override() {
    let env = TestEnv::new();
    let db = env.db("runtime-disabled-sync.sqlite");
    env.write_config("sync:\n  enabled: true\n  server_url: http://127.0.0.1:3000\n");
    let output = command_with_db(&db)
        .env("XDG_STATE_HOME", env.state_dir())
        .env("AVEN_CONFIG_DIR", env.config_dir().join("aven"))
        .env("AVEN_SYNC_DISABLED", "1")
        .env_remove("AVEN_DEV_DB")
        .env_remove("AVEN_DB")
        .env_remove("AVEN_SYNC_SERVER")
        .arg("doctor")
        .output()
        .expect("run doctor with sync disabled");

    let output = ok(output);
    contains_all(
        &output,
        &["enabled            yes", "runtime allowed    no"],
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
        .env_remove("AVEN_DEV_DB")
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
    initialize_db(&env, &db);
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
fn doctor_reports_sync_history_stats() {
    let env = TestEnv::new();
    let db = env.db("sync-history-doctor.sqlite");
    initialize_db(&env, &db);
    run_sql(
        &db,
        "INSERT INTO changes(change_id, client_id, local_seq, entity_type, entity_id, field, op_type, payload, base_version, created_at, server_seq)
         VALUES
         ('change-pending-1', 'client', 1, 'task', 'task-1', 'title', 'update_task', 'abc', NULL, '2026-01-01T00:00:00Z', NULL),
         ('change-synced-1', 'client', 2, 'task', 'task-1', 'title', 'update_task', 'abcdef', NULL, '2026-01-01T00:00:01Z', 42),
         ('change-synced-2', 'client', 3, 'task', 'task-2', 'title', 'update_task', 'é', NULL, '2026-01-01T00:00:02Z', 44)",
    );

    let output = ok(env.aven(&db, ["doctor"]));

    contains_all(
        &output,
        &[
            "change rows        3",
            "pending changes    1",
            "synced changes     2",
            "min server_seq     42",
            "max server_seq     44",
            "payload bytes      11",
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
fn doctor_with_integrity_reports_invalid_due_dates() {
    let env = TestEnv::new();
    let db = env.db("integrity-due-doctor.sqlite");

    ok(env.aven(&db, ["add", "invalid deadline", "--project", "app"]));
    run_sql(&db, "UPDATE tasks SET due_on = '2026-99-99'");

    let output = ok(env.aven(&db, ["doctor", "--integrity"]));
    contains_all(&output, &["Integrity", "!! task due dates", "!! result"]);
}

#[test]
fn doctor_with_integrity_reports_attachment_sidecar_issues() {
    let env = TestEnv::new();
    let db = env.db("integrity-attachment-doctor.sqlite");
    let task_ref = extract_ref(&ok(
        env.aven(&db, ["add", "attachment check", "--project", "app"])
    ));
    let image = env.path("photo.png");
    fs::write(&image, png_bytes(3, 2)).unwrap();
    ok(env.aven(
        &db,
        ["attachment", "add", &task_ref, image.to_str().unwrap()],
    ));
    let sha = query_string(&db, "SELECT sha256 FROM blob_inventory LIMIT 1");
    let blob_dir = default_blob_dir(&db);
    fs::remove_file(blob_dir.join("objects").join("sha256").join(&sha)).unwrap();
    fs::write(
        blob_dir
            .join("objects")
            .join("sha256")
            .join("orphan-object"),
        b"orphan",
    )
    .unwrap();

    let output = ok(env.aven(&db, ["doctor", "--integrity"]));
    contains_all(
        &output,
        &[
            "Integrity",
            "!! attachment objects",
            "!! attachment orphan objects",
            "!! result",
        ],
    );
}

#[test]
fn doctor_with_integrity_decodes_unattached_available_objects() {
    let env = TestEnv::new();
    let db = env.db("integrity-unattached-attachment.sqlite");
    initialize_db(&env, &db);
    let sha = "a".repeat(64);
    run_sql(
        &db,
        "INSERT INTO blob_inventory(sha256, byte_size, media_type, available, first_seen_at)
         VALUES ('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 7, 'image/png', 1, '2026-07-18T00:00:00Z')",
    );
    let path = default_blob_dir(&db)
        .join("objects")
        .join("sha256")
        .join(sha);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, b"corrupt").unwrap();

    let output = ok(env.aven(&db, ["doctor", "--integrity"]));
    contains_all(&output, &["!! attachment object hashes", "1 mismatched"]);
    contains_none(&output, &["aaaaaaaaaaaaaaaa"]);
}

#[test]
fn doctor_distinguishes_repairable_recurrence_gap_from_identity_corruption() {
    let env = TestEnv::new();
    let db = env.db("integrity-recurrence-doctor.sqlite");
    let today = Utc::now().date_naive().to_string();
    ok(env.aven(
        &db,
        [
            "add",
            "doctor recurrence",
            "--repeat",
            "daily",
            "--time-zone",
            "UTC",
            "--repeat-start-on",
            &today,
        ],
    ));
    run_sql(
        &db,
        "UPDATE recurrence_occurrences SET projection_state = 'archived', archived_at = '2099-01-01T00:00:00Z'",
    );

    let repairable = ok(env.aven(&db, ["doctor", "--integrity"]));
    contains_all(
        &repairable,
        &[
            "!! recurrence projection gaps",
            "repair by running `aven recur list`",
        ],
    );
    contains_none(&repairable, &["!! result"]);

    run_sql(&db, "UPDATE tasks SET created_at = '2000-01-01T00:00:00Z'");
    let corrupt = ok(env.aven(&db, ["doctor", "--integrity"]));
    contains_all(
        &corrupt,
        &[
            "!! recurrence deterministic timestamps",
            "restore a known-good backup",
            "!! result",
        ],
    );
}

#[test]
fn doctor_json_reports_default_database_health() {
    let env = TestEnv::new();
    let db = env.db("doctor-json.sqlite");
    initialize_db(&env, &db);

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
fn doctor_reports_malformed_yaml_without_exposing_config_values() {
    let env = TestEnv::new();
    let db = env.db("malformed-config.sqlite");
    initialize_db(&env, &db);
    let secret = "doctor-secret-token";
    env.write_config(&format!(
        "sync:\n  auth_token: \"{secret}\nlocal:\n  db_path: [\n"
    ));

    let human = ok(env.aven(&db, ["doctor"]));
    contains_all(
        &human,
        &[
            "!! config file",
            "configuration is invalid at line",
            "Database",
            "Workspace",
            "Daemon",
        ],
    );
    contains_none(&human, &[secret]);

    let json = ok(env.aven(&db, ["doctor", "--json"]));
    serde_json::from_str::<serde_json::Value>(&json).expect("doctor JSON report");
    contains_none(&json, &[secret]);
}

#[test]
fn doctor_uses_loose_database_path_when_unrelated_config_is_invalid() {
    let env = TestEnv::new();
    let db = env.db("invalid-unrelated-config.sqlite");
    initialize_db(&env, &db);
    env.write_config(&format!(
        "local:\n  db_path: '{}'\n  image_optimization: invalid-secret-value\n",
        db.display()
    ));

    let output = ok(env.aven_config(["doctor"]));
    contains_all(
        &output,
        &[
            "!! config file",
            "database source    config local.db_path",
            "ok sqlite",
            "active workspace",
        ],
    );
    contains_none(&output, &["invalid-secret-value"]);
}

#[test]
fn doctor_missing_database_inspection_creates_nothing() {
    let env = TestEnv::new();
    let db = env.path("absent/parent/db.sqlite");

    let output = ok(env.aven(&db, ["doctor"]));

    contains_all(
        &output,
        &[
            "!! exists",
            "database file is missing",
            "skipped: database file does not exist",
            "Sync",
            "Daemon",
        ],
    );
    assert!(!db.exists());
    assert!(!db.parent().unwrap().exists());
}

#[test]
fn doctor_reports_old_schema_without_migrating_or_mutating_it() {
    let env = TestEnv::new();
    let db = env.db("old-schema.sqlite");
    initialize_db(&env, &db);
    let previous = query_i64(
        &db,
        "SELECT MAX(version) FROM _sqlx_migrations WHERE version < (SELECT MAX(version) FROM _sqlx_migrations)",
    );
    run_sql(
        &db,
        "DELETE FROM _sqlx_migrations WHERE version = (SELECT MAX(version) FROM _sqlx_migrations)",
    );
    let output = ok(env.aven(&db, ["doctor"]));

    contains_all(
        &output,
        &[
            "!! migrations",
            "pending",
            "skipped: database schema is unavailable, pending migration, or unsupported",
            "Daemon",
        ],
    );
    assert_eq!(
        query_i64(&db, "SELECT MAX(version) FROM _sqlx_migrations"),
        previous
    );
}

#[test]
fn doctor_reports_failed_migration_without_retrying_it() {
    let env = TestEnv::new();
    let db = env.db("failed-migration.sqlite");
    initialize_db(&env, &db);
    let version = query_i64(&db, "SELECT MAX(version) FROM _sqlx_migrations");
    run_sql(
        &db,
        "UPDATE _sqlx_migrations SET success = 0 WHERE version = (SELECT MAX(version) FROM _sqlx_migrations)",
    );

    let output = ok(env.aven(&db, ["doctor"]));

    contains_all(
        &output,
        &[
            "!! migration state",
            &format!("migration {version} is marked failed"),
            "preserve the database",
            "Daemon",
        ],
    );
    assert_eq!(
        query_i64(
            &db,
            "SELECT success FROM _sqlx_migrations WHERE version = (SELECT MAX(version) FROM _sqlx_migrations)",
        ),
        0
    );
}

#[test]
fn doctor_reports_future_schema_as_unsupported() {
    let env = TestEnv::new();
    let db = env.db("future-schema.sqlite");
    initialize_db(&env, &db);
    run_sql(
        &db,
        "UPDATE _sqlx_migrations SET version = 999999999999999 WHERE version = (SELECT MAX(version) FROM _sqlx_migrations)",
    );

    let output = ok(env.aven(&db, ["doctor"]));

    contains_all(
        &output,
        &[
            "!! schema support",
            "newer than this Aven supports",
            "Workspace",
            "Sync",
            "Daemon",
        ],
    );
}

#[cfg(unix)]
#[test]
fn doctor_reports_unreadable_database_and_continues() {
    use std::os::unix::fs::PermissionsExt as _;

    let env = TestEnv::new();
    let db = env.db("unreadable.sqlite");
    initialize_db(&env, &db);
    let original_mode = fs::metadata(&db).unwrap().permissions().mode();
    fs::set_permissions(&db, fs::Permissions::from_mode(0o000)).unwrap();

    let output = ok(env.aven(&db, ["doctor"]));

    contains_all(&output, &["Database", "!! SQLite header", "Sync", "Daemon"]);
    fs::set_permissions(&db, fs::Permissions::from_mode(original_mode)).unwrap();
}

#[test]
fn doctor_open_failures_preserve_independent_sections_and_skipped_reasons() {
    let env = TestEnv::new();
    let db = env.path("database-directory");
    fs::create_dir(&db).unwrap();

    let output = ok(env.aven(&db, ["doctor", "--integrity"]));

    contains_all(
        &output,
        &[
            "!! file type",
            "Workspace",
            "skipped: database schema is unavailable, pending migration, or unsupported",
            "Sync",
            "Daemon",
            "Integrity",
        ],
    );
}

#[test]
fn doctor_does_not_mutate_healthy_database_or_config() {
    let env = TestEnv::new();
    let db = env.db("immutable-doctor.sqlite");
    initialize_db(&env, &db);
    env.write_config(&format!("local:\n  db_path: '{}'\n", db.display()));
    let database_before = fs::read(&db).unwrap();
    let config_before = fs::read(env.config_file()).unwrap();
    let wal_before = PathBuf::from(format!("{}-wal", db.display())).exists();
    let shm_before = PathBuf::from(format!("{}-shm", db.display())).exists();

    ok(env.aven_config(["doctor", "--integrity"]));

    assert_eq!(fs::read(&db).unwrap(), database_before);
    assert_eq!(fs::read(env.config_file()).unwrap(), config_before);
    assert_eq!(
        PathBuf::from(format!("{}-wal", db.display())).exists(),
        wal_before
    );
    assert_eq!(
        PathBuf::from(format!("{}-shm", db.display())).exists(),
        shm_before
    );
}

#[test]
fn doctor_json_has_stable_codes_statuses_and_skipped_reasons() {
    let env = TestEnv::new();
    let db = env.db("missing-json.sqlite");

    let output = ok(env.aven(&db, ["doctor", "--json"]));
    let report: serde_json::Value = serde_json::from_str(&output).unwrap();

    assert_eq!(report["overall_status"], "error");
    let checks = report["sections"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|section| section["rows"].as_array().unwrap())
        .collect::<Vec<_>>();
    assert!(checks.iter().any(|check| {
        check["code"] == "workspace.resolve_skipped"
            && check["status"] == "skipped"
            && check["skipped_reason"].as_str().is_some()
    }));
}

#[test]
fn doctor_fail_on_error_supports_automation() {
    let env = TestEnv::new();
    let db = env.db("missing-fail-status.sqlite");

    let output = env.aven(&db, ["doctor", "--json", "--fail-on-error"]);

    assert!(!output.status.success());
    serde_json::from_slice::<serde_json::Value>(&output.stdout).expect("JSON remains valid");
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

fn initialize_db(env: &TestEnv, db: &Path) {
    ok(env.aven(db, ["list"]));
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

fn query_i64(db: &Path, sql: &'static str) -> i64 {
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
            .expect("open db");
        sqlx::query_scalar::<_, i64>(sql)
            .fetch_one(&mut conn)
            .await
            .expect("read integer")
    })
}

fn query_string(db: &Path, sql: &'static str) -> String {
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
        sqlx::query_scalar::<_, String>(sql)
            .fetch_one(&mut conn)
            .await
            .expect("read string")
    })
}

fn default_blob_dir(db: &Path) -> PathBuf {
    let mut blob_dir = db.as_os_str().to_os_string();
    blob_dir.push(".blobs");
    PathBuf::from(blob_dir)
}
