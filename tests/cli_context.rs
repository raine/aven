mod common;

use common::{
    TestEnv, contains_all, contains_none, extract_attachment_id, extract_ref, ok, png_bytes,
};
use serde_json::Value;

fn seed_context(env: &TestEnv, db: &std::path::Path) -> (String, String, String) {
    ok(env.aven(db, ["label", "create", "bug"]));
    let root = extract_ref(&ok(env.aven(db, ["add", "root", "--project", "app"])));
    let middle = extract_ref(&ok(env.aven(
        db,
        [
            "add",
            "middle",
            "--project",
            "app",
            "--label",
            "bug",
            "--description",
            "details",
            "--available-at",
            "2000-01-01T00:00:00Z",
            "--due",
            "2099-01-01",
        ],
    )));
    let leaf = extract_ref(&ok(env.aven(db, ["add", "leaf", "--project", "app"])));
    ok(env.aven(db, ["dep", "add", &middle, &root]));
    ok(env.aven(db, ["dep", "add", &leaf, &middle]));
    ok(env.aven(db, ["note", &middle, "note body"]));
    (root, middle, leaf)
}

#[test]
fn context_json_contains_structured_snapshot() {
    let env = TestEnv::new();
    let db = env.db("context-json.sqlite");
    let (root, middle, leaf) = seed_context(&env, &db);

    let output = ok(env.aven(&db, ["context", &middle, "--json"]));
    let value: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(value["task"]["display_ref"], middle);
    assert_eq!(value["task"]["title"], "middle");
    assert_eq!(value["task"]["description"], "details");
    assert_eq!(value["task"]["available_at"], "2000-01-01T00:00:00Z");
    assert_eq!(value["task"]["due_on"], "2099-01-01");
    assert_eq!(value["task"]["deleted"], false);
    assert_eq!(value["project"]["key"], "app");
    assert_eq!(value["project"]["name"], "app");
    assert_eq!(value["workspace"]["key"], "default");
    assert_eq!(value["labels"], serde_json::json!(["bug"]));
    assert_eq!(value["dependencies"]["depends_on_total"], 1);
    assert_eq!(value["dependencies"]["blocks_total"], 1);
    assert_eq!(value["dependencies"]["depends_on"][0]["display_ref"], root);
    assert!(
        value["dependencies"]["depends_on"][0]["created_at"]
            .as_str()
            .unwrap()
            .len()
            >= 3
    );
    assert_eq!(value["dependencies"]["blocks"][0]["display_ref"], leaf);
    assert!(
        value["dependencies"]["blocks"][0]["created_at"]
            .as_str()
            .unwrap()
            .len()
            >= 3
    );
    let note_id = value["notes"][0]["id"].as_str().unwrap();
    assert!(note_id.len() >= 3);
    let text = ok(env.aven(&db, ["context", &middle]));
    contains_all(&text, &[&format!("note id={note_id} created=")]);
    assert_eq!(value["has_conflicts"], false);
    assert_eq!(value["is_blocked"], true);
    assert_eq!(value["has_open_dependents"], true);
    assert!(value["conflicts"].as_array().unwrap().is_empty());

    ok(env.aven(&db, ["delete", &middle]));
    let deleted = ok(env.aven(&db, ["context", &middle, "--json"]));
    let deleted: Value = serde_json::from_str(&deleted).unwrap();
    assert_eq!(deleted["task"]["deleted"], true);
}

#[test]
fn context_includes_attachment_metadata() {
    let env = TestEnv::new();
    let db = env.db("context-attachments.sqlite");
    let created = ok(env.aven(&db, ["add", "context attach", "--project", "app"]));
    let task_ref = extract_ref(&created);
    let image = env.path("photo.png");
    std::fs::write(&image, png_bytes(3, 2)).unwrap();
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
    let attachment_id = extract_attachment_id(&added);

    let text = ok(env.aven(&db, ["context", &task_ref]));
    contains_all(&text, &["attachment attachment_id=", "has_blob=yes"]);
    contains_none(
        &text,
        &["sha256=", "filename=", "alt_text=", "photo.png", "diagram"],
    );

    let json = ok(env.aven(&db, ["context", &task_ref, "--json"]));
    let value: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["attachments"][0]["attachment_id"], attachment_id);
    assert_eq!(value["attachments"][0]["has_blob"], true);
    assert!(value["attachments"][0]["deleted_at"].is_null());
    assert!(value["attachments"][0].get("sha256").is_none());
    assert!(value["attachments"][0].get("bytes").is_none());
}
