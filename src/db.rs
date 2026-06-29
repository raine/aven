use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde_json::Value;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteRow};
use sqlx::{Connection as _, Row, Sqlite, SqliteConnection, SqlitePool, Transaction};

use crate::choices::{TaskPriority, TaskStatus};
use crate::ids::{new_id, now};
use crate::types::Task;
use crate::workspaces::ensure_default_workspace;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
const MIGRATION_BACKUP_KEEP: usize = 20;

pub(crate) async fn open_db(path: &Path) -> Result<SqlitePool> {
    let existed_before_open = path.exists();
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
    backup_before_pending_migrations(path, existed_before_open, &pool).await?;
    MIGRATOR.run(&pool).await?;
    initialize_meta(&pool).await?;
    let mut conn = pool.acquire().await?;
    ensure_default_workspace(&mut conn).await?;
    Ok(pool)
}

async fn backup_before_pending_migrations(
    path: &Path,
    existed_before_open: bool,
    pool: &SqlitePool,
) -> Result<()> {
    if !migration_backups_enabled() || !existed_before_open || !has_pending_migrations(pool).await?
    {
        return Ok(());
    }
    let backup_path = migration_backup_path(path)?;
    run_sqlite_backup(path, &backup_path)?;
    prune_migration_backups(path)?;
    Ok(())
}

fn migration_backups_enabled() -> bool {
    std::env::var_os("AVEN_DEV_MIGRATION_BACKUPS").is_some()
}

async fn has_pending_migrations(pool: &SqlitePool) -> Result<bool> {
    let applied_versions =
        match sqlx::query_scalar::<_, i64>("SELECT version FROM _sqlx_migrations")
            .fetch_all(pool)
            .await
        {
            Ok(versions) => versions,
            Err(error) => {
                let Some(db_error) = error.as_database_error() else {
                    return Err(error.into());
                };
                if db_error.code().as_deref() == Some("1") {
                    return Ok(MIGRATOR.iter().next().is_some());
                }
                return Err(error.into());
            }
        };
    Ok(MIGRATOR
        .iter()
        .any(|migration| !applied_versions.contains(&migration.version)))
}

fn migration_backup_path(path: &Path) -> Result<PathBuf> {
    default_backup_path(path, "before-migrate")
}

pub(crate) fn default_backup_path(path: &Path, reason: &str) -> Result<PathBuf> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let backup_dir = parent.join("backups");
    fs::create_dir_all(&backup_dir)
        .with_context(|| format!("could not create {}", backup_dir.display()))?;
    let stem = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("db.sqlite");
    Ok(backup_dir.join(format!("{stem}.{reason}-{}.sqlite", backup_timestamp()?)))
}

pub(crate) fn backup_database(source: &Path, backup: &Path) -> Result<()> {
    if let Some(parent) = backup.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    run_sqlite_backup(source, backup)
}

pub(crate) fn wal_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}-wal", path.display()))
}

pub(crate) fn shm_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}-shm", path.display()))
}

pub(crate) async fn restore_database_file(target: &Path, source: &Path) -> Result<PathBuf> {
    validate_sqlite_source(source).await?;
    let safety = default_backup_path(target, "before-restore")?;
    backup_database(target, &safety)?;
    let staging = target.with_extension("restore-staging");
    if staging.exists() {
        fs::remove_file(&staging)
            .with_context(|| format!("could not remove {}", staging.display()))?;
    }
    fs::copy(source, &staging).with_context(|| {
        format!(
            "could not copy {} -> {}",
            source.display(),
            staging.display()
        )
    })?;
    for sidecar in [wal_path(target), shm_path(target)] {
        if sidecar.exists() {
            fs::remove_file(&sidecar)
                .with_context(|| format!("could not remove {}", sidecar.display()))?;
        }
    }
    fs::rename(&staging, target)
        .with_context(|| format!("could not replace {}", target.display()))?;
    Ok(safety)
}

async fn validate_sqlite_source(source: &Path) -> Result<()> {
    let mut conn = sqlx::SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(source)
            .read_only(true)
            .foreign_keys(true),
    )
    .await
    .with_context(|| format!("could not open source {}", source.display()))?;
    let quick_check: String = sqlx::query_scalar("PRAGMA quick_check")
        .fetch_one(&mut conn)
        .await?;
    if quick_check != "ok" {
        bail!("error backup-source-corrupt quick_check={quick_check}");
    }
    Ok(())
}

fn backup_timestamp() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before unix epoch")?
        .as_secs())
}

fn run_sqlite_backup(source: &Path, backup: &Path) -> Result<()> {
    let backup_sql = format!(".backup '{}'", sqlite_single_quoted(backup));
    let output = Command::new("sqlite3")
        .arg(source)
        .arg(backup_sql)
        .output()
        .context("could not run sqlite3 for backup")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "sqlite3 backup failed status={} stderr={}",
            output.status,
            stderr.trim()
        );
    }
    Ok(())
}

