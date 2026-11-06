use std::path::Path;

pub use aven_core::test_support::task_id;

/// Opens a private copy of the blank migrated database template at `path`,
/// returning the database with its own pool for raw SQL.
pub async fn open_database(
    path: &Path,
) -> anyhow::Result<(aven_core::db::Database, sqlx::SqlitePool)> {
    let database = aven_core::test_support::open_blank_database(path).await?;
    let pool = aven_core::test_support::pool(&database);
    Ok((database, pool))
}

/// A default config whose wake address is a free port, so a host started in a
/// test never takes the address a real daemon listens on.
pub fn isolated_config() -> crate::config::AppConfig {
    let mut config = crate::config::AppConfig::default();
    config.daemon.wake_addr = Some("127.0.0.1:0".to_string());
    config
}
