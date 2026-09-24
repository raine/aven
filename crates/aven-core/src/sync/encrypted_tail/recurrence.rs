use anyhow::{Context, Result};
use sqlx::SqliteConnection;

use crate::change_log::op_type;
use crate::sync::persistence;
use crate::sync::wire::ChangeWire;

/// Payload fields a generated task copies from its series template. Replicas that
/// edited the template concurrently can generate one occurrence with different values.
const TEMPLATE_DEFAULTS: [&str; 7] = [
    "title",
    "description",
    "project_id",
    "status",
    "priority",
    "labels",
    "metadata",
];

pub(super) fn is_deterministic(change: &ChangeWire) -> bool {
    change.op_type == "project_recurrence_occurrence"
        || (change.op_type == "create_task" && change.payload["series_id"].is_string())
}

/// An accepted generated task supersedes this replica's pending generation of the
/// same occurrence when the two differ only in template defaults. Occurrence identity,
/// schedule, timestamps and field-version seeds must still be equal.
pub(super) fn supersedes_pending(pending: &ChangeWire, accepted: &ChangeWire) -> bool {
    let (Some(left), Some(right)) = (pending.payload.as_object(), accepted.payload.as_object())
    else {
        return false;
    };
    pending.server_seq.is_none()
        && pending.op_type == op_type::CREATE_TASK
        && is_deterministic(pending)
        && is_deterministic(accepted)
        && pending.change_id == accepted.change_id
        && pending.entity_type == accepted.entity_type
        && pending.entity_id == accepted.entity_id
        && pending.field == accepted.field
        && pending.op_type == accepted.op_type
        && pending.base_version == accepted.base_version
        && pending.created_at == accepted.created_at
        && left
            .keys()
            .chain(right.keys())
            .all(|key| TEMPLATE_DEFAULTS.contains(&key.as_str()) || left.get(key) == right.get(key))
}

/// Replaces the pending generation with accepted history, resets the task to the
/// accepted defaults, then reapplies this replica's pending task commands in push
/// order, as other replicas apply them after the accepted generation. Notes, images,
/// relationships and occurrence links keep the stable task identity.
pub(super) async fn supersede_pending(
    conn: &mut SqliteConnection,
    prefix: i64,
    pending: &ChangeWire,
    accepted: &ChangeWire,
) -> Result<()> {
    // The outbox row protects its history row, so release it first.
    sqlx::query("DELETE FROM local_e2ee_outbox WHERE operation_id = ?")
        .bind(&pending.change_id)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM changes WHERE change_id = ? AND server_seq IS NULL")
        .bind(&pending.change_id)
        .execute(&mut *conn)
        .await?;
    persistence::insert_wire_change(conn, accepted).await?;
    crate::sync::apply::reset_generated_task(conn, pending, accepted).await?;
    let commands: Vec<String> = sqlx::query_scalar(
        "SELECT change_id FROM changes
         WHERE server_seq IS NULL AND entity_type = 'task' AND entity_id = ?
           AND op_type IN ('set_field', 'resolve_field', 'set_task_metadata', 'remove_task_metadata')
         ORDER BY local_seq",
    )
    .bind(&accepted.entity_id)
    .fetch_all(&mut *conn)
    .await?;
    for id in commands {
        let command = super::client::load_change(conn, &id)
            .await?
            .context("error encrypted-tail-history-lost")?;
        crate::sync::apply::apply_remote_change_quiet(conn, &command)
            .await
            .map_err(|_| anyhow::anyhow!("error encrypted-tail-apply"))?;
    }
    let workspace = accepted.payload["workspace_id"]
        .as_str()
        .context("error encrypted-tail-apply")?;
    let labels = |change: &ChangeWire| -> Vec<String> {
        change.payload["labels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|label| label.as_str().map(str::to_owned))
            .collect()
    };
    let (before, after) = (labels(pending), labels(accepted));
    for label in before.iter().chain(&after) {
        if before.contains(label) != after.contains(label) {
            super::labels::reconcile_pair(conn, prefix, workspace, &accepted.entity_id, label)
                .await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn generated(title: &str, available_local_time: &str) -> ChangeWire {
        ChangeWire {
            change_id: "generated".into(),
            client_id: "client".into(),
            local_seq: 1,
            entity_type: "task".into(),
            entity_id: "task".into(),
            field: None,
            op_type: op_type::CREATE_TASK.into(),
            payload: json!({
                "series_id": "series",
                "title": title,
                "labels": [title],
                "available_local_time": available_local_time,
            }),
            base_version: None,
            created_at: "2026-09-23T00:00:00Z".into(),
            server_seq: None,
        }
    }

    #[test]
    fn only_pending_template_defaults_can_be_superseded() {
        let pending = generated("local", "09:00");
        assert!(supersedes_pending(
            &pending,
            &generated("accepted", "09:00")
        ));
        assert!(!supersedes_pending(
            &pending,
            &generated("accepted", "10:00")
        ));
        let mut ranked = pending.clone();
        ranked.server_seq = Some(1);
        assert!(!supersedes_pending(
            &ranked,
            &generated("accepted", "09:00")
        ));
        let mut projection = pending.clone();
        projection.op_type = op_type::PROJECT_RECURRENCE_OCCURRENCE.into();
        assert!(!supersedes_pending(&projection, &projection.clone()));
    }
}
