mod common;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

use common::{TestEnv, contains_all, ok};

fn first_token(output: &str) -> &str {
    output
        .split_whitespace()
        .next()
        .expect("output starts with ref")
}

#[tokio::test]
async fn old_schema_database_can_be_opened_and_read() {
    let env = TestEnv::new();
    let db = env.db("old.sqlite");
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&db)
                .create_if_missing(true),
        )
        .await
        .unwrap();

    sqlx::raw_sql(include_str!("../migrations/20260618000000_initial.sql"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query!(
        "INSERT INTO projects(key, name, prefix, created_at, updated_at)
         VALUES ('app', 'app', 'APP', 't', 't')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query!(
        "INSERT INTO tasks(id, title, description, project_key, status, priority, created_at, updated_at)
         VALUES ('7KQ9A1X4MV2P8D6R', 'old task', '', 'app', 'inbox', 'none', 't', 't')",
    )
    .execute(&pool)
    .await
    .unwrap();
    drop(pool);

    let shown = ok(env.aven(&db, ["show", "7KQ"]));
    assert_eq!(first_token(&shown), "APP-7KQ9");
    contains_all(&shown, &["old task"]);
}
