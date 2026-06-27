mod common;

use common::{TestEnv, TestServer, contains_all, extract_ref, ok};
use serde_json::Value;

fn sync(env: &TestEnv, db: &std::path::Path, server: &TestServer) {
    ok(env.aven(db, ["sync", "--server", &server.url]));
}

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
        ],
    )));
    let leaf = extract_ref(&ok(env.aven(db, ["add", "leaf", "--project", "app"])));
    ok(env.aven(db, ["dep", "add", &middle, &root]));
    ok(env.aven(db, ["dep", "add", &leaf, &middle]));
    ok(env.aven(db, ["note", &middle, "note body"]));
    (root, middle, leaf)
}

#[test]
fn context_prints_text_snapshot() {
    let env = TestEnv::new();
    let db = env.db("context-text.sqlite");
    let (root, middle, leaf) = seed_context(&env, &db);

    let output = ok(env.aven(&db, ["context", &middle]));
    assert!(serde_json::from_str::<Value>(&output).is_err());
    contains_all(
        &output,
        &[
            "context ",
            &middle,
            "project=app",
            "name=\"app\"",
            "workspace=default",
            "blocked=yes",
            "conflicts=no",
            "blocks_open=yes",
            "labels=bug",
            "description<<EOF",
            "details",
            "depends_on open=1 total=1",
            "blocks open=1 total=1",
            &root,
            &leaf,
            "note created=",
            "body<<EOF",
            "note body",
        ],
    );
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
    assert!(value["notes"][0]["id"].as_str().unwrap().len() >= 3);
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
fn context_includes_unresolved_conflicts() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let a = env.db("context-conflict-a.sqlite");
    let b = env.db("context-conflict-b.sqlite");

    let task_ref = extract_ref(&ok(
        env.aven(&a, ["add", "conflict base", "--project", "app"])
    ));
    sync(&env, &a, &server);
    sync(&env, &b, &server);

    ok(env.aven(&a, ["update", &task_ref, "--title", "title from a"]));
    ok(env.aven(&b, ["update", &task_ref, "--title", "title from b"]));
    sync(&env, &a, &server);
    sync(&env, &b, &server);
    sync(&env, &a, &server);

    let text = ok(env.aven(&a, ["context", &task_ref]));
    contains_all(&text, &["conflict ", "field=title", "variant "]);

    let json = ok(env.aven(&a, ["context", &task_ref, "--json"]));
    let value: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["has_conflicts"], true);
    assert_eq!(value["conflicts"][0]["field"], "title");
    assert_eq!(
        value["conflicts"][0]["variants"].as_array().unwrap().len(),
        2
    );
    assert_eq!(
        value["conflicts"][0]["variants"][0]["value"],
        "title from a"
    );
}
