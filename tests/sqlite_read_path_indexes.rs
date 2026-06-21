mod common;

use common::{TestEnv, ok};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{Connection as _, Row, SqliteConnection};
use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

const READ_PATH_INDEXES: &[(&str, &str)] = &[
    (
        "idx_tasks_workspace_deleted_updated",
        "CREATE INDEX idx_tasks_workspace_deleted_updated ON tasks(workspace_id, deleted, updated_at DESC, created_at DESC)",
    ),
    (
        "idx_tasks_workspace_deleted_status_updated",
        "CREATE INDEX idx_tasks_workspace_deleted_status_updated ON tasks(workspace_id, deleted, status, updated_at DESC, created_at DESC)",
    ),
    (
        "idx_tasks_workspace_deleted_priority_updated",
        "CREATE INDEX idx_tasks_workspace_deleted_priority_updated ON tasks(workspace_id, deleted, priority, updated_at DESC, created_at DESC)",
    ),
    (
        "idx_tasks_workspace_project_deleted_updated",
        "CREATE INDEX idx_tasks_workspace_project_deleted_updated ON tasks(workspace_id, project_key, deleted, updated_at DESC, created_at DESC)",
    ),
    (
        "idx_tasks_workspace_project_deleted_status",
        "CREATE INDEX idx_tasks_workspace_project_deleted_status ON tasks(workspace_id, project_key, deleted, status)",
    ),
    (
        "idx_conflicts_workspace_resolved_created_task",
        "CREATE INDEX idx_conflicts_workspace_resolved_created_task ON conflicts(workspace_id, resolved, created_at, task_id)",
    ),
    (
        "idx_conflicts_workspace_resolved_task",
        "CREATE INDEX idx_conflicts_workspace_resolved_task ON conflicts(workspace_id, resolved, task_id)",
    ),
    (
        "idx_task_labels_workspace_label_task",
        "CREATE INDEX idx_task_labels_workspace_label_task ON task_labels(workspace_id, label, task_id)",
    ),
];

const READ_PATH_INDEX_MIGRATION: &str = include_str!("../migrations/20260621000000_read_path_indexes.sql");

#[test]
fn fresh_database_creates_read_path_indexes() {
    let env = TestEnv::new();
    let db = env.db("fresh.sqlite");

    ok(env.aven(&db, ["list"]));

    let indexes = read_index_names(&db);
    for (index, _) in READ_PATH_INDEXES {
        assert!(indexes.contains(*index), "missing index {index}");
    }
}

#[test]
fn fresh_database_index_ddl_matches_migration() {
    let env = TestEnv::new();
    let db = env.db("fresh-ddl.sqlite");

    ok(env.aven(&db, ["list"]));

    let ddl = read_index_ddl(&db);
    for (index, expected) in READ_PATH_INDEXES {
        let actual = ddl
            .get(*index)
            .unwrap_or_else(|| panic!("missing index ddl for {index}"));
        assert_eq!(
            normalize_sql(actual),
            normalize_sql(expected),
            "unexpected ddl for {index}"
        );
    }
}

#[test]
fn existing_database_migration_preserves_data() {
    let env = TestEnv::new();
    let db = env.db("existing.sqlite");

    ok(env.aven(&db, ["project", "create", "app"]));
    ok(env.aven(&db, ["label", "create", "bug"]));
    ok(env.aven(
        &db,
        [
            "add",
            "indexed task",
            "--project",
            "app",
            "--priority",
            "high",
            "--label",
            "bug",
        ],
    ));

    ok(env.aven(&db, ["list", "--label", "bug"]));

    let runtime = runtime();
    let (task_count, task_title, label_count) = runtime.block_on(async {
        let mut conn = open_db(&db).await;
        let task_count = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM tasks")
            .fetch_one(&mut conn)
            .await
            .expect("count tasks");
        let task_title = sqlx::query_scalar::<_, String>("SELECT title FROM tasks")
            .fetch_one(&mut conn)
            .await
            .expect("read task title");
        let label_count = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM task_labels")
            .fetch_one(&mut conn)
            .await
            .expect("count task labels");
        (task_count, task_title, label_count)
    });

    assert_eq!(task_count, 1);
    assert_eq!(task_title, "indexed task");
    assert_eq!(label_count, 1);

    let indexes = read_index_names(&db);
    for (index, _) in READ_PATH_INDEXES {
        assert!(indexes.contains(*index), "missing index {index}");
    }
}

