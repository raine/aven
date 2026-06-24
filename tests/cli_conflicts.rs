mod common;

use common::{TestEnv, TestServer, contains_all, contains_none, extract_ref, fail, ok};

fn sync(env: &TestEnv, db: &std::path::Path, server: &TestServer) {
    ok(env.aven(db, ["sync", "--server", &server.url]));
}

fn synced_task(
    env: &TestEnv,
    server: &TestServer,
    title: &str,
) -> (std::path::PathBuf, std::path::PathBuf, String) {
    let a = env.db("client-a.sqlite");
    let b = env.db("client-b.sqlite");
    let task_ref = extract_ref(&ok(env.aven(&a, ["add", title, "--project", "app"])));
    sync(env, &a, server);
    sync(env, &b, server);
    (a, b, task_ref)
}

fn title_conflict(
    env: &TestEnv,
    server: &TestServer,
) -> (std::path::PathBuf, std::path::PathBuf, String) {
    let (a, b, task_ref) = synced_task(env, server, "conflict base");
    ok(env.aven(&a, ["update", &task_ref, "--title", "title from a"]));
    ok(env.aven(&b, ["update", &task_ref, "--title", "title from b"]));
    sync(env, &a, server);
    sync(env, &b, server);
    sync(env, &a, server);
    (a, b, task_ref)
}

fn deleted_conflict(
    env: &TestEnv,
    server: &TestServer,
) -> (std::path::PathBuf, std::path::PathBuf, String) {
    let (a, b, task_ref) = synced_task(env, server, "deleted conflict");
    ok(env.aven(&b, ["delete", &task_ref]));
    ok(env.aven(&a, ["restore", &task_ref]));
    sync(env, &b, server);
    sync(env, &a, server);
    (a, b, task_ref)
}

#[test]
fn same_field_edit_creates_conflict() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let (a, _, task_ref) = title_conflict(&env, &server);

    let conflicts = ok(env.aven(&a, ["conflict", "list"]));
    contains_all(&conflicts, &[&task_ref, "conflict field=title"]);
    let listed = ok(env.aven(&a, ["list", "--all"]));
    contains_all(&listed, &[&task_ref, "conflicts=yes"]);
}

#[test]
fn conflicted_field_is_protected_but_other_fields_work() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let (a, _, task_ref) = title_conflict(&env, &server);

    let error = fail(env.aven(&a, ["update", &task_ref, "--title", "should fail"]));
    contains_all(&error, &["error conflicted-field", "field=title"]);

    ok(env.aven(&a, ["update", &task_ref, "--priority", "urgent"]));
    let shown = ok(env.aven(&a, ["show", &task_ref]));
    contains_all(&shown, &["priority=urgent", "conflicts=yes"]);
}

#[test]
fn resolve_conflict_by_variant_syncs() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let (a, b, task_ref) = title_conflict(&env, &server);

    let shown = ok(env.aven(&a, ["conflict", "show", &task_ref, "--field", "title"]));
    contains_all(&shown, &["conflict", "value<<EOF"]);
    let token = shown
        .lines()
        .find_map(|line| line.strip_prefix("variant "))
        .and_then(|line| line.split_whitespace().next())
        .expect("variant token");

    ok(env.aven(
        &a,
        ["conflict", "resolve", &task_ref, "title", "--use", token],
    ));
    sync(&env, &a, &server);
    sync(&env, &b, &server);

    let conflicts_a = ok(env.aven(&a, ["conflict", "list"]));
    let conflicts_b = ok(env.aven(&b, ["conflict", "list"]));
    contains_none(&conflicts_a, &[&task_ref]);
    contains_none(&conflicts_b, &[&task_ref]);
}

#[test]
fn conflict_export_and_diff_write_variant_files() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let (a, b, task_ref) = synced_task(&env, &server, "description conflict export");
    ok(env.aven(
        &a,
        ["update", &task_ref, "--description", "description from a\n"],
    ));
    ok(env.aven(
        &b,
        ["update", &task_ref, "--description", "description from b\n"],
    ));
    sync(&env, &a, &server);
    sync(&env, &b, &server);
    sync(&env, &a, &server);

    let diff = ok(env.aven(&a, ["conflict", "diff", &task_ref, "description"]));
    contains_all(&diff, &["---", "+++", "description from"]);

    let dir = env.path("conflicts");
    let exported = ok(env.aven(
        &a,
        [
            "conflict",
            "export",
            &task_ref,
            "description",
            "--dir",
            dir.to_str().unwrap(),
        ],
    ));
    contains_all(&exported, &["exported variant=", "path="]);

    let mut names = Vec::new();
    let mut bodies = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        names.push(path.file_name().unwrap().to_string_lossy().to_string());
        bodies.push(std::fs::read_to_string(path).unwrap());
    }
    names.sort();
    bodies.sort();
    assert!(names.iter().all(|name| name.starts_with("description-v")));
    assert!(names.iter().all(|name| name.ends_with(".md")));
    assert_eq!(bodies, vec!["description from a\n", "description from b\n"]);
}

#[test]
fn resolve_conflict_by_stdin_syncs() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let (a, b, task_ref) = synced_task(&env, &server, "description conflict");

    ok(env.aven(
        &a,
        ["update", &task_ref, "--description", "description from a"],
    ));
    ok(env.aven(
        &b,
        ["update", &task_ref, "--description", "description from b"],
    ));
    sync(&env, &a, &server);
    sync(&env, &b, &server);

    ok(env.aven_stdin(
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

    let resolved = ok(env.aven(&a, ["show", &task_ref, "--full"]));
    contains_all(&resolved, &["resolved description"]);
    contains_none(&resolved, &["conflict ", "field=description"]);
}

#[test]
fn invalid_deleted_conflict_resolution_preserves_task_and_change_log() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let (a, _, task_ref) = deleted_conflict(&env, &server);
    let changes_before = common::scalar_i64(&a, "SELECT count(*) FROM changes");

    let error = fail(env.aven(
        &a,
        [
            "conflict", "resolve", &task_ref, "deleted", "--value", "true",
        ],
    ));

    contains_all(&error, &["error invalid-deleted"]);
    assert_eq!(
        common::scalar_i64(&a, "SELECT count(*) FROM tasks WHERE deleted = 1"),
        0
    );
    assert_eq!(
        common::scalar_i64(&a, "SELECT count(*) FROM changes"),
        changes_before
    );
    let conflicts = ok(env.aven(&a, ["conflict", "list"]));
    contains_all(&conflicts, &[&task_ref, "conflict field=deleted"]);
}
