use anyhow::{Context, Result};
use sqlx::SqliteConnection;

use crate::operations::{ProjectMetadata, set_project_metadata as apply_project_metadata};
use crate::projects::{normalize_key, prefix_base};
use crate::sync::wire::ChangeWire;

use super::shared::str_payload;

pub(super) async fn create_project(conn: &mut SqliteConnection, change: &ChangeWire) -> Result<()> {
    let workspace_id = super::shared::workspace_id_payload(conn, change).await?;
    let key = str_payload(&change.payload, "key")?;
    let name = str_payload(&change.payload, "name")?;
    let prefix = str_payload(&change.payload, "prefix")?;
    let created_at =
        str_payload(&change.payload, "created_at").unwrap_or_else(|_| crate::ids::now());
    ensure_remote_project(
        conn,
        &workspace_id,
        &change.entity_id,
        &key,
        &name,
        &prefix,
        &created_at,
    )
    .await?;
    Ok(())
}

pub(super) async fn set_project_metadata(
    conn: &mut SqliteConnection,
    change: &ChangeWire,
) -> Result<()> {
    let workspace_id = super::shared::workspace_id_payload(conn, change).await?;
    let key = str_payload(&change.payload, "key")?;
    let name = str_payload(&change.payload, "name")?;
    let prefix = str_payload(&change.payload, "prefix")?;
    validate_project_metadata_payload(&key, &name, &prefix)?;
    let project_id = ensure_project_for_metadata(conn, &workspace_id, change).await?;
    let previous_key = live_project_key_by_id(conn, &workspace_id, &project_id)
        .await?
        .context("project metadata target missing")?;
    let key = unique_remote_key(conn, &workspace_id, &key, Some(&project_id)).await?;
    let prefix =
        unique_remote_prefix(conn, &workspace_id, &prefix, &key, Some(&project_id)).await?;
    let workspace = crate::workspaces::workspace_for_id(conn, &workspace_id).await?;
    apply_project_metadata(
        conn,
        &workspace,
        &project_id,
        ProjectMetadata {
            key: &key,
            name: &name,
            prefix: &prefix,
        },
        false,
    )
    .await?;
    if previous_key != key {
        crate::operations::rename_config_project_mapping(&workspace, &previous_key, &key)?;
    }
    Ok(())
}

fn validate_project_metadata_payload(key: &str, name: &str, prefix: &str) -> Result<()> {
    anyhow::ensure!(
        !key.is_empty() && normalize_key(name) == key,
        "error invalid-sync-change project-metadata key"
    );
    anyhow::ensure!(
        (2..=8).contains(&prefix.len()) && prefix.chars().all(|ch| ch.is_ascii_alphanumeric()),
        "error invalid-sync-change project-metadata prefix"
    );
    Ok(())
}

async fn ensure_project_for_metadata(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    change: &ChangeWire,
) -> Result<String> {
    if let Some(local_id) = project_id_alias(conn, workspace_id, &change.entity_id).await? {
        return Ok(local_id);
    }
    if let Some(existing_id) = live_project_by_id(conn, workspace_id, &change.entity_id).await? {
        return Ok(existing_id);
    }
    let key = str_payload(&change.payload, "key")?;
    let name = str_payload(&change.payload, "name")?;
    let prefix = str_payload(&change.payload, "prefix")?;
    ensure_remote_project(
        conn,
        workspace_id,
        &change.entity_id,
        &key,
        &name,
        &prefix,
        &change.created_at,
    )
    .await
}

pub(super) async fn ensure_project_for_payload(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    project_id: &str,
    change: &ChangeWire,
) -> Result<String> {
    let key = str_payload(&change.payload, "project_key")?;
    let name = str_payload(&change.payload, "project_name").unwrap_or_else(|_| key.clone());
    let prefix =
        str_payload(&change.payload, "project_prefix").unwrap_or_else(|_| prefix_base(&key));
    ensure_remote_project(
        conn,
        workspace_id,
        project_id,
        &key,
        &name,
        &prefix,
        &change.created_at,
    )
    .await
}

