use std::fs;
use std::io::Read as _;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use sqlx::Row;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use tokio::sync::Mutex;

use super::{Database, MIGRATOR, shm_path, wal_path};

#[derive(Debug, Clone)]
pub struct DatabaseInspection {
    pub exists: bool,
    pub is_file: bool,
    pub header_is_sqlite: bool,
    pub read_only_permissions: bool,
    pub wal_exists: bool,
    pub shm_exists: bool,
    pub schema_version: Option<i64>,
    pub latest_schema_version: Option<i64>,
    pub pending_migrations: Vec<i64>,
    pub unsupported_future_schema: bool,
    pub failed_migration: Option<i64>,
    pub open_error: Option<String>,
}

impl DatabaseInspection {
    fn unavailable(latest_schema_version: Option<i64>) -> Self {
        Self {
            exists: false,
            is_file: false,
            header_is_sqlite: false,
            read_only_permissions: false,
            wal_exists: false,
            shm_exists: false,
            schema_version: None,
            latest_schema_version,
            pending_migrations: Vec::new(),
            unsupported_future_schema: false,
            failed_migration: None,
            open_error: None,
        }
    }

    pub fn supports_runtime_checks(&self) -> bool {
        self.open_error.is_none()
            && self.failed_migration.is_none()
            && !self.unsupported_future_schema
            && self.pending_migrations.is_empty()
    }
}

pub struct InspectedDatabase {
    pub inspection: DatabaseInspection,
    pub database: Option<Database>,
}

impl Database {
    pub fn latest_schema_version() -> Option<i64> {
        MIGRATOR.iter().map(|migration| migration.version).max()
    }

    /// Copies an existing SQLite file and its sidecars into isolated temporary
    /// storage, then opens the isolated snapshot for diagnostic queries.
    ///
    /// This path does not create parent directories, databases, sidecars, metadata,
    /// workspaces, journal settings, or migrations in user state.
    pub async fn inspect(path: &Path) -> InspectedDatabase {
        let latest = Self::latest_schema_version();
        let mut inspection = DatabaseInspection::unavailable(latest);
        inspection.wal_exists = wal_path(path).exists();
        inspection.shm_exists = shm_path(path).exists();

        let metadata = match fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(error) => {
                inspection.open_error = Some(if error.kind() == std::io::ErrorKind::NotFound {
                    "database file does not exist".to_string()
                } else {
                    format!("database metadata is unavailable: {}", error.kind())
                });
                return InspectedDatabase {
                    inspection,
                    database: None,
                };
            }
        };
        inspection.exists = true;
        inspection.is_file = metadata.is_file();
        inspection.read_only_permissions = metadata.permissions().readonly();
        if !inspection.is_file {
            inspection.open_error = Some("database path is not a regular file".to_string());
            return InspectedDatabase {
                inspection,
                database: None,
            };
        }

        let mut header = [0_u8; 16];
        inspection.header_is_sqlite = fs::File::open(path)
            .and_then(|mut file| file.read_exact(&mut header))
            .is_ok()
            && &header == b"SQLite format 3\0";
        if !inspection.header_is_sqlite {
            inspection.open_error = Some("database header is not SQLite format 3".to_string());
            return InspectedDatabase {
                inspection,
                database: None,
            };
        }

        let inspection_dir = match tempfile::Builder::new().prefix("aven-doctor-").tempdir() {
            Ok(dir) => Arc::new(dir),
            Err(error) => {
                inspection.open_error = Some(format!(
                    "could not create isolated inspection storage: {error}"
                ));
                return InspectedDatabase {
                    inspection,
                    database: None,
                };
            }
        };
        let snapshot_path = inspection_dir.path().join("database.sqlite");
        if let Err(error) = fs::copy(path, &snapshot_path) {
            inspection.open_error =
                Some(format!("could not copy database for inspection: {error}"));
            return InspectedDatabase {
                inspection,
                database: None,
            };
        }
        for (source, target) in [
            (wal_path(path), wal_path(&snapshot_path)),
            (shm_path(path), shm_path(&snapshot_path)),
        ] {
            if source.exists()
                && let Err(error) = fs::copy(&source, &target)
            {
                inspection.open_error = Some(format!(
                    "could not copy SQLite sidecar {} for inspection: {error}",
                    source.display()
                ));
                return InspectedDatabase {
                    inspection,
                    database: None,
                };
            }
        }

        let options = SqliteConnectOptions::new()
            .filename(&snapshot_path)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5));
        let pool = match SqlitePoolOptions::new()
            .min_connections(1)
            .max_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_with(options)
            .await
        {
            Ok(pool) => pool,
            Err(error) => {
                inspection.open_error = Some(format!("SQLite read-only open failed: {error}"));
                return InspectedDatabase {
                    inspection,
                    database: None,
                };
            }
        };

        let migration_rows = sqlx::query("SELECT version, success FROM _sqlx_migrations")
            .fetch_all(&pool)
            .await;
        let mut applied = Vec::new();
        match migration_rows {
            Ok(rows) => {
                for row in rows {
                    let version = row.try_get::<i64, _>("version").unwrap_or_default();
                    if row.try_get::<bool, _>("success").unwrap_or(false) {
                        applied.push(version);
                    } else {
                        inspection.failed_migration = Some(version);
                    }
                }
                inspection.schema_version = applied.iter().copied().max().or(Some(0));
            }
            Err(error) => {
                let missing_table = error
                    .as_database_error()
                    .is_some_and(|error| error.code().as_deref() == Some("1"));
                if missing_table {
                    inspection.schema_version = Some(0);
                } else {
                    inspection.open_error = Some(format!("could not inspect schema: {error}"));
                }
            }
        }
        inspection.pending_migrations = MIGRATOR
            .iter()
            .filter(|migration| !applied.contains(&migration.version))
            .map(|migration| migration.version)
            .collect();
        inspection.unsupported_future_schema = inspection
            .schema_version
            .zip(latest)
            .is_some_and(|(current, latest)| current > latest);

        let database = inspection.supports_runtime_checks().then(|| Database {
            pool,
            writer: Arc::new(Mutex::new(())),
            path: path.to_path_buf(),
            file_identity: fs::canonicalize(path).ok(),
            _inspection_dir: Some(inspection_dir),
            membership_cache: Default::default(),
        });
        InspectedDatabase {
            inspection,
            database,
        }
    }
}
