mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use sqlx::ConnectOptions;
use sqlx::sqlite::SqliteConnectOptions;

use common::{
    TestEnv, contains_all, contains_none, extract_ref, fail, meta_value, ok, png_bytes, scalar_i64,
};

#[test]
fn backup_command_creates_archive() {
    let env = TestEnv::new();
    let db = env.db("backup-copy.sqlite");
    ok(env.aven(&db, ["label", "create", "safety"]));

    let _task_ref = extract_ref(&ok(env.aven(
        &db,
        ["add", "base task", "--project", "app", "--label", "safety"],
    )));

    let backup_path = env.path("backup-copy.aven-backup.tar.zst");
    let output = ok(env.aven(&db, ["backup", "--output", backup_path.to_str().unwrap()]));
    contains_all(&output, &["backup path=", "bytes="]);
    assert!(backup_path.exists());
    assert!(
        !fs::read(&backup_path)
            .unwrap()
            .starts_with(b"SQLite format 3")
    );
}

#[test]
fn backup_archive_round_trips_attachment_blobs() {
    let env = TestEnv::new();
    let db = env.db("backup-attachment.sqlite");
    let task_ref = extract_ref(&ok(env.aven(&db, ["add", "with image", "--project", "app"])));
    let image = env.path("photo.png");
    let image_bytes = png_bytes(3, 2);
    fs::write(&image, &image_bytes).unwrap();

    let add_output = ok(env.aven(
        &db,
        [
            "attachment",
            "add",
            &task_ref,
            image.to_str().unwrap(),
            "--alt",
            "sample image",
        ],
    ));
    contains_all(&add_output, &["attachment-added", "has_blob=true"]);
    let sha = query_string(&db, "SELECT sha256 FROM task_attachments LIMIT 1");

    let backup_path = env.path("backup.aven-backup.tar.zst");
    let output = ok(env.aven(&db, ["backup", "--output", backup_path.to_str().unwrap()]));
    contains_all(&output, &["backup path=", "bytes="]);

    fs::remove_file(&db).unwrap();
    let blob_dir = default_blob_dir(&db);
    fs::remove_dir_all(&blob_dir).unwrap();

    let output = ok(env.aven(
        &db,
        ["backup", "restore", backup_path.to_str().unwrap(), "--yes"],
    ));
    contains_all(&output, &["restored-backup path=", "safety_backup="]);
    let object_path = blob_dir.join("objects").join("sha256").join(&sha);
    assert_eq!(fs::read(object_path).unwrap(), image_bytes);
    contains_all(
        &ok(env.aven(&db, ["attachment", "list", &task_ref])),
        &["attachment", &sha],
    );
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
    sqlite_backup(&db, &source);
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
fn backup_restore_archive_replaces_database_and_keeps_safety_copy() {
    let env = TestEnv::new();
    let db = env.db("restore-with-archive.sqlite");
    ok(env.aven(&db, ["label", "create", "safety"]));

    ok(env.aven(
        &db,
        ["add", "kept task", "--project", "app", "--label", "safety"],
    ));
    let source = env.path("restore-source.aven-backup.tar.zst");
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
fn json_export_includes_attachment_metadata_without_bytes() {
    let env = TestEnv::new();
    let db = env.db("export-attachments.sqlite");
    let task_ref = extract_ref(&ok(env.aven(&db, ["add", "with image", "--project", "app"])));
    let image = env.path("photo.png");
    fs::write(&image, png_bytes(3, 2)).unwrap();
    ok(env.aven(
        &db,
        ["attachment", "add", &task_ref, image.to_str().unwrap()],
    ));

    let output_path = env.path("attachments-export.json");
    ok(env.aven(&db, ["export", "--output", output_path.to_str().unwrap()]));
    let text = fs::read_to_string(&output_path).unwrap();
    let snapshot: Value = serde_json::from_str(&text).unwrap();

    assert_eq!(snapshot["blobs_included"], false);
    assert_eq!(
        snapshot["tables"]["task_attachments"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        snapshot["tables"]["blob_inventory"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        !snapshot["tables"]["task_attachments"][0]
            .as_object()
            .unwrap()
            .contains_key("bytes")
    );
}

#[test]
fn import_preserves_attachment_metadata_without_local_blobs() {
    let env = TestEnv::new();
    let source_db = env.db("import-attachments-source.sqlite");
    let target_dir = env.path("target");
    fs::create_dir_all(&target_dir).unwrap();
    let target_db = target_dir.join("import-attachments-target.sqlite");
    let task_ref = extract_ref(&ok(
        env.aven(&source_db, ["add", "with image", "--project", "app"])
    ));
    let image = env.path("photo.png");
    fs::write(&image, png_bytes(3, 2)).unwrap();
    ok(env.aven(
        &source_db,
        ["attachment", "add", &task_ref, image.to_str().unwrap()],
    ));

    let output_path = env.path("attachments-import.json");
    ok(env.aven(
        &source_db,
        ["export", "--output", output_path.to_str().unwrap()],
    ));
    ok(env.aven(
        &target_db,
        ["import", "--yes", output_path.to_str().unwrap()],
    ));

    assert_eq!(
        scalar_i64(&target_db, "SELECT count(*) FROM task_attachments"),
        1
    );
    assert_eq!(
        scalar_i64(&target_db, "SELECT available FROM blob_inventory LIMIT 1"),
        0
    );
    let sha = query_string(&target_db, "SELECT sha256 FROM blob_inventory LIMIT 1");
    assert!(
        !default_blob_dir(&target_db)
            .join("objects")
            .join("sha256")
            .join(sha)
            .exists()
    );
}

#[test]
fn export_command_writes_portable_snapshot() {
    let env = TestEnv::new();
    let db = env.db("export.json.sqlite");
    seed_sample_data(&env, &db);
    ok(env.aven(
        &db,
        [
            "add",
            "exported deadline",
            "--project",
            "app",
            "--due",
            "2099-01-01",
        ],
    ));

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
    let deadline = tasks
        .iter()
        .find(|task| task["title"] == "exported deadline")
        .unwrap();
    assert_eq!(deadline["due_on"], "2099-01-01");
}

#[test]
fn import_defaults_due_on_for_older_snapshots() {
    let env = TestEnv::new();
    let db = env.db("import-old-due-snapshot.sqlite");
    let task_ref = extract_ref(&ok(
        env.aven(&db, ["add", "older snapshot task", "--project", "app"])
    ));
    let output_path = env.path("import-old-due-snapshot.json");
    ok(env.aven(&db, ["export", "--output", output_path.to_str().unwrap()]));

    let mut snapshot: Value =
        serde_json::from_str(&fs::read_to_string(&output_path).unwrap()).unwrap();
    for task in snapshot["tables"]["tasks"].as_array_mut().unwrap() {
        task.as_object_mut().unwrap().remove("due_on");
    }
    fs::write(&output_path, serde_json::to_string(&snapshot).unwrap()).unwrap();

    ok(env.aven(&db, ["import", "--yes", output_path.to_str().unwrap()]));
    let shown: Value =
        serde_json::from_str(&ok(env.aven(&db, ["show", &task_ref, "--json"]))).unwrap();
    assert_eq!(shown["due_on"], "");
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
    snapshot["tables"]["tasks"][0]["project_id"] = Value::String("0000000000000000".to_string());
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

#[test]
fn import_rejects_invalid_project_ids() {
    let env = TestEnv::new();
    let db = env.db("invalid-import-project-id.sqlite");
    seed_sample_data(&env, &db);
    let export_path = env.path("invalid-import-project-id.json");
    ok(env.aven(&db, ["export", "--output", export_path.to_str().unwrap()]));

    let mut snapshot: Value =
        serde_json::from_str(&fs::read_to_string(&export_path).unwrap()).unwrap();
    snapshot["tables"]["tasks"][0]["project_id"] = Value::String("missing-project".to_string());
    fs::write(&export_path, serde_json::to_string(&snapshot).unwrap()).unwrap();

    let output = fail(env.aven(&db, ["import", "--yes", export_path.to_str().unwrap()]));
    contains_all(
        &output,
        &[
            "could not parse",
            "project ID must be 16 Crockford Base32 characters",
        ],
    );
}

#[test]
fn import_rejects_invalid_task_ids() {
    let env = TestEnv::new();
    let db = env.db("invalid-import-task-id.sqlite");
    seed_sample_data(&env, &db);
    let export_path = env.path("invalid-import-task-id.json");
    ok(env.aven(&db, ["export", "--output", export_path.to_str().unwrap()]));

    let mut snapshot: Value =
        serde_json::from_str(&fs::read_to_string(&export_path).unwrap()).unwrap();
    snapshot["tables"]["tasks"][0]["id"] = Value::String("invalid".to_string());
    fs::write(&export_path, serde_json::to_string(&snapshot).unwrap()).unwrap();

    let output = fail(env.aven(&db, ["import", "--yes", export_path.to_str().unwrap()]));
    contains_all(
        &output,
        &[
            "could not parse",
            "task ID must be 16 Crockford Base32 characters",
        ],
    );
}

#[test]
fn import_rejects_invalid_project_record_ids() {
    let env = TestEnv::new();
    let db = env.db("invalid-import-project-record-ids.sqlite");
    seed_sample_data(&env, &db);
    let export_path = env.path("invalid-import-project-record-ids.json");
    ok(env.aven(&db, ["export", "--output", export_path.to_str().unwrap()]));

    let original: Value = serde_json::from_str(&fs::read_to_string(&export_path).unwrap()).unwrap();
    let workspace_id = original["tables"]["projects"][0]["workspace_id"]
        .as_str()
        .unwrap()
        .to_string();
    let project_id = original["tables"]["projects"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let mut snapshots = Vec::new();
    let mut project = original.clone();
    project["tables"]["projects"][0]["id"] = Value::String("invalid".to_string());
    snapshots.push(project);

    let mut path = original.clone();
    path["tables"]["project_paths"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "workspace_id": workspace_id,
            "project_id": "invalid",
            "path": "/tmp/invalid-project-id",
        }));
    snapshots.push(path);

    let mut alias = original;
    alias["tables"]["project_id_aliases"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "workspace_id": workspace_id,
            "remote_project_id": "invalid",
            "local_project_id": project_id,
        }));
    snapshots.push(alias);

    for snapshot in snapshots {
        fs::write(&export_path, serde_json::to_string(&snapshot).unwrap()).unwrap();
        let output = fail(env.aven(&db, ["import", "--yes", export_path.to_str().unwrap()]));
        contains_all(
            &output,
            &[
                "could not parse",
                "project ID must be 16 Crockford Base32 characters",
            ],
        );
    }
}

#[test]
fn import_rejects_invalid_due_on_values() {
    let env = TestEnv::new();
    let db = env.db("invalid-import-due.sqlite");
    seed_sample_data(&env, &db);
    let export_path = env.path("invalid-import-due.json");
    ok(env.aven(&db, ["export", "--output", export_path.to_str().unwrap()]));

    let mut snapshot: Value =
        serde_json::from_str(&fs::read_to_string(&export_path).unwrap()).unwrap();
    snapshot["tables"]["tasks"][0]["due_on"] = Value::String("tomorrowish".to_string());
    fs::write(&export_path, serde_json::to_string(&snapshot).unwrap()).unwrap();

    let output = fail(env.aven(&db, ["import", "--yes", export_path.to_str().unwrap()]));
    contains_all(
        &output,
        &["error invalid-export-snapshot", "task.due_on=tomorrowish"],
    );
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

fn sqlite_backup(source: &Path, target: &Path) {
    let backup_sql = format!(".backup '{}'", target.display());
    let output = Command::new("sqlite3")
        .arg(source)
        .arg(backup_sql)
        .output()
        .expect("run sqlite3");
    assert!(
        output.status.success(),
        "sqlite backup failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
