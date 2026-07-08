use std::collections::HashSet;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use sqlx::{SqliteConnection, query_scalar};

use crate::attachments::storage::{object_path, sha256_hex};
use crate::attachments::validation::SUPPORTED_MEDIA_TYPES;

use super::IntegrityCheck;

pub(crate) async fn attachment_integrity_checks(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
) -> Result<Vec<IntegrityCheck>> {
    let mut checks = Vec::new();
    checks.push(count_check(
        conn,
        "attachment inventory",
        "SELECT count(*) FROM task_attachments ta LEFT JOIN blob_inventory bi ON bi.sha256 = ta.sha256 WHERE bi.sha256 IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "attachment tasks",
        "SELECT count(*) FROM task_attachments ta LEFT JOIN tasks t ON t.workspace_id = ta.workspace_id AND t.id = ta.task_id WHERE t.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "attachment changes",
        "SELECT count(*) FROM task_attachments ta LEFT JOIN changes c ON c.change_id = ta.created_by_change_id WHERE ta.created_by_change_id IS NOT NULL AND c.change_id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "attachment inventory metadata",
        "SELECT count(*) FROM task_attachments ta JOIN blob_inventory bi ON bi.sha256 = ta.sha256 WHERE bi.byte_size != ta.byte_size OR bi.media_type != ta.media_type",
    )
    .await?);
    checks.push(count_check(
        conn,
        "attachment media types",
        "SELECT count(*) FROM blob_inventory WHERE media_type NOT IN ('image/png', 'image/jpeg', 'image/gif', 'image/webp')",
    )
    .await?);
    checks.push(
        count_check(
            conn,
            "attachment sizes",
            "SELECT count(*) FROM blob_inventory WHERE byte_size <= 0",
        )
        .await?,
    );
    checks.push(missing_blob_check(conn, blob_dir).await?);
    checks.push(mismatched_blob_check(conn, blob_dir).await?);
    checks.push(orphan_blob_check(conn, blob_dir).await?);
    Ok(checks)
}

async fn count_check(
    conn: &mut SqliteConnection,
    label: &'static str,
    query: &'static str,
) -> Result<IntegrityCheck> {
    let count: i64 = query_scalar(query).fetch_one(&mut *conn).await?;
    Ok(IntegrityCheck {
        label,
        ok: count == 0,
        value: format!("{count} orphaned"),
    })
}

async fn missing_blob_check(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
) -> Result<IntegrityCheck> {
    let rows: Vec<String> =
        sqlx::query_scalar("SELECT sha256 FROM blob_inventory WHERE available = 1")
            .fetch_all(&mut *conn)
            .await?;
    let count = rows
        .iter()
        .filter(|sha| object_path(blob_dir, sha).is_ok_and(|path| !path.exists()))
        .count();
    Ok(IntegrityCheck {
        label: "attachment objects",
        ok: count == 0,
        value: format!("{count} missing"),
    })
}

async fn mismatched_blob_check(
    conn: &mut SqliteConnection,
    blob_dir: &Path,
) -> Result<IntegrityCheck> {
    let rows: Vec<(String, i64, String)> = sqlx::query_as(
        "SELECT sha256, byte_size, media_type FROM blob_inventory WHERE available = 1",
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut count = 0_usize;
    for (sha, byte_size, media_type) in rows {
        if !SUPPORTED_MEDIA_TYPES.contains(&media_type.as_str()) {
            continue;
        }
        let Ok(path) = object_path(blob_dir, &sha) else {
            count += 1;
            continue;
        };
        if !path.exists() {
            continue;
        }
        let bytes =
            fs::read(&path).with_context(|| format!("could not read {}", path.display()))?;
        if sha256_hex(&bytes) != sha || i64::try_from(bytes.len()).unwrap_or(-1) != byte_size {
            count += 1;
        }
    }
    Ok(IntegrityCheck {
        label: "attachment object hashes",
        ok: count == 0,
        value: format!("{count} mismatched"),
    })
}

async fn orphan_blob_check(conn: &mut SqliteConnection, blob_dir: &Path) -> Result<IntegrityCheck> {
    let rows: Vec<String> = sqlx::query_scalar("SELECT sha256 FROM blob_inventory")
        .fetch_all(&mut *conn)
        .await?;
    let known = rows.into_iter().collect::<HashSet<_>>();
    let object_dir = blob_dir.join("objects").join("sha256");
    let mut count = 0_usize;
    if object_dir.exists() {
        for entry in fs::read_dir(&object_dir)
            .with_context(|| format!("could not read {}", object_dir.display()))?
        {
            let entry = entry?;
            if entry.file_type()?.is_file()
                && let Some(name) = entry.file_name().to_str()
                && !known.contains(name)
            {
                count += 1;
            }
        }
    }
    Ok(IntegrityCheck {
        label: "attachment orphan objects",
        ok: count == 0,
        value: format!("{count} orphaned"),
    })
}
