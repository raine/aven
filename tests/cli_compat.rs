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

    sqlx::raw_sql(include_str!(
        "../crates/aven-core/migrations/20260618000000_initial.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO projects(key, name, prefix, created_at, updated_at)
         VALUES ('app', 'app', 'APP', 't', 't')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO tasks(id, title, description, project_key, status, priority, created_at, updated_at)
         VALUES ('7KQ9A1X4MV2P8D6R', 'old task', '', 'app', 'inbox', 'none', 't', 't')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO tasks(id, title, description, project_key, status, priority, created_at, updated_at, deleted)
         VALUES ('8KQ9A1X4MV2P8D6R', 'orphan task', '', 'missing', 'inbox', 'none', 't', 't', 1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO project_paths(project_key, path) VALUES ('missing', '/tmp/missing')")
        .execute(&pool)
        .await
        .unwrap();
    drop(pool);

    let shown = ok(env.aven(&db, ["show", "7KQ"]));
    assert_eq!(first_token(&shown), "APP-7KQ9");
    contains_all(&shown, &["old task"]);

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&db)
                .create_if_missing(false),
        )
        .await
        .unwrap();
    let task_count: i64 = sqlx::query_scalar("SELECT count(*) FROM tasks")
        .fetch_one(&pool)
        .await
        .unwrap();
    let orphan_project_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM projects WHERE key = 'missing' AND deleted = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    let app_project_id: String = sqlx::query_scalar("SELECT id FROM projects WHERE key = 'app'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let orphan_path_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM project_paths pp
         JOIN projects p ON p.workspace_id = pp.workspace_id AND p.id = pp.project_id
         WHERE p.key = 'missing'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let source: String = sqlx::query_scalar("SELECT source FROM tasks WHERE title = 'old task'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(task_count, 2);
    assert_eq!(source, "unknown");
    assert_eq!(orphan_project_count, 1);
    assert_eq!(orphan_path_count, 0);
    assert_eq!(app_project_id.len(), 16);
}

#[tokio::test]
async fn taskless_outcomes_return_to_derived_gaps_on_upgrade() {
    let env = TestEnv::new();
    let db = env.db("recurrence-taskless-outcomes.sqlite");
    ok(env.aven(&db, ["workspace", "list"]));

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&db)
                .create_if_missing(false),
        )
        .await
        .unwrap();
    sqlx::raw_sql(
        "PRAGMA ignore_check_constraints = ON;
         INSERT INTO changes(
             change_id, client_id, local_seq, entity_type, entity_id, field,
             op_type, payload, created_at
         ) VALUES (
             'TASKLESSOUTCOME', 'client', 1, 'recurrence_series',
             '7KQ9A1X4MV2P8D6R', 'outcome', 'record_recurrence_outcome',
             '{\"slot_on\":\"2026-07-20\",\"outcome\":\"completed\",\"resolved_at\":\"2026-07-20T12:00:00Z\"}',
             '2026-07-20T12:00:00Z'
         );
         INSERT INTO recurrence_occurrences(
             workspace_id, series_id, slot_on, outcome, resolved_at,
             outcome_change_id, projection_state
         ) VALUES (
             '0000000000000000', '7KQ9A1X4MV2P8D6R', '2026-07-20',
             'completed', '2026-07-20T12:00:00Z', 'TASKLESSOUTCOME', 'corrected'
         );
         INSERT INTO conflicts(
             workspace_id, entity_type, entity_id, field, local_value,
             remote_value, remote_change_id, variant_a, variant_b, created_at
         ) VALUES (
             '0000000000000000', 'recurrence_series', '7KQ9A1X4MV2P8D6R',
             'outcome:2026-07-20', 'completed', 'skipped', 'TASKLESSOUTCOME',
             'local', 'remote', '2026-07-20T12:00:00Z'
         );
         DELETE FROM _sqlx_migrations
         WHERE version IN (20260728183706, 20260829113619);",
    )
    .execute(&pool)
    .await
    .unwrap();
    drop(pool);

    ok(env.aven(&db, ["workspace", "list"]));

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&db)
                .create_if_missing(false),
        )
        .await
        .unwrap();
    let occurrence_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM recurrence_occurrences WHERE slot_on = '2026-07-20'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let change_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM changes WHERE change_id = 'TASKLESSOUTCOME'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let conflict_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM conflicts WHERE remote_change_id = 'TASKLESSOUTCOME'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let table_sql: String = sqlx::query_scalar(
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'recurrence_occurrences'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let task_index_sql: String = sqlx::query_scalar(
        "SELECT sql FROM sqlite_master
         WHERE type = 'index' AND name = 'idx_recurrence_occurrences_task'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(occurrence_count, 0);
    assert_eq!(change_count, 0);
    assert_eq!(conflict_count, 0);
    assert!(table_sql.contains("projection_state IN ('projected', 'resolved', 'archived')"));
    assert_eq!(
        task_index_sql
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
        "CREATE UNIQUE INDEX idx_recurrence_occurrences_task ON recurrence_occurrences(workspace_id, task_id)"
    );
}