fn sqlite_single_quoted(path: &Path) -> String {
    path.display().to_string().replace('\'', "''")
}

fn prune_migration_backups(path: &Path) -> Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    let backup_dir = parent.join("backups");
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return Ok(());
    };
    let prefix = format!("{file_name}.before-migrate-");
    let mut backups = fs::read_dir(&backup_dir)
        .with_context(|| format!("could not read {}", backup_dir.display()))?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(".sqlite"))
        })
        .collect::<Vec<_>>();
    backups.sort_by_key(|entry| entry.file_name());
    let remove_count = backups.len().saturating_sub(MIGRATION_BACKUP_KEEP);
    for entry in backups.into_iter().take(remove_count) {
        let path = entry.path();
        fs::remove_file(&path).with_context(|| format!("could not remove {}", path.display()))?;
    }
    Ok(())
}

async fn initialize_meta(pool: &SqlitePool) -> Result<()> {
    let mut conn = pool.acquire().await?;
    insert_meta_if_missing(&mut conn, "client_id", &new_id()).await?;
    insert_meta_if_missing(&mut conn, "sync_cursor", "0").await?;
    insert_meta_if_missing(&mut conn, "local_seq", "0").await?;
    Ok(())
}

pub(crate) async fn current_schema_version(conn: &mut SqliteConnection) -> Result<i64> {
    let version: Option<i64> = sqlx::query_scalar("SELECT MAX(version) FROM _sqlx_migrations")
        .fetch_one(conn)
        .await?;
    Ok(version.unwrap_or(0))
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

pub(crate) async fn begin_immediate(
    conn: &mut SqliteConnection,
) -> sqlx::Result<Transaction<'_, Sqlite>> {
    conn.begin_with("BEGIN IMMEDIATE").await
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
        id: row.try_get("id")?,
        workspace_id: row.try_get("workspace_id")?,
        title: row.try_get("title")?,
        description: row.try_get("description")?,
        project_id: row.try_get("project_id")?,
        project_key: row.try_get("project_key")?,
        project_prefix: row.try_get("project_prefix")?,
        status: TaskStatus::parse(row.try_get::<String, _>("status")?.as_str())?,
        priority: TaskPriority::parse(row.try_get::<String, _>("priority")?.as_str())?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        queue_activity_at: row.try_get("queue_activity_at")?,
        deleted: row.try_get::<i64, _>("deleted")? != 0,
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

pub(crate) async fn task_has_conflict(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    task_id: &str,
) -> Result<bool> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM conflicts WHERE workspace_id = ? AND task_id = ? AND resolved = 0 LIMIT 1",
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_one(&mut *conn)
    .await?
        > 0)
}

pub(crate) async fn conflict_exists(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    task_id: &str,
    field: &str,
) -> Result<bool> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM conflicts WHERE workspace_id = ? AND task_id = ? AND field = ? AND resolved = 0 LIMIT 1",
    )
    .bind(workspace_id)
    .bind(task_id)
    .bind(field)
    .fetch_one(&mut *conn)
    .await?
        > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn task_from_row_rejects_invalid_status_and_priority() {
        let mut conn = SqliteConnection::connect(":memory:")
            .await
            .expect("open db");
        let row = sqlx::query(
            "SELECT 'TASK000000000001' AS id,
                    '0000000000000000' AS workspace_id,
                    'bad status' AS title,
                    '' AS description,
                    'PROJECT000000001' AS project_id,
                    'app' AS project_key,
                    'APP' AS project_prefix,
                    'blocked' AS status,
                    'none' AS priority,
                    't' AS created_at,
                    't' AS updated_at,
                    't' AS queue_activity_at,
                    0 AS deleted",
        )
        .fetch_one(&mut conn)
        .await
        .expect("row");
        assert_eq!(
            task_from_row(&row).unwrap_err().to_string(),
            "error invalid-status input=blocked choices=inbox,backlog,todo,active,done,canceled"
        );

        let row = sqlx::query(
            "SELECT 'TASK000000000001' AS id,
                    '0000000000000000' AS workspace_id,
                    'bad priority' AS title,
                    '' AS description,
                    'PROJECT000000001' AS project_id,
                    'app' AS project_key,
                    'APP' AS project_prefix,
                    'inbox' AS status,
                    'soon' AS priority,
                    't' AS created_at,
                    't' AS updated_at,
                    't' AS queue_activity_at,
                    0 AS deleted",
        )
        .fetch_one(&mut conn)
        .await
        .expect("row");
        assert_eq!(
            task_from_row(&row).unwrap_err().to_string(),
            "error invalid-priority input=soon choices=none,low,medium,high,urgent"
        );
    }
}
