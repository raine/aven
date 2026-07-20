use crate::ids::WorkspaceId;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use chrono::{NaiveDate, NaiveTime};
use serde_json::Value;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteRow};
use sqlx::{Connection as _, Row, Sqlite, SqliteConnection, SqlitePool, Transaction};

use crate::choices::{TaskPriority, TaskSource, TaskStatus};
use crate::ids::{new_id, now};
use crate::recurrence::{
    RecurrenceDuePolicy, RecurrenceFrequency, RecurrenceOutcome, RecurrenceProjectionState,
    RecurrenceRule, RecurrenceSeriesState, TimeZoneId, WeekdaySet,
};
use crate::types::{
    MutableEntityType, RecurrenceOccurrence, RecurrencePauseInterval, RecurrenceSeries,
    RecurrenceSeriesLabel, Task,
};
use crate::workspaces::ensure_default_workspace;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
const MIGRATION_BACKUP_KEEP: usize = 20;

#[derive(Clone)]
pub struct Database {
    pool: SqlitePool,
    path: PathBuf,
}

impl Database {
    pub async fn open(path: &Path) -> Result<Self> {
        Ok(Self {
            pool: open_db(path).await?,
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn latest_schema_version() -> Option<i64> {
        MIGRATOR.iter().map(|migration| migration.version).max()
    }

    pub async fn meta(&self, key: &str) -> Result<Option<String>> {
        let mut conn = self.acquire().await?;
        get_meta(&mut conn, key).await
    }

    pub async fn conflict_exists(
        &self,
        workspace_id: &WorkspaceId,
        task_id: &crate::ids::TaskId,
        field: &str,
    ) -> Result<bool> {
        let mut conn = self.acquire().await?;
        conflict_exists(&mut conn, workspace_id, task_id, field).await
    }

    pub(crate) async fn acquire(&self) -> Result<sqlx::pool::PoolConnection<Sqlite>> {
        Ok(self.pool.acquire().await?)
    }
}

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
    default_sqlite_backup_path(path, "before-migrate")
}

pub fn default_backup_path(path: &Path, reason: &str) -> Result<PathBuf> {
    backup_path_with_extension(path, reason, "aven-backup.tar.zst")
}

pub fn default_sqlite_backup_path(path: &Path, reason: &str) -> Result<PathBuf> {
    backup_path_with_extension(path, reason, "sqlite")
}

fn backup_path_with_extension(path: &Path, reason: &str, extension: &str) -> Result<PathBuf> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let backup_dir = parent.join("backups");
    fs::create_dir_all(&backup_dir)
        .with_context(|| format!("could not create {}", backup_dir.display()))?;
    let stem = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("db.sqlite");
    Ok(backup_dir.join(format!(
        "{stem}.{reason}-{}.{}",
        backup_timestamp()?,
        extension
    )))
}

pub fn backup_database(source: &Path, backup: &Path) -> Result<()> {
    if let Some(parent) = backup.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    run_sqlite_backup(source, backup)
}

pub fn wal_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}-wal", path.display()))
}

pub fn shm_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}-shm", path.display()))
}

pub async fn restore_database_file(target: &Path, source: &Path) -> Result<PathBuf> {
    validate_sqlite_source(source).await?;
    let safety = default_sqlite_backup_path(target, "before-restore")?;
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

pub(crate) struct IdentifiedChange<'a> {
    pub change_id: &'a str,
    pub entity_type: &'a str,
    pub entity_id: &'a str,
    pub field: Option<&'a str>,
    pub op_type: &'a str,
    pub payload: Value,
    pub base_version: Option<&'a str>,
    pub created_at: &'a str,
}

