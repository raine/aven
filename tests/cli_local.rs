mod common;

use common::{TestEnv, command, contains_all, contains_none, extract_ref, fail, ok, suffix};

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
            "update",
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
    ok(env.aven(&db, ["update", &app_docs, "--status", "active"]));
    ok(env.aven(&db, ["update", &ops_sync, "--status", "done"]));

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
    contains_all(&all, &[&app_bug, "deleted=yes", "app bug"]);

    ok(env.aven(&db, ["restore", &app_bug]));
    let restored = ok(env.aven(&db, ["list"]));
    contains_all(&restored, &[&app_bug, "app bug"]);
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
