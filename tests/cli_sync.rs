mod common;

use common::{TestEnv, TestServer, contains_all, contains_none, extract_ref, ok};

fn sync(env: &TestEnv, db: &std::path::Path, server: &TestServer) {
    let output = ok(env.atm(db, ["sync", "--server", &server.url]));
    contains_all(&output, &["synced", "cursor="]);
}

#[test]
fn offline_creates_converge() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    assert!(
        server.url.starts_with("http://127.0.0.1:"),
        "unexpected server url: {}",
        server.url
    );
    let a = env.db("client-a.sqlite");
    let b = env.db("client-b.sqlite");

    let a_ref = extract_ref(&ok(
        env.atm(&a, ["add", "offline from a", "--project", "app"])
    ));
    let b_ref = extract_ref(&ok(
        env.atm(&b, ["add", "offline from b", "--project", "app"])
    ));

    sync(&env, &a, &server);
    sync(&env, &b, &server);
    sync(&env, &a, &server);

    let list_a = ok(env.atm(&a, ["list", "--all"]));
    let list_b = ok(env.atm(&b, ["list", "--all"]));
    contains_all(
        &list_a,
        &[&a_ref, &b_ref, "offline from a", "offline from b"],
    );
    contains_all(
        &list_b,
        &[&a_ref, &b_ref, "offline from a", "offline from b"],
    );
}

#[test]
fn independent_field_edits_converge() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let a = env.db("client-a.sqlite");
    let b = env.db("client-b.sqlite");

    let task_ref = extract_ref(&ok(env.atm(&a, ["add", "merge fields", "--project", "app"])));
    sync(&env, &a, &server);
    sync(&env, &b, &server);

    ok(env.atm(&a, ["update", &task_ref, "--status", "active"]));
    ok(env.atm(&b, ["update", &task_ref, "--priority", "high"]));
    sync(&env, &a, &server);
    sync(&env, &b, &server);
    sync(&env, &a, &server);

    let merged = ok(env.atm(&a, ["show", &task_ref]));
    contains_all(&merged, &["status=active", "priority=high"]);
    contains_none(&merged, &["conflicts=yes"]);
}

#[test]
fn notes_and_labels_converge() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let a = env.db("client-a.sqlite");
    let b = env.db("client-b.sqlite");

    for label in ["docs", "sync", "bug"] {
        ok(env.atm(&a, ["label", "create", label]));
    }
    let task_ref = extract_ref(&ok(
        env.atm(&a, ["add", "merge notes and labels", "--project", "app"])
    ));
    sync(&env, &a, &server);
    sync(&env, &b, &server);

    ok(env.atm_stdin(&a, ["note", &task_ref, "--stdin"], "note a\n"));
    ok(env.atm_stdin(&b, ["note", &task_ref, "--stdin"], "note b\n"));
    ok(env.atm(&a, ["update", &task_ref, "--label", "docs"]));
    ok(env.atm(&b, ["update", &task_ref, "--label", "sync"]));
    sync(&env, &a, &server);
    sync(&env, &b, &server);
    sync(&env, &a, &server);

    let full = ok(env.atm(&a, ["show", &task_ref, "--full"]));
    contains_all(&full, &["note a", "note b", "labels=docs,sync"]);

    ok(env.atm(&a, ["update", &task_ref, "--remove-label", "docs"]));
    ok(env.atm(&b, ["update", &task_ref, "--label", "bug"]));
    sync(&env, &a, &server);
    sync(&env, &b, &server);
    sync(&env, &a, &server);

    let labels = ok(env.atm(&a, ["show", &task_ref]));
    contains_all(&labels, &["labels=bug,sync"]);
    contains_none(&labels, &["labels=docs"]);
}

#[test]
fn soft_delete_syncs_and_restores() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let a = env.db("client-a.sqlite");
    let b = env.db("client-b.sqlite");

    let task_ref = extract_ref(&ok(
        env.atm(&a, ["add", "temporary task", "--project", "app"])
    ));
    sync(&env, &a, &server);
    sync(&env, &b, &server);

    ok(env.atm(&a, ["delete", &task_ref]));
    sync(&env, &a, &server);
    sync(&env, &b, &server);

    let normal_b = ok(env.atm(&b, ["list"]));
    contains_none(&normal_b, &[&task_ref, "temporary task"]);
    let all_b = ok(env.atm(&b, ["list", "--all"]));
    contains_all(&all_b, &[&task_ref, "temporary task", "deleted=yes"]);

    ok(env.atm(&a, ["restore", &task_ref]));
    sync(&env, &a, &server);
    sync(&env, &b, &server);

    let restored_b = ok(env.atm(&b, ["list"]));
    contains_all(&restored_b, &[&task_ref, "temporary task"]);
    contains_none(&restored_b, &["deleted=yes"]);
}