async fn ensure_remote_project(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    remote_project_id: &str,
    key: &str,
    name: &str,
    prefix: &str,
    created_at: &str,
) -> Result<String> {
    if let Some(local_id) = project_id_alias(conn, workspace_id, remote_project_id).await? {
        return Ok(local_id);
    }
    if let Some(existing_id) = live_project_by_id(conn, workspace_id, remote_project_id).await? {
        return Ok(existing_id);
    }
    if let Some(local_id) = live_project_by_key(conn, workspace_id, key).await? {
        insert_project_alias(conn, workspace_id, remote_project_id, &local_id).await?;
        return Ok(local_id);
    }
    if deleted_project_by_id(conn, workspace_id, remote_project_id).await? {
        restore_remote_project(
            conn,
            workspace_id,
            remote_project_id,
            key,
            name,
            prefix,
            created_at,
        )
        .await?;
        return Ok(remote_project_id.to_string());
    }
    if let Some(local_id) = deleted_project_by_key(conn, workspace_id, key).await? {
        restore_remote_project(conn, workspace_id, &local_id, key, name, prefix, created_at)
            .await?;
        insert_project_alias(conn, workspace_id, remote_project_id, &local_id).await?;
        return Ok(local_id);
    }
    let prefix = unique_remote_prefix(conn, workspace_id, prefix, key, None).await?;
    sqlx::query(
        "INSERT INTO projects(id, workspace_id, key, name, prefix, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(remote_project_id)
    .bind(workspace_id)
    .bind(key)
    .bind(name)
    .bind(&prefix)
    .bind(created_at)
    .bind(created_at)
    .execute(&mut *conn)
    .await?;
    Ok(remote_project_id.to_string())
}

async fn live_project_by_id(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    project_id: &str,
) -> Result<Option<String>> {
    sqlx::query_scalar::<_, String>(
        "SELECT id FROM projects WHERE workspace_id = ? AND id = ? AND deleted = 0",
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(Into::into)
}

async fn live_project_key_by_id(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    project_id: &str,
) -> Result<Option<String>> {
    sqlx::query_scalar::<_, String>(
        "SELECT key FROM projects WHERE workspace_id = ? AND id = ? AND deleted = 0",
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(Into::into)
}

async fn live_project_by_key(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    key: &str,
) -> Result<Option<String>> {
    sqlx::query_scalar::<_, String>(
        "SELECT id FROM projects WHERE workspace_id = ? AND key = ? AND deleted = 0",
    )
    .bind(workspace_id)
    .bind(key)
    .fetch_optional(&mut *conn)
    .await
    .map_err(Into::into)
}

async fn deleted_project_by_id(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    project_id: &str,
) -> Result<bool> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM projects WHERE workspace_id = ? AND id = ? AND deleted = 1",
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_one(&mut *conn)
    .await?
        > 0)
}

async fn deleted_project_by_key(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    key: &str,
) -> Result<Option<String>> {
    sqlx::query_scalar::<_, String>(
        "SELECT id FROM projects WHERE workspace_id = ? AND key = ? AND deleted = 1",
    )
    .bind(workspace_id)
    .bind(key)
    .fetch_optional(&mut *conn)
    .await
    .map_err(Into::into)
}

async fn restore_remote_project(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    project_id: &str,
    key: &str,
    name: &str,
    prefix: &str,
    updated_at: &str,
) -> Result<()> {
    let prefix = unique_remote_prefix(conn, workspace_id, prefix, key, Some(project_id)).await?;
    sqlx::query(
        "UPDATE projects SET name = ?, prefix = ?, updated_at = ?, deleted = 0
         WHERE workspace_id = ? AND id = ?",
    )
    .bind(name)
    .bind(&prefix)
    .bind(updated_at)
    .bind(workspace_id)
    .bind(project_id)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

async fn insert_project_alias(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    remote_project_id: &str,
    local_project_id: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT OR IGNORE INTO project_id_aliases(workspace_id, remote_project_id, local_project_id)
         VALUES (?, ?, ?)",
    )
    .bind(workspace_id)
    .bind(remote_project_id)
    .bind(local_project_id)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

async fn unique_remote_key(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    preferred: &str,
    ignore_project_id: Option<&str>,
) -> Result<String> {
    let mut candidate = preferred.to_string();
    let mut n = 2;
    while sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM projects
         WHERE workspace_id = ? AND key = ? AND (? IS NULL OR id != ?)",
    )
    .bind(workspace_id)
    .bind(&candidate)
    .bind(ignore_project_id)
    .bind(ignore_project_id)
    .fetch_one(&mut *conn)
    .await?
        > 0
    {
        candidate = format!("{preferred}-{n}");
        n += 1;
    }
    Ok(candidate)
}

async fn unique_remote_prefix(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    preferred: &str,
    key: &str,
    ignore_project_id: Option<&str>,
) -> Result<String> {
    let base = if preferred.trim().is_empty() {
        prefix_base(key)
    } else {
        preferred.to_string()
    };
    let mut candidate = base.clone();
    let mut n = 2;
    while sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM projects
         WHERE workspace_id = ? AND prefix = ? AND (? IS NULL OR id != ?)",
    )
    .bind(workspace_id)
    .bind(&candidate)
    .bind(ignore_project_id)
    .bind(ignore_project_id)
    .fetch_one(&mut *conn)
    .await?
        > 0
    {
        candidate = format!("{}{}", base.chars().take(2).collect::<String>(), n);
        n += 1;
    }
    Ok(candidate)
}

async fn project_id_alias(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    remote_project_id: &str,
) -> Result<Option<String>> {
    Ok(sqlx::query_scalar::<_, String>(
        "SELECT a.local_project_id
         FROM project_id_aliases a
         JOIN projects p ON p.workspace_id = a.workspace_id
          AND p.id = a.local_project_id
          AND p.deleted = 0
         WHERE a.workspace_id = ? AND a.remote_project_id = ?",
    )
    .bind(workspace_id)
    .bind(remote_project_id)
    .fetch_optional(&mut *conn)
    .await?)
}
