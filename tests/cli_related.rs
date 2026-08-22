mod common;

use common::{TestEnv, contains_all, contains_none, extract_ref, fail, ok};

#[test]
fn related_links_are_symmetric_idempotent_and_removable_from_either_side() {
    let env = TestEnv::new();
    let db = env.db("related-workflow.sqlite");
    let first = extract_ref(&ok(env.aven(&db, ["add", "first", "--project", "app"])));
    let second = extract_ref(&ok(env.aven(&db, ["add", "second", "--project", "app"])));

    contains_all(
        &ok(env.aven(&db, ["related", "add", &first, &second])),
        &["related-added", "changed=yes"],
    );
    contains_all(
        &ok(env.aven(&db, ["related", "add", &second, &first])),
        &["changed=no"],
    );
    contains_all(
        &ok(env.aven(&db, ["related", "list", &second])),
        &["Related", &first, "first"],
    );
    contains_all(
        &ok(env.aven(&db, ["related", "remove", &second, &first])),
        &["related-removed", "changed=yes"],
    );
    contains_none(&ok(env.aven(&db, ["related", "list", &first])), &[&second]);
}

#[test]
fn related_links_appear_in_full_detail_and_do_not_affect_readiness() {
    let env = TestEnv::new();
    let db = env.db("related-detail.sqlite");
    let first = extract_ref(&ok(env.aven(&db, ["add", "first", "--project", "app"])));
    let second = extract_ref(&ok(env.aven(&db, ["add", "second", "--project", "app"])));
    ok(env.aven(&db, ["related", "add", &first, &second]));

    let full: serde_json::Value =
        serde_json::from_str(&ok(env.aven(&db, ["show", &first, "--full", "--json"]))).unwrap();
    assert_eq!(full["related"][0]["display_ref"], second);
    let context: serde_json::Value =
        serde_json::from_str(&ok(env.aven(&db, ["context", &second, "--json"]))).unwrap();
    assert_eq!(context["related"][0]["display_ref"], first);
    let ready = ok(env.aven(&db, ["list", "--ready"]));
    contains_all(&ready, &[&first, &second]);
}

#[test]
fn related_links_survive_soft_delete_and_restore() {
    let env = TestEnv::new();
    let db = env.db("related-delete.sqlite");
    let first = extract_ref(&ok(env.aven(&db, ["add", "first", "--project", "app"])));
    let second = extract_ref(&ok(env.aven(&db, ["add", "second", "--project", "app"])));
    ok(env.aven(&db, ["related", "add", &first, &second]));
    ok(env.aven(&db, ["delete", &second]));
    let deleted: serde_json::Value =
        serde_json::from_str(&ok(env.aven(&db, ["related", "list", &first, "--json"]))).unwrap();
    assert_eq!(deleted[0]["deleted"], true);
    ok(env.aven(&db, ["restore", &second]));
    let restored: serde_json::Value =
        serde_json::from_str(&ok(env.aven(&db, ["related", "list", &first, "--json"]))).unwrap();
    assert_eq!(restored[0]["deleted"], false);
}

#[test]
fn related_export_uses_v3_and_round_trips_tombstone_state() {
    let env = TestEnv::new();
    let source = env.db("related-export-source.sqlite");
    let target = env.db("related-export-target.sqlite");
    let first = extract_ref(&ok(env.aven(&source, ["add", "first", "--project", "app"])));
    let second = extract_ref(&ok(env.aven(&source, ["add", "second", "--project", "app"])));
    ok(env.aven(&source, ["related", "add", &first, &second]));
    ok(env.aven(&source, ["related", "remove", &first, &second]));
    let export = env.path("related.json");
    ok(env.aven(&source, ["export", "--output", export.to_str().unwrap()]));
    let snapshot: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&export).unwrap()).unwrap();
    assert_eq!(snapshot["version"], 3);
    assert_eq!(snapshot["tables"]["task_related_links"][0]["linked"], 0);

    let mut invalid = snapshot.clone();
    let link = &mut invalid["tables"]["task_related_links"][0];
    let task_a = link["task_a_id"].take();
    link["task_a_id"] = link["task_b_id"].take();
    link["task_b_id"] = task_a;
    let invalid_export = env.path("related-invalid.json");
    std::fs::write(
        &invalid_export,
        serde_json::to_vec_pretty(&invalid).unwrap(),
    )
    .unwrap();
    contains_all(
        &fail(env.aven(
            &target,
            ["import", "--yes", invalid_export.to_str().unwrap()],
        )),
        &["invalid-export-snapshot", "related pair is not canonical"],
    );

    let mut mislabeled = snapshot.clone();
    mislabeled["version"] = serde_json::json!(2);
    let mislabeled_export = env.path("related-v2.json");
    std::fs::write(
        &mislabeled_export,
        serde_json::to_vec_pretty(&mislabeled).unwrap(),
    )
    .unwrap();
    contains_all(
        &fail(env.aven(
            &target,
            ["import", "--yes", mislabeled_export.to_str().unwrap()],
        )),
        &["invalid-export-snapshot", "related links require version 3"],
    );

    ok(env.aven(&target, ["import", "--yes", export.to_str().unwrap()]));
    contains_all(
        &ok(env.aven(&target, ["doctor", "--integrity"])),
        &["related link changes", "0 invalid"],
    );
}

#[test]
fn task_references_in_prose_do_not_create_related_links() {
    let env = TestEnv::new();
    let db = env.db("related-prose.sqlite");
    let referenced = extract_ref(&ok(env.aven(&db, ["add", "referenced", "--project", "app"])));
    let task = extract_ref(&ok(env.aven(
        &db,
        [
            "add",
            "mentions a task",
            "--project",
            "app",
            "--description",
            &format!("See {referenced} for context"),
        ],
    )));

    let links: serde_json::Value =
        serde_json::from_str(&ok(env.aven(&db, ["related", "list", &task, "--json"]))).unwrap();
    assert_eq!(links, serde_json::json!([]));
}

#[test]
fn related_self_link_is_rejected() {
    let env = TestEnv::new();
    let db = env.db("related-self.sqlite");
    let task = extract_ref(&ok(env.aven(&db, ["add", "task", "--project", "app"])));
    contains_all(
        &fail(env.aven(&db, ["related", "add", &task, &task])),
        &["error related-self"],
    );
}
