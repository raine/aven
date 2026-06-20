use sqlx::{Sqlite, pool::PoolConnection};

use crate::db::open_db;

pub async fn test_conn() -> (tempfile::TempDir, PoolConnection<Sqlite>) {
    let temp = tempfile::tempdir().unwrap();
    let pool = open_db(&temp.path().join("test.sqlite")).await.unwrap();
    let conn = pool.acquire().await.unwrap();
    (temp, conn)
}
