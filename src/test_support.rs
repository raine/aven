use std::path::Path;

use crate::ids::{BASE32, TaskId};

pub(crate) async fn open_db(path: &Path) -> anyhow::Result<sqlx::SqlitePool> {
    aven_core::db::Database::open(path).await?;
    Ok(sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(true),
        )
        .await?)
}

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
