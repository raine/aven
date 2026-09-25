use super::*;

const PRIVATE_IN_MEMORY_INPUTS: &[&str] = &[
    ":memory:",
    "sqlite::memory:",
    "sqlite://:memory:",
    "file::memory:",
    "sqlite:file::memory:",
    "sqlite://file::memory:",
    "%3Amemory%3A",
    "sqlite:%3Amemory%3A",
    "sqlite://%3Amemory%3A",
    "file:%3Amemory%3A",
    "sqlite:file:%3Amemory%3A",
    "sqlite://file:%3Amemory%3A",
    "file::memory:?cache=private",
    "sqlite://?mode=memory&cache=private",
    "named?mode=memory&cache=private",
    "sqlite://named?mode=memory&cache=private",
    "sqlite://named?cache=private&mode=memory",
    "sqlite://named?mode=mem%6Fry&cache=private",
];

#[tokio::test]
async fn wal_readers_progress_while_writes_remain_serialized() {
    let temp = tempfile::tempdir().unwrap();
    let database = Database::open(&temp.path().join("concurrency.sqlite"))
        .await
        .unwrap();
    let mut writer = database.acquire_writer().await.unwrap();
    let mut tx = begin_immediate(&mut writer).await.unwrap();
    set_meta(&mut tx, "local_seq", "1").await.unwrap();

    let reader_database = database.clone();
    let reader = tokio::spawn(async move {
        let mut conn = reader_database.acquire_reader().await.unwrap();
        get_meta(&mut conn, "local_seq").await.unwrap()
    });
    let observed = tokio::time::timeout(Duration::from_secs(1), reader)
        .await
        .expect("reader should not wait for the writer")
        .unwrap();
    assert_eq!(observed.as_deref(), Some("0"));

    let second_writer_database = database.clone();
    let second_writer = tokio::spawn(async move {
        let mut conn = second_writer_database.acquire_writer().await.unwrap();
        let mut tx = begin_immediate(&mut conn).await.unwrap();
        set_meta(&mut tx, "local_seq", "2").await.unwrap();
        tx.commit().await.unwrap();
    });
    tokio::task::yield_now().await;
    assert!(!second_writer.is_finished());

    tx.commit().await.unwrap();
    drop(writer);
    tokio::time::timeout(Duration::from_secs(1), second_writer)
        .await
        .expect("second writer should proceed after the first commits")
        .unwrap();
    assert_eq!(
        database.meta("local_seq").await.unwrap().as_deref(),
        Some("2")
    );
}

#[test]
fn database_storage_follows_sqlx_connection_input_semantics() {
    for &input in PRIVATE_IN_MEMORY_INPUTS {
        let options = SqliteConnectOptions::from_str(input).unwrap();
        assert_eq!(
            database_storage(input, &options),
            DatabaseStorage::InMemory,
            "{input}"
        );
    }

    for input in [
        "mode=memory.sqlite",
        "ordinary-file::memory:.sqlite",
        "sqlite::memory:.sqlite",
        "/tmp/directory-mode=memory/database.sqlite",
    ] {
        let options = SqliteConnectOptions::from_str(input).unwrap();
        assert_eq!(
            database_storage(input, &options),
            DatabaseStorage::File,
            "{input}"
        );
    }
}

#[tokio::test]
async fn database_retains_canonical_file_identity() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("identity.sqlite");
    let database = Database::open(&path).await.unwrap();

    assert_eq!(
        database.file_identity(),
        Some(fs::canonicalize(&path).unwrap().as_path())
    );
}

#[tokio::test]
async fn in_memory_databases_have_no_file_identity() {
    for &input in PRIVATE_IN_MEMORY_INPUTS {
        let database = Database::open(Path::new(input)).await.unwrap();
        assert_eq!(database.file_identity(), None, "{input}");
    }
}

