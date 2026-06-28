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
fn renames_project_and_display_prefix() {
    let env = TestEnv::new();
    let db = env.db("rename-project.sqlite");
    let created = ok(env.aven(
        &db,
        ["add", "move project metadata", "--project", "agent-offload"],
    ));
    let task_ref = extract_ref(&created);
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
fn bulk_update_sets_status_by_project_and_status() {
    let env = TestEnv::new();
    let db = env.db("bulk-update-status.sqlite");
    ok(env.aven(&db, ["project", "create", "app"]));
    ok(env.aven(&db, ["project", "create", "ops"]));
    let app_todo = extract_ref(&ok(env.aven(&db, ["add", "app todo", "--project", "app"])));
    let app_inbox = extract_ref(&ok(env.aven(&db, ["add", "app inbox", "--project", "app"])));
    let ops_todo = extract_ref(&ok(env.aven(&db, ["add", "ops todo", "--project", "ops"])));
    ok(env.aven(&db, ["update", &app_todo, "--status", "todo"]));
    ok(env.aven(&db, ["update", &ops_todo, "--status", "todo"]));

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
