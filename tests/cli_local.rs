mod common;

use std::io::Write;
use std::path::PathBuf;

use common::{
    TestEnv, command, command_with_db, contains_all, contains_none, execute_sql, extract_ref, fail,
    ok, scalar_i64, suffix,
};

fn extract_attachment_id(output: &str) -> String {
    output
        .split_whitespace()
        .find(|word| word.starts_with("attachment_id="))
        .and_then(|word| word.strip_prefix("attachment_id="))
        .expect("attachment_id in output")
        .to_string()
}

fn extract_byte_size(output: &str) -> i64 {
    output
        .split_whitespace()
        .find(|word| word.starts_with("byte_size="))
        .and_then(|word| word.strip_prefix("byte_size="))
        .expect("byte_size in output")
        .parse()
        .unwrap()
}

fn compressible_png_bytes() -> Vec<u8> {
    let width = 16u32;
    let height = 16u32;
    let mut raw = Vec::new();
    for _ in 0..height {
        raw.push(0);
        raw.extend(std::iter::repeat_n(255, width as usize * 4));
    }
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(&raw).unwrap();
    let idat = encoder.finish().unwrap();

    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend(width.to_be_bytes());
    ihdr.extend(height.to_be_bytes());
    ihdr.extend([8, 6, 0, 0, 0]);
    append_png_chunk(&mut png, b"IHDR", &ihdr);
    append_png_chunk(&mut png, b"tEXt", &vec![b'a'; 4096]);
    append_png_chunk(&mut png, b"IDAT", &idat);
    append_png_chunk(&mut png, b"IEND", &[]);
    png
}

fn append_png_chunk(png: &mut Vec<u8>, name: &[u8; 4], data: &[u8]) {
    png.extend((data.len() as u32).to_be_bytes());
    png.extend(name);
    png.extend(data);
    let mut crc_data = Vec::with_capacity(name.len() + data.len());
    crc_data.extend(name);
    crc_data.extend(data);
    png.extend(crc32(&crc_data).to_be_bytes());
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

#[test]
fn generated_png_fixture_is_compressible() {
    let png = compressible_png_bytes();
    let mut options = oxipng::Options::from_preset(4);
    options.strip = oxipng::StripChunks::Safe;
    let optimized = oxipng::optimize_from_memory(&png, &options).unwrap();

    assert!(optimized.len() < png.len());
}

#[test]
fn attachment_add_list_get_and_delete_work_locally() {
    let env = TestEnv::new();
    let db = env.db("attachments.sqlite");
    let created = ok(env.aven(
        &db,
        [
            "add",
            "attach me",
            "--description",
            "before",
            "--project",
            "app",
        ],
    ));
    let task_ref = extract_ref(&created);
    let description_versions_before = scalar_i64(
        &db,
        "SELECT count(*) FROM field_versions WHERE field = 'description'",
    );
    let description_changes_before = scalar_i64(
        &db,
        "SELECT count(*) FROM changes WHERE field = 'description'",
    );
    let image = env.path("photo.png");
    let image_bytes = compressible_png_bytes();
    std::fs::write(&image, &image_bytes).unwrap();

    let added = ok(env.aven(
        &db,
        [
            "attachment",
            "add",
            &task_ref,
            image.to_str().unwrap(),
            "--alt",
            "diagram",
        ],
    ));
    contains_all(&added, &["attachment-added", "media_type=image/png"]);
    assert_eq!(extract_byte_size(&added), image_bytes.len() as i64);
    let attachment_id = extract_attachment_id(&added);
    contains_none(&added, &["sha256=", "photo.png", "diagram"]);
    let listed = ok(env.aven(&db, ["attachment", "list", &task_ref, "--json"]));
    let listed_json: serde_json::Value = serde_json::from_str(&listed).unwrap();
    let sha256 = listed_json[0]["sha256"].as_str().unwrap();
    let mut blob_root = db.as_os_str().to_os_string();
    blob_root.push(".blobs");
    let blob_root = PathBuf::from(blob_root);
    let blob_path = blob_root.join("objects").join("sha256").join(sha256);
    assert!(blob_path.exists(), "sidecar blob should exist");

    let full = ok(env.aven(&db, ["show", &task_ref, "--full"]));
    contains_all(
        &full,
        &[
            "description<<EOF\nbefore\nEOF\nAttachments:",
            &attachment_id,
            "attachment attachment_id=",
            "has_blob=yes",
        ],
    );
    contains_none(
        &full,
        &[
            "sha256=",
            "filename=",
            "alt_text=",
            "photo.png",
            "diagram",
            "png bytes",
        ],
    );

    let full_json = ok(env.aven(&db, ["show", &task_ref, "--full", "--json"]));
    let full_json: serde_json::Value = serde_json::from_str(&full_json).unwrap();
    assert_eq!(full_json["description"], "before");
    assert_eq!(full_json["attachments"][0]["attachment_id"], attachment_id);
    assert_eq!(full_json["attachments"][0]["has_blob"], true);
    assert!(full_json["attachments"][0].get("sha256").is_none());
    assert!(full_json["attachments"][0].get("bytes").is_none());
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT count(*) FROM field_versions WHERE field = 'description'",
        ),
        description_versions_before
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT count(*) FROM changes WHERE field = 'description'",
        ),
        description_changes_before
    );

    let listed = ok(env.aven(&db, ["attachment", "list", &task_ref, "--json"]));
    let value: serde_json::Value = serde_json::from_str(&listed).unwrap();
    assert_eq!(value[0]["attachment_id"], attachment_id);
    assert_eq!(value[0]["media_type"], "image/png");
    assert_eq!(value[0]["width"], 16);
    assert_eq!(value[0]["height"], 16);
    assert!(value[0].get("bytes").is_none());

    let output = env.path("copy.png");
    ok(env.aven(
        &db,
        [
            "attachment",
            "get",
            &attachment_id,
            "--output",
            output.to_str().unwrap(),
        ],
    ));
    assert_eq!(std::fs::read(output).unwrap(), image_bytes);

    ok(env.aven(&db, ["attachment", "delete", &attachment_id]));
    let hidden = ok(env.aven(&db, ["attachment", "list", &task_ref]));
    contains_none(&hidden, &[&attachment_id]);
    let all = ok(env.aven(&db, ["attachment", "list", &task_ref, "--all", "--json"]));
    let value: serde_json::Value = serde_json::from_str(&all).unwrap();
    assert_eq!(value[0]["deleted"], true);
    assert!(value[0].get("bytes").is_none());

    let show = ok(env.aven(&db, ["show", &task_ref, "--full"]));
    contains_all(&show, &["description<<EOF\nbefore\nEOF"]);
    contains_none(&show, &["Attachments:", &attachment_id]);

    for structured in [
        ok(env.aven(&db, ["show", &task_ref, "--full", "--json"])),
        ok(env.aven(&db, ["context", &task_ref, "--json"])),
    ] {
        let value: serde_json::Value = serde_json::from_str(&structured).unwrap();
        assert_eq!(value["attachments"][0]["attachment_id"], attachment_id);
        assert_eq!(value["attachments"][0]["deleted"], true);
        assert!(value["attachments"][0]["deleted_at"].is_string());
    }
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT count(*) FROM field_versions WHERE field = 'description'",
        ),
        description_versions_before
    );
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT count(*) FROM changes WHERE field = 'description'",
        ),
        description_changes_before
    );
}

