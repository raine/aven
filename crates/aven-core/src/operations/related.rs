use anyhow::Result;
use sqlx::{Row, SqliteConnection};

use crate::change_log::{ChangeEntity, ChangePayload, append_change, op_type};
use crate::db::{Database, begin_immediate};
use crate::error::CoreError;
use crate::ids::{TaskId, WorkspaceId};
use crate::types::Task;
use crate::undo::{UndoCommand, UndoContext, UndoPayload, record_tui_undo};
use crate::workspaces::Workspace;

pub(crate) fn canonical_related_pair<'a>(
    first: &'a TaskId,
    second: &'a TaskId,
) -> Result<(&'a TaskId, &'a TaskId)> {
    if first == second {
        return Err(CoreError::validation(format!("error related-self task_id={first}")).into());
    }
    Ok(if first < second {
        (first, second)
    } else {
        (second, first)
    })
}

#[derive(Debug)]
pub struct RelatedOutcome {
    pub task: Task,
    pub related_task: Task,
    pub changed: bool,
    pub change_id: Option<String>,
}

impl Database {
    pub async fn add_task_related_link(
        &self,
        workspace: &Workspace,
        task_id: &TaskId,
        related_task_id: &TaskId,
    ) -> Result<RelatedOutcome> {
        self.add_task_related_link_with_undo(workspace, task_id, related_task_id, UndoContext::None)
            .await
    }

    pub async fn add_task_related_link_with_undo(
        &self,
        workspace: &Workspace,
        task_id: &TaskId,
        related_task_id: &TaskId,
        undo: UndoContext,
    ) -> Result<RelatedOutcome> {
        self.set_task_related_link(workspace, task_id, related_task_id, true, undo)
            .await
    }

    pub async fn remove_task_related_link(
        &self,
        workspace: &Workspace,
        task_id: &TaskId,
        related_task_id: &TaskId,
    ) -> Result<RelatedOutcome> {
        self.remove_task_related_link_with_undo(
            workspace,
            task_id,
            related_task_id,
            UndoContext::None,
        )
        .await
    }

    pub async fn remove_task_related_link_with_undo(
        &self,
        workspace: &Workspace,
        task_id: &TaskId,
        related_task_id: &TaskId,
        undo: UndoContext,
    ) -> Result<RelatedOutcome> {
        self.set_task_related_link(workspace, task_id, related_task_id, false, undo)
            .await
    }

    async fn set_task_related_link(
        &self,
        workspace: &Workspace,
        task_id: &TaskId,
        related_task_id: &TaskId,
        linked: bool,
        undo: UndoContext,
    ) -> Result<RelatedOutcome> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let outcome = set_task_related_link_in_transaction(
            &mut tx,
            workspace,
            task_id,
            related_task_id,
            linked,
        )
        .await?;
        if let (Some(change_id), UndoContext::Tui { summary }) = (&outcome.change_id, undo) {
            record_tui_undo(
                &mut tx,
                &workspace.id,
                &summary,
                UndoPayload {
                    commands: vec![UndoCommand::SetTaskRelatedLink {
                        task_id: outcome.task.id.clone(),
                        related_task_id: outcome.related_task.id.clone(),
                        forward_change_id: change_id.clone(),
                        linked,
                    }],
                },
            )
            .await?;
        }
        tx.commit().await?;
        Ok(outcome)
    }
}

async fn load_task_by_id(conn: &mut SqliteConnection, task_id: &TaskId) -> Result<Option<Task>> {
    let row = sqlx::query(
        "SELECT t.id, t.workspace_id, t.title, t.description, t.project_id,
                p.key AS project_key, p.prefix AS project_prefix, t.status, t.priority,
                t.source, t.created_at, t.updated_at, t.queue_activity_at,
                t.available_at, t.due_on, t.deleted, t.is_epic
         FROM tasks t
         JOIN projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id
         WHERE t.id = ?",
    )
    .bind(task_id)
    .fetch_optional(&mut *conn)
    .await?;
    row.map(|row| crate::db::task_from_row(&row)).transpose()
}

async fn load_related_tasks(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &TaskId,
    related_task_id: &TaskId,
) -> Result<(Task, Task)> {
    canonical_related_pair(task_id, related_task_id)?;
    let task = load_task_by_id(conn, task_id).await?.ok_or_else(|| {
        CoreError::not_found(format!("error related-task-not-found task_id={task_id}"))
    })?;
    let related_task = load_task_by_id(conn, related_task_id)
        .await?
        .ok_or_else(|| {
            CoreError::not_found(format!(
                "error related-task-not-found task_id={related_task_id}"
            ))
        })?;
    if task.workspace_id != workspace.id || related_task.workspace_id != workspace.id {
        return Err(CoreError::validation(format!(
            "error related-cross-workspace task_id={task_id} related_task_id={related_task_id} workspace_id={}",
            workspace.id
        ))
        .into());
    }
    Ok((task, related_task))
}