#[tokio::test]
async fn private_in_memory_inputs_keep_one_connection_and_one_schema() {
    for &input in PRIVATE_IN_MEMORY_INPUTS {
        let pool = open_db(Path::new(input)).await.unwrap();
        let pool_options = pool.options();
        assert_eq!(pool_options.get_min_connections(), 1, "{input}");
        assert_eq!(pool_options.get_max_connections(), 1, "{input}");
        assert_eq!(pool_options.get_idle_timeout(), None, "{input}");
        assert_eq!(pool_options.get_max_lifetime(), None, "{input}");
        assert_eq!(pool.size(), 1, "{input}");
        let mut first = pool.acquire().await.unwrap();
        let second_pool = pool.clone();
        let mut second = tokio::spawn(async move {
            let mut connection = second_pool.acquire().await.unwrap();
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM sqlite_schema WHERE name = 'acquisition_visibility'",
            )
            .fetch_one(&mut *connection)
            .await
            .unwrap()
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut second)
                .await
                .is_err(),
            "{input} opened another connection"
        );

        sqlx::query("CREATE TABLE acquisition_visibility(id INTEGER PRIMARY KEY)")
            .execute(&mut *first)
            .await
            .unwrap();
        drop(first);

        let visible = tokio::time::timeout(Duration::from_secs(1), second)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(visible, 1, "{input}");
    }
}

#[tokio::test]
async fn filesystem_lookalike_keeps_wal_and_concurrent_connections() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp
        .path()
        .join("ordinary-file::memory:-mode=memory.sqlite");
    let pool = open_db(&path).await.unwrap();
    let first = pool.acquire().await.unwrap();
    let mut second = tokio::time::timeout(Duration::from_secs(1), pool.acquire())
        .await
        .expect("file pool should allow a concurrent acquisition")
        .unwrap();
    let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(&mut *second)
        .await
        .unwrap();

    assert_eq!(
        pool.options().get_max_connections(),
        FILE_DATABASE_CONNECTIONS
    );
    assert_eq!(journal_mode, "wal");
    drop(first);
    drop(second);
    pool.close().await;
}

