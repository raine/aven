#[cfg(test)]
use super::*;
pub use aven_protocol::wire::tail::batch::*;

#[cfg(test)]
mod tests {
    #[cfg(test)]
    use super::*;
    use sqlx::{Connection, Executor};

    #[tokio::test]
    async fn migration_preserves_frozen_observed_and_blocked_ownership() {
        let mut conn = sqlx::SqliteConnection::connect("sqlite::memory:")
            .await
            .unwrap();
        conn.execute("PRAGMA foreign_keys=ON").await.unwrap();
        conn.execute("CREATE TABLE changes(change_id TEXT PRIMARY KEY,client_id TEXT,local_seq INTEGER,entity_type TEXT,entity_id TEXT,field TEXT,op_type TEXT,payload TEXT,base_version TEXT,created_at TEXT)").await.unwrap();
        conn.execute("INSERT INTO changes(change_id) VALUES ('frozen'),('accepted')")
            .await
            .unwrap();
        let schema = include_str!("../../../migrations/20260924091148_encrypted_sync.sql");
        let start = schema.find("CREATE TABLE local_e2ee_outbox (").unwrap();
        let end = schema.find("-- Local association state").unwrap();
        // Both bounds select literal schema source, never user input.
        sqlx::raw_sql(sqlx::AssertSqlSafe(&schema[start..end]))
            .execute(&mut conn)
            .await
            .unwrap();
        sqlx::query("INSERT INTO local_e2ee_outbox(singleton,operation_id,association,sync_generation,record,observed_sequence,observed_commitment,blocked) VALUES(1,'frozen','association',7,?,8,?,1)")
            .bind(vec![1u8; 100]).bind(vec![2u8; 32]).execute(&mut conn).await.unwrap();
        sqlx::query("INSERT INTO local_e2ee_accepted(operation_id,sequence,commitment,record) VALUES('accepted',9,?,?)")
            .bind(vec![3u8;32]).bind(vec![4u8;100]).execute(&mut conn).await.unwrap();
        let mut tx = conn.begin().await.unwrap();
        sqlx::raw_sql(include_str!(
            "../../../migrations/20260928163800_ordered_e2ee_outbox.sql"
        ))
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let row: (i64, String, i64, Vec<u8>, i64, Vec<u8>, bool) = sqlx::query_as("SELECT position,association,sync_generation,record,observed_sequence,observed_commitment,blocked FROM local_e2ee_outbox")
            .fetch_one(&mut conn).await.unwrap();
        assert_eq!(
            row,
            (
                1,
                "association".into(),
                7,
                vec![1; 100],
                8,
                vec![2; 32],
                true
            )
        );
        assert!(
            conn.execute("DELETE FROM changes WHERE change_id='frozen'")
                .await
                .is_err()
        );
        assert!(
            conn.execute("UPDATE changes SET payload='changed' WHERE change_id='accepted'")
                .await
                .is_err()
        );
        sqlx::query("UPDATE local_e2ee_outbox SET record=?")
            .bind(vec![1u8; RECORD_LIMIT])
            .execute(&mut conn)
            .await
            .unwrap();
        sqlx::query("UPDATE local_e2ee_accepted SET record=?")
            .bind(vec![1u8; RECORD_LIMIT])
            .execute(&mut conn)
            .await
            .unwrap();
        assert!(
            sqlx::query("UPDATE local_e2ee_outbox SET record=?")
                .bind(vec![1u8; RECORD_LIMIT + 1])
                .execute(&mut conn)
                .await
                .is_err()
        );
    }
}