pub(crate) async fn insert_change_with_identity(
    conn: &mut SqliteConnection,
    change: IdentifiedChange<'_>,
) -> Result<()> {
    let IdentifiedChange {
        change_id,
        entity_type,
        entity_id,
        field,
        op_type,
        payload,
        base_version,
        created_at,
    } = change;
    let payload = payload.to_string();
    let existing = sqlx::query(
        "SELECT entity_type, entity_id, field, op_type, payload, base_version, created_at
         FROM changes WHERE change_id = ?",
    )
    .bind(change_id)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(existing) = existing {
        let equal = existing.try_get::<String, _>("entity_type")? == entity_type
            && existing.try_get::<String, _>("entity_id")? == entity_id
            && existing.try_get::<Option<String>, _>("field")?.as_deref() == field
            && existing.try_get::<String, _>("op_type")? == op_type
            && existing.try_get::<String, _>("payload")? == payload
            && existing
                .try_get::<Option<String>, _>("base_version")?
                .as_deref()
                == base_version
            && existing.try_get::<String, _>("created_at")? == created_at;
        if equal {
            return Ok(());
        }
        bail!("error recurrence-generation-conflict change_id={change_id}");
    }

    let client_id = get_meta(conn, "client_id")
        .await?
        .context("missing client id")?;
    let local_seq = next_local_seq(conn).await?;
    sqlx::query(
        "INSERT INTO changes(change_id, client_id, local_seq, entity_type, entity_id, field,
         op_type, payload, base_version, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(change_id)
    .bind(client_id)
    .bind(local_seq)
    .bind(entity_type)
    .bind(entity_id)
    .bind(field)
    .bind(op_type)
    .bind(payload)
    .bind(base_version)
    .bind(created_at)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

fn optional_task_date(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
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
        source: TaskSource::parse(row.try_get::<String, _>("source")?.as_str())?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        queue_activity_at: row.try_get("queue_activity_at")?,
        available_at: optional_task_date(row.try_get("available_at")?),
        due_on: optional_task_date(row.try_get("due_on")?),
        deleted: row.try_get::<i64, _>("deleted")? != 0,
        is_epic: row.try_get::<i64, _>("is_epic")? != 0,
    })
}

pub(crate) fn recurrence_series_from_row(row: &SqliteRow) -> Result<RecurrenceSeries> {
    let frequency = RecurrenceFrequency::parse(&row.try_get::<String, _>("frequency")?)?;
    let interval = u32::try_from(row.try_get::<i64, _>("interval")?)
        .context("recurrence interval must fit u32")?;
    let weekdays = row
        .try_get::<String, _>("weekdays")?
        .parse::<WeekdaySet>()
        .map_err(anyhow::Error::msg)?;
    let rule = RecurrenceRule::new(frequency, interval, weekdays)?;
    let start_on = row
        .try_get::<String, _>("start_on")?
        .parse::<NaiveDate>()
        .context("invalid recurrence start date")?;
    let available_local_time = optional_text(row.try_get("available_local_time")?)
        .map(|value| {
            value
                .parse::<NaiveTime>()
                .context("invalid recurrence availability time")
        })
        .transpose()?;
    let initial_status = TaskStatus::parse(&row.try_get::<String, _>("initial_status")?)?;
    if !initial_status.is_open() {
        bail!("recurrence initial status must be open");
    }
    let state = RecurrenceSeriesState::parse(&row.try_get::<String, _>("state")?)?;
    let stopped_at = optional_text(row.try_get("stopped_at")?);
    if matches!(state, RecurrenceSeriesState::Stopped) != stopped_at.is_some() {
        bail!("recurrence stopped state and stop time must agree");
    }
    let deleted = row.try_get::<i64, _>("deleted")?;
    if !matches!(deleted, 0 | 1) {
        bail!("recurrence deleted value must be zero or one");
    }
    Ok(RecurrenceSeries {
        workspace_id: row.try_get("workspace_id")?,
        id: row.try_get::<String, _>("id")?.parse()?,
        title: row.try_get("title")?,
        description: row.try_get("description")?,
        project_id: row.try_get("project_id")?,
        priority: TaskPriority::parse(&row.try_get::<String, _>("priority")?)?,
        initial_status,
        rule,
        timezone: row
            .try_get::<String, _>("timezone")?
            .parse::<TimeZoneId>()?,
        start_on,
        available_local_time,
        due_policy: RecurrenceDuePolicy::parse(&row.try_get::<String, _>("due_policy")?)?,
        state,
        stopped_at,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        deleted: deleted != 0,
    })
}

pub(crate) fn recurrence_series_label_from_row(row: &SqliteRow) -> Result<RecurrenceSeriesLabel> {
    Ok(RecurrenceSeriesLabel {
        workspace_id: row.try_get("workspace_id")?,
        series_id: row.try_get::<String, _>("series_id")?.parse()?,
        label: row.try_get("label")?,
    })
}

