mod common;

use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde_json::Value;
use sqlx::ConnectOptions;
use sqlx::sqlite::SqliteConnectOptions;

use common::{
    TestEnv, contains_all, contains_none, extract_ref, fail, meta_value, ok, png_bytes, scalar_i64,
};

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
    assert_eq!(
        query_string(&db, "SELECT source FROM tasks WHERE title = 'with image'"),
        "cli"
    );
    let object_path = blob_dir.join("objects").join("sha256").join(&sha);
    assert_eq!(fs::read(object_path).unwrap(), image_bytes);
    let listed = ok(env.aven(&db, ["attachment", "list", &task_ref, "--json"]));
    let listed: Value = serde_json::from_str(&listed).unwrap();
    assert_eq!(listed[0]["sha256"], sha);
}

#[test]
fn backup_restore_replaces_attachment_objects_and_repairs_bytes() {
    let env = TestEnv::new();
    let db = env.db("backup-replace-objects.sqlite");
    let first_ref = extract_ref(&ok(
        env.aven(&db, ["add", "first image", "--project", "app"])
    ));
    let first_image = env.path("first.png");
    let first_bytes = png_bytes(3, 2);
    fs::write(&first_image, &first_bytes).unwrap();
    ok(env.aven(
        &db,
        [
            "attachment",
            "add",
            &first_ref,
            first_image.to_str().unwrap(),
        ],
    ));
    let first_sha = query_string(&db, "SELECT sha256 FROM task_attachments LIMIT 1");
    let backup_path = env.path("replace-objects.aven-backup.tar.zst");
    ok(env.aven(&db, ["backup", "--output", backup_path.to_str().unwrap()]));

    let second_ref = extract_ref(&ok(
        env.aven(&db, ["add", "second image", "--project", "app"])
    ));
    let second_image = env.path("second.png");
    fs::write(&second_image, png_bytes(4, 3)).unwrap();
    ok(env.aven(
        &db,
        [
            "attachment",
            "add",
            &second_ref,
            second_image.to_str().unwrap(),
        ],
    ));
    let second_sha = query_strings(&db, "SELECT sha256 FROM task_attachments")
        .into_iter()
        .find(|sha| sha != &first_sha)
        .unwrap();
    let objects = default_blob_dir(&db).join("objects").join("sha256");
    fs::write(objects.join(&first_sha), b"corrupt").unwrap();

    ok(env.aven(
        &db,
        ["backup", "restore", backup_path.to_str().unwrap(), "--yes"],
    ));
    assert_eq!(fs::read(objects.join(&first_sha)).unwrap(), first_bytes);
    assert!(!objects.join(second_sha).exists());
    contains_all(
        &ok(env.aven(&db, ["doctor", "--integrity"])),
        &[
            "ok attachment objects 0 missing",
            "ok attachment object hashes 0 mismatched",
            "ok attachment orphan objects 0 orphaned",
        ],
    );
}

