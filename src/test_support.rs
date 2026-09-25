pub(crate) mod e2ee_http;

use std::path::Path;

use crate::ids::{BASE32, TaskId};

/// Opens a private copy of the blank migrated database template at `path`,
/// returning the database with its own pool for raw SQL.
pub(crate) async fn open_database(
    path: &Path,
) -> anyhow::Result<(aven_core::db::Database, sqlx::SqlitePool)> {
    let database = aven_core::test_support::open_blank_database(path).await?;
    let pool = aven_core::test_support::pool(&database);
    Ok((database, pool))
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