pub(crate) fn recurrence_occurrence_from_row(row: &SqliteRow) -> Result<RecurrenceOccurrence> {
    let task_id = optional_text(row.try_get("task_id")?)
        .map(|value| value.parse())
        .transpose()?;
    let outcome = optional_text(row.try_get("outcome")?)
        .map(|value| RecurrenceOutcome::parse(&value))
        .transpose()?;
    let resolved_at = optional_text(row.try_get("resolved_at")?);
    let outcome_change_id = optional_text(row.try_get("outcome_change_id")?);
    let projection_state =
        RecurrenceProjectionState::parse(&row.try_get::<String, _>("projection_state")?)?;
    let archived_at = optional_text(row.try_get("archived_at")?);
    let valid_shape = match projection_state {
        RecurrenceProjectionState::Projected => {
            task_id.is_some()
                && outcome.is_none()
                && resolved_at.is_none()
                && outcome_change_id.is_none()
                && archived_at.is_none()
        }
        RecurrenceProjectionState::Resolved => {
            task_id.is_some()
                && outcome.is_some()
                && resolved_at.is_some()
                && outcome_change_id.is_some()
                && archived_at.is_none()
        }
        RecurrenceProjectionState::Archived => {
            task_id.is_some()
                && outcome.is_none()
                && resolved_at.is_none()
                && outcome_change_id.is_none()
                && archived_at.is_some()
        }
        RecurrenceProjectionState::Corrected => {
            task_id.is_none()
                && outcome.is_some()
                && resolved_at.is_some()
                && outcome_change_id.is_some()
                && archived_at.is_none()
        }
    };
    if !valid_shape {
        bail!("recurrence occurrence fields do not match projection state");
    }
    Ok(RecurrenceOccurrence {
        workspace_id: row.try_get("workspace_id")?,
        series_id: row.try_get::<String, _>("series_id")?.parse()?,
        slot_on: row
            .try_get::<String, _>("slot_on")?
            .parse::<NaiveDate>()
            .context("invalid recurrence slot date")?,
        task_id,
        outcome,
        resolved_at,
        outcome_change_id,
        projection_state,
        archived_at,
    })
}

pub(crate) fn recurrence_pause_interval_from_row(
    row: &SqliteRow,
) -> Result<RecurrencePauseInterval> {
    let suspended_slot_on = optional_text(row.try_get("suspended_slot_on")?)
        .map(|value| {
            value
                .parse::<NaiveDate>()
                .context("invalid suspended recurrence slot date")
        })
        .transpose()?;
    let suspended_task_id = optional_text(row.try_get("suspended_task_id")?)
        .map(|value| value.parse())
        .transpose()?;
    let resumed_at = optional_text(row.try_get("resumed_at")?);
    let resolved_by_change_id = optional_text(row.try_get("resolved_by_change_id")?);
    if resumed_at.is_some() != resolved_by_change_id.is_some() {
        bail!("recurrence pause resume time and change must agree");
    }
    if suspended_slot_on.is_some() != suspended_task_id.is_some() {
        bail!("recurrence suspended slot and task must agree");
    }
    Ok(RecurrencePauseInterval {
        workspace_id: row.try_get("workspace_id")?,
        id: row.try_get("id")?,
        series_id: row.try_get::<String, _>("series_id")?.parse()?,
        paused_at: row.try_get("paused_at")?,
        resumed_at,
        suspended_slot_on,
        suspended_task_id,
        created_by_change_id: row.try_get("created_by_change_id")?,
        resolved_by_change_id,
    })
}

