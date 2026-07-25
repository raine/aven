use crate::ids::WorkspaceId;
use std::collections::HashSet;

use anyhow::{Result, bail};
use sqlx::SqliteConnection;

use crate::change_log::{ChangeEntity, ChangePayload, append_change, op_type};
use crate::db::{Database, begin_immediate};
use crate::ids::{TaskId, now};
use crate::refs::get_task_in_workspace;
use crate::undo::{UndoCommand, UndoContext, UndoPayload, record_tui_undo};
use crate::workspaces::Workspace;

impl Database {
    pub async fn add_task_dependency(
        &self,
        workspace: &Workspace,
        task_id: &TaskId,
        depends_on_id: &TaskId,
    ) -> Result<DependencyOutcome> {
        self.add_task_dependency_with_undo(workspace, task_id, depends_on_id, UndoContext::None)
            .await
    }

    pub async fn add_task_dependency_with_undo(
        &self,
        workspace: &Workspace,
        task_id: &TaskId,
        depends_on_id: &TaskId,
        undo: UndoContext,
    ) -> Result<DependencyOutcome> {
        let mut conn = self.acquire().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let outcome =
            add_task_dependency_in_transaction(&mut tx, workspace, task_id, depends_on_id).await?;
        if outcome.changed
            && let UndoContext::Tui { summary } = undo
        {
            record_tui_undo(
                &mut tx,
                &workspace.id,
                &summary,
                UndoPayload {
                    commands: vec![UndoCommand::AddTaskDependency {
                        task_id: outcome.task.id.clone(),
                        depends_on_task_id: outcome.depends_on.id.clone(),
                    }],
                },
            )
            .await?;
        }
        tx.commit().await?;
        Ok(outcome)
    }

    pub async fn remove_task_dependency(
        &self,
        workspace: &Workspace,
        task_id: &TaskId,
        depends_on_id: &TaskId,
    ) -> Result<DependencyOutcome> {
        self.remove_task_dependency_with_undo(workspace, task_id, depends_on_id, UndoContext::None)
            .await
    }

    pub async fn remove_task_dependency_with_undo(
        &self,
        workspace: &Workspace,
        task_id: &TaskId,
        depends_on_id: &TaskId,
        undo: UndoContext,
    ) -> Result<DependencyOutcome> {
        let mut conn = self.acquire().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let outcome =
            remove_task_dependency_in_transaction(&mut tx, workspace, task_id, depends_on_id)
                .await?;
        if outcome.changed
            && let UndoContext::Tui { summary } = undo
        {
            record_tui_undo(
                &mut tx,
                &workspace.id,
                &summary,
                UndoPayload {
                    commands: vec![UndoCommand::RemoveTaskDependency {
                        task_id: outcome.task.id.clone(),
                        depends_on_task_id: outcome.depends_on.id.clone(),
                    }],
                },
            )
            .await?;
        }
        tx.commit().await?;
        Ok(outcome)
    }
}

pub struct DependencyOutcome {
    pub task: crate::types::Task,
    pub depends_on: crate::types::Task,
    pub changed: bool,
}

struct DependencyPair {
    task: crate::types::Task,
    depends_on: crate::types::Task,
}

async fn load_dependency_pair(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &crate::ids::TaskId,
    depends_on_id: &crate::ids::TaskId,
) -> Result<DependencyPair> {
    if task_id == depends_on_id {
        bail!("error dependency-self task_id={task_id}");
    }

    let task = get_task_in_workspace(conn, workspace, task_id).await?;
    let depends_on = get_task_in_workspace(conn, workspace, depends_on_id).await?;

    Ok(DependencyPair { task, depends_on })
}

async fn record_dependency_change(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    pair: &DependencyPair,
    op_type: &'static str,
) -> Result<()> {
    append_change(
        conn,
        ChangeEntity::Task,
        &pair.task.id,
        Some("dependencies"),
        op_type,
        ChangePayload::workspace(workspace).set("depends_on_task_id", pair.depends_on.id.clone()),
    )
    .await?;
    Ok(())
}

pub async fn add_task_dependency(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &crate::ids::TaskId,
    depends_on_id: &crate::ids::TaskId,
) -> Result<DependencyOutcome> {
    let mut tx = begin_immediate(conn).await?;
    let outcome =
        add_task_dependency_in_transaction(&mut tx, workspace, task_id, depends_on_id).await?;
    tx.commit().await?;
    Ok(outcome)
}

async fn add_task_dependency_in_transaction(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &crate::ids::TaskId,
    depends_on_id: &crate::ids::TaskId,
) -> Result<DependencyOutcome> {
    let pair = load_dependency_pair(conn, workspace, task_id, depends_on_id).await?;

    if dependency_path_exists(
        conn,
        &pair.task.workspace_id,
        &pair.depends_on.id,
        &pair.task.id,
    )
    .await?
    {
        bail!("error dependency-cycle task_id={task_id} depends_on_task_id={depends_on_id}");
    }

    let created_at = now();
    let changed = sqlx::query(
        "INSERT OR IGNORE INTO task_dependencies(workspace_id, task_id, depends_on_task_id, created_at)
         VALUES (?, ?, ?, ?)",
    )
    .bind(&pair.task.workspace_id)
    .bind(&pair.task.id)
    .bind(&pair.depends_on.id)
    .bind(&created_at)
    .execute(&mut *conn)
    .await?
    .rows_affected()
        > 0;

    if changed {
        record_dependency_change(conn, workspace, &pair, op_type::DEPENDENCY_ADD).await?;
    }

    Ok(DependencyOutcome {
        task: pair.task,
        depends_on: pair.depends_on,
        changed,
    })
}

async fn remove_task_dependency_in_transaction(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &crate::ids::TaskId,
    depends_on_id: &crate::ids::TaskId,
) -> Result<DependencyOutcome> {
    let pair = load_dependency_pair(conn, workspace, task_id, depends_on_id).await?;

    let changed = sqlx::query(
        "DELETE FROM task_dependencies
         WHERE workspace_id = ? AND task_id = ? AND depends_on_task_id = ?",
    )
    .bind(&pair.task.workspace_id)
    .bind(&pair.task.id)
    .bind(&pair.depends_on.id)
    .execute(&mut *conn)
    .await?
    .rows_affected()
        > 0;

    if changed {
        record_dependency_change(conn, workspace, &pair, op_type::DEPENDENCY_REMOVE).await?;
    }

    Ok(DependencyOutcome {
        task: pair.task,
        depends_on: pair.depends_on,
        changed,
    })
}

pub async fn dependency_path_exists(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    from_task_id: &crate::ids::TaskId,
    to_task_id: &crate::ids::TaskId,
) -> Result<bool> {
    let mut visited = HashSet::new();
    let mut stack = vec![from_task_id.clone()];
    while let Some(current) = stack.pop() {
        if !visited.insert(current.clone()) {
            continue;
        }
        if &current == to_task_id {
            return Ok(true);
        }
        let next = sqlx::query_scalar::<_, crate::ids::TaskId>(
            "SELECT depends_on_task_id
             FROM task_dependencies
             WHERE workspace_id = ? AND task_id = ?",
        )
        .bind(workspace_id)
        .bind(&current)
        .fetch_all(&mut *conn)
        .await?;
        stack.extend(next);
    }
    Ok(false)
}
