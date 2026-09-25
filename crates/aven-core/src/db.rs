use crate::ids::WorkspaceId;
use std::fs;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{Connection as _, Sqlite, SqliteConnection, SqlitePool, Transaction};
use tokio::sync::{Mutex, OwnedMutexGuard};

use crate::ids::new_id;
use crate::workspaces::ensure_default_workspace;

mod backup;
mod changes;
mod field_versions;
mod inspection;
pub mod installation;
mod rows;

pub use backup::{
    backup_database, default_backup_path, default_sqlite_backup_path, restore_database_file,
    shm_path, wal_path,
};
pub(crate) use backup::{
    backup_database_with_connection, create_restore_safety_backup, detach_backup_snapshot,
    ensure_file_has_no_active_local_shared_capture,
};
pub(crate) use changes::{IdentifiedChange, insert_change, insert_change_with_identity};
pub(crate) use field_versions::{
    conflict_exists, entity_conflict_exists, entity_field_version, field_version,
    set_entity_field_version, set_field_version,
};
pub use inspection::{DatabaseInspection, InspectedDatabase};
pub(crate) use rows::{
    recurrence_occurrence_from_row, recurrence_pause_interval_from_row, recurrence_series_from_row,
    recurrence_series_label_from_row, task_from_row,
};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
const FILE_DATABASE_CONNECTIONS: u32 = 5;

#[derive(Clone)]
pub struct Database {
    pool: SqlitePool,
    writer: Arc<Mutex<()>>,
    path: PathBuf,
    file_identity: Option<PathBuf>,
    _inspection_dir: Option<Arc<tempfile::TempDir>>,
    pub(crate) membership_cache: crate::sync::seed_claim::membership::persistence::Cache,
}

pub(crate) struct WriterConnection {
    connection: sqlx::pool::PoolConnection<Sqlite>,
    _guard: OwnedMutexGuard<()>,
}

impl Deref for WriterConnection {
    type Target = SqliteConnection;

    fn deref(&self) -> &Self::Target {
        &self.connection
    }
}

impl DerefMut for WriterConnection {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.connection
    }
}

impl Database {
    pub async fn open(path: &Path) -> Result<Self> {
        let connection_input = path.to_string_lossy();
        let options = SqliteConnectOptions::from_str(&connection_input)?;
        let storage = database_storage(&connection_input, &options);
        let pool = open_db(path).await?;
        let file_identity = match storage {
            DatabaseStorage::InMemory => None,
            DatabaseStorage::File => {
                Some(fs::canonicalize(options.get_filename()).with_context(|| {
                    format!(
                        "could not resolve database path {}",
                        options.get_filename().display()
                    )
                })?)
            }
        };
        Ok(Self {
            pool,
            writer: Arc::new(Mutex::new(())),
            path: path.to_path_buf(),
            file_identity,
            _inspection_dir: None,
            membership_cache: Default::default(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    #[doc(hidden)]
    pub fn file_identity(&self) -> Option<&Path> {
        self.file_identity.as_deref()
    }

    pub async fn meta(&self, key: &str) -> Result<Option<String>> {
        let mut conn = self.acquire_reader().await?;
        get_meta(&mut conn, key).await
    }

    pub async fn conflict_exists(
        &self,
        workspace_id: &WorkspaceId,
        task_id: &crate::ids::TaskId,
        field: &str,
    ) -> Result<bool> {
        let mut conn = self.acquire_reader().await?;
        conflict_exists(&mut conn, workspace_id, task_id, field).await
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub(crate) async fn acquire_reader(&self) -> Result<sqlx::pool::PoolConnection<Sqlite>> {
        Ok(self.pool.acquire().await?)
    }

    pub(crate) async fn acquire_writer(&self) -> Result<WriterConnection> {
        let guard = self.writer.clone().lock_owned().await;
        let connection = self.pool.acquire().await?;
        Ok(WriterConnection {
            connection,
            _guard: guard,
        })
    }
}

pub(crate) async fn open_db(path: &Path) -> Result<SqlitePool> {
    let connection_input = path.to_string_lossy();
    let mut options = SqliteConnectOptions::from_str(&connection_input)?;
    let storage = database_storage(&connection_input, &options);
    let existed_before_open = storage == DatabaseStorage::File && path.exists();
    if storage == DatabaseStorage::File
        && let Some(parent) = path.parent()
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    options = options
        .create_if_missing(true)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    let pool_options = match storage {
        DatabaseStorage::InMemory => SqlitePoolOptions::new()
            .min_connections(1)
            .max_connections(1)
            .idle_timeout(None)
            .max_lifetime(None),
        DatabaseStorage::File => {
            options = options.journal_mode(SqliteJournalMode::Wal);
            SqlitePoolOptions::new().max_connections(FILE_DATABASE_CONNECTIONS)
        }
    };
    let pool = pool_options
        .connect_with(options)
        .await
        .with_context(|| format!("could not open {}", path.display()))?;
    backup::backup_before_pending_migrations(path, existed_before_open, &pool).await?;
    MIGRATOR.run(&pool).await?;
    initialize_meta(&pool).await?;
    let mut conn = pool.acquire().await?;
    crate::sync::protocol::replica_protocol(&mut conn).await?;
    ensure_default_workspace(&mut conn).await?;
    let mut tx = begin_immediate(&mut conn).await?;
    crate::epic_membership::recover(&mut tx, false).await?;
    tx.commit().await?;
    Ok(pool)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DatabaseStorage {
    InMemory,
    File,
}

fn database_storage(connection_input: &str, options: &SqliteConnectOptions) -> DatabaseStorage {
    let connection_input = connection_input
        .trim_start_matches("sqlite://")
        .trim_start_matches("sqlite:");
    let mut database_and_params = connection_input.splitn(2, '?');
    let database = database_and_params.next().unwrap_or_default();
    let uses_memory_mode = database_and_params.next().is_some_and(|params| {
        url::form_urlencoded::parse(params.as_bytes())
            .any(|(key, value)| key == "mode" && value == "memory")
    });
    let filename = options.get_filename();

    if database == ":memory:"
        || database == "file::memory:"
        || filename == Path::new(":memory:")
        || filename == Path::new("file::memory:")
        || uses_memory_mode
    {
        DatabaseStorage::InMemory
    } else {
        DatabaseStorage::File
    }
}

async fn initialize_meta(pool: &SqlitePool) -> Result<()> {
    let mut conn = pool.acquire().await?;
    insert_meta_if_missing(&mut conn, "client_id", &new_id()).await?;
    insert_meta_if_missing(&mut conn, "sync_cursor", "0").await?;
    insert_meta_if_missing(&mut conn, "local_seq", "0").await?;
    insert_meta_if_missing(&mut conn, "sync_generation", "0").await?;
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

#[cfg(test)]
mod tests;

#[cfg(test)]
mod payload_tests;