#[test]
fn attachment_add_preserves_png_by_default_and_optimizes_when_requested() {
    let env = TestEnv::new();
    let db = env.db("attachment-optimize.sqlite");
    let task_ref = extract_ref(&ok(
        env.aven(&db, ["add", "optimize attachment", "--project", "app"])
    ));
    let image = env.path("compressible.png");
    let bytes = compressible_png_bytes();
    std::fs::write(&image, &bytes).unwrap();

    let preserved = ok(env.aven(
        &db,
        ["attachment", "add", &task_ref, image.to_str().unwrap()],
    ));
    contains_all(&preserved, &["attachment-added", "optimized=false"]);
    assert_eq!(extract_byte_size(&preserved), bytes.len() as i64);
    let preserved_id = extract_attachment_id(&preserved);
    let preserved_output = env.path("preserved.png");
    ok(env.aven(
        &db,
        [
            "attachment",
            "get",
            &preserved_id,
            "--output",
            preserved_output.to_str().unwrap(),
        ],
    ));
    assert_eq!(std::fs::read(preserved_output).unwrap(), bytes);

    let optimized = ok(env.aven(
        &db,
        [
            "attachment",
            "add",
            &task_ref,
            image.to_str().unwrap(),
            "--optimize",
        ],
    ));
    contains_all(&optimized, &["attachment-added", "optimized=true"]);
    assert!(extract_byte_size(&optimized) < bytes.len() as i64);

    let conflict = fail(env.aven(
        &db,
        [
            "attachment",
            "add",
            &task_ref,
            image.to_str().unwrap(),
            "--optimize",
            "--no-optimize",
        ],
    ));
    contains_all(&conflict, &["cannot be used with"]);
}
#[test]
fn attachment_add_obeys_image_optimization_config() {
    let env = TestEnv::new();
    env.write_config(
        r#"
local:
  image_optimization: on
"#,
    );
    let db = env.db("attachment-config-optimize.sqlite");
    let task_ref = extract_ref(&ok(
        env.aven(&db, ["add", "configured attachment", "--project", "app"])
    ));
    let image = env.path("configured-compressible.png");
    let bytes = compressible_png_bytes();
    std::fs::write(&image, &bytes).unwrap();

    let optimized = ok(env.aven(
        &db,
        ["attachment", "add", &task_ref, image.to_str().unwrap()],
    ));
    contains_all(&optimized, &["attachment-added", "optimized=true"]);
    assert!(extract_byte_size(&optimized) < bytes.len() as i64);

    let preserved = ok(env.aven(
        &db,
        [
            "attachment",
            "add",
            &task_ref,
            image.to_str().unwrap(),
            "--no-optimize",
        ],
    ));
    contains_all(&preserved, &["attachment-added", "optimized=false"]);
    assert_eq!(extract_byte_size(&preserved), bytes.len() as i64);
}

#[test]
fn attachment_add_and_delete_work_when_sync_enabled() {
    let env = TestEnv::new();
    let db = env.db("attachment-sync-enabled.sqlite");
    env.write_config(&format!(
        r#"
sync:
  enabled: true
  server_url: "http://127.0.0.1:9"
daemon:
  wake_addr: "{}"
"#,
        env.free_loopback_addr()
    ));
    let created = ok(env.aven(&db, ["add", "sync attachment", "--project", "app"]));
    let task_ref = extract_ref(&created);
    let image = env.path("sync-photo.png");
    std::fs::write(&image, compressible_png_bytes()).unwrap();

    let added = ok(env.aven(
        &db,
        ["attachment", "add", &task_ref, image.to_str().unwrap()],
    ));
    contains_all(&added, &["attachment-added", "has_blob=true"]);
    let attachment_id = extract_attachment_id(&added);
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT count(*) FROM changes WHERE op_type = 'attachment_add'"
        ),
        1
    );

    let deleted = ok(env.aven(&db, ["attachment", "delete", &attachment_id]));
    contains_all(&deleted, &["attachment-deleted", "deleted=yes"]);
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT count(*) FROM changes WHERE op_type = 'attachment_delete'",
        ),
        1
    );
}

#[test]
fn attachment_get_refuses_existing_output_and_mime_mismatch() {
    let env = TestEnv::new();
    let db = env.db("attachment-errors.sqlite");
    let created = ok(env.aven(&db, ["add", "attach me", "--project", "app"]));
    let task_ref = extract_ref(&created);

    let unknown = env.path("photo.bin");
    let image_bytes = compressible_png_bytes();
    std::fs::write(&unknown, &image_bytes).unwrap();
    let added = ok(env.aven(
        &db,
        ["attachment", "add", &task_ref, unknown.to_str().unwrap()],
    ));
    contains_all(&added, &["media_type=image/png"]);

    let error = fail(env.aven(
        &db,
        [
            "attachment",
            "add",
            &task_ref,
            unknown.to_str().unwrap(),
            "--media-type",
            "image/jpeg",
        ],
    ));
    contains_all(&error, &["error attachment-media-type-mismatch"]);

    let image = env.path("photo.png");
    std::fs::write(&image, image_bytes).unwrap();
    let added = ok(env.aven(
        &db,
        ["attachment", "add", &task_ref, image.to_str().unwrap()],
    ));
    let attachment_id = extract_attachment_id(&added);
    let output = env.path("copy.png");
    std::fs::write(&output, b"existing").unwrap();
    let error = fail(env.aven(
        &db,
        [
            "attachment",
            "get",
            &attachment_id,
            "--output",
            output.to_str().unwrap(),
        ],
    ));
    contains_all(&error, &["error output-exists"]);
    assert_eq!(std::fs::read(output).unwrap(), b"existing");
}

#[test]
fn attachment_get_refuses_unavailable_imported_blob() {
    let env = TestEnv::new();
    let source_db = env.db("attachment-import-source.sqlite");
    let target_db = env.db("attachment-import-target.sqlite");
    let task_ref = extract_ref(&ok(
        env.aven(&source_db, ["add", "attach me", "--project", "app"])
    ));
    let image = env.path("photo.png");
    std::fs::write(&image, compressible_png_bytes()).unwrap();
    let added = ok(env.aven(
        &source_db,
        ["attachment", "add", &task_ref, image.to_str().unwrap()],
    ));
    let attachment_id = extract_attachment_id(&added);
    let export_path = env.path("attachment-export.json");
    ok(env.aven(
        &source_db,
        ["export", "--output", export_path.to_str().unwrap()],
    ));
    ok(env.aven(
        &target_db,
        ["import", "--yes", export_path.to_str().unwrap()],
    ));

    let output = env.path("copy.png");
    let error = fail(env.aven(
        &target_db,
        [
            "attachment",
            "get",
            &attachment_id,
            "--output",
            output.to_str().unwrap(),
        ],
    ));
    contains_all(&error, &["error attachment-blob-unavailable"]);
    assert!(!output.exists());
}

#[test]
fn attachment_delete_is_idempotent() {
    let env = TestEnv::new();
    let db = env.db("attachment-delete-idempotent.sqlite");
    let created = ok(env.aven(&db, ["add", "attach me", "--project", "app"]));
    let task_ref = extract_ref(&created);
    let image = env.path("photo.png");
    std::fs::write(&image, compressible_png_bytes()).unwrap();
    let added = ok(env.aven(
        &db,
        ["attachment", "add", &task_ref, image.to_str().unwrap()],
    ));
    let attachment_id = extract_attachment_id(&added);

    ok(env.aven(&db, ["attachment", "delete", &attachment_id]));
    ok(env.aven(&db, ["attachment", "delete", &attachment_id]));
    let all = ok(env.aven(&db, ["attachment", "list", &task_ref, "--all", "--json"]));
    let value: serde_json::Value = serde_json::from_str(&all).unwrap();
    assert_eq!(value.as_array().unwrap().len(), 1);
    assert_eq!(value[0]["deleted"], true);
}

#[test]
fn version_flag_prints_package_version() {
    let output = ok(command()
        .arg("--version")
        .output()
        .expect("run aven --version"));
    assert_eq!(output.trim(), format!("aven {}", env!("CARGO_PKG_VERSION")));
}

