mod common;

use common::{TestEnv, contains_all, contains_none, extract_ref, fail, ok};

#[test]
fn dependency_list_json_returns_structured_summary() {
    let env = TestEnv::new();
    let db = env.db("dep-list-json.sqlite");

    let root = extract_ref(&ok(env.aven(&db, ["add", "root task", "--project", "app"])));
    let blocked = extract_ref(&ok(
        env.aven(&db, ["add", "blocked task", "--project", "app"])
    ));
    ok(env.aven(&db, ["dep", "add", &blocked, &root]));

    let output = ok(env.aven(&db, ["dep", "list", &blocked, "--json"]));
    let summary: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(summary["depends_on_open"], 1);
    assert_eq!(summary["depends_on_total"], 1);
    assert_eq!(summary["blocks_open"], 0);
    assert_eq!(summary["blocks_total"], 0);
    assert_eq!(summary["depends_on"].as_array().unwrap().len(), 1);
    assert_eq!(summary["depends_on"][0]["ref"], root);
    assert_eq!(summary["depends_on"][0]["title"], "root task");
    assert_eq!(summary["blocks"].as_array().unwrap().len(), 0);
}

#[test]
fn dependency_add_list_ready_blocked_and_remove_workflow() {
    let env = TestEnv::new();
    let db = env.db("dependency-workflow.sqlite");

    let root = extract_ref(&ok(env.aven(&db, ["add", "root task", "--project", "app"])));
    let blocked = extract_ref(&ok(
        env.aven(&db, ["add", "blocked task", "--project", "app"])
    ));
    let blocked_by_blocked = extract_ref(&ok(
        env.aven(&db, ["add", "blocked by blocked", "--project", "app"])
    ));
    let ready_task = extract_ref(&ok(env.aven(&db, ["add", "free task", "--project", "app"])));

    ok(env.aven(&db, ["dep", "add", &blocked, &root]));
    ok(env.aven(&db, ["dep", "add", &blocked_by_blocked, &blocked]));

    let dep_list = ok(env.aven(&db, ["dep", "list", &blocked]));
    contains_all(
        &dep_list,
        &["depends_on open=1 total=1", "blocks open=1 total=1", &root],
    );

    let ready = ok(env.aven(&db, ["list", "--ready"]));
    contains_all(&ready, &[&root, &ready_task]);
    contains_none(&ready, &[&blocked, &blocked_by_blocked]);

    let blocked_list = ok(env.aven(&db, ["list", "--blocked"]));
    contains_all(&blocked_list, &[&blocked, &blocked_by_blocked]);
    contains_none(&blocked_list, &[&root, &ready_task]);

    let removed = ok(env.aven(&db, ["dep", "remove", &blocked_by_blocked, &blocked]));
    contains_all(&removed, &["dependency-removed", "changed=yes"]);

    let blocked_list_after_remove = ok(env.aven(&db, ["list", "--blocked"]));
    contains_none(&blocked_list_after_remove, &[&blocked_by_blocked]);
    contains_all(&blocked_list_after_remove, &[&blocked]);

    let ready_after_remove = ok(env.aven(&db, ["list", "--ready"]));
    contains_all(
        &ready_after_remove,
        &[&blocked_by_blocked, &root, &ready_task],
    );
}

#[test]
fn dependency_self_and_cycle_rejections() {
    let env = TestEnv::new();
    let db = env.db("dependency-cycles.sqlite");

    let task_a = extract_ref(&ok(env.aven(&db, ["add", "task A", "--project", "app"])));
    let task_b = extract_ref(&ok(env.aven(&db, ["add", "task B", "--project", "app"])));

    let self_error = fail(env.aven(&db, ["dep", "add", &task_a, &task_a]));
    contains_all(&self_error, &["error dependency-self", "task_id="]);

    ok(env.aven(&db, ["dep", "add", &task_b, &task_a]));
    let cycle_error = fail(env.aven(&db, ["dep", "add", &task_a, &task_b]));
    contains_all(&cycle_error, &["error dependency-cycle"]);
}