pub(crate) async fn set_task_related_link_in_transaction(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &TaskId,
    related_task_id: &TaskId,
    linked: bool,
) -> Result<RelatedOutcome> {
    let (task, related_task) =
        load_related_tasks(conn, workspace, task_id, related_task_id).await?;
    let (task_a_id, task_b_id) = canonical_related_pair(task_id, related_task_id)?;
    let current = sqlx::query(
        "SELECT linked FROM task_related_links
         WHERE workspace_id = ? AND task_a_id = ? AND task_b_id = ?",
    )
    .bind(&workspace.id)
    .bind(task_a_id)
    .bind(task_b_id)
    .fetch_optional(&mut *conn)
    .await?
    .map(|row| row.get::<i64, _>("linked") != 0)
    .unwrap_or(false);
    if current == linked {
        return Ok(RelatedOutcome {
            task,
            related_task,
            changed: false,
            change_id: None,
        });
    }

    let op = if linked {
        op_type::RELATED_ADD
    } else {
        op_type::RELATED_REMOVE
    };
    let change_id = append_change(
        conn,
        ChangeEntity::Task,
        task_id,
        Some("related"),
        op,
        ChangePayload::workspace(workspace).set("related_task_id", related_task_id),
    )
    .await?;
    sqlx::query(
        "INSERT INTO task_related_links(
             workspace_id, task_a_id, task_b_id, linked, last_change_id
         ) VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(workspace_id, task_a_id, task_b_id) DO UPDATE SET
             linked = excluded.linked,
             last_change_id = excluded.last_change_id",
    )
    .bind(&workspace.id)
    .bind(task_a_id)
    .bind(task_b_id)
    .bind(i64::from(linked))
    .bind(&change_id)
    .execute(&mut *conn)
    .await?;

    Ok(RelatedOutcome {
        task,
        related_task,
        changed: true,
        change_id: Some(change_id),
    })
}

pub(crate) async fn task_has_related_state(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: &TaskId,
) -> Result<bool> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT EXISTS(
             SELECT 1 FROM task_related_links
             WHERE workspace_id = ? AND (task_a_id = ? OR task_b_id = ?)
         )",
    )
    .bind(workspace_id)
    .bind(task_id)
    .bind(task_id)
    .fetch_one(&mut *conn)
    .await?
        != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn seed(conn: &mut SqliteConnection) -> (Workspace, TaskId, TaskId) {
        let workspace = Workspace::default();
        let project = crate::projects::create_project(conn, &workspace, "related")
            .await
            .unwrap();
        let first: TaskId = "AAAA000000000001".parse().unwrap();
        let second: TaskId = "BBBB000000000002".parse().unwrap();
        for (id, title) in [(&first, "first"), (&second, "second")] {
            sqlx::query(
                "INSERT INTO tasks(id, workspace_id, title, description, project_id, status, priority, created_at, updated_at)
                 VALUES (?, ?, ?, '', ?, 'todo', 'none', 't', 't')",
            )
            .bind(id)
            .bind(&workspace.id)
            .bind(title)
            .bind(&project.id)
            .execute(&mut *conn)
            .await
            .unwrap();
        }
        (workspace, first, second)
    }

    #[tokio::test]
    async fn canonical_add_reverse_idempotency_and_symmetric_remove() {
        let (_temp, mut conn) = crate::test_support::test_conn().await;
        let (workspace, first, second) = seed(&mut conn).await;
        let added =
            set_task_related_link_in_transaction(&mut conn, &workspace, &second, &first, true)
                .await
                .unwrap();
        assert!(added.changed);
        let row: (TaskId, TaskId, i64) =
            sqlx::query_as("SELECT task_a_id, task_b_id, linked FROM task_related_links")
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(row, (first.clone(), second.clone(), 1));
        assert!(
            !set_task_related_link_in_transaction(&mut conn, &workspace, &first, &second, true,)
                .await
                .unwrap()
                .changed
        );
        assert!(
            set_task_related_link_in_transaction(&mut conn, &workspace, &second, &first, false,)
                .await
                .unwrap()
                .changed
        );
        let linked: i64 = sqlx::query_scalar("SELECT linked FROM task_related_links")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(linked, 0);
    }

    #[tokio::test]
    async fn rejects_self_links_with_typed_error() {
        let (_temp, mut conn) = crate::test_support::test_conn().await;
        let (workspace, first, _) = seed(&mut conn).await;
        let error =
            set_task_related_link_in_transaction(&mut conn, &workspace, &first, &first, true)
                .await
                .unwrap_err();
        let core = error.downcast_ref::<CoreError>().unwrap();
        assert_eq!(core.kind(), crate::error::ErrorKind::Validation);
        assert!(error.to_string().contains("related-self"));
    }

    #[tokio::test]
    async fn rejects_cross_workspace_links_with_typed_error() {
        let (_temp, mut conn) = crate::test_support::test_conn().await;
        let (workspace, first, _) = seed(&mut conn).await;
        let other_workspace = Workspace {
            id: "CCCC000000000003".parse().unwrap(),
            key: "other".to_string(),
            name: "Other".to_string(),
        };
        sqlx::query(
            "INSERT INTO workspaces(id, key, name, created_at, updated_at)
             VALUES (?, ?, ?, 't', 't')",
        )
        .bind(&other_workspace.id)
        .bind(&other_workspace.key)
        .bind(&other_workspace.name)
        .execute(&mut *conn)
        .await
        .unwrap();
        let project = crate::projects::create_project(&mut conn, &other_workspace, "other")
            .await
            .unwrap();
        let other_task: TaskId = "DDDD000000000004".parse().unwrap();
        sqlx::query(
            "INSERT INTO tasks(id, workspace_id, title, description, project_id, status, priority, created_at, updated_at)
             VALUES (?, ?, 'other', '', ?, 'todo', 'none', 't', 't')",
        )
        .bind(&other_task)
        .bind(&other_workspace.id)
        .bind(&project.id)
        .execute(&mut *conn)
        .await
        .unwrap();

        let error =
            set_task_related_link_in_transaction(&mut conn, &workspace, &first, &other_task, true)
                .await
                .unwrap_err();
        let core = error.downcast_ref::<CoreError>().unwrap();
        assert_eq!(core.kind(), crate::error::ErrorKind::Validation);
        assert!(error.to_string().contains("related-cross-workspace"));
    }
}
