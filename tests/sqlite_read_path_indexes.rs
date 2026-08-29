mod common;

use common::{TestEnv, ok};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{Connection as _, Row, SqliteConnection};
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
        "CREATE INDEX idx_tasks_workspace_project_deleted_updated ON tasks(workspace_id, project_id, deleted, updated_at DESC, created_at DESC)",
    ),
    (
        "idx_tasks_workspace_project_deleted_status",
        "CREATE INDEX idx_tasks_workspace_project_deleted_status ON tasks(workspace_id, project_id, deleted, status)",
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
    (
        "idx_recurrence_occurrences_task",
        "CREATE UNIQUE INDEX idx_recurrence_occurrences_task ON recurrence_occurrences(workspace_id, task_id)",
    ),
    (
        "idx_changes_recurrence_resolution",
        "CREATE INDEX idx_changes_recurrence_resolution ON changes(change_id) WHERE entity_type = 'recurrence_series' AND op_type = 'resolve_recurrence_occurrence'",
    ),
];

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

        sqlx::raw_sql(include_str!("../crates/aven-core/migrations/20260618000000_initial.sql"))
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
    assert!(
        shown.contains("old task"),
        "expected old task after upgrade\n{shown}"
    );

    let ddl = read_index_ddl(&db);
    for (index, expected) in READ_PATH_INDEXES {
        let actual = ddl
            .get(*index)
            .unwrap_or_else(|| panic!("missing index ddl for {index} after upgrade"));
        assert_eq!(
            normalize_sql(actual),
            normalize_sql(expected),
            "unexpected ddl for {index} after upgrade"
        );
    }
}

#[test]
fn recurrence_task_lookups_use_task_index() {
    let env = TestEnv::new();
    let db = env.db("recurrence-plans.sqlite");
    ok(env.aven(&db, ["project", "create", "app"]));

    runtime().block_on(async {
        let mut conn = open_db(&db).await;
        seed_plan_rows(&mut conn).await;
        seed_recurrence_plan_rows(&mut conn).await;
        sqlx::query("ANALYZE")
            .execute(&mut conn)
            .await
            .expect("analyze");

        let ordinary_clause = aven_core::query::fragments::ordinary_task_clause("t");
        let ordinary_sql = format!(
            "EXPLAIN QUERY PLAN
             SELECT t.id FROM tasks t
             WHERE t.workspace_id = ? AND t.deleted = 0 AND {ordinary_clause}"
        );
        assert_plan_uses_alias(
            &mut conn,
            ordinary_sql.as_str(),
            &["0000000000000000"],
            "ro",
            "idx_recurrence_occurrences_task",
        )
        .await;

        assert_plan_uses_alias(
            &mut conn,
            "EXPLAIN QUERY PLAN
             SELECT o.task_id, o.series_id, o.slot_on, o.outcome, o.projection_state,
                    s.frequency, s.interval, s.weekdays, s.timezone, s.state
             FROM recurrence_occurrences o
             JOIN recurrence_series s
               ON s.workspace_id = o.workspace_id AND s.id = o.series_id
             WHERE o.workspace_id = ? AND o.task_id IN (?, ?)",
            &["0000000000000000", "0000000000001001", "0000000000001002"],
            "o",
            "idx_recurrence_occurrences_task",
        )
        .await;

        assert_plan_uses_alias(
            &mut conn,
            "EXPLAIN QUERY PLAN
             SELECT c.change_id, ro.series_id
             FROM changes c
             LEFT JOIN recurrence_occurrences ro
               ON c.entity_type = 'task'
              AND ro.workspace_id = ?
              AND ro.task_id = c.entity_id
             WHERE json_extract(c.payload, '$.workspace_id') = ?
               AND c.entity_type = 'task'",
            &["0000000000000000", "0000000000000000"],
            "ro",
            "idx_recurrence_occurrences_task",
        )
        .await;

        assert_plan_uses_alias(
            &mut conn,
            "EXPLAIN QUERY PLAN
             SELECT EXISTS(SELECT 1 FROM recurrence_occurrences
                           WHERE workspace_id = ? AND task_id = ?)",
            &["0000000000000000", "0000000000001001"],
            "recurrence_occurrences",
            "idx_recurrence_occurrences_task",
        )
        .await;
    });
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
             WHERE t.workspace_id = ? AND t.deleted = 0 AND t.project_id = ?
             ORDER BY t.updated_at DESC, t.created_at DESC",
            &["0000000000000000", "A550000000000001"],
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

        assert_plan_uses(
            &mut conn,
            "EXPLAIN QUERY PLAN
             SELECT 1 FROM changes
             WHERE entity_type = 'recurrence_series'
               AND op_type = 'resolve_recurrence_occurrence'",
            &[],
            "idx_changes_recurrence_resolution",
        )
        .await;
    });
}

