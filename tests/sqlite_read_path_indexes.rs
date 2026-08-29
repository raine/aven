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
        "idx_notes_workspace_task_created_id",
        "CREATE INDEX idx_notes_workspace_task_created_id ON notes(workspace_id, task_id, created_at DESC, id DESC)",
    ),
    (
        "idx_recurrence_occurrences_task",
        "CREATE UNIQUE INDEX idx_recurrence_occurrences_task ON recurrence_occurrences(workspace_id, task_id)",
    ),
    (
        "idx_changes_recurrence_resolution",
        "CREATE INDEX idx_changes_recurrence_resolution ON changes(change_id) WHERE entity_type = 'recurrence_series' AND op_type = 'resolve_recurrence_occurrence'",
    ),
    (
        "idx_changes_task_activity",
        "CREATE INDEX idx_changes_task_activity ON changes(entity_id, created_at DESC, local_seq DESC) WHERE entity_type = 'task'",
    ),
    (
        "idx_changes_recurrence_task_status_change",
        "CREATE INDEX idx_changes_recurrence_task_status_change ON changes(json_extract(payload, '$.task_status_change_id')) WHERE entity_type = 'recurrence_series' AND op_type = 'resolve_recurrence_occurrence'",
    ),
    (
        "idx_changes_recurrence_successor_task",
        "CREATE INDEX idx_changes_recurrence_successor_task ON changes(json_extract(payload, '$.successor_task_id')) WHERE entity_type = 'recurrence_series' AND op_type = 'resolve_recurrence_occurrence'",
    ),
    (
        "idx_task_attachments_sha256_deleted_workspace_task",
        "CREATE INDEX idx_task_attachments_sha256_deleted_workspace_task ON task_attachments(sha256, deleted, workspace_id, task_id)",
    ),
    (
        "idx_server_blob_references_sha256_deleted_workspace_task",
        "CREATE INDEX idx_server_blob_references_sha256_deleted_workspace_task ON server_blob_references(sha256, deleted, workspace_id, task_id)",
    ),
    (
        "idx_changes_pending_attachment_add_sha256",
        "CREATE INDEX idx_changes_pending_attachment_add_sha256 ON changes(json_extract(payload, '$.sha256')) WHERE server_seq IS NULL AND op_type = 'attachment_add'",
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
fn task_note_lookups_use_notes_index() {
    let env = TestEnv::new();
    let db = env.db("note-plans.sqlite");
    ok(env.aven(&db, ["project", "create", "app"]));

    runtime().block_on(async {
        let mut conn = open_db(&db).await;
        seed_plan_rows(&mut conn).await;
        seed_note_plan_rows(&mut conn).await;
        sqlx::query("ANALYZE")
            .execute(&mut conn)
            .await
            .expect("analyze");

        assert_plan_uses_search_without_temp_sort(
            &mut conn,
            "EXPLAIN QUERY PLAN
             SELECT body FROM notes n
             WHERE n.workspace_id = ? AND n.task_id = ?
             ORDER BY n.created_at DESC, n.id DESC",
            &["0000000000000000", "0000000000001001"],
            "n",
            "idx_notes_workspace_task_created_id",
        )
        .await;

        assert_plan_uses_search_without_temp_sort(
            &mut conn,
            "EXPLAIN QUERY PLAN
             SELECT task_id, id, body, created_at FROM notes
             WHERE workspace_id = ? AND task_id IN (?, ?)
             ORDER BY task_id, created_at DESC, id DESC",
            &["0000000000000000", "0000000000001001", "0000000000001002"],
            "notes",
            "idx_notes_workspace_task_created_id",
        )
        .await;

        assert_plan_uses_search_without_temp_sort(
            &mut conn,
            "EXPLAIN QUERY PLAN
             SELECT id, body, created_at FROM notes
             WHERE workspace_id = ? AND task_id = ?
             ORDER BY created_at, id",
            &["0000000000000000", "0000000000001001"],
            "notes",
            "idx_notes_workspace_task_created_id",
        )
        .await;
    });
}

#[test]
fn attachment_liveness_probes_use_hash_indexes() {
    let env = TestEnv::new();
    let db = env.db("attachment-liveness-plans.sqlite");
    ok(env.aven(&db, ["project", "create", "app"]));

    runtime().block_on(async {
        let mut conn = open_db(&db).await;
        seed_plan_rows(&mut conn).await;
        seed_attachment_liveness_plan_rows(&mut conn).await;
        sqlx::query("ANALYZE")
            .execute(&mut conn)
            .await
            .expect("analyze");

        assert_plan_uses_alias(
            &mut conn,
            "EXPLAIN QUERY PLAN
             SELECT ta.workspace_id, ta.task_id FROM task_attachments ta
             WHERE ta.sha256 = ? AND ta.deleted = 0",
            &["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"],
            "ta",
            "idx_task_attachments_sha256_deleted_workspace_task",
        )
        .await;

        assert_plan_uses_alias(
            &mut conn,
            "EXPLAIN QUERY PLAN
             SELECT sbr.workspace_id, sbr.task_id
             FROM server_blob_references sbr
             WHERE sbr.sha256 = ? AND sbr.deleted = 0",
            &["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"],
            "sbr",
            "idx_server_blob_references_sha256_deleted_workspace_task",
        )
        .await;

        assert_plan_uses(
            &mut conn,
            "EXPLAIN QUERY PLAN
             SELECT change_id FROM changes
             WHERE server_seq IS NULL AND op_type = 'attachment_add'
               AND json_extract(payload, '$.sha256') = ?",
            &["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"],
            "idx_changes_pending_attachment_add_sha256",
        )
        .await;

        let rows = sqlx::query(
            "EXPLAIN QUERY PLAN
             UPDATE blob_lifecycle SET unreferenced_at = NULL
             WHERE unreferenced_at IS NOT NULL
               AND sha256 IN (SELECT value FROM json_each(?))
               AND (EXISTS(
                   SELECT 1 FROM task_attachments ta
                   JOIN tasks t ON t.workspace_id = ta.workspace_id AND t.id = ta.task_id
                   WHERE ta.sha256 = blob_lifecycle.sha256
                     AND ta.deleted = 0 AND t.deleted = 0
               ) OR EXISTS(
                   SELECT 1 FROM server_blob_references sbr
                   LEFT JOIN server_task_tombstones st
                     ON st.workspace_id = sbr.workspace_id AND st.task_id = sbr.task_id
                   WHERE sbr.sha256 = blob_lifecycle.sha256
                     AND sbr.deleted = 0 AND COALESCE(st.deleted, 0) = 0
               ))",
        )
        .bind("[\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"]")
        .fetch_all(&mut conn)
        .await
        .expect("explain affected liveness update");
        let details = rows
            .iter()
            .map(|row| row.get::<String, _>("detail"))
            .collect::<Vec<_>>();
        assert!(
            details
                .iter()
                .any(|detail| detail.contains("SEARCH blob_lifecycle")),
            "expected affected update to search lifecycle by hash\n{}",
            details.join("\n")
        );
        assert!(
            details
                .iter()
                .all(|detail| !detail.contains("SCAN blob_lifecycle")),
            "expected affected update to avoid a lifecycle sweep\n{}",
            details.join("\n")
        );
        assert!(
            details.iter().any(|detail| {
                detail.contains("idx_task_attachments_sha256_deleted_workspace_task")
            }),
            "expected affected update to use local hash index\n{}",
            details.join("\n")
        );
        assert!(
            details.iter().any(|detail| {
                detail.contains("idx_server_blob_references_sha256_deleted_workspace_task")
            }),
            "expected affected update to use server hash index\n{}",
            details.join("\n")
        );
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
        seed_activity_plan_rows(&mut conn).await;
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
             SELECT 1 FROM changes INDEXED BY idx_changes_recurrence_resolution
             WHERE entity_type = 'recurrence_series'
               AND op_type = 'resolve_recurrence_occurrence'",
            &[],
            "idx_changes_recurrence_resolution",
        )
        .await;

        assert_plan_uses_alias(
            &mut conn,
            "EXPLAIN QUERY PLAN
             SELECT candidate.rowid FROM changes candidate
             WHERE candidate.entity_type = 'task'
               AND candidate.entity_id = ?
               AND json_extract(candidate.payload, '$.workspace_id') = ?
             ORDER BY candidate.created_at DESC, candidate.local_seq DESC
             LIMIT 8",
            &["0000000000001001", "0000000000000000"],
            "candidate",
            "idx_changes_task_activity",
        )
        .await;

        assert_plan_contains(
            &mut conn,
            "EXPLAIN QUERY PLAN
             WITH resolved_recurrence_changes(status_change_id, successor_task_id) AS MATERIALIZED (
                 SELECT json_extract(payload, '$.task_status_change_id'),
                        json_extract(payload, '$.successor_task_id')
                 FROM changes
                 WHERE entity_type = 'recurrence_series'
                   AND op_type = 'resolve_recurrence_occurrence'
             )
             SELECT candidate.rowid FROM changes candidate
             WHERE candidate.entity_type = 'task'
               AND candidate.entity_id = ?
               AND json_extract(candidate.payload, '$.workspace_id') = ?
               AND candidate.change_id NOT IN (
                   SELECT status_change_id FROM resolved_recurrence_changes
                   WHERE status_change_id IS NOT NULL
               )
               AND (candidate.op_type != 'create_task' OR candidate.entity_id NOT IN (
                   SELECT successor_task_id FROM resolved_recurrence_changes
                   WHERE successor_task_id IS NOT NULL
               ))
               AND (candidate.op_type != 'project_recurrence_occurrence'
                    OR json_extract(candidate.payload, '$.task_id') IS NULL
                    OR json_extract(candidate.payload, '$.task_id') NOT IN (
                        SELECT successor_task_id FROM resolved_recurrence_changes
                        WHERE successor_task_id IS NOT NULL
                    ))
             ORDER BY candidate.created_at DESC, candidate.local_seq DESC
             LIMIT 8",
            &["0000000000001001", "0000000000000000"],
            "SEARCH candidate USING INDEX idx_changes_task_activity",
        )
        .await;

        assert_plan_contains(
            &mut conn,
            "EXPLAIN QUERY PLAN
             WITH requested(task_id) AS (VALUES (?))
             SELECT c.rowid
             FROM requested r
             JOIN changes c ON c.rowid IN (
                 SELECT candidate.rowid
                 FROM changes candidate
                 WHERE candidate.entity_type = 'task'
                   AND candidate.entity_id = r.task_id
                   AND json_extract(candidate.payload, '$.workspace_id') = ?
                 ORDER BY candidate.created_at DESC, candidate.local_seq DESC
                 LIMIT 8
             )",
            &["0000000000001001", "0000000000000000"],
            "SEARCH c USING INTEGER PRIMARY KEY (rowid=?)",
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

async fn seed_attachment_liveness_plan_rows(conn: &mut SqliteConnection) {
    for index in 0..200 {
        let hash = if index == 0 {
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        } else {
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        };
        sqlx::query(
            "INSERT INTO blob_inventory(sha256, byte_size, media_type, available, first_seen_at)
             VALUES (?, 4, 'image/png', 1, 't')",
        )
        .bind(hash)
        .execute(&mut *conn)
        .await
        .ok();
        sqlx::query(
            "INSERT OR IGNORE INTO task_attachments(
                 workspace_id, attachment_id, task_id, sha256, byte_size, media_type,
                 width, height, created_at
             ) VALUES ('0000000000000000', ?, '0000000000001001', ?, 4, 'image/png', 1, 1, 't')",
        )
        .bind(format!("{index:016X}"))
        .bind(hash)
        .execute(&mut *conn)
        .await
        .expect("insert task attachment");
        sqlx::query(
            "INSERT OR IGNORE INTO server_blob_references(
                 workspace_id, attachment_id, task_id, sha256, byte_size
             ) VALUES ('server', ?, '0000000000001001', ?, 4)",
        )
        .bind(format!("{index:016X}"))
        .bind(hash)
        .execute(&mut *conn)
        .await
        .expect("insert server attachment");
    }
    sqlx::query(
        "INSERT INTO changes(
             change_id, client_id, local_seq, entity_type, entity_id, op_type, payload, created_at
         ) VALUES ('attachment-plan-change', 'client', 1, 'task', '0000000000001001',
                   'attachment_add', ?, 't')",
    )
    .bind("{\"sha256\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"}")
    .execute(&mut *conn)
    .await
    .expect("insert pending attachment change");
}

async fn seed_note_plan_rows(conn: &mut SqliteConnection) {
    for index in 0..256 {
        let task_id = match index % 4 {
            0 | 1 => "0000000000001001",
            2 => "0000000000001002",
            _ => "0000000000001003",
        };
        let created_at = if index % 8 == 0 {
            "2026-08-29T00:00:00Z".to_owned()
        } else {
            format!("2026-08-29T00:{:02}:00Z", index % 60)
        };
        sqlx::query(
            "INSERT INTO notes(workspace_id, id, task_id, body, created_at, change_id)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind("0000000000000000")
        .bind(format!("note-{index}"))
        .bind(task_id)
        .bind(format!("note body {index}"))
        .bind(created_at)
        .bind(format!("note-change-{index}"))
        .execute(&mut *conn)
        .await
        .expect("insert note");
    }

    sqlx::query(
        "INSERT INTO notes(workspace_id, id, task_id, body, created_at, change_id)
         VALUES ('0000000000000001', 'other-workspace-note', '0000000000001001',
                 'other workspace note', '2026-08-29T00:00:00Z', 'other-workspace-change')",
    )
    .execute(&mut *conn)
    .await
    .expect("insert other workspace note");
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

async fn seed_activity_plan_rows(conn: &mut SqliteConnection) {
    for sequence in 0..1_000 {
        sqlx::query(
            "INSERT INTO changes(
                 change_id, client_id, local_seq, entity_type, entity_id, field,
                 op_type, payload, created_at
             ) VALUES (?, 'client', ?, 'task', '0000000000001001', 'title',
                       'set_field', '{\"workspace_id\":\"0000000000000000\"}', ?)",
        )
        .bind(format!("activity-change-{sequence}"))
        .bind(sequence + 1)
        .bind(format!("{sequence:04}"))
        .execute(&mut *conn)
        .await
        .expect("insert activity change");
    }
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

async fn assert_plan_uses_search_without_temp_sort(
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
        "expected {alias} to SEARCH {index_name}\n{}",
        details.join("\n")
    );
    assert!(
        details.iter().all(|detail| !detail.contains("TEMP B-TREE")),
        "expected {alias} plan to avoid temporary ordering\n{}",
        details.join("\n")
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

async fn assert_plan_contains(
    conn: &mut SqliteConnection,
    sql: &str,
    binds: &[&str],
    expected: &str,
) {
    let mut query = sqlx::query(sqlx::AssertSqlSafe(sql));
    for bind in binds {
        query = query.bind(*bind);
    }
    let plan = query
        .fetch_all(&mut *conn)
        .await
        .expect("explain query plan")
        .iter()
        .map(|row| row.get::<String, _>("detail"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        plan.contains(expected),
        "expected plan to contain {expected}\n{plan}"
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
