use anyhow::Result;
use serde_json::Value;
use sqlx::{Row, SqliteConnection};

use crate::change_log::op_type;
use crate::db::{Database, begin_immediate};

impl Database {
    /// Rebuild attachment retention before server maintenance can prune old objects.
    pub async fn reconcile_server_attachment_parents(&self) -> Result<()> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let parents: Vec<(String, String)> = sqlx::query_as(
            "SELECT workspace_id, task_id FROM server_task_tombstones
             UNION SELECT workspace_id, task_id FROM server_blob_references",
        )
        .fetch_all(&mut *tx)
        .await?;
        for (workspace, task) in parents {
            reconcile_parent(&mut tx, &workspace, &task).await?;
        }
        crate::attachments::lifecycle::reconcile_liveness_in_transaction(
            &mut tx,
            &crate::attachments::lifecycle::SystemClock,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }
}

pub(super) async fn reconcile_parent(
    conn: &mut SqliteConnection,
    workspace: &str,
    task: &str,
) -> Result<()> {
    let rows = sqlx::query(
        "SELECT change_id, op_type, payload, base_version FROM changes
         WHERE entity_type = 'task' AND entity_id = ? AND server_seq IS NOT NULL
           AND json_extract(payload, '$.workspace_id') = ?
           AND (op_type = 'create_task'
                OR (op_type IN ('set_field', 'resolve_field') AND field = 'deleted'))
         ORDER BY server_seq",
    )
    .bind(task)
    .bind(workspace)
    .fetch_all(&mut *conn)
    .await?;
    let mut state = ParentState::default();
    for row in rows {
        let id: String = row.try_get("change_id")?;
        let operation: String = row.try_get("op_type")?;
        let payload: Value = serde_json::from_str(row.try_get("payload")?)?;
        let base: Option<String> = row.try_get("base_version")?;
        let seed = payload["task_field_version_seed"].as_str().unwrap_or(&id);
        state.apply(
            if operation == op_type::CREATE_TASK {
                0
            } else if operation == op_type::RESOLVE_FIELD {
                2
            } else {
                1
            },
            &id,
            payload["value"].as_str() == Some("1"),
            if operation == op_type::CREATE_TASK {
                Some(seed)
            } else {
                base.as_deref()
            },
        );
    }
    let unreferenced = state.deleted && state.version.is_some() && !state.protected;
    sqlx::query(
        "INSERT INTO server_task_tombstones(workspace_id, task_id, deleted)
         VALUES (?, ?, ?)
         ON CONFLICT(workspace_id, task_id) DO UPDATE SET deleted = excluded.deleted",
    )
    .bind(workspace)
    .bind(task)
    .bind(unreferenced)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Accepted-order conservative retention, independent of decrypted task storage.
#[derive(Default)]
pub(crate) struct ParentState {
    pub version: Option<String>,
    pub deleted: bool,
    pub protected: bool,
}
impl ParentState {
    /// A reference hint can add protection but cannot establish deletion evidence.
    pub fn protect_hint(&mut self, deleted: bool, version: Option<&str>) {
        self.protected |=
            self.version.is_none() || self.version.as_deref() != version || self.deleted != deleted;
    }

    pub fn apply(&mut self, action: u8, id: &str, deleted: bool, version: Option<&str>) {
        if action == 0 {
            if self.version.is_none() {
                self.version = version.map(str::to_owned);
            }
        } else if self.version.is_none() || (action == 1 && version != self.version.as_deref()) {
            self.protected = true;
        } else {
            self.deleted = deleted;
            self.version = Some(id.to_owned());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    async fn project(history: &[(&str, &str, Option<&str>, &str)]) -> bool {
        let (_temp, mut conn) = crate::test_support::test_conn().await;
        for (index, (id, op, base, value)) in history.iter().enumerate() {
            let payload = if *op == op_type::CREATE_TASK {
                json!({"workspace_id": "workspace", "task_field_version_seed": value})
            } else {
                json!({"workspace_id": "workspace", "value": value})
            };
            sqlx::query(
                "INSERT INTO changes(change_id, client_id, local_seq, entity_type, entity_id,
                 field, op_type, payload, base_version, created_at, server_seq)
                 VALUES (?, 'client', ?, 'task', 'task', 'deleted', ?, ?, ?, 'now', ?)",
            )
            .bind(id)
            .bind(index as i64 + 1)
            .bind(op)
            .bind(payload.to_string())
            .bind(base)
            .bind(index as i64 + 1)
            .execute(&mut *conn)
            .await
            .unwrap();
        }
        reconcile_parent(&mut conn, "workspace", "task")
            .await
            .unwrap();
        sqlx::query_scalar("SELECT deleted FROM server_task_tombstones WHERE task_id = 'task'")
            .fetch_one(&mut *conn)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn ordered_deletion_restore_and_custom_seed() {
        let mut history = vec![
            ("create", op_type::CREATE_TASK, None, "seed"),
            ("delete", op_type::SET_FIELD, Some("seed"), "1"),
        ];
        assert!(project(&history).await);
        history.push(("restore", op_type::SET_FIELD, Some("delete"), "0"));
        assert!(!project(&history).await);
        history.push(("delete2", op_type::SET_FIELD, Some("restore"), "1"));
        assert!(project(&history).await);
    }

    #[tokio::test]
    async fn stale_delete_and_force_resolution_keep_protection() {
        let mut history = vec![
            ("create", op_type::CREATE_TASK, None, "seed"),
            ("delete", op_type::SET_FIELD, Some("seed"), "1"),
            ("restore", op_type::SET_FIELD, Some("delete"), "0"),
            ("stale", op_type::SET_FIELD, Some("seed"), "1"),
        ];
        assert!(!project(&history).await);
        history.push(("resolve", op_type::RESOLVE_FIELD, Some("restore"), "1"));
        assert!(!project(&history).await);
    }

    #[tokio::test]
    async fn equal_value_conflicts_and_missing_creation_are_conservative() {
        assert!(
            !project(&[
                ("create", op_type::CREATE_TASK, None, "seed"),
                ("delete", op_type::SET_FIELD, Some("seed"), "1"),
                ("other", op_type::SET_FIELD, Some("seed"), "1"),
            ])
            .await
        );
        assert!(!project(&[("delete", op_type::SET_FIELD, None, "1")]).await);
        assert!(!project(&[]).await);
    }
}