#[test]
fn explicit_zero_result_limits_are_value_errors() {
    let env = TestEnv::new();
    let db = env.db("invalid-result-limits.sqlite");

    for arguments in [
        vec!["search", "task", "--json", "--limit", "0"],
        vec!["list", "--limit", "0"],
        vec!["prime", "--limit", "0"],
        vec!["label", "list", "--limit", "0"],
        vec!["project", "list", "--limit", "0"],
        vec!["conflict", "list", "--limit", "0"],
    ] {
        let output = env.aven(&db, arguments);
        assert_eq!(output.status.code(), Some(2));
        let error = fail(output);
        contains_all(&error, &["invalid value '0'", "--limit <LIMIT>", "1.."]);
    }
}

#[test]
fn list_json_supports_limit() {
    let env = TestEnv::new();
    let db = env.db("list-json.sqlite");
    ok(env.aven(&db, ["add", "task one", "--project", "app"]));
    ok(env.aven(&db, ["add", "task two", "--project", "app"]));

    let output = ok(env.aven(&db, ["list", "--json", "--limit", "1"]));
    let items: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(items.as_array().unwrap().len(), 1);
    assert!(items[0]["ref"].is_string());
    assert!(items[0]["title"].is_string());
    assert!(items[0]["status"].is_string());
    assert!(items[0]["project"].is_string());
}

#[test]
fn list_open_filters_available_nonterminal_tasks_and_composes() {
    let env = TestEnv::new();
    let db = env.db("list-open.sqlite");
    ok(env.aven(&db, ["label", "create", "bug"]));
    ok(env.aven(&db, ["project", "create", "app"]));
    ok(env.aven(&db, ["project", "create", "ops"]));

    let app_open = extract_ref(&ok(env.aven(
        &db,
        [
            "add",
            "available app work",
            "--project",
            "app",
            "--status",
            "todo",
            "--priority",
            "high",
            "--label",
            "bug",
        ],
    )));
    let ops_open = extract_ref(&ok(env.aven(
        &db,
        [
            "add",
            "available ops work",
            "--project",
            "ops",
            "--status",
            "active",
        ],
    )));
    let done = extract_ref(&ok(env.aven(
        &db,
        [
            "add",
            "completed work",
            "--project",
            "app",
            "--status",
            "done",
        ],
    )));
    let canceled = extract_ref(&ok(env.aven(
        &db,
        [
            "add",
            "canceled work",
            "--project",
            "app",
            "--status",
            "canceled",
        ],
    )));
    let future = extract_ref(&ok(env.aven(
        &db,
        [
            "add",
            "future work",
            "--project",
            "app",
            "--status",
            "todo",
            "--available-at",
            "2099-01-01T00:00:00Z",
        ],
    )));
    let deleted = extract_ref(&ok(env.aven(
        &db,
        [
            "add",
            "deleted work",
            "--project",
            "app",
            "--status",
            "todo",
        ],
    )));
    ok(env.aven(&db, ["delete", &deleted]));

    let open = ok(env.aven(&db, ["list", "--open"]));
    contains_all(&open, &[&app_open, &ops_open]);
    contains_none(&open, &[&done, &canceled, &future, &deleted]);

    let composed = ok(env.aven(
        &db,
        [
            "list",
            "--open",
            "--project",
            "app",
            "--priority",
            "high",
            "--label",
            "bug",
        ],
    ));
    contains_all(&composed, &[&app_open, "available app work"]);
    contains_none(&composed, &[&ops_open, &done, &canceled, &future, &deleted]);
}

#[test]
fn list_open_rejects_incompatible_selectors() {
    let env = TestEnv::new();
    let db = env.db("list-open-conflicts.sqlite");

    for args in [
        ["list", "--open", "--status", "todo"].as_slice(),
        ["list", "--open", "--all"].as_slice(),
        ["list", "--open", "--deleted"].as_slice(),
        ["list", "--open", "--upcoming"].as_slice(),
    ] {
        contains_all(
            &fail(env.aven(&db, args.iter().copied())),
            &["--open", "cannot be used with"],
        );
    }
}

#[test]
fn availability_preserves_attention_filters_and_explicit_discovery() {
    let env = TestEnv::new();
    let db = env.db("availability.sqlite");
    let open_ref = extract_ref(&ok(env.aven(
        &db,
        [
            "add",
            "future open needle",
            "--project",
            "app",
            "--available-at",
            "2099-01-01T00:00:00Z",
        ],
    )));
    let done_ref = extract_ref(&ok(env.aven(
        &db,
        [
            "add",
            "future terminal needle",
            "--project",
            "app",
            "--available-at",
            "2099-01-02T00:00:00Z",
        ],
    )));
    ok(env.aven(&db, ["edit", &done_ref, "--status", "done"]));
    let deleted_ref = extract_ref(&ok(env.aven(
        &db,
        [
            "add",
            "future deleted needle",
            "--project",
            "app",
            "--available-at",
            "2099-01-03T00:00:00Z",
        ],
    )));
    ok(env.aven(&db, ["delete", &deleted_ref]));

    let regular = ok(env.aven(&db, ["list"]));
    contains_none(
        &regular,
        &[
            "future open needle",
            "future terminal needle",
            "future deleted needle",
        ],
    );

    let upcoming = ok(env.aven(&db, ["list", "--upcoming", "--json"]));
    let items: serde_json::Value = serde_json::from_str(&upcoming).unwrap();
    assert_eq!(items.as_array().unwrap().len(), 1);
    assert_eq!(items[0]["ref"], open_ref);
    assert_eq!(items[0]["available_at"], "2099-01-01T00:00:00Z");

    let terminal = ok(env.aven(&db, ["list", "--status", "done"]));
    contains_all(&terminal, &[&done_ref, "future terminal needle"]);
    contains_none(&terminal, &["future open needle", "future deleted needle"]);

    let deleted = ok(env.aven(&db, ["list", "--deleted"]));
    contains_all(
        &deleted,
        &[&deleted_ref, "future deleted needle", "deleted=yes"],
    );
    contains_none(&deleted, &["future open needle", "future terminal needle"]);

    let shown = ok(env.aven(&db, ["show", &open_ref]));
    contains_all(
        &shown,
        &[
            &open_ref,
            "future open needle",
            "available_at=2099-01-01T00:00:00Z",
        ],
    );

    let searched = ok(env.aven(&db, ["search", "future open needle"]));
    contains_all(&searched, &[&open_ref, "future open needle"]);
    contains_none(
        &searched,
        &["future terminal needle", "future deleted needle"],
    );

    ok(env.aven(&db, ["edit", &open_ref, "--clear-available-at"]));
    let regular = ok(env.aven(&db, ["list", "--json"]));
    let items: serde_json::Value = serde_json::from_str(&regular).unwrap();
    assert_eq!(items.as_array().unwrap().len(), 1);
    assert_eq!(items[0]["ref"], open_ref);
    assert_eq!(items[0]["available_at"], "");
}