#[tokio::test]
async fn recurrence_migration_enforces_schedule_immutability_and_task_conflict_compatibility() {
    let (_temp, mut conn) = crate::test_support::test_conn().await;
    sqlx::query(
        "INSERT INTO recurrence_series(
                workspace_id, id, title, description, project_id, priority, initial_status,
                frequency, interval, weekdays, timezone, start_on, available_local_time,
                due_policy, state, created_at, updated_at
             ) VALUES (
                '0000000000000000', '7KQ9A1X4MV2P8D6R', 'journal', '',
                '7KQ9A1X4MV2P8D6S', 'none', 'todo', 'daily', 1, '', 'UTC',
                '2026-07-20', '09:00:00', 'same_day', 'active', 't', 't'
             )",
    )
    .execute(&mut *conn)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO recurrence_series(
                workspace_id, id, title, description, project_id, priority, initial_status,
                frequency, interval, weekdays, timezone, start_on, available_local_time,
                due_policy, state, created_at, updated_at
             ) VALUES
                ('0000000000000000', '7KQ9A1X4MV2P8D7A', 'days', '',
                 '7KQ9A1X4MV2P8D6S', 'none', 'todo', 'daily', 3, '', 'UTC',
                 '2026-07-20', '', 'same_day', 'active', 't', 't'),
                ('0000000000000000', '7KQ9A1X4MV2P8D7B', 'months', '',
                 '7KQ9A1X4MV2P8D6S', 'none', 'todo', 'monthly', 3, '', 'UTC',
                 '2026-07-20', '', 'same_day', 'active', 't', 't'),
                ('0000000000000000', '7KQ9A1X4MV2P8D7C', 'years', '',
                 '7KQ9A1X4MV2P8D6S', 'none', 'todo', 'yearly', 2, '', 'UTC',
                 '2026-07-20', '', 'same_day', 'active', 't', 't')",
    )
    .execute(&mut *conn)
    .await
    .unwrap();
    for invalid in [
        "INSERT INTO recurrence_series(workspace_id,id,title,description,project_id,priority,initial_status,frequency,interval,weekdays,timezone,start_on,available_local_time,due_policy,state,created_at,updated_at) VALUES ('0000000000000000','7KQ9A1X4MV2P8D7D','bad','','7KQ9A1X4MV2P8D6S','none','todo','daily',0,'','UTC','2026-07-20','','same_day','active','t','t')",
        "INSERT INTO recurrence_series(workspace_id,id,title,description,project_id,priority,initial_status,frequency,interval,weekdays,timezone,start_on,available_local_time,due_policy,state,created_at,updated_at) VALUES ('0000000000000000','7KQ9A1X4MV2P8D7E','bad','','7KQ9A1X4MV2P8D6S','none','todo','monthly',2,'mon','UTC','2026-07-20','','same_day','active','t','t')",
        "INSERT INTO recurrence_series(workspace_id,id,title,description,project_id,priority,initial_status,frequency,interval,weekdays,timezone,start_on,available_local_time,due_policy,state,created_at,updated_at) VALUES ('0000000000000000','7KQ9A1X4MV2P8D7F','bad','','7KQ9A1X4MV2P8D6S','none','todo','weekly',2,'','UTC','2026-07-20','','same_day','active','t','t')",
    ] {
        assert!(sqlx::query(invalid).execute(&mut *conn).await.is_err());
    }

    sqlx::query(
        "UPDATE recurrence_series SET title = 'future journal' WHERE id = '7KQ9A1X4MV2P8D6R'",
    )
    .execute(&mut *conn)
    .await
    .unwrap();
    let error = sqlx::query(
            "UPDATE recurrence_series SET frequency = 'weekly', weekdays = 'mon' WHERE id = '7KQ9A1X4MV2P8D6R'",
        )
        .execute(&mut *conn)
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("recurrence schedule is immutable")
    );

    sqlx::query(
            "INSERT INTO conflicts(task_id, field, local_value, remote_value, remote_change_id, variant_a, variant_b, created_at)
             VALUES ('7KQ9A1X4MV2P8D6T', 'title', 'a', 'b', 'remote', 'a', 'b', 't')",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
    let identity: (String, String) = sqlx::query_as(
        "SELECT entity_type, entity_id FROM conflicts WHERE task_id = '7KQ9A1X4MV2P8D6T'",
    )
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        identity,
        ("task".to_string(), "7KQ9A1X4MV2P8D6T".to_string())
    );
}

const PRE_ENCRYPTED_SYNC: i64 = 20260913105309;

