use anyhow::{Context, Result, bail};
use sqlx::{Row, SqliteConnection};

use crate::ids::TaskId;
use crate::operations::canonical_related_pair;
use crate::sync::wire::ChangeWire;

use super::shared::{str_payload, task_id, workspace_id_payload};

pub(super) async fn apply_related(
    conn: &mut SqliteConnection,
    change: &ChangeWire,
    linked: bool,
) -> Result<()> {
    let workspace_id = workspace_id_payload(conn, change).await?;
    let initiating_task_id = task_id(change)?;
    let related_task_id: TaskId = str_payload(&change.payload, "related_task_id")?.parse()?;
    let (task_a_id, task_b_id) = canonical_related_pair(&initiating_task_id, &related_task_id)?;
    let existing_tasks: i64 =
        sqlx::query_scalar("SELECT count(*) FROM tasks WHERE workspace_id = ? AND id IN (?, ?)")
            .bind(&workspace_id)
            .bind(task_a_id)
            .bind(task_b_id)
            .fetch_one(&mut *conn)
            .await?;
    if existing_tasks != 2 {
        bail!(
            "error related-missing-task task_id={} related_task_id={}",
            initiating_task_id,
            related_task_id
        );
    }

    let incoming_seq = change
        .server_seq
        .context("error invalid-sync-change related mutation missing server_seq")?;
    let current = sqlx::query(
        "SELECT c.server_seq
         FROM task_related_links r
         JOIN changes c ON c.change_id = r.last_change_id
         WHERE r.workspace_id = ? AND r.task_a_id = ? AND r.task_b_id = ?",
    )
    .bind(&workspace_id)
    .bind(task_a_id)
    .bind(task_b_id)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(current) = current {
        let current_seq: Option<i64> = current.get("server_seq");
        if current_seq.is_none() || current_seq.is_some_and(|seq| seq >= incoming_seq) {
            return Ok(());
        }
    }

    sqlx::query(
        "INSERT INTO task_related_links(
             workspace_id, task_a_id, task_b_id, linked, last_change_id
         ) VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(workspace_id, task_a_id, task_b_id) DO UPDATE SET
             linked = excluded.linked,
             last_change_id = excluded.last_change_id",
    )
    .bind(&workspace_id)
    .bind(task_a_id)
    .bind(task_b_id)
    .bind(i64::from(linked))
    .bind(&change.change_id)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::sync::wire::ChangeWire;
    use crate::workspaces::Workspace;

    async fn seed(conn: &mut SqliteConnection) -> Workspace {
        let workspace = Workspace::default();
        let project = crate::projects::create_project(conn, &workspace, "sync-related")
            .await
            .unwrap();
        for id in ["AAAA000000000001", "BBBB000000000002"] {
            sqlx::query(
                "INSERT INTO tasks(id, workspace_id, title, description, project_id, status, priority, created_at, updated_at)
                 VALUES (?, ?, 'task', '', ?, 'todo', 'none', 't', 't')",
            )
            .bind(id)
            .bind(&workspace.id)
            .bind(&project.id)
            .execute(&mut *conn)
            .await
            .unwrap();
        }
        workspace
    }

    fn change(workspace: &Workspace, id: &str, seq: i64, op: &str) -> ChangeWire {
        ChangeWire {
            change_id: id.to_string(),
            client_id: "remote".to_string(),
            local_seq: seq,
            entity_type: "task".to_string(),
            entity_id: "AAAA000000000001".to_string(),
            field: Some("related".to_string()),
            op_type: op.to_string(),
            payload: json!({
                "workspace_id": workspace.id,
                "workspace_key": workspace.key,
                "related_task_id": "BBBB000000000002"
            }),
            base_version: None,
            created_at: format!("t{seq}"),
            server_seq: Some(seq),
        }
    }

    async fn insert_change(conn: &mut SqliteConnection, change: &ChangeWire) {
        sqlx::query(
            "INSERT INTO changes(change_id, client_id, local_seq, entity_type, entity_id, field, op_type, payload, created_at, server_seq)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&change.change_id)
        .bind(&change.client_id)
        .bind(change.local_seq)
        .bind(&change.entity_type)
        .bind(&change.entity_id)
        .bind(&change.field)
        .bind(&change.op_type)
        .bind(change.payload.to_string())
        .bind(&change.created_at)
        .bind(change.server_seq)
        .execute(&mut *conn)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn unseen_remove_creates_state_and_newer_same_state_advances_change() {
        let (_temp, mut conn) = crate::test_support::test_conn().await;
        let workspace = seed(&mut conn).await;
        let first = change(&workspace, "CCCC000000000001", 1, "related_remove");
        insert_change(&mut conn, &first).await;
        apply_related(&mut conn, &first, false).await.unwrap();
        let second = change(&workspace, "CCCC000000000002", 2, "related_remove");
        insert_change(&mut conn, &second).await;
        apply_related(&mut conn, &second, false).await.unwrap();
        let state: (i64, String) =
            sqlx::query_as("SELECT linked, last_change_id FROM task_related_links")
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(state, (0, second.change_id));
    }

    #[tokio::test]
    async fn pending_local_state_wins_over_remote_change() {
        let (_temp, mut conn) = crate::test_support::test_conn().await;
        let workspace = seed(&mut conn).await;
        let local = change(&workspace, "CCCC000000000003", 1, "related_add");
        let mut local = local;
        local.server_seq = None;
        insert_change(&mut conn, &local).await;
        sqlx::query(
            "INSERT INTO task_related_links(workspace_id, task_a_id, task_b_id, linked, last_change_id)
             VALUES (?, 'AAAA000000000001', 'BBBB000000000002', 1, ?)",
        )
        .bind(&workspace.id)
        .bind(&local.change_id)
        .execute(&mut *conn)
        .await
        .unwrap();
        let remote = change(&workspace, "CCCC000000000004", 9, "related_remove");
        insert_change(&mut conn, &remote).await;
        apply_related(&mut conn, &remote, false).await.unwrap();
        let state: (i64, String) =
            sqlx::query_as("SELECT linked, last_change_id FROM task_related_links")
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(state, (1, local.change_id));
    }
}
