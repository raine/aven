use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode};

#[tokio::test]
async fn migrated_database_contains_attachment_tables_and_indexes() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("test.sqlite");
    let pool = open_migrated_db(&db_path).await;

    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name IN ('task_attachments', 'blob_inventory')",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(tables.contains(&"task_attachments".to_string()));
    assert!(tables.contains(&"blob_inventory".to_string()));

    let indexes: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'index' AND sql IS NOT NULL
         AND name IN ('idx_task_attachments_workspace_task', 'idx_task_attachments_workspace_sha256', 'idx_blob_inventory_available')",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(indexes.contains(&"idx_task_attachments_workspace_task".to_string()));
    assert!(indexes.contains(&"idx_task_attachments_workspace_sha256".to_string()));
    assert!(indexes.contains(&"idx_blob_inventory_available".to_string()));

    let table_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN ('task_attachments', 'blob_inventory') AND sql LIKE '%CHECK%'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        table_count, 2,
        "both attachment tables should have CHECK constraints"
    );

    let lifecycle_tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name IN
         ('blob_lifecycle', 'blob_leases', 'blob_upload_reservations',
          'server_blob_references', 'server_task_tombstones')",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(lifecycle_tables.len(), 5);

    let validation_triggers: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'trigger' AND name IN
         ('task_attachments_validate_insert', 'task_attachments_validate_update',
          'blob_inventory_validate_insert', 'blob_inventory_validate_update')",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(validation_triggers.len(), 4);

    for (sha256, byte_size) in [
        ("g".repeat(64), 1_i64),
        ("a".repeat(64), 25_i64 * 1024 * 1024 + 1),
    ] {
        let result = sqlx::query(
            "INSERT INTO blob_inventory(sha256, byte_size, media_type, available, first_seen_at)
             VALUES (?, ?, 'image/png', 1, '2026-07-18T00:00:00Z')",
        )
        .bind(sha256)
        .bind(byte_size)
        .execute(&pool)
        .await;
        assert!(result.is_err());
    }

    let col_names: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('task_attachments')")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert!(col_names.contains(&"workspace_id".to_string()));
    assert!(col_names.contains(&"attachment_id".to_string()));
    assert!(col_names.contains(&"task_id".to_string()));
    assert!(col_names.contains(&"sha256".to_string()));
    assert!(col_names.contains(&"byte_size".to_string()));
    assert!(col_names.contains(&"media_type".to_string()));
    assert!(col_names.contains(&"filename".to_string()));
    assert!(col_names.contains(&"alt_text".to_string()));
    assert!(col_names.contains(&"width".to_string()));
    assert!(col_names.contains(&"height".to_string()));
    assert!(col_names.contains(&"created_at".to_string()));
    assert!(col_names.contains(&"created_by_change_id".to_string()));
    assert!(col_names.contains(&"deleted".to_string()));
    assert!(col_names.contains(&"deleted_at".to_string()));
    assert!(col_names.contains(&"deleted_by_change_id".to_string()));
}

#[tokio::test]
async fn attachment_liveness_upgrade_uses_latest_accepted_server_order() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("upgrade.sqlite");
    let pool = open_migrated_db(&db_path).await;
    let live_hash = "a".repeat(64);
    let deleted_hash = "b".repeat(64);

    sqlx::query(
        "INSERT INTO blob_inventory(
           sha256, byte_size, media_type, available, first_seen_at, last_verified_at
         ) VALUES (?, 1, 'image/png', 1, '2026-01-01T00:00:00Z', NULL),
                  (?, 1, 'image/png', 1, '2026-01-01T00:00:00Z', NULL)",
    )
    .bind(&live_hash)
    .bind(&deleted_hash)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO blob_lifecycle(sha256, unreferenced_at)
         VALUES (?, '2026-01-01T00:00:00Z'), (?, NULL)",
    )
    .bind(&live_hash)
    .bind(&deleted_hash)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO server_blob_references(
           workspace_id, attachment_id, task_id, sha256, byte_size, deleted
         ) VALUES ('0000000000000000', 'AAAAAAAAAAAAAAA1', 'BBBBBBBBBBBBBBB1', ?, 1, 0),
                  ('0000000000000000', 'AAAAAAAAAAAAAAA2', 'BBBBBBBBBBBBBBB2', ?, 1, 0)",
    )
    .bind(&live_hash)
    .bind(&deleted_hash)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::raw_sql(
        "INSERT INTO changes(
           change_id, client_id, local_seq, entity_type, entity_id, field,
           op_type, payload, created_at, server_seq
         ) VALUES
           ('CCCCCCCCCCCCCCC1', 'client', 100, 'task', 'BBBBBBBBBBBBBBB1',
            'deleted', 'set_field',
            '{\"workspace_id\":\"0000000000000000\",\"value\":\"1\"}',
            '2026-01-01T00:00:00Z', 10),
           ('CCCCCCCCCCCCCCC2', 'client', 1, 'task', 'BBBBBBBBBBBBBBB1',
            'deleted', 'resolve_field',
            '{\"workspace_id\":\"0000000000000000\",\"value\":\"0\"}',
            '2026-01-01T00:00:01Z', 20),
           ('CCCCCCCCCCCCCCC3', 'client', 100, 'task', 'BBBBBBBBBBBBBBB2',
            'deleted', 'set_field',
            '{\"workspace_id\":\"0000000000000000\",\"value\":\"0\"}',
            '2026-01-01T00:00:02Z', 30),
           ('CCCCCCCCCCCCCCC4', 'client', 1, 'task', 'BBBBBBBBBBBBBBB2',
            'deleted', 'resolve_field',
            '{\"workspace_id\":\"0000000000000000\",\"value\":\"1\"}',
            '2026-01-01T00:00:03Z', 40),
           ('CCCCCCCCCCCCCCC5', 'client', 1000, 'task', 'BBBBBBBBBBBBBBB2',
            'deleted', 'resolve_field',
            '{\"workspace_id\":\"0000000000000000\",\"value\":\"0\"}',
            '2026-01-01T00:00:04Z', NULL);
         INSERT INTO server_task_tombstones(workspace_id, task_id, deleted)
         VALUES ('0000000000000000', 'BBBBBBBBBBBBBBB1', 1),
                ('0000000000000000', 'BBBBBBBBBBBBBBB2', 0),
                ('0000000000000000', 'BBBBBBBBBBBBBBB3', 1);
         DELETE FROM _sqlx_migrations WHERE version = 20260829152855;",
    )
    .execute(&pool)
    .await
    .unwrap();

    let migrator: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
    migrator.run(&pool).await.unwrap();

    let states: Vec<(String, bool)> =
        sqlx::query_as("SELECT task_id, deleted FROM server_task_tombstones ORDER BY task_id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        states,
        vec![
            ("BBBBBBBBBBBBBBB1".to_string(), false),
            ("BBBBBBBBBBBBBBB2".to_string(), true),
        ]
    );
    let lifecycle: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT sha256, unreferenced_at FROM blob_lifecycle ORDER BY sha256")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(lifecycle[0], (live_hash, None));
    assert!(lifecycle[1].1.is_some());
}

async fn open_migrated_db(path: &Path) -> SqlitePool {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let options = SqliteConnectOptions::from_str(&path.display().to_string())
        .unwrap()
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5));

    let pool = SqlitePool::connect_with(options).await.unwrap();
    let migrator: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
    migrator.run(&pool).await.unwrap();
    pool
}
