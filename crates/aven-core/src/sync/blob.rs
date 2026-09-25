use std::path::Path;

use anyhow::{Context, Result};
use sqlx::SqliteConnection;

use crate::attachments::lifecycle::ByteCount;
use crate::db::Database;

impl Database {
    /// Accepted attachments whose image bytes are missing on this device.
    pub async fn missing_sync_attachment_counts(&self) -> Result<ByteCount> {
        let mut conn = self.acquire_reader().await?;
        missing_local_blob_counts(&mut conn).await
    }

    /// Live setup images whose object file is absent, regardless of the
    /// inventory's last-known availability bit.
    pub async fn missing_setup_attachment_counts(&self, blob_dir: &Path) -> Result<ByteCount> {
        let mut conn = self.acquire_reader().await?;
        let rows: Vec<(String, i64, String, String)> = sqlx::query_as(
            "SELECT ta.sha256, MAX(ta.byte_size), MIN(t.title), MIN(ta.filename)
             FROM task_attachments ta
             JOIN tasks t ON t.workspace_id = ta.workspace_id AND t.id = ta.task_id
             WHERE ta.deleted = 0 AND t.deleted = 0
             GROUP BY ta.sha256 ORDER BY ta.sha256",
        )
        .fetch_all(&mut *conn)
        .await?;
        let mut missing = ByteCount::default();
        for (sha256, byte_size, task, attachment) in rows {
            let path = crate::attachments::storage::object_path(blob_dir, &sha256)?;
            match std::fs::metadata(path) {
                Ok(metadata) if metadata.is_file() => {}
                Ok(_) => {
                    missing.count += 1;
                    missing.bytes += u64::try_from(byte_size)?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    missing.count += 1;
                    missing.bytes += u64::try_from(byte_size)?;
                }
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("error setup-image-file task={task:?} attachment={attachment:?}")
                    });
                }
            }
        }
        Ok(missing)
    }
}

async fn missing_local_blob_counts(conn: &mut SqliteConnection) -> Result<ByteCount> {
    let (count, bytes): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(SUM(byte_size), 0) FROM (
           SELECT ta.sha256, MAX(ta.byte_size) AS byte_size
           FROM task_attachments AS ta INDEXED BY idx_task_attachments_live_sha256
           JOIN tasks t ON t.workspace_id = ta.workspace_id AND t.id = ta.task_id
           JOIN changes c ON c.change_id = ta.created_by_change_id
                         AND c.op_type = 'attachment_add' AND c.server_seq IS NOT NULL
           LEFT JOIN blob_inventory bi ON bi.sha256 = ta.sha256
           WHERE ta.deleted = 0 AND t.deleted = 0
           GROUP BY ta.sha256
           HAVING MAX(COALESCE(bi.available, 0)) = 0
         )",
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(ByteCount {
        count: u64::try_from(count)?,
        bytes: u64::try_from(bytes)?,
    })
}

#[cfg(test)]
mod tests {
    use crate::attachments::lifecycle::SystemClock;

    use super::*;

    async fn seed_missing_attachments(database: &Database, count: usize) {
        let mut conn = database.acquire_writer().await.unwrap();
        for index in 0..count {
            let task_id = format!("{index:016X}");
            let attachment_id = format!("{:016X}", index + 100);
            let change_id = format!("{:016X}", index + 200);
            let sha256 = format!("{index:064x}");
            sqlx::query(
                "INSERT INTO tasks(
                   workspace_id, id, title, description, project_id, status, priority,
                   created_at, updated_at, queue_activity_at
                 ) VALUES ('0000000000000000', ?, 'task', '', 'project', 'inbox', 'none',
                           '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z',
                           '2026-01-01T00:00:00Z')",
            )
            .bind(&task_id)
            .execute(&mut *conn)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO changes(
                   change_id, client_id, local_seq, entity_type, entity_id, field, op_type,
                   payload, created_at, server_seq
                 ) VALUES (?, 'remote', ?, 'task', ?, 'attachments', 'attachment_add', '{}',
                           '2026-01-01T00:00:00Z', ?)",
            )
            .bind(&change_id)
            .bind(i64::try_from(index + 1).unwrap())
            .bind(&task_id)
            .bind(i64::try_from(index + 1).unwrap())
            .execute(&mut *conn)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO task_attachments(
                   workspace_id, attachment_id, task_id, sha256, byte_size, media_type,
                   width, height, created_at, created_by_change_id
                 ) VALUES ('0000000000000000', ?, ?, ?, 4, 'image/png', 1, 1,
                           '2026-01-01T00:00:00Z', ?)",
            )
            .bind(attachment_id)
            .bind(task_id)
            .bind(sha256)
            .bind(change_id)
            .execute(&mut *conn)
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn imported_local_only_attachment_is_not_scheduled_for_download() {
        let temp = tempfile::tempdir().unwrap();
        let database = Database::open(&temp.path().join("replica.sqlite"))
            .await
            .unwrap();
        let mut conn = database.acquire_writer().await.unwrap();
        sqlx::query(
            "INSERT INTO tasks(
               workspace_id, id, title, description, project_id, status, priority,
               created_at, updated_at, queue_activity_at
             ) VALUES ('0000000000000000', '0000000000000001', 'task', '', 'project',
                       'inbox', 'none', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z',
                       '2026-01-01T00:00:00Z')",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO task_attachments(
               workspace_id, attachment_id, task_id, sha256, byte_size, media_type,
               width, height, created_at, created_by_change_id
             ) VALUES ('0000000000000000', '0000000000000002', '0000000000000001',
                       'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                       4, 'image/png', 1, 1, '2026-01-01T00:00:00Z', NULL)",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
        drop(conn);

        assert_eq!(
            database.missing_sync_attachment_counts().await.unwrap(),
            ByteCount { count: 0, bytes: 0 }
        );
    }

    #[tokio::test]
    async fn missing_counts_follow_inventory_and_object_reconciliation() {
        let temp = tempfile::tempdir().unwrap();
        let database = Database::open(&temp.path().join("replica.sqlite"))
            .await
            .unwrap();
        seed_missing_attachments(&database, 3).await;
        {
            let mut conn = database.acquire_writer().await.unwrap();
            crate::attachments::storage::upsert_inventory_available(
                &mut conn,
                &format!("{:064x}", 0),
                4,
                "image/png",
            )
            .await
            .unwrap();
        }

        assert_eq!(
            database.missing_sync_attachment_counts().await.unwrap(),
            ByteCount { count: 2, bytes: 8 }
        );

        let blob_dir = temp.path().join("blobs");
        assert_eq!(
            database
                .missing_setup_attachment_counts(&blob_dir)
                .await
                .unwrap(),
            ByteCount {
                count: 3,
                bytes: 12
            }
        );
        let mut conn = database.acquire_writer().await.unwrap();
        let missing = crate::attachments::lifecycle::reconcile_missing_objects(
            &mut conn,
            &blob_dir,
            &SystemClock,
        )
        .await
        .unwrap();
        assert_eq!(missing.count, 1);
        drop(conn);

        assert_eq!(
            database.missing_sync_attachment_counts().await.unwrap(),
            ByteCount {
                count: 3,
                bytes: 12
            }
        );
    }
}
