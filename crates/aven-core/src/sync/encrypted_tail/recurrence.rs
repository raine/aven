use anyhow::{Context, Result};
use sqlx::SqliteConnection;

use crate::sync::wire::ChangeWire;

pub(super) fn is_deterministic(change: &ChangeWire) -> bool {
    change.op_type == "project_recurrence_occurrence"
        || (change.op_type == "create_task" && change.payload["series_id"].is_string())
}

/// A generated `create_task` for a task that already exists is another generation
/// of the same occurrence. The task keeps its installed baseline when any
/// generation is in the captured or installed prefix, or when it has no other
/// generation. Otherwise its earliest generation in accepted order, then pending
/// order, is its baseline; an accepted generation ranked before that baseline
/// supplies the untouched defaults instead. Explicit changes, conflicts and history
/// are never replaced.
pub(super) async fn adopt_earlier_generation(
    conn: &mut SqliteConnection,
    prefix: i64,
    accepted: &ChangeWire,
) -> Result<()> {
    let sequence = accepted
        .server_seq
        .context("error encrypted-tail-generation-rank")?;
    let rows: Vec<(String, Option<i64>)> = sqlx::query_as(
        "SELECT change_id, server_seq FROM changes
         WHERE entity_type = 'task' AND entity_id = ? AND op_type = 'create_task'
           AND json_extract(payload, '$.series_id') IS NOT NULL
         ORDER BY server_seq IS NULL, server_seq, local_seq",
    )
    .bind(&accepted.entity_id)
    .fetch_all(&mut *conn)
    .await?;
    if rows
        .iter()
        .any(|(_, rank)| rank.is_some_and(|rank| rank <= prefix))
    {
        return Ok(());
    }
    let Some((baseline, rank)) = rows.iter().find(|(id, _)| *id != accepted.change_id) else {
        return Ok(());
    };
    if rank.is_some_and(|rank| rank < sequence) {
        return Ok(());
    }
    let baseline = super::client::load_change(conn, baseline)
        .await?
        .context("error encrypted-tail-history-lost")?;
    let labels = crate::sync::apply::adopt_generated_defaults(conn, &baseline, accepted)
        .await
        .map_err(|_| anyhow::anyhow!("error encrypted-tail-apply"))?;
    if !labels.is_empty() {
        let workspace = accepted.payload["workspace_id"]
            .as_str()
            .context("error encrypted-tail-apply")?;
        let count = labels.len();
        super::labels::reconcile_names(
            conn,
            prefix,
            workspace,
            labels,
            Some((&accepted.entity_id, count)),
        )
        .await?;
    }
    Ok(())
}
