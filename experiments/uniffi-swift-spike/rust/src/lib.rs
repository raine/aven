use std::sync::{Arc, OnceLock};
use std::time::Duration;

use sqlx::sqlite::SqlitePoolOptions;
use sqlx::{Row, SqlitePool};
use tokio::runtime::{Builder, Runtime};

#[derive(Clone, Debug, uniffi::Enum)]
pub enum PayloadKind {
    Binary,
    Text,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct ProbeSnapshot {
    pub row_count: u64,
    pub name: String,
    pub nickname: Option<String>,
    pub payload: Vec<u8>,
    pub kind: PayloadKind,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum ProbeError {
    #[error("database operation failed: {reason}")]
    Database { reason: String },
    #[error("runtime initialization failed: {reason}")]
    Runtime { reason: String },
    #[error("intentional typed failure")]
    Intentional,
}

impl ProbeError {
    fn database(error: impl std::fmt::Display) -> Self {
        Self::Database {
            reason: error.to_string(),
        }
    }
}

static OWNED_RUNTIME: OnceLock<Result<Runtime, String>> = OnceLock::new();

fn owned_runtime() -> Result<&'static Runtime, ProbeError> {
    match OWNED_RUNTIME.get_or_init(|| {
        Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|error| error.to_string())
    }) {
        Ok(runtime) => Ok(runtime),
        Err(reason) => Err(ProbeError::Runtime {
            reason: reason.clone(),
        }),
    }
}

#[derive(uniffi::Object)]
pub struct ProbeStore {
    pool: SqlitePool,
}

#[uniffi::export]
impl ProbeStore {
    #[uniffi::constructor]
    pub fn new() -> Result<Arc<Self>, ProbeError> {
        let runtime = owned_runtime()?;

        let pool = runtime.block_on(async {
            let pool = SqlitePoolOptions::new()
                .max_connections(1)
                .connect("sqlite::memory:")
                .await
                .map_err(ProbeError::database)?;
            sqlx::query(
                "CREATE TABLE proof (\
                    id INTEGER PRIMARY KEY, \
                    name TEXT NOT NULL, \
                    nickname TEXT, \
                    payload BLOB NOT NULL, \
                    kind TEXT NOT NULL\
                )",
            )
            .execute(&pool)
            .await
            .map_err(ProbeError::database)?;
            sqlx::query("INSERT INTO proof (name, nickname, payload, kind) VALUES (?, ?, ?, ?)")
                .bind("aven")
                .bind(Option::<String>::None)
                .bind(vec![0_u8, 1, 2, 0xff])
                .bind("binary")
                .execute(&pool)
                .await
                .map_err(ProbeError::database)?;
            Ok(pool)
        })?;

        Ok(Arc::new(Self { pool }))
    }

    pub fn snapshot_via_owned_runtime(&self) -> Result<ProbeSnapshot, ProbeError> {
        owned_runtime()?.block_on(self.load_snapshot())
    }

    pub fn fail_typed(&self) -> Result<(), ProbeError> {
        Err(ProbeError::Intentional)
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl ProbeStore {
    pub async fn snapshot_via_native_async(
        self: Arc<Self>,
        delay_ms: u64,
    ) -> Result<ProbeSnapshot, ProbeError> {
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        self.load_snapshot().await
    }
}

impl ProbeStore {
    async fn load_snapshot(&self) -> Result<ProbeSnapshot, ProbeError> {
        let row = sqlx::query(
            "SELECT COUNT(*) OVER () AS row_count, name, nickname, payload, kind \
             FROM proof ORDER BY id LIMIT 1",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(ProbeError::database)?;
        let kind: String = row.try_get("kind").map_err(ProbeError::database)?;

        Ok(ProbeSnapshot {
            row_count: row
                .try_get::<i64, _>("row_count")
                .map_err(ProbeError::database)? as u64,
            name: row.try_get("name").map_err(ProbeError::database)?,
            nickname: row.try_get("nickname").map_err(ProbeError::database)?,
            payload: row.try_get("payload").map_err(ProbeError::database)?,
            kind: if kind == "binary" {
                PayloadKind::Binary
            } else {
                PayloadKind::Text
            },
        })
    }
}

uniffi::setup_scaffolding!();
