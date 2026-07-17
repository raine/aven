use sqlx::{Sqlite, pool::PoolConnection};

use crate::db::open_db;
use crate::ids::{BASE32, TaskId};

pub(crate) fn task_id(value: &str) -> TaskId {
    let mut encoded = value
        .bytes()
        .map(|byte| match byte.to_ascii_uppercase() {
            b'O' => '0',
            b'I' | b'L' => '1',
            byte if BASE32.contains(&byte) => char::from(byte),
            byte => char::from(BASE32[usize::from(byte) % BASE32.len()]),
        })
        .take(16)
        .collect::<String>();
    encoded.extend(std::iter::repeat_n('0', 16 - encoded.len()));
    encoded.parse().unwrap()
}

pub async fn test_conn() -> (tempfile::TempDir, PoolConnection<Sqlite>) {
    let temp = tempfile::tempdir().unwrap();
    let pool = open_db(&temp.path().join("test.sqlite")).await.unwrap();
    let conn = pool.acquire().await.unwrap();
    (temp, conn)
}
