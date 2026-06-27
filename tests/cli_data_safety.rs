mod common;

use std::fs;
use std::path::Path;

use serde_json::Value;
use sqlx::ConnectOptions;
use sqlx::sqlite::SqliteConnectOptions;

use common::{TestEnv, contains_all, contains_none, extract_ref, fail, meta_value, ok, scalar_i64};

#[test]
fn backup_command_creates_sqlite_copy() {
    let env = TestEnv::new();
    let db = env.db("backup-copy.sqlite");
    ok(env.aven(&db, ["label", "create", "safety"]));

    let task_ref = extract_ref(&ok(env.aven(
        &db,
        ["add", "base task", "--project", "app", "--label", "safety"],
    )));

    let backup_path = env.path("backup-copy.sqlite.backup");
    let output = ok(env.aven(&db, ["backup", "--output", backup_path.to_str().unwrap()]));
    contains_all(&output, &["backup path=", "bytes="]);
    assert!(backup_path.exists());

    let listed = ok(env.aven(&backup_path, ["list"]));
    contains_all(&listed, &[&task_ref, "base task"]);
}

#[test]
fn backup_restore_rejects_without_confirmation() {
    let env = TestEnv::new();
    let db = env.db("restore-requires-yes.sqlite");
    ok(env.aven(&db, ["add", "to keep", "--project", "app"]));

    let source = env.path("source-for-restore.sqlite");
    ok(env.aven(&db, ["backup", "--output", source.to_str().unwrap()]));
    ok(env.aven(&db, ["add", "extra local task", "--project", "app"]));

    let output = fail(env.aven(&db, ["backup", "restore", source.to_str().unwrap()]));
    contains_all(&output, &["error backup-restore-requires-confirmation"]);
}

#[test]
fn backup_restore_replaces_database_and_keeps_safety_copy() {
    let env = TestEnv::new();
    let db = env.db("restore-with-backup.sqlite");
    ok(env.aven(&db, ["label", "create", "safety"]));

    ok(env.aven(
        &db,
        ["add", "kept task", "--project", "app", "--label", "safety"],
    ));
    let source = env.path("restore-source.sqlite");
    ok(env.aven(&db, ["backup", "--output", source.to_str().unwrap()]));
    ok(env.aven(&db, ["add", "local-only task", "--project", "app"]));

    let before = backup_count(&db, "before-restore");
    let output = ok(env.aven(
        &db,
        ["backup", "restore", source.to_str().unwrap(), "--yes"],
    ));
    contains_all(&output, &["restored-backup path=", "safety_backup="]);

    let list = ok(env.aven(&db, ["list", "--all"]));
    contains_all(&list, &["kept task"]);
    contains_none(&list, &["local-only task"]);

    let after = backup_count(&db, "before-restore");
    assert!(after > before);
}

#[test]
fn export_command_writes_portable_snapshot() {
    let env = TestEnv::new();
    let db = env.db("export.json.sqlite");
    seed_sample_data(&env, &db);

    let output_path = env.path("export.json");
    let output = ok(env.aven(&db, ["export", "--output", output_path.to_str().unwrap()]));
    contains_all(&output, &["exported path=", "workspaces=", "tasks="]);

    let text = fs::read_to_string(&output_path).unwrap();
    let snapshot: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(snapshot["format"], "aven-export");
    assert_eq!(snapshot["version"], 1);

    let tables = snapshot["tables"].as_object().unwrap();
    assert!(tables.contains_key("workspaces"));
    assert!(tables.contains_key("projects"));
    assert!(tables.contains_key("project_paths"));
    assert!(tables.contains_key("project_id_aliases"));
    assert!(tables.contains_key("labels"));
    assert!(tables.contains_key("tasks"));
    assert!(tables.contains_key("notes"));
    assert!(tables.contains_key("changes"));
    assert!(tables.contains_key("conflicts"));
    assert!(!tables.contains_key("tui_undo_entries"));

    let tasks = tables["tasks"].as_array().unwrap();
    assert!(!tasks.is_empty());
}

#[test]
fn import_command_rejects_without_confirmation() {
    let env = TestEnv::new();
    let db = env.db("import-requires-yes.sqlite");
    ok(env.aven(&db, ["label", "create", "alpha"]));

    seed_sample_data(&env, &db);
    let output_path = env.path("import-requires-yes.json");
    ok(env.aven(&db, ["export", "--output", output_path.to_str().unwrap()]));

    let output = fail(env.aven(&db, ["import", output_path.to_str().unwrap()]));
    contains_all(&output, &["error import-requires-confirmation"]);
}