#[test]
fn old_schema_database_upgrade_creates_read_path_indexes() {
    let env = TestEnv::new();
    let db = env.db("old-upgrade.sqlite");

    let runtime = runtime();
    runtime.block_on(async {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&db)
                    .create_if_missing(true),
            )
            .await
            .expect("open old schema db");

        sqlx::raw_sql(include_str!("../migrations/20260618000000_initial.sql"))
            .execute(&pool)
            .await
            .expect("apply initial migration");
        sqlx::query(
            "INSERT INTO projects(key, name, prefix, created_at, updated_at)
             VALUES ('app', 'app', 'APP', 't', 't')",
        )
        .execute(&pool)
        .await
        .expect("insert project");
        sqlx::query(
            "INSERT INTO tasks(id, title, description, project_key, status, priority, created_at, updated_at)
             VALUES ('7KQ9A1X4MV2P8D6R', 'old task', '', 'app', 'inbox', 'none', 't', 't')",
        )
        .execute(&pool)
        .await
        .expect("insert task");
    });

    let shown = ok(env.aven(&db, ["show", "7KQ"]));
    assert!(shown.contains("old task"), "expected old task after upgrade\n{shown}");

    let indexes = read_index_names(&db);
    for (index, _) in READ_PATH_INDEXES {
        assert!(indexes.contains(*index), "missing index {index} after upgrade");
    }
}

#[test]
fn read_path_index_migration_sql_is_idempotent() {
    let env = TestEnv::new();
    let db = env.db("idempotent.sqlite");

    ok(env.aven(&db, ["list"]));

    let runtime = runtime();
    runtime.block_on(async {
        let mut conn = open_db(&db).await;
        sqlx::raw_sql(READ_PATH_INDEX_MIGRATION)
            .execute(&mut conn)
            .await
            .expect("first direct migration apply");
        sqlx::raw_sql(READ_PATH_INDEX_MIGRATION)
            .execute(&mut conn)
            .await
            .expect("second direct migration apply");
    });

    let indexes = read_index_names(&db);
    for (index, _) in READ_PATH_INDEXES {
        assert!(indexes.contains(*index), "missing index {index}");
    }
}

#[test]
fn common_read_filters_have_workspace_scoped_query_plans() {
    let env = TestEnv::new();
    let db = env.db("plans.sqlite");
    ok(env.aven(&db, ["project", "create", "app"]));

    let runtime = runtime();
    runtime.block_on(async {
        let mut conn = open_db(&db).await;
        seed_plan_rows(&mut conn).await;
        sqlx::query("ANALYZE").execute(&mut conn).await.expect("analyze");

        assert_plan_uses(
            &mut conn,
            "EXPLAIN QUERY PLAN
             SELECT t.id FROM tasks t
             WHERE t.workspace_id = ? AND t.deleted = 0
             ORDER BY t.updated_at DESC, t.created_at DESC",
            &["0000000000000000"],
            "idx_tasks_workspace_deleted_updated",
        )
        .await;

        assert_plan_uses(
            &mut conn,
            "EXPLAIN QUERY PLAN
             SELECT t.id FROM tasks t
             WHERE t.workspace_id = ? AND t.deleted = 0 AND t.status = ?
             ORDER BY t.updated_at DESC, t.created_at DESC",
            &["0000000000000000", "todo"],
            "idx_tasks_workspace_deleted_status_updated",
        )
        .await;

        assert_plan_uses(
            &mut conn,
            "EXPLAIN QUERY PLAN
             SELECT t.id FROM tasks t
             WHERE t.workspace_id = ? AND t.deleted = 0 AND t.priority = ?
             ORDER BY t.updated_at DESC, t.created_at DESC",
            &["0000000000000000", "high"],
            "idx_tasks_workspace_deleted_priority_updated",
        )
        .await;

        assert_plan_uses(
            &mut conn,
            "EXPLAIN QUERY PLAN
             SELECT t.id FROM tasks t
             WHERE t.workspace_id = ? AND t.deleted = 0 AND t.project_key = ?
             ORDER BY t.updated_at DESC, t.created_at DESC",
            &["0000000000000000", "app"],
            "idx_tasks_workspace_project_deleted_updated",
        )
        .await;

        assert_plan_uses(
            &mut conn,
            "EXPLAIN QUERY PLAN
             SELECT t.id FROM tasks t
             WHERE t.workspace_id = ? AND t.deleted = 0
             AND t.id IN (
                 SELECT tl.task_id FROM task_labels tl INDEXED BY idx_task_labels_workspace_label_task
                 WHERE tl.workspace_id = ? AND tl.label = ?
             )
             ORDER BY t.updated_at DESC, t.created_at DESC",
            &["0000000000000000", "0000000000000000", "bug"],
            "idx_task_labels_workspace_label_task",
        )
        .await;

        assert_plan_uses(
            &mut conn,
            "EXPLAIN QUERY PLAN
             SELECT c.task_id FROM conflicts c
             WHERE c.workspace_id = ? AND c.resolved = 0
             ORDER BY c.created_at",
            &["0000000000000000"],
            "idx_conflicts_workspace_resolved_created_task",
        )
        .await;

        assert_plan_uses(
            &mut conn,
            "EXPLAIN QUERY PLAN
             SELECT count(*) FROM tasks
             WHERE workspace_id = ? AND deleted = 0 AND status = ?",
            &["0000000000000000", "active"],
            "idx_tasks_workspace_deleted_status_updated",
        )
        .await;
    });
}

