mod common;

use rusqlite::{Connection, params};

use common::{TestEnv, contains_all, extract_ref, fail, ok, suffix};

#[test]
fn short_ref_resolves_when_unambiguous() {
    let env = TestEnv::new();
    let db = env.db("short.sqlite");
    let created = ok(env.atm(&db, ["add", "short ref task", "--project", "app"]));
    let task_ref = extract_ref(&created);
    let short = &suffix(&task_ref)[..3];

    let shown = ok(env.atm(&db, ["show", short]));
    contains_all(&shown, &[&task_ref, "short ref task"]);
}

#[test]
fn qualified_ref_prefix_is_a_hint() {
    let env = TestEnv::new();
    let db = env.db("hint.sqlite");
    let original = extract_ref(&ok(env.atm(&db, ["add", "moving task", "--project", "app"])));
    let stale_ref = original.clone();
    let moved = extract_ref(&ok(
        env.atm(&db, ["update", &original, "--project", "homelab"])
    ));

    let shown = ok(env.atm(&db, ["show", &stale_ref]));
    contains_all(&shown, &[&moved, "moving task"]);
}

#[test]
fn ambiguous_ref_fails_with_choices() {
    let env = TestEnv::new();
    let db = env.db("ambiguous.sqlite");
    ok(env.atm(&db, ["project", "create", "ambig"]));

    let conn = Connection::open(&db).unwrap();
    for (id, title) in [
        ("7KQ1111111111111", "ambig one"),
        ("7KQ2222222222222", "ambig two"),
    ] {
        conn.execute(
            "INSERT INTO tasks(id,title,description,project_key,status,priority,created_at,updated_at)
             VALUES (?, ?, '', 'ambig', 'inbox', 'none', 't', 't')",
            params![id, title],
        )
        .unwrap();
    }

    let error = fail(env.atm(&db, ["show", "7KQ"]));
    contains_all(
        &error,
        &[
            "error ambiguous-ref",
            "match AMB-7KQ1111",
            "match AMB-7KQ2222",
            "retry with longer ref",
        ],
    );
}
