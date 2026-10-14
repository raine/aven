mod common;

use common::{TestEnv, contains_all, contains_none, extract_ref, fail, ok};

#[test]
fn epic_add_list_ready_and_remove_workflow() {
    let env = TestEnv::new();
    let db = env.db("epic-workflow.sqlite");

    let epic = extract_ref(&ok(
        env.aven(&db, ["add", "parent outcome", "--project", "app", "--epic"])
    ));
    let child = extract_ref(&ok(env.aven(&db, ["add", "child task", "--project", "app"])));
    let blocker = extract_ref(&ok(
        env.aven(&db, ["add", "true blocker", "--project", "app"])
    ));

    let linked = ok(env.aven(&db, ["epic", "add", &child, &epic]));
    contains_all(&linked, &["epic-added", "changed=yes", &child, &epic]);

    let epics = ok(env.aven(&db, ["list", "--epics"]));
    contains_all(&epics, &[&epic, "parent outcome"]);
    contains_none(&epics, &[&child, &blocker]);

    let children = ok(env.aven(&db, ["epic", "list", &epic]));
    contains_all(&children, &["children=1", &child, "child task"]);

    let ready = ok(env.aven(&db, ["list", "--ready"]));
    contains_all(&ready, &[&child, &blocker]);
    contains_none(&ready, &[&epic]);

    ok(env.aven(&db, ["dep", "add", &child, &blocker]));
    let ready_after_dep = ok(env.aven(&db, ["list", "--ready"]));
    contains_all(&ready_after_dep, &[&blocker]);
    contains_none(&ready_after_dep, &[&child, &epic]);

    let demote_error = fail(env.aven(&db, ["edit", &epic, "--epic", "off"]));
    contains_all(&demote_error, &["error epic-has-children"]);

    let removed = ok(env.aven(&db, ["epic", "remove", &child, &epic]));
    contains_all(&removed, &["epic-removed", "changed=yes"]);

    let children_after_remove = ok(env.aven(&db, ["epic", "list", &epic]));
    contains_all(&children_after_remove, &["children=0"]);

    let demoted = ok(env.aven(&db, ["edit", &epic, "--epic", "off"]));
    contains_all(&demoted, &["updated", "changed=yes"]);

    let epics_after_demote = ok(env.aven(&db, ["list", "--epics"]));
    contains_none(&epics_after_demote, &[&epic]);
}

#[test]
fn epic_json_surfaces_include_state_and_links() {
    let env = TestEnv::new();
    let db = env.db("epic-json.sqlite");

    let epic = extract_ref(&ok(
        env.aven(&db, ["add", "json parent", "--project", "app", "--epic"])
    ));
    let child = extract_ref(&ok(env.aven(&db, ["add", "json child", "--project", "app"])));
    ok(env.aven(&db, ["epic", "add", &child, &epic]));

    let json = |args: &[&str]| -> serde_json::Value {
        serde_json::from_str(&ok(env.aven(&db, args.iter().copied()))).unwrap()
    };
    let list = json(&["list", "--epics", "--json"]);
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["ref"], epic);
    assert_eq!(list[0]["is_epic"], true);
    assert_eq!(list[0].get("epic_parent"), Some(&serde_json::Value::Null));
    assert_eq!(list[0]["epic_children"].as_array().unwrap().len(), 1);
    assert_eq!(list[0]["epic_children"][0]["ref"], child);
    assert_eq!(list[0]["epic_children"][0]["title"], "json child");

    let shown = json(&["show", &child, "--json"]);
    let context = json(&["context", &child, "--json"]);
    for (state, parent) in [
        (&shown["is_epic"], &shown["epic_parent"]),
        (&context["task"]["is_epic"], &context["epic_parent"]),
    ] {
        assert_eq!(state, false);
        assert_eq!(parent["ref"], epic);
        assert_eq!(parent["title"], "json parent");
    }

    let children = json(&["epic", "list", &epic, "--json"]);
    assert_eq!(children["epic"]["ref"], epic);
    assert_eq!(children["epic"]["is_epic"], true);
    assert_eq!(children["children"].as_array().unwrap().len(), 1);
    assert_eq!(children["children"][0]["ref"], child);
    assert_eq!(children["children"][0]["title"], "json child");
}

#[test]
fn list_ready_and_epics_are_mutually_exclusive() {
    let env = TestEnv::new();
    let db = env.db("epic-ready-conflict.sqlite");

    let error = fail(env.aven(&db, ["list", "--ready", "--epics"]));

    contains_all(&error, &["error list-epic-ready-conflict"]);
}

#[test]
fn epic_rejects_self_cross_project_nested_and_second_parent() {
    let env = TestEnv::new();
    let db = env.db("epic-rejections.sqlite");

    let epic = extract_ref(&ok(
        env.aven(&db, ["add", "epic", "--project", "app", "--epic"])
    ));
    let second_epic = extract_ref(&ok(
        env.aven(&db, ["add", "second epic", "--project", "app", "--epic"])
    ));
    let child = extract_ref(&ok(env.aven(&db, ["add", "child", "--project", "app"])));
    let other_project_child = extract_ref(&ok(
        env.aven(&db, ["add", "other project child", "--project", "other"])
    ));

    let self_error = fail(env.aven(&db, ["epic", "add", &epic, &epic]));
    contains_all(&self_error, &["error epic-self"]);

    let nested_error = fail(env.aven(&db, ["epic", "add", &second_epic, &epic]));
    contains_all(&nested_error, &["error epic-child-is-epic"]);

    let cross_project_error = fail(env.aven(&db, ["epic", "add", &other_project_child, &epic]));
    contains_all(&cross_project_error, &["error epic-cross-project"]);

    ok(env.aven(&db, ["epic", "add", &child, &epic]));
    let second_parent_error = fail(env.aven(&db, ["epic", "add", &child, &second_epic]));
    contains_all(&second_parent_error, &["error epic-child-already-linked"]);
}