async fn seed_plan_rows(conn: &mut SqliteConnection) {
    sqlx::query(
        "INSERT INTO tasks(id, workspace_id, title, description, project_key, status, priority, created_at, updated_at)
         VALUES
         ('0000000000001001', '0000000000000000', 'todo bug', '', 'app', 'todo', 'high', '001', '003'),
         ('0000000000001002', '0000000000000000', 'active', '', 'app', 'active', 'low', '002', '004')",
    )
    .execute(&mut *conn)
    .await
    .expect("insert tasks");

    sqlx::query(
        "INSERT INTO task_labels(workspace_id, task_id, label)
         VALUES ('0000000000000000', '0000000000001001', 'bug')",
    )
    .execute(&mut *conn)
    .await
    .expect("insert task label");

    sqlx::query(
        "INSERT INTO conflicts(workspace_id, task_id, field, base_version, local_value, remote_value,
         local_change_id, remote_change_id, variant_a, variant_b, created_at, resolved)
         VALUES ('0000000000000000', '0000000000001001', 'title', NULL, 'local', 'remote', NULL,
         'remote-change', 'a', 'b', '005', 0)",
    )
    .execute(&mut *conn)
    .await
    .expect("insert conflict");
}

async fn assert_plan_uses(
    conn: &mut SqliteConnection,
    sql: &'static str,
    binds: &[&str],
    index_name: &str,
) {
    let mut query = sqlx::query(sql);
    for bind in binds {
        query = query.bind(*bind);
    }
    let rows = query.fetch_all(&mut *conn).await.expect("explain query plan");
    let plan = rows
        .iter()
        .map(|row| row.get::<String, _>("detail"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        plan.contains(index_name),
        "expected plan to use {index_name}\n{plan}"
    );
}

fn normalize_sql(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn read_index_names(db: &Path) -> HashSet<String> {
    runtime().block_on(async {
        let mut conn = open_db(db).await;
        sqlx::query_scalar::<_, String>(
            "SELECT name FROM sqlite_master WHERE type = 'index' AND name LIKE 'idx_%'",
        )
        .fetch_all(&mut conn)
        .await
        .expect("read indexes")
        .into_iter()
        .collect()
    })
}

fn read_index_ddl(db: &Path) -> std::collections::HashMap<String, String> {
    runtime().block_on(async {
        let mut conn = open_db(db).await;
        let rows = sqlx::query(
            "SELECT name, sql FROM sqlite_master WHERE type = 'index' AND name LIKE 'idx_%'",
        )
        .fetch_all(&mut conn)
        .await
        .expect("read index ddl");
        rows.into_iter()
            .map(|row| (row.get("name"), row.get("sql")))
            .collect()
    })
}

async fn open_db(db: &Path) -> SqliteConnection {
    let options = SqliteConnectOptions::new()
        .filename(db)
        .create_if_missing(false)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5));
    SqliteConnection::connect_with(&options)
        .await
        .expect("open sqlite db")
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("create tokio runtime")
}