#[test]
fn due_dates_round_trip_filter_and_preserve_defer_semantics() {
    let env = TestEnv::new();
    let db = env.db("due-dates.sqlite");
    let overdue_ref = extract_ref(&ok(env.aven(
        &db,
        [
            "add",
            "overdue task",
            "--project",
            "app",
            "--due",
            "2000-01-01",
        ],
    )));
    let newer_overdue_ref = extract_ref(&ok(env.aven(
        &db,
        [
            "add",
            "newer overdue task",
            "--project",
            "app",
            "--due",
            "2010-01-01",
        ],
    )));
    let future_ref = extract_ref(&ok(env.aven(
        &db,
        [
            "add",
            "future deadline",
            "--project",
            "app",
            "--due",
            "2099-01-01",
        ],
    )));
    let deferred_ref = extract_ref(&ok(env.aven(
        &db,
        [
            "add",
            "deferred overdue task",
            "--project",
            "app",
            "--available-at",
            "2099-02-01T00:00:00Z",
            "--due",
            "2000-02-01",
        ],
    )));

    let shown: serde_json::Value =
        serde_json::from_str(&ok(env.aven(&db, ["show", &future_ref, "--json"]))).unwrap();
    assert_eq!(shown["due_on"], "2099-01-01");

    let overdue: serde_json::Value =
        serde_json::from_str(&ok(env.aven(&db, ["list", "--overdue", "--json"]))).unwrap();
    assert_eq!(overdue.as_array().unwrap().len(), 2);
    assert_eq!(overdue[0]["ref"], overdue_ref);
    assert_eq!(overdue[1]["ref"], newer_overdue_ref);

    let mismatch: serde_json::Value = serde_json::from_str(&ok(
        env.aven(&db, ["list", "--upcoming", "--overdue", "--json"])
    ))
    .unwrap();
    assert_eq!(mismatch.as_array().unwrap().len(), 1);
    assert_eq!(mismatch[0]["ref"], deferred_ref);
    assert_eq!(mismatch[0]["due_on"], "2000-02-01");

    ok(env.aven(&db, ["edit", &future_ref, "--due", "2099-03-01"]));
    let edited: serde_json::Value =
        serde_json::from_str(&ok(env.aven(&db, ["show", &future_ref, "--json"]))).unwrap();
    assert_eq!(edited["due_on"], "2099-03-01");

    ok(env.aven(&db, ["edit", &future_ref, "--clear-due"]));
    let cleared: serde_json::Value =
        serde_json::from_str(&ok(env.aven(&db, ["show", &future_ref, "--json"]))).unwrap();
    assert_eq!(cleared["due_on"], "");
}

#[test]
fn due_dates_accept_calendar_expressions_and_reject_times() {
    let env = TestEnv::new();
    let db = env.db("due-date-input.sqlite");
    let task_ref = extract_ref(&ok(env.aven(
        &db,
        [
            "add",
            "fuzzy deadline",
            "--project",
            "app",
            "--due",
            "in 2 weeks",
        ],
    )));
    let shown: serde_json::Value =
        serde_json::from_str(&ok(env.aven(&db, ["show", &task_ref, "--json"]))).unwrap();
    assert_eq!(shown["due_on"].as_str().unwrap().len(), 10);

    let error = fail(env.aven(&db, ["edit", &task_ref, "--due", "2099-01-01T09:00:00Z"]));
    contains_all(&error, &["invalid-due", "ISO date"]);
}

#[test]
fn fuzzy_availability_is_shared_by_cli_add_and_edit() {
    let env = TestEnv::new();
    let db = env.db("fuzzy-availability.sqlite");
    let task_ref = extract_ref(&ok(env.aven(
        &db,
        [
            "add",
            "fuzzy deferred task",
            "--project",
            "app",
            "--available-at",
            "in 2 weeks",
        ],
    )));

    let created: serde_json::Value =
        serde_json::from_str(&ok(env.aven(&db, ["show", &task_ref, "--json"]))).unwrap();
    let created_at = created["available_at"].as_str().unwrap();
    assert_eq!(created_at.len(), 20);
    assert!(created_at.ends_with('Z'));

    ok(env.aven(
        &db,
        ["edit", &task_ref, "--available-at", "next monday at 9am"],
    ));
    let edited: serde_json::Value =
        serde_json::from_str(&ok(env.aven(&db, ["show", &task_ref, "--json"]))).unwrap();
    let edited_at = edited["available_at"].as_str().unwrap();
    assert_eq!(edited_at.len(), 20);
    assert!(edited_at.ends_with('Z'));
    assert_ne!(edited_at, created_at);

    let error = fail(env.aven(&db, ["edit", &task_ref, "--available-at", "monday"]));
    contains_all(
        &error,
        &["ambiguous-available-at-weekday", "use next monday"],
    );
}

#[test]
fn local_calendar_dates_use_offsets_across_daylight_saving_boundaries() {
    let env = TestEnv::new();
    let db = env.db("availability-timezones.sqlite");
    let cases = [
        (
            "spring transition",
            "2026-03-08",
            "America/New_York",
            "2026-03-08T05:00:00Z",
        ),
        (
            "fall transition",
            "2026-11-01",
            "America/New_York",
            "2026-11-01T04:00:00Z",
        ),
        (
            "quarter hour offset",
            "2026-07-16",
            "Asia/Kathmandu",
            "2026-07-15T18:15:00Z",
        ),
    ];

    for (title, date, timezone, expected) in cases {
        let created = ok(command_with_db(&db)
            .env("TZ", timezone)
            .env("XDG_STATE_HOME", env.state_dir())
            .env("AVEN_CONFIG_DIR", env.config_dir().join("aven"))
            .args(["add", title, "--project", "app", "--available-at", date])
            .output()
            .expect("run aven in timezone"));
        let task_ref = extract_ref(&created);
        let shown = ok(command_with_db(&db)
            .env("TZ", timezone)
            .env("XDG_STATE_HOME", env.state_dir())
            .env("AVEN_CONFIG_DIR", env.config_dir().join("aven"))
            .args(["show", &task_ref, "--json"])
            .output()
            .expect("show timezone task"));
        let task: serde_json::Value = serde_json::from_str(&shown).unwrap();
        assert_eq!(task["available_at"], expected, "timezone={timezone}");
    }
}

#[test]
fn show_json_returns_single_task() {
    let env = TestEnv::new();
    let db = env.db("show-json.sqlite");
    let task_ref = extract_ref(&ok(env.aven(
        &db,
        [
            "add",
            "show json test",
            "--project",
            "app",
            "--priority",
            "high",
        ],
    )));

    let output = ok(env.aven(&db, ["show", &task_ref, "--json"]));
    let item: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(item["ref"], task_ref);
    assert_eq!(item["title"], "show json test");
    assert_eq!(item["priority"], "high");
    assert!(item.get("task").is_none());
    assert!(item.get("description").is_none());
}

#[test]
fn show_json_full_includes_description_dependencies_notes_conflicts() {
    let env = TestEnv::new();
    let db = env.db("show-json-full.sqlite");
    let task_ref = extract_ref(&ok(env.aven(
        &db,
        [
            "add",
            "full json test",
            "--project",
            "app",
            "--description",
            "full json description",
        ],
    )));

    let noted = ok(env.aven(&db, ["note", &task_ref, "full json note"]));
    let note_id = noted
        .split_whitespace()
        .find_map(|part| part.strip_prefix("note="))
        .expect("note id in output");

    let output = ok(env.aven(&db, ["show", &task_ref, "--json", "--full"]));
    let item: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(item["task"]["ref"], task_ref);
    assert_eq!(item["description"], "full json description");
    assert!(item["dependencies"]["depends_on"].is_array());
    assert!(item["dependencies"]["blocks"].is_array());
    assert_eq!(item["notes"][0]["id"], note_id);
    assert_eq!(item["notes"][0]["body"], "full json note");
    assert!(item["notes"][0]["created_at"].is_string());
    assert!(item["conflicts"].is_array());
}

#[test]
fn project_list_json_supports_search_and_limit() {
    let env = TestEnv::new();
    let db = env.db("project-list-json.sqlite");
    ok(env.aven(&db, ["project", "create", "agent-offload"]));
    ok(env.aven(&db, ["project", "create", "docs-api"]));
    ok(env.aven(&db, ["project", "create", "docs-site"]));

    let output = ok(env.aven(
        &db,
        [
            "project", "list", "--search", "docs", "--json", "--limit", "1",
        ],
    ));
    let items: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(items.as_array().unwrap().len(), 1);
    assert!(items[0]["key"].as_str().unwrap().starts_with("docs-"));
    assert!(items[0]["prefix"].is_string());
}

