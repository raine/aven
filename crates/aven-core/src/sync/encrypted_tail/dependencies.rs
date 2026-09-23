//! Association-lifetime graph baseline and sequential dependency-tail reduction.
use anyhow::{Context, Result, ensure};
use sqlx::SqliteConnection;

use crate::data_safety::TaskDependencyRow;
use crate::sync::wire::ChangeWire;

pub(crate) async fn initialize(
    conn: &mut SqliteConnection,
    association: &str,
    generation: i64,
    prefix: i64,
    edges: &[TaskDependencyRow],
) -> Result<()> {
    let occupied: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM local_e2ee_dependency_edges)")
            .fetch_one(&mut *conn)
            .await?;
    ensure!(
        !occupied,
        "error encrypted-dependency-baseline-reinitialization-required"
    );
    sqlx::query("INSERT INTO local_e2ee_dependency_baseline(singleton, association, sync_generation, prefix_count) VALUES (1, ?, ?, ?)")
        .bind(association).bind(generation).bind(prefix).execute(&mut *conn).await?;
    for edge in edges {
        sqlx::query("INSERT INTO local_e2ee_dependency_edges(workspace_id, task_id, depends_on_task_id, created_at) VALUES (?, ?, ?, ?)")
            .bind(&edge.workspace_id).bind(&edge.task_id).bind(&edge.depends_on_task_id).bind(&edge.created_at)
            .execute(&mut *conn).await?;
    }
    Ok(())
}

pub(crate) async fn validate(
    conn: &mut SqliteConnection,
    association: &str,
    prefix: i64,
) -> Result<()> {
    let matches: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM local_e2ee_dependency_baseline
         WHERE singleton = 1 AND association = ? AND prefix_count = ?
           AND association = (SELECT value FROM meta WHERE key = 'e2ee_association')
           AND sync_generation = CAST((SELECT value FROM meta WHERE key = 'sync_generation') AS INTEGER))",
    ).bind(association).bind(prefix).fetch_one(conn).await?;
    ensure!(
        matches,
        "error encrypted-dependency-baseline-reinitialization-required"
    );
    Ok(())
}

pub(super) fn affected_workspace(change: &ChangeWire) -> Result<Option<&str>> {
    if !matches!(
        change.op_type.as_str(),
        "dependency_add" | "dependency_remove"
    ) {
        return Ok(None);
    }
    Ok(Some(
        change.payload["workspace_id"]
            .as_str()
            .context("error encrypted-dependency-history")?,
    ))
}

// The caller owns the association check and the complete outcome/page transaction.
// Only dependency commands are replayed: task activity and other domains are untouched.
pub(super) async fn reconcile(
    conn: &mut SqliteConnection,
    prefix: i64,
    workspace: &str,
) -> Result<()> {
    let dangling: bool = sqlx::query_scalar("SELECT EXISTS(
        SELECT 1 FROM local_e2ee_dependency_edges e WHERE e.workspace_id = ?
        AND (NOT EXISTS(SELECT 1 FROM tasks t WHERE t.workspace_id=e.workspace_id AND t.id=e.task_id)
          OR NOT EXISTS(SELECT 1 FROM tasks t WHERE t.workspace_id=e.workspace_id AND t.id=e.depends_on_task_id)))")
        .bind(workspace).fetch_one(&mut *conn).await?;
    ensure!(
        !dangling,
        "error encrypted-dependency-baseline-reinitialization-required"
    );
    sqlx::query("DELETE FROM task_dependencies WHERE workspace_id = ?")
        .bind(workspace)
        .execute(&mut *conn)
        .await?;
    sqlx::query(
        "INSERT INTO task_dependencies(workspace_id, task_id, depends_on_task_id, created_at)
        SELECT workspace_id, task_id, depends_on_task_id, created_at
        FROM local_e2ee_dependency_edges WHERE workspace_id = ?",
    )
    .bind(workspace)
    .execute(&mut *conn)
    .await?;
    // Read one bounded batch at a time without retaining the whole association history.
    // Graph writes never change the ordered command set inside this transaction.
    let mut offset = 0_i64;
    loop {
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT change_id FROM changes
             WHERE entity_type = 'task' AND op_type IN ('dependency_add', 'dependency_remove')
               AND json_extract(payload, '$.workspace_id') = ?
               AND (server_seq > ? OR server_seq IS NULL)
             ORDER BY server_seq IS NULL, server_seq, local_seq, created_at, change_id
             LIMIT 128 OFFSET ?",
        )
        .bind(workspace)
        .bind(prefix)
        .bind(offset)
        .fetch_all(&mut *conn)
        .await?;
        if ids.is_empty() {
            break;
        }
        offset += i64::try_from(ids.len())?;
        for id in ids {
            let change = super::client::load_change(conn, &id)
                .await?
                .context("error encrypted-dependency-history")?;
            crate::sync::apply::apply_remote_change_quiet(conn, &change).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
