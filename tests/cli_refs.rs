mod common;

use std::path::PathBuf;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

use common::{TestEnv, contains_all, extract_ref, fail, insert_task_fixtures, ok, suffix};

async fn setup_display_ref_fixtures(
    db_name: &str,
    projects: &[&str],
    fixtures: &[(&str, &str, &str)],
) -> (TestEnv, PathBuf) {
    let env = TestEnv::new();
    let db = env.db(db_name);
    for project in projects {
        ok(env.aven(&db, ["project", "create", project]));
    }

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(SqliteConnectOptions::new().filename(&db))
        .await
        .unwrap();
    insert_task_fixtures(&pool, fixtures).await;

    (env, db)
}

fn first_tokens(output: &str) -> Vec<&str> {
    output
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .collect()
}

#[test]
fn short_ref_resolves_when_unambiguous() {
    let env = TestEnv::new();
    let db = env.db("short.sqlite");
    let created = ok(env.aven(&db, ["add", "short ref task", "--project", "app"]));
    let task_ref = extract_ref(&created);
    let short = &suffix(&task_ref)[..3];

    let shown = ok(env.aven(&db, ["show", short]));
    contains_all(&shown, &[&task_ref, "short ref task"]);
}

#[test]
fn qualified_ref_prefix_is_a_hint() {
    let env = TestEnv::new();
    let db = env.db("hint.sqlite");
    let original = extract_ref(&ok(
        env.aven(&db, ["add", "moving task", "--project", "app"])
    ));
    let stale_ref = original.clone();
    let moved = extract_ref(&ok(
        env.aven(&db, ["update", &original, "--project", "homelab"])
    ));

    let shown = ok(env.aven(&db, ["show", &stale_ref]));
    contains_all(&shown, &[&moved, "moving task"]);
}

#[tokio::test]
async fn display_refs_use_project_prefix_and_unique_suffix_floor() {
    let (env, db) = setup_display_ref_fixtures(
        "display-floor.sqlite",
        &["app", "ops"],
        &[
            ("W3ZX111111111111", "app shared", "app"),
            ("W3ZX222222222222", "ops shared", "ops"),
            ("A111111111111111", "short unique", "app"),
        ],
    )
    .await;

    let list = ok(env.aven(&db, ["list", "--all"]));
    assert_eq!(first_tokens(&list), ["APP-W3ZX1", "OPS-W3ZX2", "APP-A111"]);
}

#[tokio::test]
async fn displayed_refs_resolve_after_filtering() {
    let (env, db) = setup_display_ref_fixtures(
        "display-visible.sqlite",
        &["app", "ops"],
        &[
            ("W3ZX111111111111", "app shared", "app"),
            ("W3ZX222222222222", "ops shared", "ops"),
        ],
    )
    .await;

    let list = ok(env.aven(&db, ["list", "--project", "app"]));
    assert_eq!(first_tokens(&list), ["APP-W3ZX1"]);

    let shown = ok(env.aven(&db, ["show", "APP-W3ZX1"]));
    contains_all(&shown, &["APP-W3ZX1", "app shared"]);
}

#[tokio::test]
async fn ambiguous_ref_fails_with_choices() {
    let env = TestEnv::new();
    let db = env.db("ambiguous.sqlite");
    ok(env.aven(&db, ["project", "create", "ambig"]));

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(SqliteConnectOptions::new().filename(&db))
        .await
        .unwrap();
    for (id, title) in [
        ("7KQ1111111111111", "ambig one"),
        ("7KQ2222222222222", "ambig two"),
    ] {
        sqlx::query!(
            "INSERT INTO tasks(id,title,description,project_key,status,priority,created_at,updated_at)
             VALUES (?, ?, '', 'ambig', 'inbox', 'none', 't', 't')",
            id,
            title,
        )
        .execute(&pool)
        .await
        .unwrap();
    }

    let error = fail(env.aven(&db, ["show", "7KQ"]));
    contains_all(&error, &["error ambiguous-ref", "retry with longer ref"]);
    let matches = error
        .lines()
        .filter_map(|line| line.strip_prefix("match "))
        .filter_map(|line| line.split_whitespace().next())
        .collect::<Vec<_>>();
    assert_eq!(matches, ["AMB-7KQ1", "AMB-7KQ2"]);
}