#[test]
fn backup_archive_excludes_disposable_and_incomplete_attachment_files() {
    let env = TestEnv::new();
    let db = env.db("backup-exclusions.sqlite");
    ok(env.aven(&db, ["add", "backup exclusions", "--project", "app"]));
    let blob_dir = default_blob_dir(&db);
    for relative in [
        "cache/previews/profile/preview.png",
        "trash/trashed-object",
        "objects/sha256/.aven-stage-incomplete",
        "staging/incomplete",
    ] {
        let path = blob_dir.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"private disposable bytes").unwrap();
    }

    let backup_path = env.path("exclusions.aven-backup.tar.zst");
    ok(env.aven(&db, ["backup", "--output", backup_path.to_str().unwrap()]));
    let entries = backup_entries(&backup_path);
    assert_eq!(entries, vec!["manifest.json", "database.sqlite"]);
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
    assert_eq!(
        scalar_i64(
            &target_db,
            "SELECT count(*) FROM changes WHERE server_seq IS NULL AND field = 'attachments'",
        ),
        0
    );
    assert_eq!(
        scalar_i64(
            &target_db,
            "SELECT count(*) FROM task_attachments WHERE created_by_change_id IS NOT NULL OR deleted_by_change_id IS NOT NULL",
        ),
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
fn import_rejects_attachment_inventory_metadata_mismatch() {
    let env = TestEnv::new();
    let db = env.db("import-attachment-mismatch.sqlite");
    let task_ref = extract_ref(&ok(
        env.aven(&db, ["add", "attachment mismatch", "--project", "app"])
    ));
    let image = env.path("mismatch.png");
    fs::write(&image, png_bytes(3, 2)).unwrap();
    ok(env.aven(
        &db,
        ["attachment", "add", &task_ref, image.to_str().unwrap()],
    ));
    let export = env.path("attachment-mismatch.json");
    ok(env.aven(&db, ["export", "--output", export.to_str().unwrap()]));
    let mut snapshot: Value = serde_json::from_str(&fs::read_to_string(&export).unwrap()).unwrap();
    let size = snapshot["tables"]["blob_inventory"][0]["byte_size"]
        .as_i64()
        .unwrap();
    snapshot["tables"]["blob_inventory"][0]["byte_size"] = Value::from(size + 1);
    fs::write(&export, serde_json::to_string(&snapshot).unwrap()).unwrap();

    let output = fail(env.aven(&db, ["import", "--yes", export.to_str().unwrap()]));
    contains_all(
        &output,
        &["error invalid-export-snapshot attachment inventory metadata mismatch"],
    );
}

#[test]
fn recurrence_export_import_preserves_interval_and_yearly_rules() {
    let env = TestEnv::new();
    let source = env.db("recurrence-interval-source.sqlite");
    let target = env.db("recurrence-interval-target.sqlite");
    let today = Utc::now().date_naive().to_string();
    for (title, rule) in [
        ("multi day", "every 3 days"),
        ("multi month", "every 3 months"),
        ("yearly", "yearly"),
        ("multi year", "every 2 years"),
    ] {
        ok(env.aven(
            &source,
            [
                "add",
                title,
                "--project",
                "app",
                "--repeat",
                rule,
                "--time-zone",
                "UTC",
                "--repeat-start-on",
                &today,
            ],
        ));
    }
    let export = env.path("recurrence-intervals.json");
    ok(env.aven(&source, ["export", "--output", export.to_str().unwrap()]));
    ok(env.aven(&target, ["import", export.to_str().unwrap(), "--yes"]));
    assert_eq!(
        scalar_i64(&target, "SELECT count(*) FROM recurrence_series"),
        4
    );
    assert_eq!(
        scalar_i64(
            &target,
            "SELECT count(*) FROM recurrence_series WHERE frequency = 'daily' AND interval = 3"
        ),
        1
    );
    assert_eq!(
        scalar_i64(
            &target,
            "SELECT count(*) FROM recurrence_series WHERE frequency = 'monthly' AND interval = 3"
        ),
        1
    );
    assert_eq!(
        scalar_i64(
            &target,
            "SELECT count(*) FROM recurrence_series WHERE frequency = 'yearly' AND interval = 1"
        ),
        1
    );
    assert_eq!(
        scalar_i64(
            &target,
            "SELECT count(*) FROM recurrence_series WHERE frequency = 'yearly' AND interval = 2"
        ),
        1
    );
}

#[test]
fn stopped_series_with_future_current_occurrence_round_trips() {
    let env = TestEnv::new();
    let source = env.db("recurrence-stopped-future-source.sqlite");
    let target = env.db("recurrence-stopped-future-target.sqlite");
    let start = (Utc::now().date_naive() + chrono::Days::new(1)).to_string();
    let created = ok(env.aven(
        &source,
        [
            "add",
            "future yearly",
            "--project",
            "app",
            "--repeat",
            "yearly",
            "--time-zone",
            "UTC",
            "--repeat-start-on",
            &start,
        ],
    ));
    let series_ref = created.split_whitespace().nth(1).unwrap();
    ok(env.aven(&source, ["recur", "stop", series_ref]));

    let export = env.path("recurrence-stopped-future.json");
    ok(env.aven(&source, ["export", "--output", export.to_str().unwrap()]));
    ok(env.aven(&target, ["import", export.to_str().unwrap(), "--yes"]));
    contains_all(
        &ok(env.aven(&source, ["doctor", "--integrity"])),
        &["recurrence stop boundaries", "0 invalid"],
    );
    contains_all(
        &ok(env.aven(&target, ["doctor", "--integrity"])),
        &["recurrence stop boundaries", "0 invalid"],
    );
    for database in [&source, &target] {
        assert_eq!(
            scalar_i64(
                database,
                "SELECT count(*) FROM recurrence_series WHERE state = 'stopped' AND stopped_at <> ''"
            ),
            1
        );
        assert_eq!(
            scalar_i64(
                database,
                "SELECT count(*) FROM recurrence_occurrences WHERE projection_state = 'projected'"
            ),
            1
        );
    }
}

#[test]
fn recurrence_export_import_round_trips_aggregate_and_occurrence_local_data() {
    let env = TestEnv::new();
    let source_db = env.db("recurrence-export-source.sqlite");
    let target_db = env.db("recurrence-export-target.sqlite");
    ok(env.aven(&source_db, ["label", "create", "ritual"]));
    let (series_ref, occurrence_ref) =
        add_daily_series(&env, &source_db, "daily journal", &["--label", "ritual"]);
    ok(env.aven_stdin(
        &source_db,
        ["note", &occurrence_ref, "--stdin"],
        "occurrence-local note\n",
    ));
    ok(env.aven(&source_db, ["edit", &occurrence_ref, "--status", "done"]));
    ok(env.aven(&source_db, ["recur", "pause", &series_ref]));
    execute_sql(
        &source_db,
        "INSERT INTO conflicts(workspace_id, entity_type, entity_id, task_id, field, local_value, remote_value, remote_change_id, variant_a, variant_b, created_at) SELECT workspace_id, 'recurrence_series', id, '', 'state', 'paused', 'active', '7KQ9A1X4MV2P8D6R', 'paused', 'active', '2026-07-20T00:00:00Z' FROM recurrence_series LIMIT 1",
    );

    let export_path = env.path("recurrence-round-trip.json");
    ok(env.aven(
        &source_db,
        ["export", "--output", export_path.to_str().unwrap()],
    ));
    let snapshot: Value = serde_json::from_str(&fs::read_to_string(&export_path).unwrap()).unwrap();
    assert_eq!(snapshot["version"], 2);
    assert_eq!(
        snapshot["tables"]["recurrence_series"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        snapshot["tables"]["recurrence_series_labels"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        snapshot["tables"]["recurrence_occurrences"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        snapshot["tables"]["recurrence_pause_intervals"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        snapshot["tables"]["field_versions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["entity_type"] == "recurrence_series")
    );
    assert!(
        snapshot["tables"]["conflicts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["entity_type"] == "recurrence_series")
    );

    ok(env.aven(
        &target_db,
        ["import", "--yes", export_path.to_str().unwrap()],
    ));
    contains_all(
        &ok(env.aven(&target_db, ["recur", "show", &series_ref])),
        &["state=paused", "daily journal"],
    );
    contains_all(
        &ok(env.aven(&target_db, ["show", &occurrence_ref, "--full"])),
        &["occurrence-local note", "ritual"],
    );
    assert_eq!(
        scalar_i64(&target_db, "SELECT count(*) FROM recurrence_series"),
        1
    );
    assert_eq!(
        scalar_i64(
            &target_db,
            "SELECT count(*) FROM recurrence_pause_intervals"
        ),
        1
    );
    assert_eq!(
        scalar_i64(
            &target_db,
            "SELECT count(*) FROM conflicts WHERE entity_type = 'recurrence_series'",
        ),
        1
    );
}

#[test]
fn older_task_only_export_imports_tasks_as_nonrecurring_and_ignores_schema_version() {
    let env = TestEnv::new();
    let source_db = env.db("portable-v1-source.sqlite");
    let target_db = env.db("portable-v1-target.sqlite");
    let task_ref = extract_ref(&ok(
        env.aven(&source_db, ["add", "legacy task", "--project", "app"])
    ));
    let export_path = env.path("portable-v1.json");
    ok(env.aven(
        &source_db,
        ["export", "--output", export_path.to_str().unwrap()],
    ));
    let mut snapshot: Value =
        serde_json::from_str(&fs::read_to_string(&export_path).unwrap()).unwrap();
    snapshot["schema_version"] = Value::from(1);
    for table in [
        "recurrence_series",
        "recurrence_series_labels",
        "recurrence_occurrences",
        "recurrence_pause_intervals",
    ] {
        snapshot["tables"].as_object_mut().unwrap().remove(table);
    }
    fs::write(&export_path, serde_json::to_string(&snapshot).unwrap()).unwrap();

    ok(env.aven(
        &target_db,
        ["import", "--yes", export_path.to_str().unwrap()],
    ));
    contains_all(
        &ok(env.aven(&target_db, ["show", &task_ref])),
        &["legacy task"],
    );
    assert_eq!(
        scalar_i64(&target_db, "SELECT count(*) FROM recurrence_series"),
        0
    );
    assert_eq!(
        scalar_i64(&target_db, "SELECT count(*) FROM recurrence_occurrences"),
        0
    );
}

#[test]
fn recurrence_import_rejects_malformed_zone_identity_lattice_and_outcome() {
    let env = TestEnv::new();
    let db = env.db("recurrence-invalid-import.sqlite");
    let (_series_ref, _occurrence_ref) = add_daily_series(&env, &db, "validated series", &[]);
    let ordinary_ref = extract_ref(&ok(
        env.aven(&db, ["add", "ordinary task", "--project", "app"])
    ));
    let ordinary_id = query_string(
        &db,
        "SELECT id FROM tasks WHERE title = 'ordinary task' LIMIT 1",
    );
    let export_path = env.path("recurrence-invalid.json");
    ok(env.aven(&db, ["export", "--output", export_path.to_str().unwrap()]));
    let original: Value = serde_json::from_str(&fs::read_to_string(&export_path).unwrap()).unwrap();

    let mut invalid = Vec::new();
    let mut zone = original.clone();
    zone["tables"]["recurrence_series"][0]["timezone"] = Value::from("Mars/Olympus");
    invalid.push((zone, "invalid IANA time zone"));

    let mut identity = original.clone();
    identity["tables"]["recurrence_occurrences"][0]["task_id"] = Value::from(ordinary_id);
    invalid.push((identity, "deterministic task identity mismatch"));

    let mut lattice = original.clone();
    lattice["tables"]["recurrence_series"][0]["start_on"] = Value::from("2099-01-01");
    invalid.push((lattice, "outside the series lattice"));

    let mut outcome = original.clone();
    outcome["tables"]["recurrence_occurrences"][0]["outcome"] = Value::from("completed");
    outcome["tables"]["recurrence_occurrences"][0]["resolved_at"] =
        Value::from("2026-07-20T12:00:00Z");
    outcome["tables"]["recurrence_occurrences"][0]["outcome_change_id"] =
        Value::from("7KQ9A1X4MV2P8D6R");
    outcome["tables"]["recurrence_occurrences"][0]["projection_state"] = Value::from("resolved");
    invalid.push((outcome, "completed outcome requires done task"));

    let mut pauses = original.clone();
    let workspace_id = pauses["tables"]["recurrence_series"][0]["workspace_id"]
        .as_str()
        .unwrap()
        .to_string();
    let series_id = pauses["tables"]["recurrence_series"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let series_change_id = pauses["tables"]["changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|change| change["entity_type"] == "recurrence_series")
        .unwrap()["change_id"]
        .as_str()
        .unwrap()
        .to_string();
    pauses["tables"]["recurrence_pause_intervals"] = serde_json::json!([
        {
            "workspace_id": workspace_id.clone(),
            "id": "pause-a",
            "series_id": series_id.clone(),
            "paused_at": "2090-01-01T00:00:00Z",
            "resumed_at": "2090-01-03T00:00:00Z",
            "suspended_slot_on": "",
            "suspended_task_id": "",
            "created_by_change_id": series_change_id.clone(),
            "resolved_by_change_id": series_change_id.clone(),
        },
        {
            "workspace_id": workspace_id,
            "id": "pause-b",
            "series_id": series_id,
            "paused_at": "2090-01-02T00:00:00Z",
            "resumed_at": "2090-01-04T00:00:00Z",
            "suspended_slot_on": "",
            "suspended_task_id": "",
            "created_by_change_id": series_change_id.clone(),
            "resolved_by_change_id": series_change_id.clone(),
        }
    ]);
    invalid.push((pauses, "pause intervals overlap"));

    let mut lifecycle = original;
    lifecycle["tables"]["recurrence_series"][0]["state"] = Value::from("paused");
    invalid.push((lifecycle, "state and open pause disagree"));

    for (snapshot, expected) in invalid {
        fs::write(&export_path, serde_json::to_string(&snapshot).unwrap()).unwrap();
        contains_all(
            &fail(env.aven(&db, ["import", "--yes", export_path.to_str().unwrap()])),
            &[expected],
        );
    }
    contains_all(
        &ok(env.aven(&db, ["show", &ordinary_ref])),
        &["ordinary task"],
    );
}

#[test]
fn backup_restore_preserves_recurrence_and_archived_occurrence_data() {
    let env = TestEnv::new();
    let db = env.db("recurrence-backup.sqlite");
    let (series_ref, occurrence_ref) = add_daily_series(&env, &db, "archived journal", &[]);
    ok(env.aven_stdin(
        &db,
        ["note", &occurrence_ref, "--stdin"],
        "preserved archive note\n",
    ));
    execute_sql(
        &db,
        "UPDATE recurrence_occurrences SET projection_state = 'archived', archived_at = '2099-01-01T00:00:00Z'; UPDATE recurrence_series SET state = 'stopped', stopped_at = '2099-01-01T00:00:00Z'",
    );
    let backup_path = env.path("recurrence.aven-backup.tar.zst");
    ok(env.aven(&db, ["backup", "--output", backup_path.to_str().unwrap()]));
    fs::remove_file(&db).unwrap();

    ok(env.aven(
        &db,
        ["backup", "restore", backup_path.to_str().unwrap(), "--yes"],
    ));
    contains_all(
        &ok(env.aven(&db, ["recur", "show", &series_ref])),
        &["archived journal"],
    );
    contains_all(
        &ok(env.aven(&db, ["show", &occurrence_ref, "--full"])),
        &["preserved archive note"],
    );
    assert_eq!(
        query_string(
            &db,
            "SELECT projection_state FROM recurrence_occurrences LIMIT 1"
        ),
        "archived"
    );
    assert_eq!(scalar_i64(&db, "SELECT count(*) FROM tasks"), 1);
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
            "--metadata",
            "legacy-id=42",
        ],
    ));

    let output_path = env.path("export.json");
    let output = ok(env.aven(&db, ["export", "--output", output_path.to_str().unwrap()]));
    contains_all(&output, &["exported path=", "workspaces=", "tasks="]);

    let text = fs::read_to_string(&output_path).unwrap();
    let snapshot: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(snapshot["format"], "aven-export");
    assert_eq!(snapshot["version"], 2);

    let tables = snapshot["tables"].as_object().unwrap();
    assert!(tables.contains_key("workspaces"));
    assert!(tables.contains_key("projects"));
    assert!(tables.contains_key("project_paths"));
    assert!(tables.contains_key("project_id_aliases"));
    assert!(tables.contains_key("labels"));
    assert!(tables.contains_key("metadata_fields"));
    assert!(tables.contains_key("task_metadata"));
    assert!(tables.contains_key("recurrence_series_metadata"));
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
    assert_eq!(tables["metadata_fields"].as_array().unwrap().len(), 1);
    assert_eq!(tables["metadata_fields"][0]["key"], "legacy-id");
    assert_eq!(tables["task_metadata"].as_array().unwrap().len(), 1);
    assert_eq!(tables["task_metadata"][0]["value"], "42");
}

#[test]
fn import_accepts_v1_snapshots_without_metadata_tables() {
    let env = TestEnv::new();
    let db = env.db("import-v1-metadata-defaults.sqlite");
    let task_ref = extract_ref(&ok(
        env.aven(&db, ["add", "v1 portable task", "--project", "app"])
    ));
    let output_path = env.path("import-v1-metadata-defaults.json");
    ok(env.aven(&db, ["export", "--output", output_path.to_str().unwrap()]));

    let mut snapshot: Value =
        serde_json::from_str(&fs::read_to_string(&output_path).unwrap()).unwrap();
    snapshot["version"] = Value::from(1);
    let tables = snapshot["tables"].as_object_mut().unwrap();
    for table in [
        "metadata_fields",
        "metadata_field_id_aliases",
        "task_metadata",
        "recurrence_series_metadata",
    ] {
        tables.remove(table);
    }
    fs::write(&output_path, serde_json::to_string(&snapshot).unwrap()).unwrap();

    ok(env.aven(&db, ["import", "--yes", output_path.to_str().unwrap()]));
    contains_all(
        &ok(env.aven(&db, ["show", &task_ref, "--full"])),
        &["v1 portable task"],
    );
    assert_eq!(scalar_i64(&db, "SELECT count(*) FROM metadata_fields"), 0);
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
fn export_and_import_preserve_and_validate_task_source() {
    let env = TestEnv::new();
    let db = env.db("task-source-portability.sqlite");
    ok(env.aven(&db, ["add", "portable source", "--project", "app"]));
    let output_path = env.path("task-source-portability.json");
    ok(env.aven(&db, ["export", "--output", output_path.to_str().unwrap()]));

    let mut snapshot: Value =
        serde_json::from_str(&fs::read_to_string(&output_path).unwrap()).unwrap();
    assert_eq!(snapshot["tables"]["tasks"][0]["source"], "cli");
    snapshot["tables"]["tasks"][0]
        .as_object_mut()
        .unwrap()
        .remove("source");
    fs::write(&output_path, serde_json::to_string(&snapshot).unwrap()).unwrap();

    ok(env.aven(&db, ["import", "--yes", output_path.to_str().unwrap()]));
    assert_eq!(
        query_string(
            &db,
            "SELECT source FROM tasks WHERE title = 'portable source'"
        ),
        "unknown"
    );

    fs::remove_file(&output_path).unwrap();
    ok(env.aven(&db, ["export", "--output", output_path.to_str().unwrap()]));
    let mut snapshot: Value =
        serde_json::from_str(&fs::read_to_string(&output_path).unwrap()).unwrap();
    snapshot["tables"]["tasks"][0]["source"] = Value::String("agent".to_string());
    fs::write(&output_path, serde_json::to_string(&snapshot).unwrap()).unwrap();

    let output = fail(env.aven(&db, ["import", "--yes", output_path.to_str().unwrap()]));
    contains_all(&output, &["invalid-task-source", "input=agent"]);
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
    ok(env.aven(
        &db,
        [
            "add",
            "portable metadata",
            "--project",
            "app",
            "--metadata",
            "legacy-id=42",
        ],
    ));
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
        query_string(
            &db,
            "SELECT m.value FROM task_metadata m JOIN metadata_fields f ON f.workspace_id = m.workspace_id AND f.id = m.field_id WHERE f.key = 'legacy-id'"
        ),
        "42"
    );
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

fn add_daily_series(
    env: &TestEnv,
    db: &Path,
    title: &str,
    extra_args: &[&str],
) -> (String, String) {
    let today = Utc::now().date_naive().to_string();
    let mut args = vec![
        "add".to_string(),
        title.to_string(),
        "--repeat".to_string(),
        "daily".to_string(),
        "--repeat-due".to_string(),
        "same-day".to_string(),
        "--time-zone".to_string(),
        "UTC".to_string(),
        "--repeat-start-on".to_string(),
        today,
    ];
    args.extend(extra_args.iter().map(|arg| (*arg).to_string()));
    let output = ok(env.aven(db, &args));
    let series_ref = output.split_whitespace().nth(1).unwrap().to_string();
    let occurrence_ref = output
        .split_whitespace()
        .find_map(|part| part.strip_prefix("occurrence="))
        .unwrap()
        .trim_matches('"')
        .to_string();
    (series_ref, occurrence_ref)
}

fn execute_sql(db: &Path, sql: &'static str) {
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
        sqlx::query(sql)
            .execute(&mut conn)
            .await
            .expect("execute test SQL");
    });
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

fn query_strings(db: &Path, sql: &'static str) -> Vec<String> {
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
            .fetch_all(&mut conn)
            .await
            .expect("read strings")
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

fn backup_entries(path: &Path) -> Vec<String> {
    let file = fs::File::open(path).unwrap();
    let decoder = zstd::stream::read::Decoder::new(file).unwrap();
    let mut archive = tar::Archive::new(decoder);
    archive
        .entries()
        .unwrap()
        .map(|entry| {
            entry
                .unwrap()
                .path()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

fn default_blob_dir(db: &Path) -> PathBuf {
    let mut blob_dir = db.as_os_str().to_os_string();
    blob_dir.push(".blobs");
    PathBuf::from(blob_dir)
}

fn sqlite_backup(source: &Path, target: &Path) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("create runtime");
    runtime.block_on(async {
        let mut conn = SqliteConnectOptions::new()
            .filename(source)
            .read_only(true)
            .foreign_keys(true)
            .connect()
            .await
            .expect("open backup source");
        sqlx::query("VACUUM INTO ?")
            .bind(target.display().to_string())
            .execute(&mut conn)
            .await
            .expect("back up sqlite database");
    });
}