#[test]
fn import_replaces_database_and_preserves_identity_meta() {
    let env = TestEnv::new();
    let db = env.db("import-success.sqlite");
    ok(env.aven(&db, ["label", "create", "alpha"]));

    seed_sample_data(&env, &db);
    let task_count = scalar_i64(&db, "SELECT count(*) FROM tasks");
    let source_local_seq = scalar_i64(&db, "SELECT COALESCE(MAX(local_seq), 0) FROM changes");
    set_meta(&db, "client_id", "target-client-id");
    set_meta(&db, "sync_cursor", "999");

    let output_path = env.path("import-success.json");
    set_meta(&db, "sync_server_url", "https://export-server");
    let export_output = ok(env.aven(&db, ["export", "--output", output_path.to_str().unwrap()]));
    contains_all(&export_output, &["exported path="]);

    ok(env.aven(&db, ["add", "temporary-local", "--project", "app"]));
    set_meta(&db, "sync_server_url", "https://target-server");

    let before = backup_count(&db, "before-import");
    let output = ok(env.aven(&db, ["import", "--yes", output_path.to_str().unwrap()]));
    contains_all(&output, &["imported path=", "safety_backup="]);

    assert_eq!(scalar_i64(&db, "SELECT count(*) FROM tasks"), task_count);
    assert_eq!(
        meta_value(&db, "client_id"),
        Some("target-client-id".to_string())
    );
    assert_eq!(meta_value(&db, "sync_cursor"), Some("0".to_string()));
    assert_eq!(
        meta_value(&db, "local_seq"),
        Some(source_local_seq.to_string())
    );
    assert_eq!(meta_value(&db, "sync_server_url"), None);

    let after = backup_count(&db, "before-import");
    assert!(after > before);
}

#[test]
fn import_rejects_invalid_snapshot_without_replacing_existing_data() {
    let env = TestEnv::new();
    let source_db = env.db("invalid-import-source.sqlite");
    let target_db = env.db("invalid-import-target.sqlite");
    seed_sample_data(&env, &source_db);
    ok(env.aven(&target_db, ["add", "target stays", "--project", "app"]));

    let export_path = env.path("invalid-import.json");
    ok(env.aven(
        &source_db,
        ["export", "--output", export_path.to_str().unwrap()],
    ));
    let mut snapshot: Value =
        serde_json::from_str(&fs::read_to_string(&export_path).unwrap()).unwrap();
    snapshot["tables"]["tasks"][0]["project_id"] = Value::String("missing-project".to_string());
    fs::write(&export_path, serde_json::to_string(&snapshot).unwrap()).unwrap();

    let output = fail(env.aven(
        &target_db,
        ["import", "--yes", export_path.to_str().unwrap()],
    ));
    contains_all(&output, &["error invalid-export-snapshot"]);

    let list = ok(env.aven(&target_db, ["list", "--all"]));
    contains_all(&list, &["target stays"]);
    contains_none(&list, &["seed alpha", "seed beta"]);
}

fn seed_sample_data(env: &TestEnv, db: &Path) {
    ok(env.aven(db, ["label", "create", "alpha"]));
    let first = extract_ref(&ok(env.aven(
        db,
        ["add", "seed alpha", "--project", "app", "--label", "alpha"],
    )));
    let second = extract_ref(&ok(env.aven(
        db,
        ["add", "seed beta", "--project", "app", "--label", "alpha"],
    )));
    ok(env.aven(db, ["dep", "add", &second, &first]));
    ok(env.aven_stdin(db, ["note", &first, "--stdin"], "seed note\n"));
}

fn set_meta(db: &Path, key: &str, value: &str) {
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
        sqlx::query("INSERT OR REPLACE INTO meta(key, value) VALUES (?, ?)")
            .bind(key)
            .bind(value)
            .execute(&mut conn)
            .await
            .expect("set meta");
    });
}

fn backup_count(db: &Path, reason: &str) -> usize {
    let Some(parent) = db.parent() else {
        return 0;
    };
    let Some(stem) = db.file_name().and_then(|name| name.to_str()) else {
        return 0;
    };
    let prefix = format!("{}.{}-", stem, reason);
    let backup_dir = parent.join("backups");
    let Ok(entries) = fs::read_dir(&backup_dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str().map(|name| name.to_string()))
        .filter(|name| name.starts_with(&prefix) && name.ends_with(".sqlite"))
        .count()
}