#[test]
fn dependency_filter_ignores_done_canceled_and_deleted_blockers() {
    let env = TestEnv::new();
    let db = env.db("dependency-state-filtering.sqlite");

    let done_blocker = extract_ref(&ok(
        env.aven(&db, ["add", "done blocker", "--project", "app"])
    ));
    let done_blocked = extract_ref(&ok(
        env.aven(&db, ["add", "blocked by done", "--project", "app"])
    ));
    let canceled_blocker = extract_ref(&ok(
        env.aven(&db, ["add", "canceled blocker", "--project", "app"])
    ));
    let canceled_blocked = extract_ref(&ok(
        env.aven(&db, ["add", "blocked by canceled", "--project", "app"])
    ));
    let deleted_blocker = extract_ref(&ok(
        env.aven(&db, ["add", "deleted blocker", "--project", "app"])
    ));
    let deleted_blocked = extract_ref(&ok(
        env.aven(&db, ["add", "blocked by deleted", "--project", "app"])
    ));

    ok(env.aven(&db, ["update", &done_blocker, "--status", "done"]));
    ok(env.aven(&db, ["update", &canceled_blocker, "--status", "canceled"]));
    ok(env.aven(&db, ["delete", &deleted_blocker]));

    ok(env.aven(&db, ["dep", "add", &done_blocked, &done_blocker]));
    ok(env.aven(&db, ["dep", "add", &canceled_blocked, &canceled_blocker]));
    ok(env.aven(&db, ["dep", "add", &deleted_blocked, &deleted_blocker]));

    let ready = ok(env.aven(&db, ["list", "--ready"]));
    contains_all(
        &ready,
        &[&done_blocked, &canceled_blocked, &deleted_blocked],
    );

    let blocked = ok(env.aven(&db, ["list", "--blocked"]));
    contains_none(
        &blocked,
        &[&done_blocked, &canceled_blocked, &deleted_blocked],
    );

    let all = ok(env.aven(&db, ["list", "--all"]));
    contains_none(&all, &["blocks=1"]);

    let done_dep_list = ok(env.aven(&db, ["dep", "list", &done_blocker]));
    contains_all(&done_dep_list, &["blocks open=0 total=1"]);
}

#[test]
fn dependency_show_full_prints_dependents_sections() {
    let env = TestEnv::new();
    let db = env.db("dependency-show-full.sqlite");

    let root = extract_ref(&ok(env.aven(&db, ["add", "dep-root", "--project", "app"])));
    let middle = extract_ref(&ok(env.aven(&db, ["add", "dep-middle", "--project", "app"])));
    let leaf = extract_ref(&ok(env.aven(&db, ["add", "dep-leaf", "--project", "app"])));

    ok(env.aven(&db, ["dep", "add", &middle, &root]));
    ok(env.aven(&db, ["dep", "add", &leaf, &middle]));

    let full = ok(env.aven(&db, ["show", &middle, "--full"]));
    contains_all(
        &full,
        &[
            "depends_on open=1 total=1",
            "blocks open=1 total=1",
            &root,
            &leaf,
        ],
    );
}

#[test]
fn dependency_list_filters_enforce_conflicts() {
    let env = TestEnv::new();
    let db = env.db("dependency-filter-errors.sqlite");

    let _ = extract_ref(&ok(
        env.aven(&db, ["add", "filter task", "--project", "app"])
    ));

    let conflict_error = fail(env.aven(&db, ["list", "--ready", "--blocked"]));
    contains_all(&conflict_error, &["error list-dependency-filter-conflict"]);

    let all_error = fail(env.aven(&db, ["list", "--ready", "--all"]));
    contains_all(&all_error, &["error list-dependency-filter-all-conflict"]);
}