async fn seed_recurrence_plan_rows(conn: &mut SqliteConnection) {
    sqlx::query(
        "INSERT INTO recurrence_series(
             workspace_id, id, title, description, project_id, priority, initial_status,
             frequency, interval, weekdays, timezone, start_on, available_local_time,
             due_policy, state, stopped_at, created_at, updated_at, deleted
         ) VALUES (
             '0000000000000000', '7KQ9A1X4MV2P8D6R', 'recurring', '', 'A550000000000001',
             'none', 'todo', 'daily', 1, '', 'UTC', '2026-07-20', '', 'same_day',
             'active', '', 't', 't', 0)",
    )
    .execute(&mut *conn)
    .await
    .expect("insert recurrence series");

    for index in 0..200 {
        let (task_id, slot_on, outcome, resolved_at, outcome_change_id, projection_state) =
            if index == 0 {
                (
                    "0000000000001001".to_string(),
                    "2026-07-20".to_string(),
                    String::new(),
                    String::new(),
                    String::new(),
                    "projected",
                )
            } else {
                let slot_on = format!("2020-{:02}-{:02}", 1 + index / 28, 1 + index % 28);
                (
                    format!("{index:016X}", index = 0x2000 + index),
                    slot_on.clone(),
                    "completed".to_string(),
                    format!("{slot_on}T00:00:00Z"),
                    format!("outcome-change-{index}"),
                    "resolved",
                )
            };
        sqlx::query(
            "INSERT INTO recurrence_occurrences(
                 workspace_id, series_id, slot_on, task_id, outcome, resolved_at,
                 outcome_change_id, projection_state, archived_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, '')",
        )
        .bind("0000000000000000")
        .bind("7KQ9A1X4MV2P8D6R")
        .bind(slot_on)
        .bind(task_id)
        .bind(outcome)
        .bind(resolved_at)
        .bind(outcome_change_id)
        .bind(projection_state)
        .execute(&mut *conn)
        .await
        .expect("insert recurrence occurrence");
    }

    sqlx::query(
        "INSERT INTO changes(
             change_id, client_id, local_seq, entity_type, entity_id, field,
             op_type, payload, created_at
         ) VALUES (
             'activity-change', 'client', 1, 'task', '0000000000001001', 'title',
             'set_field', '{\"workspace_id\":\"0000000000000000\"}', 't')",
    )
    .execute(&mut *conn)
    .await
    .expect("insert activity change");
}

async fn seed_plan_rows(conn: &mut SqliteConnection) {
    sqlx::query(
        "INSERT INTO projects(id, workspace_id, key, name, prefix, created_at, updated_at)
         VALUES ('A550000000000001', '0000000000000000', 'app2', 'app2', 'AP2', 't', 't');
         INSERT INTO tasks(id, workspace_id, title, description, project_id, status, priority, created_at, updated_at)
         VALUES
         ('0000000000001001', '0000000000000000', 'todo bug', '', 'A550000000000001', 'todo', 'high', '001', '003'),
         ('0000000000001002', '0000000000000000', 'active', '', 'A550000000000001', 'active', 'low', '002', '004')",
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
    sql: &str,
    binds: &[&str],
    index_name: &str,
) {
    let mut query = sqlx::query(sqlx::AssertSqlSafe(sql));
    for bind in binds {
        query = query.bind(*bind);
    }
    let rows = query
        .fetch_all(&mut *conn)
        .await
        .expect("explain query plan");
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

async fn assert_plan_uses_alias(
    conn: &mut SqliteConnection,
    sql: &str,
    binds: &[&str],
    alias: &str,
    index_name: &str,
) {
    let mut query = sqlx::query(sqlx::AssertSqlSafe(sql));
    for bind in binds {
        query = query.bind(*bind);
    }
    let rows = query
        .fetch_all(&mut *conn)
        .await
        .expect("explain query plan");
    let details = rows
        .iter()
        .map(|row| row.get::<String, _>("detail"))
        .collect::<Vec<_>>();
    let alias_marker = format!(" {alias} ");
    assert!(
        details.iter().any(|detail| {
            detail.contains(&alias_marker) && plan_uses_search_index(detail, index_name)
        }),
        "expected {alias} to use {index_name}\n{}",
        details.join("\n")
    );
}

fn plan_uses_search_index(detail: &str, index_name: &str) -> bool {
    let index_marker = format!("USING INDEX {index_name}");
    let covering_index_marker = format!("USING COVERING INDEX {index_name}");
    detail.contains("SEARCH ")
        && (detail.contains(&index_marker) || detail.contains(&covering_index_marker))
}

fn normalize_sql(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
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