#[tokio::test]
async fn pre_encrypted_sync_database_keeps_its_data_through_the_upgrade() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("pre-e2ee.sqlite");
    let options = SqliteConnectOptions::from_str(&path.to_string_lossy())
        .unwrap()
        .create_if_missing(true)
        .foreign_keys(true);
    let pool = SqlitePool::connect_with(options).await.unwrap();
    MIGRATOR.run_to(PRE_ENCRYPTED_SYNC, &pool).await.unwrap();
    sqlx::raw_sql(
        "INSERT INTO meta VALUES ('client_id', 'C1'), ('sync_cursor', '7'),
             ('local_seq', '2'), ('sync_generation', '3');
         INSERT INTO projects(id, key, name, prefix, created_at, updated_at)
             VALUES ('7KQ9A1X4MV2P8D6S', 'app', 'App', 'APP', 't', 't');
         INSERT INTO tasks(id, title, description, project_id, status, priority,
                 created_at, updated_at, source)
             VALUES ('7KQ9A1X4MV2P8D6T', 'first', 'body', '7KQ9A1X4MV2P8D6S',
                     'todo', 'high', 't', 't', 'ios'),
                    ('7KQ9A1X4MV2P8D6V', 'second', '', '7KQ9A1X4MV2P8D6S',
                     'done', 'none', 't', 't', 'cli');
         INSERT INTO task_dependencies
             VALUES ('0000000000000000', '7KQ9A1X4MV2P8D6T', '7KQ9A1X4MV2P8D6V', 't');
         INSERT INTO changes(change_id, client_id, local_seq, entity_type, entity_id,
                 field, op_type, payload, created_at, server_seq)
             VALUES ('X1', 'C1', 1, 'task', '7KQ9A1X4MV2P8D6T', 'title', 'set',
                     '\"first\"', 't', 5),
                    ('X2', 'C1', 2, 'task', '7KQ9A1X4MV2P8D6T', NULL, 'add_note',
                     '{}', 't', NULL);
         INSERT INTO notes(id, task_id, body, created_at, change_id)
             VALUES ('N1', '7KQ9A1X4MV2P8D6T', 'note', 't', 'X2');",
    )
    .execute(&pool)
    .await
    .unwrap();
    let dump = "SELECT group_concat(v, '|') FROM (
        SELECT key || '=' || value AS v FROM meta
            WHERE key IN ('client_id', 'sync_cursor', 'local_seq', 'sync_generation')
        UNION ALL SELECT id || title || description || status || priority || source FROM tasks
        UNION ALL SELECT task_id || depends_on_task_id FROM task_dependencies
        UNION ALL SELECT change_id || local_seq || payload || ifnull(server_seq, '-') FROM changes
        UNION ALL SELECT id || body || change_id FROM notes
        ORDER BY 1)";
    let before: String = sqlx::query_scalar(dump).fetch_one(&pool).await.unwrap();
    pool.close().await;

    let database = Database::open(&path).await.unwrap();
    let mut conn = database.acquire_reader().await.unwrap();
    let after: String = sqlx::query_scalar(dump)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(after, before);
    assert_eq!(
        current_schema_version(&mut conn).await.unwrap(),
        MIGRATOR.iter().last().unwrap().version
    );
    sqlx::query("UPDATE changes SET server_seq = 6 WHERE change_id = 'X2'")
        .execute(&mut *conn)
        .await
        .unwrap();
}

#[tokio::test]
async fn encrypted_sync_schema_bounds_match_protocol_limits() {
    let (_temp, mut conn) = crate::test_support::test_conn().await;
    let widest = vec![0_u8; crate::sync::bootstrap_format::MAX_DESCRIPTOR_BYTES];
    let insert = "INSERT INTO server_bootstrap_candidates
        (bootstrap, descriptor, canceled, expires_at, byte_budget, chunk_budget)
        VALUES (?, ?, 1, 0, 0, 0)";
    sqlx::query(insert)
        .bind([1_u8; 32].as_slice())
        .bind(&widest)
        .execute(&mut *conn)
        .await
        .unwrap();
    assert!(
        sqlx::query(insert)
            .bind([2_u8; 32].as_slice())
            .bind([widest.as_slice(), &[0]].concat())
            .execute(&mut *conn)
            .await
            .is_err()
    );
    let highest = crate::sync::seed_claim::membership::MAX_TRANSITIONS as i64 + 1;
    for (table, insert) in [
        (
            "server_membership_transitions",
            "INSERT INTO server_membership_transitions VALUES (?, NULL, x'01')",
        ),
        (
            "local_membership_checkpoint",
            "INSERT OR REPLACE INTO local_membership_checkpoint
             VALUES (1, zeroblob(32), ?, zeroblob(32), zeroblob(32))",
        ),
    ] {
        sqlx::query(insert)
            .bind(highest)
            .execute(&mut *conn)
            .await
            .unwrap();
        assert!(
            sqlx::query(insert)
                .bind(highest + 1)
                .execute(&mut *conn)
                .await
                .is_err(),
            "{table}"
        );
    }
}
