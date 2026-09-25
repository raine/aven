//! Blank database template for test fixtures.
//!
//! Every fixture copies one fully migrated, checkpointed, single-file database
//! and then opens the copy through [`Database::open`], so each test still runs
//! the normal open path without paying for migrations. The template omits
//! `meta.client_id`, which `Database::open` fills with a fresh value per copy.
//!
//! Tests whose subject is first-open behavior (migrations, historical schemas,
//! WAL handling, backup and restore, installation identity) should call
//! [`Database::open`] on a new path instead.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use crate::db::Database;

const TEMPLATE_DIR: &str = "aven-db-templates";

/// Copies the blank template to `path` and opens it with [`Database::open`].
pub async fn open_blank_database(path: &Path) -> Result<Database> {
    if path.exists() {
        bail!("blank database target already exists: {}", path.display());
    }
    let template = blank_database_template().await?;
    std::fs::copy(&template, path).with_context(|| {
        format!(
            "could not copy {} to {}",
            template.display(),
            path.display()
        )
    })?;
    Database::open(path).await
}

/// Returns the template path, creating it once per test binary build.
pub async fn blank_database_template() -> Result<PathBuf> {
    static TEMPLATE: OnceLock<PathBuf> = OnceLock::new();
    if let Some(path) = TEMPLATE.get() {
        return Ok(path.clone());
    }
    let (dir, name) = template_location()?;
    let path = dir.join(&name);
    if !path.is_file() {
        build_template(&dir, &path).await?;
    }
    Ok(TEMPLATE.get_or_init(|| path).clone())
}

/// Names the template after the running test binary's path, size and
/// modification time, so every rebuild gets its own template and concurrent
/// test processes of one build share it.
fn template_location() -> Result<(PathBuf, String)> {
    let exe = std::env::current_exe().context("could not resolve test binary")?;
    let metadata =
        std::fs::metadata(&exe).with_context(|| format!("could not inspect {}", exe.display()))?;
    let modified = metadata
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let mut hasher = Sha256::new();
    hasher.update(exe.as_os_str().as_encoded_bytes());
    hasher.update(metadata.len().to_le_bytes());
    hasher.update(modified.to_le_bytes());
    let key = hex::encode(&hasher.finalize()[..12]);
    let stem = exe
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "test".to_string());
    let dir = exe
        .parent()
        .context("test binary has no parent directory")?
        .join(TEMPLATE_DIR);
    Ok((dir, format!("{stem}-{key}.sqlite")))
}

/// Builds the template in a private directory and renames it into place, so
/// readers only ever observe a complete file. Concurrent builders produce
/// equivalent files and the last rename wins.
async fn build_template(dir: &Path, path: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
    let staging = tempfile::tempdir_in(dir)?;
    let staged = staging.path().join("blank.sqlite");
    let database = Database::open(&staged).await?;
    database.pool().close().await;
    drop(database);

    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&staged)
        .create_if_missing(false);
    let mut conn = <sqlx::SqliteConnection as sqlx::Connection>::connect_with(&options).await?;
    sqlx::query("DELETE FROM meta WHERE key = 'client_id'")
        .execute(&mut conn)
        .await?;
    let (busy, log_frames, checkpointed_frames): (i64, i64, i64) =
        sqlx::query_as("PRAGMA wal_checkpoint(TRUNCATE)")
            .fetch_one(&mut conn)
            .await?;
    if busy != 0 || log_frames != checkpointed_frames {
        bail!("blank database template checkpoint was incomplete");
    }
    let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode=DELETE")
        .fetch_one(&mut conn)
        .await?;
    if !journal_mode.eq_ignore_ascii_case("delete") {
        bail!("blank database template kept journal mode {journal_mode}");
    }
    sqlx::Connection::close(conn).await?;
    for sidecar in [crate::db::wal_path(&staged), crate::db::shm_path(&staged)] {
        if sidecar.exists() {
            bail!("blank database template left {}", sidecar.display());
        }
    }
    std::fs::rename(&staged, path)
        .with_context(|| format!("could not install {}", path.display()))?;
    remove_stale_templates(dir, path);
    Ok(())
}

