use sqlx::{Connection, SqliteConnection};

#[tokio::test]
async fn ios_source_migration_preserves_existing_sources_and_schema_objects() {
    assert_source_migration(20260913060740, &["cli", "tui", "api", "unknown"]).await;
}

#[tokio::test]
async fn android_source_migration_preserves_existing_sources_and_schema_objects() {
    assert_source_migration(20260913105309, &["cli", "tui", "api", "ios", "unknown"]).await;
}

async fn assert_source_migration(version: i64, old_sources: &[&str]) {
    let mut connection = SqliteConnection::connect("sqlite::memory:").await.unwrap();
    let migrator = sqlx::migrate!("./migrations");
    let migration = migrator
        .iter()
        .find(|migration| migration.version == version)
        .unwrap();
    for previous in migrator
        .iter()
        .filter(|previous| previous.version < migration.version)
    {
        sqlx::raw_sql(previous.sql.clone())
            .execute(&mut connection)
            .await
            .unwrap();
    }
    for (index, source) in old_sources.iter().enumerate() {
        sqlx::query(
            "INSERT INTO tasks(id, title, description, project_id, status, priority,
             created_at, updated_at, source)
             VALUES (?, 'existing task', '', '0000000000000000', 'inbox', 'none',
             '2026-09-01T00:00:00Z', '2026-09-01T00:00:00Z', ?)",
        )
        .bind(format!("{index:016}"))
        .bind(source)
        .execute(&mut connection)
        .await
        .unwrap();
    }
    let objects_sql = "SELECT name, sql FROM sqlite_schema
                       WHERE type IN ('index', 'trigger') ORDER BY name";
    let before: Vec<(String, Option<String>)> = sqlx::query_as(objects_sql)
        .fetch_all(&mut connection)
        .await
        .unwrap();
    sqlx::raw_sql(migration.sql.clone())
        .execute(&mut connection)
        .await
        .unwrap();
    let after: Vec<(String, Option<String>)> = sqlx::query_as(objects_sql)
        .fetch_all(&mut connection)
        .await
        .unwrap();
    assert_eq!(before, after);
    let sources: Vec<String> = sqlx::query_scalar("SELECT source FROM tasks ORDER BY id")
        .fetch_all(&mut connection)
        .await
        .unwrap();
    assert_eq!(sources, old_sources);
    sqlx::query("UPDATE tasks SET source = 'ios' WHERE source = 'unknown'")
        .execute(&mut connection)
        .await
        .unwrap();
    assert!(
        sqlx::query("UPDATE tasks SET source = 'Invalid'")
            .execute(&mut connection)
            .await
            .is_err()
    );
}

#[test]
fn ios_source_protocol_rejects_older_peers_and_source_mutations() {
    use aven_core::sync::wire::{
        SYNC_PROTOCOL_VERSION, validate_sync_protocol_version,
        validate_sync_request_protocol_version,
    };

    for version in [16, 17] {
        assert!(validate_sync_request_protocol_version(Some(version)).is_err());
        assert!(validate_sync_protocol_version(SYNC_PROTOCOL_VERSION, version).is_err());
    }
    assert!(validate_sync_request_protocol_version(Some(SYNC_PROTOCOL_VERSION)).is_ok());
    assert!(aven_core::task_fields::TaskField::parse("source").is_none());
}

#[tokio::test]
async fn source_parser_and_database_accept_the_same_fixed_values() {
    use aven_core::choices::{TASK_SOURCES, TaskSource};

    let mut connection = SqliteConnection::connect("sqlite::memory:").await.unwrap();
    sqlx::migrate!("./migrations")
        .run(&mut connection)
        .await
        .unwrap();
    let mut values: Vec<String> = TASK_SOURCES.iter().map(|value| value.to_string()).collect();
    values.extend([
        "future-client".to_string(),
        "agent".to_string(),
        String::new(),
        "Android".to_string(),
        "android ".to_string(),
        "android\0suffix".to_string(),
    ]);
    for value in values {
        let parsed = TaskSource::parse(&value);
        let expected_valid = TASK_SOURCES.contains(&value.as_str());
        assert_eq!(parsed.is_ok(), expected_valid, "source={value:?}");
        let stored = sqlx::query(
            "INSERT OR REPLACE INTO tasks(id, title, description, project_id, status,
             priority, created_at, updated_at, source)
             VALUES ('0000000000000001', 'source', '', '0000000000000000',
             'inbox', 'none', '2026-09-13T00:00:00Z', '2026-09-13T00:00:00Z', ?)",
        )
        .bind(&value)
        .execute(&mut connection)
        .await;
        assert_eq!(parsed.is_ok(), stored.is_ok(), "source={value:?}");
        if let Ok(parsed) = parsed {
            let persisted: String = sqlx::query_scalar("SELECT source FROM tasks")
                .fetch_one(&mut connection)
                .await
                .unwrap();
            assert_eq!(persisted, value);
            assert_eq!(parsed.as_str(), value);
            assert_eq!(parsed.to_string(), value);
        }
    }
    assert_eq!(TaskSource::parse("android").unwrap(), TaskSource::Android);
    assert_eq!(TaskSource::default(), TaskSource::Unknown);
}
