mod common;

use common::{TestEnv, TestServer, contains_all, contains_none, extract_ref, fail, ok};

fn sync(env: &TestEnv, db: &std::path::Path, server: &TestServer) {
    ok(env.atm(db, ["sync", "--server", &server.url]));
}

fn synced_task(
    env: &TestEnv,
    server: &TestServer,
    title: &str,
) -> (std::path::PathBuf, std::path::PathBuf, String) {
    let a = env.db("client-a.sqlite");
    let b = env.db("client-b.sqlite");
    let task_ref = extract_ref(&ok(env.atm(&a, ["add", title, "--project", "app"])));
    sync(env, &a, server);
    sync(env, &b, server);
    (a, b, task_ref)
}

fn title_conflict(
    env: &TestEnv,
    server: &TestServer,
) -> (std::path::PathBuf, std::path::PathBuf, String) {
    let (a, b, task_ref) = synced_task(env, server, "conflict base");
    ok(env.atm(&a, ["update", &task_ref, "--title", "title from a"]));
    ok(env.atm(&b, ["update", &task_ref, "--title", "title from b"]));
    sync(env, &a, server);
    sync(env, &b, server);
    sync(env, &a, server);
    (a, b, task_ref)
}

#[test]
fn same_field_edit_creates_conflict() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let (a, _, task_ref) = title_conflict(&env, &server);

    let conflicts = ok(env.atm(&a, ["conflict", "list"]));
    contains_all(&conflicts, &[&task_ref, "conflict field=title"]);
    let listed = ok(env.atm(&a, ["list", "--all"]));
    contains_all(&listed, &[&task_ref, "conflicts=yes"]);
}

#[test]
fn conflicted_field_is_protected_but_other_fields_work() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let (a, _, task_ref) = title_conflict(&env, &server);

    let error = fail(env.atm(&a, ["update", &task_ref, "--title", "should fail"]));
    contains_all(&error, &["error conflicted-field", "field=title"]);

    ok(env.atm(&a, ["update", &task_ref, "--priority", "urgent"]));
    let shown = ok(env.atm(&a, ["show", &task_ref]));
    contains_all(&shown, &["priority=urgent", "conflicts=yes"]);
}

#[test]
fn resolve_conflict_by_variant_syncs() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let (a, b, task_ref) = title_conflict(&env, &server);

    let shown = ok(env.atm(&a, ["conflict", "show", &task_ref, "--field", "title"]));
    let token = shown
        .lines()
        .find_map(|line| line.strip_prefix("variant "))
        .and_then(|line| line.split_whitespace().next())
        .expect("variant token");

    ok(env.atm(
        &a,
        ["conflict", "resolve", &task_ref, "title", "--use", token],
    ));
    sync(&env, &a, &server);
    sync(&env, &b, &server);

    let conflicts_a = ok(env.atm(&a, ["conflict", "list"]));
    let conflicts_b = ok(env.atm(&b, ["conflict", "list"]));
    contains_none(&conflicts_a, &[&task_ref]);
    contains_none(&conflicts_b, &[&task_ref]);
}

#[test]
fn resolve_conflict_by_stdin_syncs() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let (a, b, task_ref) = synced_task(&env, &server, "description conflict");

    ok(env.atm(
        &a,
        ["update", &task_ref, "--description", "description from a"],
    ));
    ok(env.atm(
        &b,
        ["update", &task_ref, "--description", "description from b"],
    ));
    sync(&env, &a, &server);
    sync(&env, &b, &server);

    ok(env.atm_stdin(
        &b,
        [
            "conflict",
            "resolve",
            &task_ref,
            "description",
            "--value-stdin",
        ],
        "resolved description\n",
    ));
    sync(&env, &b, &server);
    sync(&env, &a, &server);

    let resolved = ok(env.atm(&a, ["show", &task_ref, "--full"]));
    contains_all(&resolved, &["resolved description"]);
    contains_none(&resolved, &["conflict ", "field=description"]);
}