/// Removes templates left by earlier builds of the same test binary.
fn remove_stale_templates(dir: &Path, current: &Path) {
    let (Some(current_name), Ok(entries)) = (current.file_name(), std::fs::read_dir(dir)) else {
        return;
    };
    let current_name = current_name.to_string_lossy();
    let Some((stem, _)) = current_name.rsplit_once('-') else {
        return;
    };
    let prefix = format!("{stem}-");
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name != current_name && name.starts_with(&prefix) && name.ends_with(".sqlite") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn schema(pool: &sqlx::SqlitePool) -> Vec<(String, String, Option<String>)> {
        sqlx::query_as(
            "SELECT type, name, sql FROM sqlite_schema
             WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name",
        )
        .fetch_all(pool)
        .await
        .unwrap()
    }

    async fn meta_keys(pool: &sqlx::SqlitePool) -> Vec<String> {
        sqlx::query_scalar("SELECT key FROM meta ORDER BY key")
            .fetch_all(pool)
            .await
            .unwrap()
    }

    async fn migrations(pool: &sqlx::SqlitePool) -> Vec<(i64, Vec<u8>)> {
        sqlx::query_as("SELECT version, checksum FROM _sqlx_migrations ORDER BY version")
            .fetch_all(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn template_copy_matches_fresh_database() {
        let dir = tempfile::tempdir().unwrap();
        let fresh = Database::open(&dir.path().join("fresh.sqlite"))
            .await
            .unwrap();
        let copy = open_blank_database(&dir.path().join("copy.sqlite"))
            .await
            .unwrap();

        assert_eq!(schema(fresh.pool()).await, schema(copy.pool()).await);
        assert_eq!(
            migrations(fresh.pool()).await,
            migrations(copy.pool()).await
        );
        assert_eq!(meta_keys(fresh.pool()).await, meta_keys(copy.pool()).await);
        for key in ["sync_cursor", "local_seq", "sync_generation"] {
            assert_eq!(
                fresh.meta(key).await.unwrap(),
                copy.meta(key).await.unwrap()
            );
        }
        let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(copy.pool())
            .await
            .unwrap();
        assert_eq!(journal_mode, "wal");
        assert_eq!(
            copy.file_identity(),
            Some(
                std::fs::canonicalize(dir.path().join("copy.sqlite"))
                    .unwrap()
                    .as_path()
            )
        );
    }

    #[tokio::test]
    async fn template_is_single_blank_file_without_identity() {
        let template = blank_database_template().await.unwrap();
        assert!(template.is_file());
        assert!(!crate::db::wal_path(&template).exists());
        assert!(!crate::db::shm_path(&template).exists());

        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&template)
            .read_only(true);
        let mut conn = <sqlx::SqliteConnection as sqlx::Connection>::connect_with(&options)
            .await
            .unwrap();
        let client_ids: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM meta WHERE key = 'client_id'")
                .fetch_one(&mut conn)
                .await
                .unwrap();
        assert_eq!(client_ids, 0);
        let tasks: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks")
            .fetch_one(&mut conn)
            .await
            .unwrap();
        assert_eq!(tasks, 0);
        let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&mut conn)
            .await
            .unwrap();
        assert_eq!(journal_mode, "delete");
    }

    #[tokio::test]
    async fn copies_are_isolated_with_distinct_client_ids() {
        let dir = tempfile::tempdir().unwrap();
        let first = open_blank_database(&dir.path().join("first.sqlite"))
            .await
            .unwrap();
        let second = open_blank_database(&dir.path().join("second.sqlite"))
            .await
            .unwrap();
        let first_client = first.meta("client_id").await.unwrap().unwrap();
        let second_client = second.meta("client_id").await.unwrap().unwrap();
        assert_ne!(first_client, second_client);

        let mut conn = first.acquire_writer().await.unwrap();
        crate::db::set_meta(&mut conn, "isolation_probe", "first")
            .await
            .unwrap();
        drop(conn);
        assert_eq!(second.meta("isolation_probe").await.unwrap(), None);

        let template = blank_database_template().await.unwrap();
        assert!(!crate::db::wal_path(&template).exists());
        assert!(!crate::db::shm_path(&template).exists());
    }

    #[tokio::test]
    async fn refuses_existing_target() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("existing.sqlite");
        std::fs::write(&path, b"").unwrap();
        assert!(open_blank_database(&path).await.is_err());
    }
}