#[test]
fn label_list_json_supports_search_and_limit() {
    let env = TestEnv::new();
    let db = env.db("label-list-json.sqlite");
    ok(env.aven(&db, ["label", "create", "bug"]));
    ok(env.aven(&db, ["label", "create", "sync-api"]));
    ok(env.aven(&db, ["label", "create", "sync-ui"]));

    let output = ok(env.aven(
        &db,
        [
            "label", "list", "--search", "sync", "--json", "--limit", "1",
        ],
    ));
    let items: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(items.as_array().unwrap().len(), 1);
    assert!(items[0].as_str().unwrap().starts_with("sync-"));
}

#[test]
fn creates_db_and_captures_task() {
    let env = TestEnv::new();
    let implicit = env.db("implicit.sqlite");
    let output = command()
        .env("AVEN_DB", &implicit)
        .args(["project", "create", "implicit"])
        .output()
        .expect("run aven with AVEN_DB");
    ok(output);
    assert!(implicit.exists(), "implicit database was not created");

    let db = env.db("local.sqlite");
    ok(env.aven(&db, ["label", "create", "bug"]));
    let created = ok(env.aven(
        &db,
        [
            "add",
            "fix sync conflict display",
            "--project",
            "app",
            "--label",
            "bug",
            "--priority",
            "high",
        ],
    ));
    let task_ref = extract_ref(&created);
    let bare = suffix(&task_ref);
    contains_all(
        &created,
        &[
            "created",
            "APP-",
            "ref=",
            &format!("ref={bare}"),
            "project=app",
            "status=inbox",
            "priority=high",
            r#"title="fix sync conflict display""#,
        ],
    );

    let list = ok(env.aven(&db, ["list"]));
    contains_all(
        &list,
        &[&task_ref, "status=inbox", "priority=high", "labels=bug"],
    );

    let shown = ok(env.aven(&db, ["show", &task_ref]));
    contains_all(
        &shown,
        &[&task_ref, "status=inbox", "priority=high", "labels=bug"],
    );
}

#[test]
fn updates_task_and_preserves_suffix_on_project_move() {
    let env = TestEnv::new();
    let db = env.db("move.sqlite");
    ok(env.aven(&db, ["label", "create", "bug"]));
    ok(env.aven(&db, ["label", "create", "sync"]));
    let created = ok(env.aven(
        &db,
        [
            "add",
            "fix sync conflict display",
            "--project",
            "app",
            "--label",
            "bug",
        ],
    ));
    let original = extract_ref(&created);
    let original_suffix = suffix(&original);

    let updated = ok(env.aven(
        &db,
        [
            "edit",
            &original,
            "--title",
            "fix conflict display",
            "--status",
            "active",
            "--priority",
            "urgent",
            "--label",
            "sync",
            "--remove-label",
            "bug",
            "--project",
            "homelab",
        ],
    ));
    let moved = extract_ref(&updated);
    contains_all(
        &updated,
        &[
            "updated HML-",
            "changed=yes",
            "status=active",
            "priority=urgent",
        ],
    );
    assert_eq!(
        original_suffix,
        suffix(&moved),
        "project move changed suffix"
    );

    let shown = ok(env.aven(&db, ["show", &moved]));
    contains_all(
        &shown,
        &[
            &moved,
            "status=active",
            "priority=urgent",
            "labels=sync",
            r#"title="fix conflict display""#,
        ],
    );
    contains_none(&shown, &["labels=bug"]);
}

