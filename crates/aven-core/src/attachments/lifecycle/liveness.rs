use anyhow::Result;
use sqlx::SqliteConnection;

use crate::db::{begin_immediate, get_meta, set_meta};

use super::{Clock, timestamp};

const LIVENESS_CURSOR_META_KEY: &str = "attachment_liveness_cursor";

pub(super) fn live_blob_references_sql(sha256_expr: &str) -> String {
    format!(
        "EXISTS(
           SELECT 1 FROM task_attachments ta
           JOIN tasks t ON t.workspace_id = ta.workspace_id AND t.id = ta.task_id
           WHERE ta.sha256 = {sha256_expr} AND ta.deleted = 0 AND t.deleted = 0
         ) OR EXISTS(
           SELECT 1 FROM server_blob_references sbr
           LEFT JOIN server_task_tombstones st
             ON st.workspace_id = sbr.workspace_id AND st.task_id = sbr.task_id
           WHERE sbr.sha256 = {sha256_expr} AND sbr.deleted = 0
             AND COALESCE(st.deleted, 0) = 0
         ) OR EXISTS(
           SELECT 1 FROM local_shared_capture_pins pin
           WHERE pin.sha256 = {sha256_expr}
         ) OR EXISTS(
           SELECT 1 FROM local_e2ee_image_preparation pin WHERE pin.sha256 = {sha256_expr}
         )"
    )
}

pub async fn reconcile_liveness(conn: &mut SqliteConnection, clock: &dyn Clock) -> Result<()> {
    let mut tx = begin_immediate(conn).await?;
    reconcile_liveness_in_transaction(&mut tx, clock).await?;
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn reconcile_liveness_in_transaction(
    conn: &mut SqliteConnection,
    clock: &dyn Clock,
) -> Result<()> {
    let now = timestamp(clock.now());
    sqlx::query("DELETE FROM blob_leases WHERE expires_at <= ?")
        .bind(&now)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM blob_upload_reservations WHERE expires_at <= ?")
        .bind(&now)
        .execute(&mut *conn)
        .await?;
    sqlx::query(
        "INSERT OR IGNORE INTO blob_lifecycle(sha256, unreferenced_at)
         SELECT sha256, NULL FROM blob_inventory",
    )
    .execute(&mut *conn)
    .await?;
    let live_blob_references = live_blob_references_sql("blob_lifecycle.sha256");
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE blob_lifecycle SET unreferenced_at = NULL
         WHERE unreferenced_at IS NOT NULL
           AND ({live_blob_references})"
    )))
    .execute(&mut *conn)
    .await?;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE blob_lifecycle SET unreferenced_at = ?
         WHERE unreferenced_at IS NULL
           AND NOT ({live_blob_references})"
    )))
    .bind(&now)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

pub(crate) async fn reconcile_liveness_for_hashes_in_transaction(
    conn: &mut SqliteConnection,
    hashes: &[String],
    clock: &dyn Clock,
) -> Result<()> {
    if hashes.is_empty() {
        return Ok(());
    }
    let now = timestamp(clock.now());
    let hashes = serde_json::to_string(hashes)?;
    let live_inventory_references = live_blob_references_sql("bi.sha256");
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT OR IGNORE INTO blob_lifecycle(sha256, unreferenced_at)
         SELECT bi.sha256,
                CASE WHEN ({live_inventory_references}) THEN NULL ELSE ? END
         FROM blob_inventory bi
         JOIN (SELECT DISTINCT value AS sha256 FROM json_each(?)) affected
           ON affected.sha256 = bi.sha256"
    )))
    .bind(&now)
    .bind(&hashes)
    .execute(&mut *conn)
    .await?;

    let live_blob_references = live_blob_references_sql("blob_lifecycle.sha256");
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE blob_lifecycle SET unreferenced_at = NULL
         WHERE unreferenced_at IS NOT NULL
           AND sha256 IN (SELECT value FROM json_each(?))
           AND ({live_blob_references})"
    )))
    .bind(&hashes)
    .execute(&mut *conn)
    .await?;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE blob_lifecycle SET unreferenced_at = ?
         WHERE unreferenced_at IS NULL
           AND sha256 IN (SELECT value FROM json_each(?))
           AND NOT ({live_blob_references})"
    )))
    .bind(&now)
    .bind(&hashes)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

