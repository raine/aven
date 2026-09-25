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

async fn before_bootstrap_catalog_slices(path: &Path) -> SqlitePool {
    let options = SqliteConnectOptions::from_str(&path.to_string_lossy())
        .unwrap()
        .create_if_missing(true)
        .foreign_keys(true);
    let pool = SqlitePool::connect_with(options).await.unwrap();
    MIGRATOR.run_to(20260924091148, &pool).await.unwrap();
    pool
}

#[tokio::test]
async fn old_bootstrap_state_refuses_the_catalog_slice_migration_unchanged() {
    const JOURNAL: &str = "INSERT INTO local_shared_capture_journal(singleton, candidate_id,
        stream_id, state, internal_format, internal_version, snapshot_json,
        local_seq_floor, sync_generation, created_at, frozen_descriptor_commitment)
        VALUES (1, 'c', 's', 'never_dispatched', 'f', 1, '{}', 0, 1, 't', zeroblob(32));";
    const CANDIDATE: &str = "INSERT INTO server_bootstrap_candidates
        VALUES (zeroblob(32), x'41564250000101', 0, 3, 9, 10, 2, 0, 0);";
    for (name, setup) in [
        ("frozen marker", JOURNAL.to_string()),
        (
            "frozen package",
            format!(
                "{JOURNAL} INSERT INTO local_shared_capture_publication
                 VALUES ('c', x'41564250000101', x'00', x'01', x'02');"
            ),
        ),
        (
            "seed intent",
            "INSERT INTO local_seed_publication_intent(singleton, candidate_id, intent, state)
             VALUES (1, 'c', x'7b7d', 'sealed');"
                .to_string(),
        ),
        ("candidate", CANDIDATE.to_string()),
        (
            "quarantined slice",
            format!(
                "{CANDIDATE} INSERT INTO server_bootstrap_chunks VALUES (zeroblob(32), x'01', 0, 0, x'bb');"
            ),
        ),
        (
            "publication",
            format!(
                "{CANDIDATE} INSERT INTO server_bootstrap_publication
                 VALUES (1, zeroblob(32), x'41564250000101', zeroblob(805), 5);"
            ),
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("old.sqlite");
        let pool = before_bootstrap_catalog_slices(&path).await;
        sqlx::raw_sql(sqlx::AssertSqlSafe(setup))
            .execute(&pool)
            .await
            .unwrap();
        let dump = "SELECT group_concat(hex(v), ',') FROM (
            SELECT descriptor AS v FROM local_shared_capture_publication
            UNION ALL SELECT frozen_descriptor_commitment FROM local_shared_capture_journal
            UNION ALL SELECT intent FROM local_seed_publication_intent
            UNION ALL SELECT descriptor FROM server_bootstrap_candidates
            UNION ALL SELECT quote(verified) || hex(bytes) FROM server_bootstrap_chunks
            UNION ALL SELECT descriptor FROM server_bootstrap_publication)";
        let before: Option<String> = sqlx::query_scalar(dump).fetch_one(&pool).await.unwrap();
        let error = MIGRATOR.run(&pool).await.unwrap_err().to_string();
        assert!(
            error.contains("error bootstrap-development-format-unsupported"),
            "{name}: {error}"
        );
        let after: Option<String> = sqlx::query_scalar(dump).fetch_one(&pool).await.unwrap();
        assert_eq!(after, before, "{name}");
        let version: i64 = sqlx::query_scalar("SELECT max(version) FROM _sqlx_migrations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(version, 20260924091148, "{name}");
        pool.close().await;
        // Opt-in development backups refuse active captures first; either way
        // the open fails and nothing changes.
        assert!(Database::open(&path).await.is_err(), "{name}");
        let pool = SqlitePool::connect(&format!("sqlite:{}", path.display()))
            .await
            .unwrap();
        let reopened: Option<String> = sqlx::query_scalar(dump).fetch_one(&pool).await.unwrap();
        assert_eq!(reopened, before, "{name}");
    }
}

