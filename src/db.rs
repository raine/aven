use std::fs;
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::Value;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteRow};
use sqlx::{Row, SqliteConnection, SqlitePool};

use crate::{Task, new_id, now};

pub(crate) async fn open_db(path: &Path) -> Result<SqlitePool> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    let options = SqliteConnectOptions::from_str(&path.display().to_string())?
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .with_context(|| format!("could not open {}", path.display()))?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    initialize_meta(&pool).await?;
    Ok(pool)
}

async fn initialize_meta(pool: &SqlitePool) -> Result<()> {
    let mut conn = pool.acquire().await?;
    insert_meta_if_missing(&mut conn, "client_id", &new_id()).await?;
    insert_meta_if_missing(&mut conn, "sync_cursor", "0").await?;
    insert_meta_if_missing(&mut conn, "local_seq", "0").await?;
    Ok(())
}

pub(crate) async fn get_meta(conn: &mut SqliteConnection, key: &str) -> Result<Option<String>> {
    Ok(
        sqlx::query_scalar!("SELECT value FROM meta WHERE key = ?", key)
            .fetch_optional(&mut *conn)
            .await?,
    )
}

pub(crate) async fn set_meta(conn: &mut SqliteConnection, key: &str, value: &str) -> Result<()> {
    sqlx::query!(
        "INSERT INTO meta(key, value) VALUES (?, ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        key,
        value,
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

async fn insert_meta_if_missing(conn: &mut SqliteConnection, key: &str, value: &str) -> Result<()> {
    sqlx::query!(
        "INSERT OR IGNORE INTO meta(key, value) VALUES (?, ?)",
        key,
        value
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

async fn next_local_seq(conn: &mut SqliteConnection) -> Result<i64> {
    let seq = get_meta(conn, "local_seq")
        .await?
        .unwrap_or_else(|| "0".to_string())
        .parse::<i64>()?
        + 1;
    set_meta(conn, "local_seq", &seq.to_string()).await?;
    Ok(seq)
}

pub(crate) async fn insert_change(
    conn: &mut SqliteConnection,
    entity_type: &str,
    entity_id: &str,
    field: Option<&str>,
    op_type: &str,
    payload: Value,
    base_version: Option<&str>,
) -> Result<String> {
    let change_id = new_id();
    let client_id = get_meta(conn, "client_id")
        .await?
        .context("missing client id")?;
    let local_seq = next_local_seq(conn).await?;
    let created_at = now();
    let payload = payload.to_string();
    sqlx::query!(
        "INSERT INTO changes(change_id, client_id, local_seq, entity_type, entity_id, field,
         op_type, payload, base_version, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        change_id,
        client_id,
        local_seq,
        entity_type,
        entity_id,
        field,
        op_type,
        payload,
        base_version,
        created_at,
    )
    .execute(&mut *conn)
    .await?;
    Ok(change_id)
}

pub(crate) fn task_from_row(row: &SqliteRow) -> Result<Task> {
    Ok(Task {
        id: row.try_get(0)?,
        title: row.try_get(1)?,
        description: row.try_get(2)?,
        project_key: row.try_get(3)?,
        project_prefix: row.try_get(4)?,
        status: row.try_get(5)?,
        priority: row.try_get(6)?,
        created_at: row.try_get(7)?,
        updated_at: row.try_get(8)?,
        deleted: row.try_get::<i64, _>(9)? != 0,
    })
}

pub(crate) async fn field_version(
    conn: &mut SqliteConnection,
    entity_id: &str,
    field: &str,
) -> Result<Option<String>> {
    Ok(
        sqlx::query_scalar!(
            r#"SELECT version AS "version!: String" FROM field_versions WHERE entity_id = ? AND field = ?"#,
            entity_id,
            field
        )
            .fetch_optional(&mut *conn)
            .await?,
    )
}

pub(crate) async fn set_field_version(
    conn: &mut SqliteConnection,
    entity_id: &str,
    field: &str,
    version: &str,
) -> Result<()> {
    sqlx::query!(
        "INSERT INTO field_versions(entity_id, field, version) VALUES (?, ?, ?)
         ON CONFLICT(entity_id, field) DO UPDATE SET version = excluded.version",
        entity_id,
        field,
        version,
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

pub(crate) async fn task_has_conflict(conn: &mut SqliteConnection, task_id: &str) -> Result<bool> {
    Ok(
        sqlx::query_scalar!(
            r#"SELECT count(*) AS "count!: i64" FROM conflicts WHERE task_id = ? AND resolved = 0 LIMIT 1"#,
            task_id,
        )
        .fetch_one(&mut *conn)
        .await?
            > 0,
    )
}

pub(crate) async fn conflict_exists(
    conn: &mut SqliteConnection,
    task_id: &str,
    field: &str,
) -> Result<bool> {
    Ok(
        sqlx::query_scalar!(
            r#"SELECT count(*) AS "count!: i64" FROM conflicts WHERE task_id = ? AND field = ? AND resolved = 0 LIMIT 1"#,
            task_id,
            field,
        )
        .fetch_one(&mut *conn)
        .await?
            > 0,
    )
}