fn optional_text(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

pub(crate) async fn entity_field_version(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    entity_type: MutableEntityType,
    entity_id: &str,
    field: &str,
) -> Result<Option<String>> {
    Ok(sqlx::query_scalar(
        "SELECT version FROM field_versions
         WHERE workspace_id = ? AND entity_type = ? AND entity_id = ? AND field = ?",
    )
    .bind(workspace_id)
    .bind(entity_type.as_str())
    .bind(entity_id)
    .bind(field)
    .fetch_optional(&mut *conn)
    .await?)
}

pub(crate) async fn set_entity_field_version(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    entity_type: MutableEntityType,
    entity_id: &str,
    field: &str,
    version: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO field_versions(workspace_id, entity_type, entity_id, field, version)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(workspace_id, entity_type, entity_id, field)
         DO UPDATE SET version = excluded.version",
    )
    .bind(workspace_id)
    .bind(entity_type.as_str())
    .bind(entity_id)
    .bind(field)
    .bind(version)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

pub(crate) async fn field_version(
    conn: &mut SqliteConnection,
    entity_id: &str,
    field: &str,
) -> Result<Option<String>> {
    let workspace_id =
        sqlx::query_scalar::<_, WorkspaceId>("SELECT workspace_id FROM tasks WHERE id = ? LIMIT 1")
            .bind(entity_id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(workspace_id) = workspace_id else {
        return Ok(None);
    };
    entity_field_version(
        conn,
        &workspace_id,
        MutableEntityType::Task,
        entity_id,
        field,
    )
    .await
}

pub(crate) async fn set_field_version(
    conn: &mut SqliteConnection,
    entity_id: &str,
    field: &str,
    version: &str,
) -> Result<()> {
    let workspace_id =
        sqlx::query_scalar::<_, WorkspaceId>("SELECT workspace_id FROM tasks WHERE id = ? LIMIT 1")
            .bind(entity_id)
            .fetch_optional(&mut *conn)
            .await?;
    if let Some(workspace_id) = workspace_id {
        set_entity_field_version(
            conn,
            &workspace_id,
            MutableEntityType::Task,
            entity_id,
            field,
            version,
        )
        .await?;
    }
    Ok(())
}

pub(crate) async fn entity_conflict_exists(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    entity_type: MutableEntityType,
    entity_id: &str,
    field: &str,
) -> Result<bool> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM conflicts
         WHERE workspace_id = ? AND entity_type = ? AND entity_id = ?
         AND field = ? AND resolved = 0 LIMIT 1",
    )
    .bind(workspace_id)
    .bind(entity_type.as_str())
    .bind(entity_id)
    .bind(field)
    .fetch_one(&mut *conn)
    .await?
        > 0)
}

