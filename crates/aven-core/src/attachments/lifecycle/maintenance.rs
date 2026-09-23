use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::{Row, SqliteConnection};

use crate::attachments::storage::object_path;
use crate::attachments::validation::validate_sha256;
use crate::db::begin_immediate;

use super::filesystem::{
    FileMove, move_file_without_replacing, remove_files, restore_trashed_files, scan_directory_all,
    scan_directory_page,
};
use super::liveness::{is_protected, live_blob_references_sql, reconcile_liveness_bounded};
use super::{
    ByteCount, Clock, LEASE_TTL, LifecyclePolicy, PruneSummary, cutoff, staging_dir, timestamp,
    trash_dir,
};

async fn reconcile_trash_files(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    files: Vec<super::filesystem::ScannedFile>,
) -> Result<()> {
    let mut tx = begin_immediate(conn).await?;
    reconcile_trash_files_in_transaction(&mut tx, blob_dir, files).await?;
    tx.commit().await?;
    Ok(())
}

async fn reconcile_trash_files_in_transaction(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    files: Vec<super::filesystem::ScannedFile>,
) -> Result<()> {
    let hashes = files
        .iter()
        .filter(|file| validate_sha256(&file.name).is_ok())
        .map(|file| file.name.clone())
        .collect::<Vec<_>>();
    let available = if hashes.is_empty() {
        HashSet::new()
    } else {
        sqlx::query_scalar::<_, String>(
            "SELECT sha256 FROM blob_inventory
             WHERE available = 1 AND sha256 IN (SELECT value FROM json_each(?))",
        )
        .bind(serde_json::to_string(&hashes)?)
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .collect()
    };
    let blob_dir = blob_dir.to_path_buf();
    crate::attachments::blocking::run(move || {
        for file in files {
            if validate_sha256(&file.name).is_err() {
                continue;
            }
            let target = object_path(&blob_dir, &file.name)?;
            if available.contains(&file.name) {
                if target.exists() {
                    fs::remove_file(file.path)?;
                } else {
                    if let Some(parent) = target.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    match move_file_without_replacing(&file.path, &target)? {
                        FileMove::Moved | FileMove::SourceMissing | FileMove::TargetExists => {}
                    }
                }
            } else {
                match fs::remove_file(file.path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }
        Ok(())
    })
    .await
}

pub async fn reconcile_trash(conn: &mut SqliteConnection, blob_dir: &Path) -> Result<()> {
    reconcile_trash_files(
        conn,
        blob_dir,
        scan_directory_all(trash_dir(blob_dir)).await?,
    )
    .await
}

async fn reconcile_trash_page(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    limit: usize,
) -> Result<()> {
    reconcile_trash_files(
        conn,
        blob_dir,
        scan_directory_page(trash_dir(blob_dir), limit).await?,
    )
    .await
}

pub async fn reconcile_staging(blob_dir: &Path) -> Result<ByteCount> {
    let files = scan_directory_all(staging_dir(blob_dir)).await?;
    let stale = files
        .into_iter()
        .filter(|file| {
            file.name.starts_with(".aven-stage-")
                && file.modified.elapsed().is_ok_and(|age| age >= LEASE_TTL)
        })
        .collect::<Vec<_>>();
    let removed = ByteCount {
        count: u64::try_from(stale.len())?,
        bytes: stale.iter().map(|file| file.len).sum(),
    };
    remove_files(stale.into_iter().map(|file| file.path).collect()).await?;
    Ok(removed)
}

pub async fn reconcile_missing_objects(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    clock: &dyn Clock,
) -> Result<ByteCount> {
    let rows = sqlx::query(
        "SELECT sha256, byte_size FROM blob_inventory
         WHERE available = 1 ORDER BY sha256",
    )
    .fetch_all(&mut *conn)
    .await?;
    let verified_at = timestamp(clock.now());
    let mut missing = ByteCount::default();
    for row in rows {
        let sha256: String = row.get("sha256");
        if object_path(blob_dir, &sha256)?.exists() {
            continue;
        }
        sqlx::query(
            "UPDATE blob_inventory SET available = 0, last_verified_at = ?
             WHERE sha256 = ? AND available = 1",
        )
        .bind(&verified_at)
        .bind(&sha256)
        .execute(&mut *conn)
        .await?;
        missing.count += 1;
        missing.bytes += u64::try_from(row.get::<i64, _>("byte_size"))?;
    }
    Ok(missing)
}

async fn reconcile_object_directory(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    grace: Duration,
    limit: usize,
    clock: &dyn Clock,
) -> Result<ByteCount> {
    // Ownership checks and filesystem removal share writer exclusion with
    // adoption. A stale scan must never remove an object adopted after its query.
    let mut tx = begin_immediate(conn).await?;
    let result =
        reconcile_object_directory_in_transaction(&mut tx, blob_dir, grace, limit, clock).await?;
    tx.commit().await?;
    Ok(result)
}

async fn reconcile_object_directory_in_transaction(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    grace: Duration,
    limit: usize,
    clock: &dyn Clock,
) -> Result<ByteCount> {
    let files = scan_directory_page(staging_dir(blob_dir), limit).await?;
    let stale_staging = files
        .iter()
        .filter(|file| {
            file.name.starts_with(".aven-stage-")
                && file.modified.elapsed().is_ok_and(|age| age >= LEASE_TTL)
        })
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    remove_files(stale_staging).await?;

    let cutoff = clock.now() - chrono::Duration::from_std(grace)?;
    let canonical = files
        .iter()
        .filter(|file| {
            validate_sha256(&file.name).is_ok() && DateTime::<Utc>::from(file.modified) <= cutoff
        })
        .collect::<Vec<_>>();
    if canonical.is_empty() {
        return Ok(ByteCount::default());
    }
    let hashes = canonical
        .iter()
        .map(|file| file.name.clone())
        .collect::<Vec<_>>();
    let candidates = serde_json::to_string(&hashes)?;
    let now = timestamp(clock.now());
    let live_blob_references = live_blob_references_sql("candidate.value");
    let protected = sqlx::query_scalar::<_, String>(sqlx::AssertSqlSafe(format!(
        "SELECT candidate.value FROM json_each(?) candidate
         WHERE EXISTS(SELECT 1 FROM blob_inventory bi WHERE bi.sha256 = candidate.value)
            OR {live_blob_references}
            OR EXISTS(
              SELECT 1 FROM changes
              WHERE server_seq IS NULL AND op_type = 'attachment_add'
                AND json_extract(payload, '$.sha256') = candidate.value
            )
            OR EXISTS(
              SELECT 1 FROM blob_leases
              WHERE sha256 = candidate.value AND expires_at > ?
            )
            OR EXISTS(
              SELECT 1 FROM blob_upload_reservations
              WHERE sha256 = candidate.value AND expires_at > ?
            )"
    )))
    .bind(candidates)
    .bind(&now)
    .bind(&now)
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .collect::<HashSet<_>>();
    let removable = canonical
        .into_iter()
        .filter(|file| !protected.contains(&file.name))
        .map(|file| (file.path.clone(), file.name.clone(), file.len))
        .collect::<Vec<_>>();
    if removable.is_empty() {
        return Ok(ByteCount::default());
    }
    let blob_dir = blob_dir.to_path_buf();
    crate::attachments::blocking::run(move || {
        let trash = trash_dir(&blob_dir);
        fs::create_dir_all(&trash)?;
        let mut removed = ByteCount::default();
        for (source, name, len) in removable {
            let target = trash.join(name);
            if move_file_without_replacing(&source, &target)? == FileMove::Moved {
                fs::remove_file(target)?;
                removed.count += 1;
                removed.bytes += len;
            }
        }
        Ok(removed)
    })
    .await
}

pub async fn reconcile_orphan_objects(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    grace: Duration,
    clock: &dyn Clock,
) -> Result<ByteCount> {
    reconcile_object_directory(conn, blob_dir, grace, usize::MAX, clock).await
}

pub async fn prune(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
    policy: LifecyclePolicy,
    apply: bool,
    clock: &dyn Clock,
) -> Result<PruneSummary> {
    if apply {
        reconcile_trash_page(conn, blob_dir, policy.maintenance_limit).await?;
        reconcile_object_directory(
            conn,
            blob_dir,
            policy.grace,
            policy.maintenance_limit,
            clock,
        )
        .await?;
        reconcile_missing_objects(conn, blob_dir, clock).await?;
    }
    reconcile_liveness_bounded(conn, policy.maintenance_limit, clock).await?;
    let now = timestamp(clock.now());
    let cutoff = cutoff(clock.now(), policy.grace)?;
    let rows = sqlx::query(
        "SELECT bi.sha256, bi.byte_size
         FROM blob_inventory bi
         JOIN blob_lifecycle bl ON bl.sha256 = bi.sha256
         WHERE bi.available = 1 AND bl.unreferenced_at IS NOT NULL
           AND bl.unreferenced_at <= ?
         ORDER BY bl.unreferenced_at, bi.sha256 LIMIT ?",
    )
    .bind(&cutoff)
    .bind(i64::try_from(policy.maintenance_limit)?)
    .fetch_all(&mut *conn)
    .await?;
    let mut summary = PruneSummary::default();
    for row in rows {
        let sha256: String = row.get("sha256");
        let byte_size: i64 = row.get("byte_size");
        if is_protected(conn, &sha256, &now).await? {
            continue;
        }
        summary.eligible.count += 1;
        summary.eligible.bytes += u64::try_from(byte_size)?;
        if !apply {
            continue;
        }
        let mut tx = begin_immediate(conn).await?;
        let still_eligible: bool = sqlx::query_scalar(
            "SELECT EXISTS(
               SELECT 1 FROM blob_inventory bi
               JOIN blob_lifecycle bl ON bl.sha256 = bi.sha256
               WHERE bi.sha256 = ? AND bi.available = 1
                 AND bl.unreferenced_at IS NOT NULL AND bl.unreferenced_at <= ?
             )",
        )
        .bind(&sha256)
        .bind(&cutoff)
        .fetch_one(&mut *tx)
        .await?;
        if !still_eligible || is_protected(&mut tx, &sha256, &now).await? {
            tx.rollback().await?;
            continue;
        }
        let source = object_path(blob_dir, &sha256)?;
        let trash = trash_dir(blob_dir);
        let trashed = trash.join(&sha256);
        let source_for_move = source.clone();
        let trashed_for_move = trashed.clone();
        let moved = crate::attachments::blocking::run(move || {
            fs::create_dir_all(&trash)?;
            move_file_without_replacing(&source_for_move, &trashed_for_move)
        })
        .await?;
        if moved == FileMove::TargetExists {
            tx.rollback().await?;
            continue;
        }
        if let Err(error) = sqlx::query(
            "UPDATE blob_inventory SET available = 0, last_verified_at = ? WHERE sha256 = ?",
        )
        .bind(&now)
        .bind(&sha256)
        .execute(&mut *tx)
        .await
        {
            let _ = tx.rollback().await;
            if moved == FileMove::Moved {
                restore_trashed_files(vec![(source, trashed)]).await;
            }
            return Err(error.into());
        }
        if let Err(error) = tx.commit().await {
            if moved == FileMove::Moved {
                restore_trashed_files(vec![(source, trashed)]).await;
            }
            return Err(error.into());
        }
        if moved == FileMove::Moved {
            remove_files(vec![trashed]).await?;
        }
        summary.pruned.count += 1;
        summary.pruned.bytes += u64::try_from(byte_size)?;
    }
    let blob_dir = blob_dir.to_path_buf();
    crate::attachments::blocking::run(move || {
        prune_preview_cache(&blob_dir, policy.preview_quota_bytes)
    })
    .await?;
    Ok(summary)
}

pub fn prune_preview_cache(blob_dir: &Path, quota: u64) -> Result<ByteCount> {
    let root = blob_dir.join("cache").join("previews");
    if !root.exists() {
        return Ok(ByteCount::default());
    }
    let mut files = Vec::new();
    let mut dirs = vec![root];
    while let Some(dir) = dirs.pop() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                dirs.push(entry.path());
            } else if entry.file_type()?.is_file() {
                let metadata = entry.metadata()?;
                files.push((metadata.modified()?, metadata.len(), entry.path()));
            }
        }
    }
    let mut total: u64 = files.iter().map(|(_, size, _)| *size).sum();
    files.sort_by_key(|(modified, _, path)| (*modified, path.clone()));
    let mut removed = ByteCount::default();
    for (_, size, path) in files {
        if total <= quota {
            break;
        }
        fs::remove_file(path)?;
        total -= size;
        removed.count += 1;
        removed.bytes += size;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::Duration;

    use sqlx::Connection as _;
    use sqlx::SqliteConnection;
    use sqlx::sqlite::SqliteConnectOptions;

    use super::super::test_support::{TestClock, insert_attachment, insert_task};
    use super::super::*;
    use super::*;
    use crate::attachments::storage::{object_path, upsert_inventory_available};
    use crate::db::{begin_immediate, open_db};

    #[tokio::test]
    async fn concurrent_attach_wins_prune_recheck() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("test.sqlite");
        let pool = open_db(&db_path).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let blob_dir = temp.path().join("blobs");
        let hash = "abababababababababababababababababababababababababababababababab";
        upsert_inventory_available(&mut conn, hash, 4, "image/png")
            .await
            .unwrap();
        insert_task(&mut conn, "0000000000000003").await;
        let path = object_path(&blob_dir, hash).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"blob").unwrap();
        let clock = TestClock::at("2026-07-01T00:00:00Z");
        reconcile_liveness(&mut conn, &clock).await.unwrap();
        clock.advance(chrono::Duration::days(8));
        let options = SqliteConnectOptions::new()
            .filename(&db_path)
            .busy_timeout(Duration::from_secs(5));
        let prune_conn = SqliteConnection::connect_with(&options).await.unwrap();

        let mut tx = begin_immediate(&mut conn).await.unwrap();
        insert_attachment(&mut tx, "0000000000000013", "0000000000000003", hash, false).await;
        let prune_dir = blob_dir.clone();
        let prune_clock = clock.clone();
        let pruning = tokio::spawn(async move {
            let mut prune_conn = prune_conn;
            prune(
                &mut prune_conn,
                &prune_dir,
                LifecyclePolicy::default(),
                true,
                &prune_clock,
            )
            .await
            .unwrap()
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        tx.commit().await.unwrap();

        let summary = pruning.await.unwrap();
        assert_eq!(summary.pruned.count, 0);
        assert!(path.exists());
    }

    #[tokio::test]
    async fn orphan_cleanup_waits_for_transactional_object_adoption() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("test.sqlite");
        let pool = open_db(&db_path).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let blob_dir = temp.path().join("blobs");
        let hash = "ab".repeat(32);
        let path = object_path(&blob_dir, &hash).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"blob").unwrap();
        let options = SqliteConnectOptions::new()
            .filename(&db_path)
            .busy_timeout(Duration::from_secs(5));
        let mut cleanup_conn = SqliteConnection::connect_with(&options).await.unwrap();
        let mut tx = begin_immediate(&mut conn).await.unwrap();
        upsert_inventory_available(&mut tx, &hash, 4, "image/png")
            .await
            .unwrap();
        let mut cleanup = tokio::spawn(async move {
            reconcile_orphan_objects(
                &mut cleanup_conn,
                &blob_dir,
                Duration::ZERO,
                &TestClock::at("2030-07-01T00:00:00Z"),
            )
            .await
            .unwrap()
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut cleanup)
                .await
                .is_err()
        );
        tx.commit().await.unwrap();
        assert_eq!(cleanup.await.unwrap().count, 0);
        assert!(path.exists());
    }

    #[tokio::test]
    async fn interrupted_atomic_create_object_is_reconciled_after_grace() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("test.sqlite");
        let pool = open_db(&db_path).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let blob_dir = temp.path().join("blobs");
        let hash = "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";
        let path = object_path(&blob_dir, hash).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"orphan").unwrap();
        let clock = TestClock::at("2030-07-01T00:00:00Z");
        let policy = LifecyclePolicy {
            grace: Duration::ZERO,
            ..LifecyclePolicy::default()
        };

        prune(&mut conn, &blob_dir, policy, true, &clock)
            .await
            .unwrap();
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn orphan_traversal_obeys_limit_and_resumes_between_steps() {
        let temp = tempfile::tempdir().unwrap();
        let pool = open_db(&temp.path().join("test.sqlite")).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let blob_dir = temp.path().join("blobs");
        for digit in ['6', '7', '8', '9', 'a'] {
            let hash = digit.to_string().repeat(64);
            let path = object_path(&blob_dir, &hash).unwrap();
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"orphan").unwrap();
        }
        let clock = TestClock::at("2030-07-01T00:00:00Z");
        let policy = LifecyclePolicy {
            grace: Duration::ZERO,
            maintenance_limit: 2,
            ..LifecyclePolicy::default()
        };

        prune(&mut conn, &blob_dir, policy, true, &clock)
            .await
            .unwrap();
        let after_one = fs::read_dir(staging_dir(&blob_dir)).unwrap().count();
        assert!(after_one >= 3);

        for _ in 0..4 {
            prune(&mut conn, &blob_dir, policy, true, &clock)
                .await
                .unwrap();
        }
        assert_eq!(fs::read_dir(staging_dir(&blob_dir)).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn interrupted_trash_move_restores_available_object() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("test.sqlite");
        let pool = open_db(&db_path).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let blob_dir = temp.path().join("blobs");
        let hash = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
        upsert_inventory_available(&mut conn, hash, 4, "image/png")
            .await
            .unwrap();
        let source = object_path(&blob_dir, hash).unwrap();
        let trash = trash_dir(&blob_dir).join(hash);
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::create_dir_all(trash.parent().unwrap()).unwrap();
        fs::write(&source, b"blob").unwrap();
        fs::rename(&source, &trash).unwrap();

        reconcile_trash(&mut conn, &blob_dir).await.unwrap();
        assert!(source.exists());
        assert!(!trash.exists());
    }
}