pub(super) async fn reconcile_liveness_bounded(
    conn: &mut SqliteConnection,
    limit: usize,
    clock: &dyn Clock,
) -> Result<()> {
    let mut tx = begin_immediate(conn).await?;
    let now = timestamp(clock.now());
    sqlx::query("DELETE FROM blob_leases WHERE expires_at <= ?")
        .bind(&now)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM blob_upload_reservations WHERE expires_at <= ?")
        .bind(&now)
        .execute(&mut *tx)
        .await?;
    if limit == 0 {
        tx.commit().await?;
        return Ok(());
    }

    let cursor = get_meta(&mut tx, LIVENESS_CURSOR_META_KEY)
        .await?
        .unwrap_or_default();
    let hashes = sqlx::query_scalar::<_, String>(
        "SELECT sha256 FROM blob_inventory
         WHERE sha256 > ? ORDER BY sha256 LIMIT ?",
    )
    .bind(&cursor)
    .bind(i64::try_from(limit)?)
    .fetch_all(&mut *tx)
    .await?;
    reconcile_liveness_for_hashes_in_transaction(&mut tx, &hashes, clock).await?;
    set_meta(
        &mut tx,
        LIVENESS_CURSOR_META_KEY,
        if hashes.len() < limit {
            ""
        } else {
            hashes.last().map(String::as_str).unwrap_or_default()
        },
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

pub(super) async fn is_protected(
    conn: &mut SqliteConnection,
    sha256: &str,
    now: &str,
) -> Result<bool> {
    let live_blob_references = live_blob_references_sql("?");
    Ok(sqlx::query_scalar::<_, bool>(sqlx::AssertSqlSafe(format!(
        "SELECT
           {live_blob_references} OR EXISTS(
             SELECT 1 FROM changes
             WHERE server_seq IS NULL AND op_type = 'attachment_add'
               AND json_extract(payload, '$.sha256') = ?
           ) OR EXISTS(
             SELECT 1 FROM blob_leases WHERE sha256 = ? AND expires_at > ?
           ) OR EXISTS(
             SELECT 1 FROM blob_upload_reservations WHERE sha256 = ? AND expires_at > ?
           )"
    )))
    .bind(sha256)
    .bind(sha256)
    .bind(sha256)
    .bind(sha256)
    .bind(sha256)
    .bind(sha256)
    .bind(now)
    .bind(sha256)
    .bind(now)
    .fetch_one(&mut *conn)
    .await?)
}

#[cfg(test)]
mod tests {

    use super::super::test_support::{TestClock, insert_attachment, insert_task};

    use super::*;
    use crate::attachments::storage::upsert_inventory_available;
    use crate::db::open_db;

    #[tokio::test]
    async fn final_live_reference_starts_grace_once_and_restore_clears_it() {
        let temp = tempfile::tempdir().unwrap();
        let pool = open_db(&temp.path().join("test.sqlite")).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let hash = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        upsert_inventory_available(&mut conn, hash, 4, "image/png")
            .await
            .unwrap();
        insert_task(&mut conn, "0000000000000001").await;
        insert_task(&mut conn, "0000000000000002").await;
        insert_attachment(
            &mut conn,
            "0000000000000011",
            "0000000000000001",
            hash,
            false,
        )
        .await;
        insert_attachment(
            &mut conn,
            "0000000000000012",
            "0000000000000002",
            hash,
            false,
        )
        .await;
        let clock = TestClock::at("2026-07-01T00:00:00Z");

        reconcile_liveness(&mut conn, &clock).await.unwrap();
        sqlx::query("UPDATE task_attachments SET deleted = 1, deleted_at = 'x' WHERE attachment_id = '0000000000000011'")
                .execute(&mut *conn).await.unwrap();
        reconcile_liveness(&mut conn, &clock).await.unwrap();
        let value: Option<String> =
            sqlx::query_scalar("SELECT unreferenced_at FROM blob_lifecycle WHERE sha256 = ?")
                .bind(hash)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(value, None, "one live reference keeps the hash live");

        sqlx::query("UPDATE task_attachments SET deleted = 1, deleted_at = 'x' WHERE attachment_id = '0000000000000012'")
                .execute(&mut *conn).await.unwrap();
        reconcile_liveness(&mut conn, &clock).await.unwrap();
        let first: String =
            sqlx::query_scalar("SELECT unreferenced_at FROM blob_lifecycle WHERE sha256 = ?")
                .bind(hash)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        clock.advance(chrono::Duration::days(1));
        reconcile_liveness(&mut conn, &clock).await.unwrap();
        let second: String =
            sqlx::query_scalar("SELECT unreferenced_at FROM blob_lifecycle WHERE sha256 = ?")
                .bind(hash)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(first, second, "grace starts exactly once");

        sqlx::query("UPDATE task_attachments SET deleted = 0, deleted_at = NULL WHERE attachment_id = '0000000000000012'")
                .execute(&mut *conn).await.unwrap();
        reconcile_liveness(&mut conn, &clock).await.unwrap();
        let restored: Option<String> =
            sqlx::query_scalar("SELECT unreferenced_at FROM blob_lifecycle WHERE sha256 = ?")
                .bind(hash)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(restored, None);
    }

    #[tokio::test]
    async fn affected_liveness_reconciliation_is_scoped_and_write_minimal() {
        let temp = tempfile::tempdir().unwrap();
        let pool = open_db(&temp.path().join("test.sqlite")).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let affected =
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string();
        let unrelated =
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string();
        let clock = TestClock::at("2026-07-01T00:00:00Z");
        for hash in [&affected, &unrelated] {
            upsert_inventory_available(&mut conn, hash, 4, "image/png")
                .await
                .unwrap();
        }
        insert_task(&mut conn, "0000000000000001").await;
        insert_attachment(
            &mut conn,
            "0000000000000011",
            "0000000000000001",
            &affected,
            false,
        )
        .await;
        for hash in [&affected, &unrelated] {
            sqlx::query("INSERT INTO blob_lifecycle(sha256, unreferenced_at) VALUES (?, ?)")
                .bind(hash)
                .bind("2026-06-01T00:00:00Z")
                .execute(&mut *conn)
                .await
                .unwrap();
        }
        sqlx::query(
            "CREATE TABLE lifecycle_updates(count INTEGER NOT NULL DEFAULT 0);
                 CREATE TRIGGER count_lifecycle_updates
                 AFTER UPDATE OF unreferenced_at ON blob_lifecycle
                 BEGIN
                     INSERT INTO lifecycle_updates(count) VALUES (1);
                 END",
        )
        .execute(&mut *conn)
        .await
        .unwrap();

        reconcile_liveness_for_hashes_in_transaction(
            &mut conn,
            std::slice::from_ref(&affected),
            &clock,
        )
        .await
        .unwrap();
        let unrelated_at: String =
            sqlx::query_scalar("SELECT unreferenced_at FROM blob_lifecycle WHERE sha256 = ?")
                .bind(&unrelated)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(unrelated_at, "2026-06-01T00:00:00Z");
        let updates: i64 = sqlx::query_scalar("SELECT count(*) FROM lifecycle_updates")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(updates, 1, "the live affected row changes once");

        reconcile_liveness_for_hashes_in_transaction(
            &mut conn,
            std::slice::from_ref(&affected),
            &clock,
        )
        .await
        .unwrap();
        let repeated_updates: i64 = sqlx::query_scalar("SELECT count(*) FROM lifecycle_updates")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(repeated_updates, 1, "an already-live row is not rewritten");

        sqlx::query(
            "UPDATE task_attachments SET deleted = 1, deleted_at = 'x'
                 WHERE attachment_id = '0000000000000011'",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
        reconcile_liveness_for_hashes_in_transaction(
            &mut conn,
            std::slice::from_ref(&affected),
            &clock,
        )
        .await
        .unwrap();
        let first_unreferenced: String =
            sqlx::query_scalar("SELECT unreferenced_at FROM blob_lifecycle WHERE sha256 = ?")
                .bind(&affected)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(first_unreferenced, "2026-07-01T00:00:00Z");
        clock.advance(chrono::Duration::days(1));
        reconcile_liveness_for_hashes_in_transaction(
            &mut conn,
            std::slice::from_ref(&affected),
            &clock,
        )
        .await
        .unwrap();
        let second_unreferenced: String =
            sqlx::query_scalar("SELECT unreferenced_at FROM blob_lifecycle WHERE sha256 = ?")
                .bind(&affected)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(first_unreferenced, second_unreferenced);

        sqlx::query(
            "UPDATE task_attachments SET deleted = 0, deleted_at = NULL
                 WHERE attachment_id = '0000000000000011'",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
        reconcile_liveness(&mut conn, &clock).await.unwrap();
        let restored: Option<String> =
            sqlx::query_scalar("SELECT unreferenced_at FROM blob_lifecycle WHERE sha256 = ?")
                .bind(&affected)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(restored, None);
        let full_updates: i64 = sqlx::query_scalar("SELECT count(*) FROM lifecycle_updates")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(full_updates, 3, "the full reset writes only the transition");
    }

    #[tokio::test]
    async fn accepted_server_reference_keeps_blob_live() {
        let temp = tempfile::tempdir().unwrap();
        let pool = open_db(&temp.path().join("test.sqlite")).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let hash = "acacacacacacacacacacacacacacacacacacacacacacacacacacacacacacacac";
        upsert_inventory_available(&mut conn, hash, 4, "image/png")
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO server_blob_references(
                   workspace_id, attachment_id, task_id, sha256, byte_size
                 ) VALUES ('workspace', '0000000000000042', '0000000000000004', ?, 4)",
        )
        .bind(hash)
        .execute(&mut *conn)
        .await
        .unwrap();
        let clock = TestClock::at("2026-07-01T00:00:00Z");

        reconcile_liveness(&mut conn, &clock).await.unwrap();
        let unreferenced_at: Option<String> =
            sqlx::query_scalar("SELECT unreferenced_at FROM blob_lifecycle WHERE sha256 = ?")
                .bind(hash)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(unreferenced_at, None);
    }
}