#[tokio::test]
async fn unpopulated_bootstrap_tables_take_the_catalog_slice_schema() {
    let temp = tempfile::tempdir().unwrap();
    let pool = before_bootstrap_catalog_slices(&temp.path().join("fresh.sqlite")).await;
    MIGRATOR.run(&pool).await.unwrap();
    for (table, column) in [
        ("server_bootstrap_chunks", "verified"),
        ("server_bootstrap_candidates", "catalog_failure"),
        ("server_bootstrap_candidates", "failure_reason"),
    ] {
        let present: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pragma_table_info(?) WHERE name = ?)")
                .bind(table)
                .bind(column)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(!present, "{table}.{column}");
    }
    let widest = vec![0_u8; crate::sync::bootstrap_format::MAX_DESCRIPTOR_BYTES];
    let insert = "INSERT INTO server_bootstrap_candidates VALUES (?, ?, 1, 1, 0, 0, 0)";
    sqlx::query(insert)
        .bind([1_u8; 32].as_slice())
        .bind(&widest)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        sqlx::query(insert)
            .bind([2_u8; 32].as_slice())
            .bind([widest.as_slice(), &[0]].concat())
            .execute(&pool)
            .await
            .is_err()
    );
}

const BEFORE_MEMBERSHIP_LIMITS: i64 = 20260924164546;

async fn before_membership_limits(path: &Path) -> SqlitePool {
    let pool = before_bootstrap_catalog_slices(path).await;
    MIGRATOR
        .run_to(BEFORE_MEMBERSHIP_LIMITS, &pool)
        .await
        .unwrap();
    pool
}

#[tokio::test]
async fn enrolled_devices_refuse_the_membership_limit_migration_unchanged() {
    const ENROLLMENT: &str =
        "INSERT INTO local_peer_enrollment VALUES (1, zeroblob(32), 'c', 'peer');";
    for (name, setup) in [
        ("enrollment", ENROLLMENT.to_string()),
        (
            "checkpoint",
            format!(
                "{ENROLLMENT} INSERT INTO local_membership_checkpoint
                 VALUES (1, zeroblob(32), 129, zeroblob(32), zeroblob(32));"
            ),
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("old.sqlite");
        let pool = before_membership_limits(&path).await;
        sqlx::raw_sql(sqlx::AssertSqlSafe(setup))
            .execute(&pool)
            .await
            .unwrap();
        let dump = "SELECT group_concat(v, ',') FROM (
            SELECT role AS v FROM local_peer_enrollment
            UNION ALL SELECT sequence FROM local_membership_checkpoint)";
        let before: Option<String> = sqlx::query_scalar(dump).fetch_one(&pool).await.unwrap();
        let error = MIGRATOR.run(&pool).await.unwrap_err().to_string();
        assert!(
            error.contains("error membership-development-format-unsupported"),
            "{name}: {error}"
        );
        let after: Option<String> = sqlx::query_scalar(dump).fetch_one(&pool).await.unwrap();
        assert_eq!(after, before, "{name}");
        let version: i64 = sqlx::query_scalar("SELECT max(version) FROM _sqlx_migrations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(version, BEFORE_MEMBERSHIP_LIMITS, "{name}");
    }
}

#[tokio::test]
async fn server_membership_rows_survive_the_membership_limit_migration() {
    let temp = tempfile::tempdir().unwrap();
    let pool = before_membership_limits(&temp.path().join("server.sqlite")).await;
    sqlx::query("INSERT INTO server_membership_transitions VALUES (129, NULL, x'01')")
        .execute(&pool)
        .await
        .unwrap();
    MIGRATOR.run(&pool).await.unwrap();
    let kept: Vec<u8> =
        sqlx::query_scalar("SELECT record FROM server_membership_transitions WHERE sequence = 129")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(kept, [1]);
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
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            sqlx::query(insert)
                .bind(highest + 1)
                .execute(&pool)
                .await
                .is_err(),
            "{table}"
        );
    }
}