#[test]
fn renames_project_and_display_prefix() {
    let env = TestEnv::new();
    let db = env.db("rename-project.sqlite");
    let created = ok(env.aven(
        &db,
        ["add", "move project metadata", "--project", "agent-offload"],
    ));
    let task_ref = extract_ref(&created);
    let before_context = ok(env.aven(&db, ["context", &task_ref, "--json"]));
    let before_context: serde_json::Value = serde_json::from_str(&before_context).unwrap();
    let project_id = before_context["project"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    contains_all(&created, &["project=agent-offload", "AO-"]);

    let renamed = ok(env.aven(
        &db,
        [
            "project",
            "rename",
            "agent-offload",
            "sideagent",
            "--prefix",
            "SIDE",
        ],
    ));
    contains_all(
        &renamed,
        &[
            "renamed-project sideagent",
            "changed=yes",
            "old=agent-offload",
            "old_prefix=AO",
            "prefix=SIDE",
            r#"name="sideagent""#,
        ],
    );

    let shown = ok(env.aven(&db, ["show", &task_ref]));
    contains_all(&shown, &["SIDE-"]);
    contains_none(&shown, &["agent-offload"]);
    let after_context = ok(env.aven(&db, ["context", &task_ref, "--json"]));
    let after_context: serde_json::Value = serde_json::from_str(&after_context).unwrap();
    assert_eq!(after_context["project"]["id"], project_id);

    let filtered = ok(env.aven(&db, ["list", "--project", "sideagent"]));
    contains_all(&filtered, &[&suffix(&task_ref), "move project metadata"]);

    let projects = ok(env.aven(&db, ["project", "list"]));
    contains_all(&projects, &["sideagent prefix=SIDE"]);
    contains_none(&projects, &["agent-offload"]);
}

#[test]
fn singular_list_commands_list_workspace_values() {
    let env = TestEnv::new();
    let db = env.db("singular-list.sqlite");
    ok(env.aven(&db, ["label", "create", "bug"]));
    ok(env.aven(&db, ["project", "create", "agent-offload"]));

    let projects = ok(env.aven(&db, ["project", "list", "--search", "agent"]));
    contains_all(&projects, &["agent-offload prefix=AO"]);
    contains_none(&projects, &["app prefix=APP"]);

    let labels = ok(env.aven(&db, ["label", "list", "--search", "bu"]));
    contains_all(&labels, &["bug"]);
    contains_none(&labels, &["sync"]);
}

#[test]
fn project_rename_updates_managed_path_mapping() {
    let env = TestEnv::new();
    let db = env.db("rename-project-path.sqlite");
    let project_dir = env.path("mapped-project");
    std::fs::create_dir_all(&project_dir).unwrap();

    ok(env.aven(&db, ["project", "create", "agent-offload"]));
    ok(env.aven(
        &db,
        [
            "project",
            "path",
            "add",
            "agent-offload",
            project_dir.to_str().unwrap(),
        ],
    ));
    let renamed = ok(env.aven(
        &db,
        [
            "project",
            "rename",
            "agent-offload",
            "sideagent",
            "--prefix",
            "SIDE",
        ],
    ));

    contains_all(&renamed, &["updated-config-project-mapping sideagent"]);
    let config = std::fs::read_to_string(env.config_file()).unwrap();
    contains_all(&config, &["project: sideagent"]);
    contains_none(&config, &["project: agent-offload"]);
}

#[test]
fn project_rename_rolls_back_when_config_mapping_write_fails() {
    let env = TestEnv::new();
    let db = env.db("rename-project-config-failure.sqlite");
    let project_dir = env.path("mapped-project");
    std::fs::create_dir_all(&project_dir).unwrap();

    ok(env.aven(&db, ["project", "create", "agent-offload"]));
    ok(env.aven(
        &db,
        [
            "project",
            "path",
            "add",
            "agent-offload",
            project_dir.to_str().unwrap(),
        ],
    ));
    let blocked_tmp = env.config_file().with_extension("yaml.tmp");
    std::fs::create_dir(&blocked_tmp).unwrap();

    let error = fail(env.aven(&db, ["project", "rename", "agent-offload", "sideagent"]));

    contains_all(&error, &["could not write"]);
    assert_eq!(
        scalar_i64(
            &db,
            "SELECT count(*) FROM projects WHERE key = 'agent-offload'"
        ),
        1
    );
    assert_eq!(
        scalar_i64(&db, "SELECT count(*) FROM projects WHERE key = 'sideagent'"),
        0
    );
}

#[test]
fn project_rename_noop_does_not_claim_config_update() {
    let env = TestEnv::new();
    let db = env.db("rename-project-noop.sqlite");
    let project_dir = env.path("mapped-project");
    std::fs::create_dir_all(&project_dir).unwrap();

    ok(env.aven(&db, ["project", "create", "agent-offload"]));
    ok(env.aven(
        &db,
        [
            "project",
            "path",
            "add",
            "agent-offload",
            project_dir.to_str().unwrap(),
        ],
    ));
    let renamed = ok(env.aven(
        &db,
        [
            "project",
            "rename",
            "agent-offload",
            "agent-offload",
            "--prefix",
            "AO",
        ],
    ));

    contains_all(&renamed, &["changed=none"]);
    contains_none(&renamed, &["updated-config-project-mapping"]);
}

#[test]
fn delete_restore_and_filters_work() {
    let env = TestEnv::new();
    let db = env.db("filters.sqlite");
    for label in ["bug", "sync", "docs"] {
        ok(env.aven(&db, ["label", "create", label]));
    }
    ok(env.aven(&db, ["project", "create", "app"]));
    ok(env.aven(&db, ["project", "create", "ops"]));
    let app_bug = extract_ref(&ok(env.aven(
        &db,
        [
            "add",
            "app bug",
            "--project",
            "app",
            "--label",
            "bug",
            "--priority",
            "high",
        ],
    )));
    let app_docs = extract_ref(&ok(env.aven(
        &db,
        [
            "add",
            "app docs",
            "--project",
            "app",
            "--label",
            "docs",
            "--priority",
            "low",
        ],
    )));
    let ops_sync = extract_ref(&ok(env.aven(
        &db,
        [
            "add",
            "ops sync",
            "--project",
            "ops",
            "--label",
            "sync",
            "--priority",
            "urgent",
        ],
    )));
    ok(env.aven(&db, ["edit", &app_docs, "--status", "active"]));
    ok(env.aven(&db, ["edit", &ops_sync, "--status", "done"]));

    let by_project = ok(env.aven(&db, ["list", "--project", "app"]));
    contains_all(&by_project, &["app bug", "app docs"]);
    contains_none(&by_project, &["ops sync"]);

    let by_status = ok(env.aven(&db, ["list", "--status", "active"]));
    contains_all(&by_status, &["app docs"]);
    contains_none(&by_status, &["app bug", "ops sync"]);

    let by_priority = ok(env.aven(&db, ["list", "--priority", "urgent"]));
    contains_all(&by_priority, &["ops sync"]);
    contains_none(&by_priority, &["app bug", "app docs"]);

    let by_label = ok(env.aven(&db, ["list", "--label", "bug"]));
    contains_all(&by_label, &["app bug"]);
    contains_none(&by_label, &["app docs", "ops sync"]);

    ok(env.aven(&db, ["delete", &app_bug]));
    let normal = ok(env.aven(&db, ["list"]));
    contains_none(&normal, &[&app_bug, "app bug"]);

    let all = ok(env.aven(&db, ["list", "--all"]));
    contains_all(&all, &[&app_bug, "deleted=yes", "app bug", "app docs"]);

    let deleted = ok(env.aven(&db, ["list", "--deleted"]));
    contains_all(&deleted, &[&app_bug, "deleted=yes", "app bug"]);
    contains_none(&deleted, &["app docs", "ops sync"]);

    let all_project = ok(env.aven(&db, ["list", "--project", "app", "--all"]));
    contains_all(&all_project, &[&app_bug, "deleted=yes", "app bug"]);
    contains_none(&all_project, &["ops sync"]);

    ok(env.aven(&db, ["restore", &app_bug]));
    let restored = ok(env.aven(&db, ["list"]));
    contains_all(&restored, &[&app_bug, "app bug"]);
}

#[test]
fn search_project_scope_filters_before_ranking_and_limit() {
    let env = TestEnv::new();
    let db = env.db("search-project.sqlite");
    ok(env.aven(&db, ["project", "create", "Operations"]));
    let selected = extract_ref(&ok(env.aven(
        &db,
        [
            "add",
            "Selected project result",
            "--project",
            "app",
            "--description",
            "needle in selected description",
        ],
    )));
    let unrelated = extract_ref(&ok(env.aven(
        &db,
        [
            "add",
            "needle",
            "--project",
            "Operations",
            "--priority",
            "urgent",
        ],
    )));

    let unscoped = ok(env.aven(&db, ["search", "needle", "--limit", "1"]));
    contains_all(&unscoped, &[&unrelated, "needle"]);
    contains_none(&unscoped, &[&selected, "Selected project result"]);

    let scoped = ok(env.aven(
        &db,
        ["search", "--project", "app", "needle", "--limit", "1"],
    ));
    contains_all(
        &scoped,
        &[&selected, "Selected project result", "match=description"],
    );
    contains_none(&scoped, &[&unrelated]);

    let all = ok(env.aven(&db, ["search", "needle"]));
    contains_all(&all, &[&selected, &unrelated]);

    let missing = fail(env.aven(&db, ["search", "--project", "missing", "needle"]));
    contains_all(&missing, &["error unknown-project input=missing"]);
}

#[test]
fn search_controls_deleted_visibility() {
    let env = TestEnv::new();
    let db = env.db("search-deleted.sqlite");
    let live = extract_ref(&ok(
        env.aven(&db, ["add", "live needle", "--project", "app"])
    ));
    let deleted = extract_ref(&ok(
        env.aven(&db, ["add", "deleted needle", "--project", "app"])
    ));
    ok(env.aven(&db, ["delete", &deleted]));

    let ordinary = ok(env.aven(&db, ["search", "needle"]));
    contains_all(&ordinary, &[&live, "live needle"]);
    contains_none(&ordinary, &[&deleted, "deleted needle", "deleted=yes"]);

    let all = ok(env.aven(&db, ["search", "needle", "--all"]));
    contains_all(&all, &[&live, &deleted, "deleted needle", "deleted=yes"]);

    let by_ref = ok(env.aven(&db, ["search", &deleted]));
    contains_all(
        &by_ref,
        &[&deleted, "deleted needle", "match=ref", "deleted=yes"],
    );

    let by_ref_json = ok(env.aven(&db, ["search", "--json", &deleted]));
    let items: serde_json::Value = serde_json::from_str(&by_ref_json).unwrap();
    assert_eq!(items[0]["ref"], serde_json::json!(deleted));
    assert_eq!(items[0]["deleted"], serde_json::json!(true));
    assert_eq!(items[0]["matched_field"], serde_json::json!("ref"));
}

#[test]
fn search_json_includes_complete_task_shape_and_search_metadata() {
    let env = TestEnv::new();
    let db = env.db("search-json-shape.sqlite");
    let epic = extract_ref(&ok(env.aven(
        &db,
        ["add", "complete json epic", "--project", "app", "--epic"],
    )));
    let blocker = extract_ref(&ok(
        env.aven(&db, ["add", "complete json blocker", "--project", "app"])
    ));
    let task = extract_ref(&ok(env.aven(
        &db,
        [
            "add",
            "complete json target",
            "--project",
            "app",
            "--priority",
            "high",
            "--available-at",
            "2030-01-02T03:04:05Z",
            "--due",
            "2030-01-03",
        ],
    )));
    ok(env.aven(&db, ["epic", "add", &task, &epic]));
    ok(env.aven(&db, ["dep", "add", &task, &blocker]));

    let listed_task: serde_json::Value =
        serde_json::from_str(&ok(env.aven(&db, ["show", &task, "--json"]))).unwrap();
    let searched: serde_json::Value = serde_json::from_str(&ok(env.aven(
        &db,
        ["search", "complete json target", "--limit", "1", "--json"],
    )))
    .unwrap();
    let searched_task = &searched[0];

    for field in [
        "ref",
        "id",
        "title",
        "project",
        "status",
        "priority",
        "labels",
        "deleted",
        "is_epic",
        "epic_parent",
        "epic_children",
        "has_conflict",
        "blocked_by",
        "blocks",
        "available_at",
        "due_on",
        "recurrence",
        "recurrence_group",
        "created_at",
        "updated_at",
    ] {
        assert_eq!(searched_task[field], listed_task[field], "field {field}");
    }
    assert_eq!(searched_task["available_at"], "2030-01-02T03:04:05Z");
    assert_eq!(searched_task["due_on"], "2030-01-03");
    assert_eq!(searched_task["epic_parent"]["ref"], epic);
    assert_eq!(searched_task["blocked_by"], 1);
    assert_eq!(searched_task["blocks"], 0);
    assert!(
        searched_task["created_at"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
    assert!(
        searched_task["updated_at"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
    assert!(searched_task["score"].as_i64().is_some());
    assert_eq!(searched_task["matched_field"], "title");
    assert!(searched_task.get("snippet").is_some());
    assert_eq!(searched.as_array().unwrap().len(), 1);

    let epic_search: serde_json::Value = serde_json::from_str(&ok(
        env.aven(&db, ["search", &epic, "--limit", "1", "--json"])
    ))
    .unwrap();
    assert_eq!(epic_search[0]["is_epic"], true);
    assert_eq!(epic_search[0]["epic_children"][0]["ref"], task);
    assert_eq!(epic_search[0]["matched_field"], "ref");
}

#[test]
fn invalid_filter_values_fail() {
    let env = TestEnv::new();
    let db = env.db("bad-filter.sqlite");
    let error = fail(env.aven(&db, ["list", "--status", "blocked"]));
    contains_all(
        &error,
        &[
            "error invalid-status",
            "choices=inbox,backlog,todo,active,done,canceled",
        ],
    );
}

#[test]
fn bulk_update_filters_and_removes_label() {
    let env = TestEnv::new();
    let db = env.db("bulk-update.sqlite");
    for label in ["bug", "docs"] {
        ok(env.aven(&db, ["label", "create", label]));
    }
    let bug_one = extract_ref(&ok(env.aven(
        &db,
        ["add", "bug one", "--project", "app", "--label", "bug"],
    )));
    let bug_two = extract_ref(&ok(env.aven(
        &db,
        ["add", "bug two", "--project", "app", "--label", "bug"],
    )));
    let docs = extract_ref(&ok(
        env.aven(&db, ["add", "docs", "--project", "app", "--label", "docs"])
    ));

    let dry_run = ok(env.aven(
        &db,
        [
            "bulk-update",
            "--filter-label",
            "bug",
            "--remove-label",
            "bug",
            "--dry-run",
        ],
    ));
    contains_all(
        &dry_run,
        &[
            "would-update",
            "changed=yes",
            "bulk-update-summary matched=2 changed=0 would_change=2 unchanged=0 dry_run=yes",
        ],
    );
    contains_all(&ok(env.aven(&db, ["show", &bug_one])), &["labels=bug"]);

    let updated = ok(env.aven(
        &db,
        [
            "bulk-update",
            "--filter-label",
            "bug",
            "--remove-label",
            "bug",
        ],
    ));
    contains_all(
        &updated,
        &[
            "bulk-updated",
            "bulk-update-summary matched=2 changed=2 would_change=2 unchanged=0 dry_run=no",
        ],
    );
    contains_none(&ok(env.aven(&db, ["show", &bug_one])), &["labels=bug"]);
    contains_none(&ok(env.aven(&db, ["show", &bug_two])), &["labels=bug"]);
    contains_all(&ok(env.aven(&db, ["show", &docs])), &["labels=docs"]);
}

#[test]
fn bulk_update_reports_updated_project_display_ref() {
    let env = TestEnv::new();
    let db = env.db("bulk-update-project-ref.sqlite");
    ok(env.aven(&db, ["project", "create", "app"]));
    ok(env.aven(&db, ["project", "create", "ops"]));
    ok(env.aven(&db, ["add", "move me", "--project", "app"]));

    let updated = ok(env.aven(
        &db,
        ["bulk-update", "--project", "app", "--set-project", "ops"],
    ));
    let updated_ref = extract_ref(&updated);

    assert!(updated_ref.starts_with("OPS-"), "{updated}");
    contains_all(
        &ok(env.aven(&db, ["show", &updated_ref])),
        &["title=\"move me\""],
    );
}

#[test]
fn bulk_update_sets_and_removes_metadata() {
    let env = TestEnv::new();
    let db = env.db("bulk-update-metadata.sqlite");
    let first = extract_ref(&ok(env.aven(
        &db,
        [
            "add",
            "first metadata",
            "--project",
            "app",
            "--metadata",
            "legacy-id=1",
        ],
    )));
    let second = extract_ref(&ok(
        env.aven(&db, ["add", "second metadata", "--project", "app"])
    ));

    contains_all(
        &ok(env.aven(&db, ["bulk-update", "--all", "--metadata", "legacy-id=2"])),
        &["matched=2 changed=2 would_change=2 unchanged=0"],
    );
    for task_ref in [&first, &second] {
        contains_all(
            &ok(env.aven(&db, ["show", task_ref, "--full"])),
            &["key=legacy-id", "2"],
        );
    }
    contains_all(
        &ok(env.aven(&db, ["bulk-update", "--all", "--metadata", "legacy-id=2"])),
        &["matched=2 changed=0 would_change=0 unchanged=2"],
    );
    contains_all(
        &ok(env.aven(
            &db,
            ["bulk-update", "--all", "--remove-metadata", "legacy-id"],
        )),
        &["matched=2 changed=2 would_change=2 unchanged=0"],
    );
    for task_ref in [&first, &second] {
        contains_none(
            &ok(env.aven(&db, ["show", task_ref, "--full"])),
            &["key=legacy-id"],
        );
    }
}

#[test]
fn bulk_update_rejects_conflicted_field_before_mutation() {
    let env = TestEnv::new();
    let db = env.db("bulk-update-conflict.sqlite");
    let first = extract_ref(&ok(env.aven(&db, ["add", "first", "--project", "app"])));
    let second = extract_ref(&ok(env.aven(&db, ["add", "second", "--project", "app"])));
    execute_sql(
        &db,
        "INSERT INTO conflicts(workspace_id, task_id, field, local_value, remote_value,
         remote_change_id, variant_a, variant_b, created_at)
         SELECT workspace_id, id, 'priority', 'none', 'high', 'bulk-conflict',
                'none', 'high', 't'
         FROM tasks WHERE title = 'first'",
    );

    let error = fail(env.aven(&db, ["bulk-update", "--all", "--set-priority", "high"]));

    contains_all(
        &error,
        &[
            "error bulk-update-conflicted-field",
            &first,
            "field=priority",
        ],
    );
    contains_all(&ok(env.aven(&db, ["show", &first])), &["priority=none"]);
    contains_all(&ok(env.aven(&db, ["show", &second])), &["priority=none"]);
}

#[test]
fn bulk_update_rolls_back_when_a_later_task_update_fails() {
    let env = TestEnv::new();
    let db = env.db("bulk-update-atomic.sqlite");
    let first = extract_ref(&ok(env.aven(&db, ["add", "first", "--project", "app"])));
    let second = extract_ref(&ok(env.aven(&db, ["add", "second", "--project", "app"])));
    let priority_changes_before =
        scalar_i64(&db, "SELECT count(*) FROM changes WHERE field = 'priority'");
    execute_sql(
        &db,
        "CREATE TRIGGER reject_second_bulk_priority
         BEFORE UPDATE OF priority ON tasks
         WHEN NEW.priority = 'high'
           AND (SELECT count(*) FROM tasks WHERE priority = 'high') > 0
         BEGIN
             SELECT RAISE(ABORT, 'forced second bulk update failure');
         END",
    );

    let error = fail(env.aven(&db, ["bulk-update", "--all", "--set-priority", "high"]));

    contains_all(&error, &["forced second bulk update failure"]);
    contains_none(&error, &["bulk-updated", "bulk-update-summary"]);
    contains_all(&ok(env.aven(&db, ["show", &first])), &["priority=none"]);
    contains_all(&ok(env.aven(&db, ["show", &second])), &["priority=none"]);
    assert_eq!(
        scalar_i64(&db, "SELECT count(*) FROM changes WHERE field = 'priority'",),
        priority_changes_before
    );
}

#[test]
fn bulk_update_sets_status_by_project_and_status() {
    let env = TestEnv::new();
    let db = env.db("bulk-update-status.sqlite");
    ok(env.aven(&db, ["project", "create", "app"]));
    ok(env.aven(&db, ["project", "create", "ops"]));
    let app_todo = extract_ref(&ok(env.aven(&db, ["add", "app todo", "--project", "app"])));
    let app_inbox = extract_ref(&ok(env.aven(&db, ["add", "app inbox", "--project", "app"])));
    let ops_todo = extract_ref(&ok(env.aven(&db, ["add", "ops todo", "--project", "ops"])));
    ok(env.aven(&db, ["edit", &app_todo, "--status", "todo"]));
    ok(env.aven(&db, ["edit", &ops_todo, "--status", "todo"]));

    let updated = ok(env.aven(
        &db,
        [
            "bulk-update",
            "--project",
            "app",
            "--status",
            "todo",
            "--set-status",
            "active",
        ],
    ));
    contains_all(
        &updated,
        &["bulk-update-summary matched=1 changed=1 would_change=1 unchanged=0 dry_run=no"],
    );
    contains_all(&ok(env.aven(&db, ["show", &app_todo])), &["status=active"]);
    contains_all(&ok(env.aven(&db, ["show", &app_inbox])), &["status=inbox"]);
    contains_all(&ok(env.aven(&db, ["show", &ops_todo])), &["status=todo"]);
}

#[test]
fn bulk_update_selectors_include_future_tasks() {
    let env = TestEnv::new();
    let db = env.db("bulk-update-future.sqlite");
    let open = extract_ref(&ok(env.aven(
        &db,
        [
            "add",
            "future open",
            "--project",
            "app",
            "--available-at",
            "2099-01-01T00:00:00Z",
        ],
    )));
    let terminal = extract_ref(&ok(env.aven(
        &db,
        [
            "add",
            "future terminal",
            "--project",
            "app",
            "--available-at",
            "2099-01-02T00:00:00Z",
        ],
    )));
    ok(env.aven(&db, ["edit", &terminal, "--status", "done"]));
    let deleted = extract_ref(&ok(env.aven(
        &db,
        [
            "add",
            "future deleted",
            "--project",
            "app",
            "--available-at",
            "2099-01-03T00:00:00Z",
        ],
    )));
    ok(env.aven(&db, ["delete", &deleted]));

    let all = ok(env.aven(&db, ["bulk-update", "--all", "--set-priority", "high"]));
    contains_all(
        &all,
        &["bulk-update-summary matched=2 changed=2 would_change=2 unchanged=0 dry_run=no"],
    );
    contains_all(&ok(env.aven(&db, ["show", &open])), &["priority=high"]);
    contains_all(&ok(env.aven(&db, ["show", &terminal])), &["priority=high"]);
    contains_all(&ok(env.aven(&db, ["show", &deleted])), &["priority=none"]);

    let terminal_only = ok(env.aven(
        &db,
        [
            "bulk-update",
            "--status",
            "done",
            "--set-priority",
            "urgent",
        ],
    ));
    contains_all(
        &terminal_only,
        &["bulk-update-summary matched=1 changed=1 would_change=1 unchanged=0 dry_run=no"],
    );
    contains_all(
        &ok(env.aven(&db, ["show", &terminal])),
        &["priority=urgent"],
    );

    let including_deleted = ok(env.aven(
        &db,
        [
            "bulk-update",
            "--all",
            "--include-deleted",
            "--set-priority",
            "medium",
        ],
    ));
    contains_all(
        &including_deleted,
        &["bulk-update-summary matched=3 changed=3 would_change=3 unchanged=0 dry_run=no"],
    );
    contains_all(&ok(env.aven(&db, ["show", &deleted])), &["priority=medium"]);
}

#[test]
fn bulk_update_requires_selector_and_mutation() {
    let env = TestEnv::new();
    let db = env.db("bulk-update-guards.sqlite");
    contains_all(
        &fail(env.aven(&db, ["bulk-update", "--set-status", "done"])),
        &["error bulk-update-requires-selector"],
    );
    contains_all(
        &fail(env.aven(&db, ["bulk-update", "--all"])),
        &["error bulk-update-requires-mutation"],
    );
}

#[test]
fn bulk_update_all_excludes_deleted_unless_requested() {
    let env = TestEnv::new();
    let db = env.db("bulk-update-deleted.sqlite");
    let live = extract_ref(&ok(env.aven(&db, ["add", "live", "--project", "app"])));
    let deleted = extract_ref(&ok(env.aven(&db, ["add", "deleted", "--project", "app"])));
    ok(env.aven(&db, ["delete", &deleted]));

    let updated = ok(env.aven(&db, ["bulk-update", "--all", "--set-priority", "high"]));
    contains_all(
        &updated,
        &["bulk-update-summary matched=1 changed=1 would_change=1 unchanged=0 dry_run=no"],
    );
    contains_all(&ok(env.aven(&db, ["show", &live])), &["priority=high"]);
    contains_all(&ok(env.aven(&db, ["show", &deleted])), &["priority=none"]);

    let included = ok(env.aven(
        &db,
        [
            "bulk-update",
            "--all",
            "--include-deleted",
            "--set-priority",
            "urgent",
        ],
    ));
    contains_all(
        &included,
        &["bulk-update-summary matched=2 changed=2 would_change=2 unchanged=0 dry_run=no"],
    );
    contains_all(&ok(env.aven(&db, ["show", &deleted])), &["priority=urgent"]);
}

#[test]
fn bulk_update_rejects_contradictory_label_mutation() {
    let env = TestEnv::new();
    let db = env.db("bulk-update-label-conflict.sqlite");
    ok(env.aven(&db, ["label", "create", "bug"]));
    ok(env.aven(&db, ["add", "bug", "--project", "app", "--label", "bug"]));

    let error = fail(env.aven(
        &db,
        [
            "bulk-update",
            "--filter-label",
            "bug",
            "--label",
            "bug",
            "--remove-label",
            "bug",
        ],
    ));
    contains_all(&error, &["error bulk-update-label-conflict label=bug"]);
}

#[test]
fn bulk_update_ignores_duplicate_label_mutation_args() {
    let env = TestEnv::new();
    let db = env.db("bulk-update-duplicate-labels.sqlite");
    ok(env.aven(&db, ["label", "create", "bug"]));
    let task_ref = extract_ref(&ok(env.aven(&db, ["add", "task", "--project", "app"])));

    let updated = ok(env.aven(
        &db,
        ["bulk-update", "--all", "--label", "bug", "--label", "bug"],
    ));
    contains_all(
        &updated,
        &["bulk-update-summary matched=1 changed=1 would_change=1 unchanged=0 dry_run=no"],
    );
    let noop = ok(env.aven(
        &db,
        ["bulk-update", "--all", "--label", "bug", "--label", "bug"],
    ));
    contains_all(
        &noop,
        &["bulk-update-summary matched=1 changed=0 would_change=0 unchanged=1 dry_run=no"],
    );
    contains_all(&ok(env.aven(&db, ["show", &task_ref])), &["labels=bug"]);
}