pub(crate) async fn conflict_exists(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: &crate::ids::TaskId,
    field: &str,
) -> Result<bool> {
    entity_conflict_exists(
        conn,
        workspace_id,
        MutableEntityType::Task,
        task_id.as_str(),
        field,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn task_from_row_maps_empty_dates_to_absence() {
        let mut conn = SqliteConnection::connect(":memory:")
            .await
            .expect("open db");
        let row = sqlx::query(
            "SELECT 'TASK000000000001' AS id,
                    '0000000000000000' AS workspace_id,
                    'optional dates' AS title,
                    '' AS description,
                    '0000000000000001' AS project_id,
                    'app' AS project_key,
                    'APP' AS project_prefix,
                    'todo' AS status,
                    'none' AS priority,
                    'unknown' AS source,
                    't' AS created_at,
                    't' AS updated_at,
                    't' AS queue_activity_at,
                    '' AS available_at,
                    '' AS due_on,
                    0 AS deleted,
                    0 AS is_epic",
        )
        .fetch_one(&mut conn)
        .await
        .expect("row");

        let task = task_from_row(&row).unwrap();

        assert_eq!(task.available_at, None);
        assert_eq!(task.due_on, None);
    }

    #[test]
    fn task_date_boundary_preserves_present_values() {
        assert_eq!(
            optional_task_date("2099-01-01T00:00:00Z".to_string()).as_deref(),
            Some("2099-01-01T00:00:00Z")
        );
        assert_eq!(
            optional_task_date("2099-01-01".to_string()).as_deref(),
            Some("2099-01-01")
        );
    }

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
                    '0000000000000001' AS project_id,
                    'app' AS project_key,
                    'APP' AS project_prefix,
                    'blocked' AS status,
                    'none' AS priority,
                    'unknown' AS source,
                    't' AS created_at,
                    't' AS updated_at,
                    't' AS queue_activity_at,
                    0 AS deleted,
                    0 AS is_epic",
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
                    '0000000000000001' AS project_id,
                    'app' AS project_key,
                    'APP' AS project_prefix,
                    'inbox' AS status,
                    'soon' AS priority,
                    't' AS created_at,
                    't' AS updated_at,
                    't' AS queue_activity_at,
                    0 AS deleted,
                    0 AS is_epic",
        )
        .fetch_one(&mut conn)
        .await
        .expect("row");
        assert_eq!(
            task_from_row(&row).unwrap_err().to_string(),
            "error invalid-priority input=soon choices=none,low,medium,high,urgent"
        );
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

    #[tokio::test]
    async fn recurrence_rows_map_through_validated_domain_types() {
        let mut conn = SqliteConnection::connect(":memory:").await.unwrap();
        let series_row = sqlx::query(
            "SELECT '0000000000000000' AS workspace_id,
                    '7KQ9A1X4MV2P8D6R' AS id, 'journal' AS title, '' AS description,
                    '7KQ9A1X4MV2P8D6S' AS project_id, 'high' AS priority,
                    'todo' AS initial_status, 'weekly' AS frequency, 2 AS interval,
                    'mon,fri' AS weekdays, 'Europe/Stockholm' AS timezone,
                    '2026-07-20' AS start_on, '09:30:00' AS available_local_time,
                    'same_day' AS due_policy, 'active' AS state, '' AS stopped_at,
                    'created' AS created_at, 'updated' AS updated_at, 0 AS deleted",
        )
        .fetch_one(&mut conn)
        .await
        .unwrap();
        let series = recurrence_series_from_row(&series_row).unwrap();
        assert_eq!(series.id.as_str(), "7KQ9A1X4MV2P8D6R");
        assert_eq!(series.rule.interval(), 2);
        assert_eq!(series.available_local_time.unwrap().to_string(), "09:30:00");

        let occurrence_row = sqlx::query(
            "SELECT '0000000000000000' AS workspace_id,
                    '7KQ9A1X4MV2P8D6R' AS series_id, '2026-07-20' AS slot_on,
                    '' AS task_id, 'completed' AS outcome, 'resolved' AS resolved_at,
                    'change' AS outcome_change_id, 'corrected' AS projection_state,
                    '' AS archived_at",
        )
        .fetch_one(&mut conn)
        .await
        .unwrap();
        let occurrence = recurrence_occurrence_from_row(&occurrence_row).unwrap();
        assert_eq!(occurrence.task_id, None);
        assert_eq!(occurrence.outcome, Some(RecurrenceOutcome::Completed));
        assert_eq!(
            occurrence.projection_state,
            RecurrenceProjectionState::Corrected
        );

        let invalid_row = sqlx::query(
            "SELECT '0000000000000000' AS workspace_id,
                    '7KQ9A1X4MV2P8D6R' AS id, 'journal' AS title, '' AS description,
                    '7KQ9A1X4MV2P8D6S' AS project_id, 'none' AS priority,
                    'todo' AS initial_status, 'weekly' AS frequency, 1 AS interval,
                    'fri,mon' AS weekdays, 'UTC' AS timezone, '2026-07-20' AS start_on,
                    '' AS available_local_time, 'none' AS due_policy, 'active' AS state,
                    '' AS stopped_at, 't' AS created_at, 't' AS updated_at, 0 AS deleted",
        )
        .fetch_one(&mut conn)
        .await
        .unwrap();
        assert!(recurrence_series_from_row(&invalid_row).is_err());
    }

    #[tokio::test]
    async fn field_versions_support_task_and_recurrence_series_identity() {
        let (_temp, mut conn) = crate::test_support::test_conn().await;
        sqlx::query(
            "INSERT INTO tasks(id, title, description, project_id, status, priority, created_at, updated_at)
             VALUES ('7KQ9A1X4MV2P8D6T', 'task', '', '7KQ9A1X4MV2P8D6S', 'todo', 'none', 't', 't')",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
        set_field_version(&mut conn, "7KQ9A1X4MV2P8D6T", "title", "task-version")
            .await
            .unwrap();
        set_entity_field_version(
            &mut conn,
            &crate::workspaces::default_workspace_id(),
            MutableEntityType::RecurrenceSeries,
            "7KQ9A1X4MV2P8D6R",
            "title",
            "series-version",
        )
        .await
        .unwrap();

        assert_eq!(
            field_version(&mut conn, "7KQ9A1X4MV2P8D6T", "title")
                .await
                .unwrap()
                .as_deref(),
            Some("task-version")
        );
        assert_eq!(
            entity_field_version(
                &mut conn,
                &crate::workspaces::default_workspace_id(),
                MutableEntityType::RecurrenceSeries,
                "7KQ9A1X4MV2P8D6R",
                "title",
            )
            .await
            .unwrap()
            .as_deref(),
            Some("series-version")
        );
    }
}
