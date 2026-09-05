use crate::ids::WorkspaceId;
use anyhow::{Result, bail};
use sqlx::SqliteConnection;

use crate::change_log::{ChangeEntity, ChangePayload, append_change, op_type};
use crate::db::{Database, begin_immediate, field_version, insert_change, set_field_version};
use crate::ids::{TaskId, now};
use crate::refs::get_task_in_workspace;
use crate::task_fields::TaskField;
use crate::types::Task;
use crate::undo::{UndoCommand, UndoContext, UndoPayload, record_tui_undo};
use crate::workspaces::Workspace;

impl Database {
    pub async fn add_task_to_epic(
        &self,
        workspace: &Workspace,
        child_id: &TaskId,
        epic_id: &TaskId,
    ) -> Result<EpicLinkOutcome> {
        self.add_task_to_epic_with_undo(workspace, child_id, epic_id, UndoContext::None)
            .await
    }

    pub async fn add_task_to_epic_with_undo(
        &self,
        workspace: &Workspace,
        child_id: &TaskId,
        epic_id: &TaskId,
        undo: UndoContext,
    ) -> Result<EpicLinkOutcome> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let outcome =
            add_task_to_epic_in_transaction(&mut tx, workspace, child_id, epic_id).await?;
        if outcome.changed
            && let UndoContext::Tui { summary } = undo
        {
            let mut commands = vec![UndoCommand::AddEpicChild {
                epic_id: outcome.epic.id.clone(),
                child_id: outcome.child.id.clone(),
            }];
            if outcome.promoted {
                commands.push(UndoCommand::SetTaskField {
                    task_id: outcome.epic.id.clone(),
                    field: TaskField::IsEpic.as_str().to_string(),
                    before: "0".to_string(),
                    after: "1".to_string(),
                    queue_activity_before: None,
                    queue_activity_after: None,
                });
            }
            record_tui_undo(&mut tx, &workspace.id, &summary, UndoPayload { commands }).await?;
        }
        tx.commit().await?;
        Ok(outcome)
    }

    pub async fn remove_task_from_epic(
        &self,
        workspace: &Workspace,
        child_id: &TaskId,
        epic_id: &TaskId,
    ) -> Result<EpicLinkOutcome> {
        self.remove_task_from_epic_with_undo(workspace, child_id, epic_id, UndoContext::None)
            .await
    }

    pub async fn remove_task_from_epic_with_undo(
        &self,
        workspace: &Workspace,
        child_id: &TaskId,
        epic_id: &TaskId,
        undo: UndoContext,
    ) -> Result<EpicLinkOutcome> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let outcome =
            remove_task_from_epic_in_transaction(&mut tx, workspace, child_id, epic_id).await?;
        if outcome.changed
            && let UndoContext::Tui { summary } = undo
        {
            record_tui_undo(
                &mut tx,
                &workspace.id,
                &summary,
                UndoPayload {
                    commands: vec![UndoCommand::RemoveEpicChild {
                        epic_id: outcome.epic.id.clone(),
                        child_id: outcome.child.id.clone(),
                    }],
                },
            )
            .await?;
        }
        tx.commit().await?;
        Ok(outcome)
    }
}

pub struct EpicLinkOutcome {
    pub epic: Task,
    pub child: Task,
    pub changed: bool,
    pub promoted: bool,
}

struct EpicPair {
    epic: Task,
    child: Task,
}

async fn load_epic_pair(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    child_id: &crate::ids::TaskId,
    epic_id: &crate::ids::TaskId,
) -> Result<EpicPair> {
    if child_id == epic_id {
        bail!("error epic-self task_id={child_id}");
    }

    let child = get_task_in_workspace(conn, workspace, child_id).await?;
    let epic = get_task_in_workspace(conn, workspace, epic_id).await?;
    Ok(EpicPair { epic, child })
}

async fn insert_epic_link_if_absent(
    conn: &mut SqliteConnection,
    pair: &EpicPair,
    created_at: &str,
) -> Result<bool> {
    let existing_epic_id = sqlx::query_scalar::<_, crate::ids::TaskId>(
        "SELECT epic_task_id FROM task_epic_links WHERE workspace_id = ? AND child_task_id = ?",
    )
    .bind(&pair.child.workspace_id)
    .bind(&pair.child.id)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(existing_epic_id) = existing_epic_id
        && existing_epic_id != pair.epic.id
    {
        bail!(
            "error epic-child-already-linked child_task_id={} epic_task_id={}",
            pair.child.id,
            existing_epic_id
        );
    }

    Ok(sqlx::query(
        "INSERT OR IGNORE INTO task_epic_links(workspace_id, epic_task_id, child_task_id, created_at)
         VALUES (?, ?, ?, ?)",
    )
    .bind(&pair.child.workspace_id)
    .bind(&pair.epic.id)
    .bind(&pair.child.id)
    .bind(created_at)
    .execute(&mut *conn)
    .await?
    .rows_affected()
        > 0)
}

async fn record_epic_change(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    pair: &EpicPair,
    op_type: &'static str,
) -> Result<()> {
    append_change(
        conn,
        ChangeEntity::Task,
        &pair.child.id,
        Some("epics"),
        op_type,
        ChangePayload::workspace(workspace)
            .set("epic_task_id", pair.epic.id.clone())
            .set("created_at", now()),
    )
    .await?;
    Ok(())
}

async fn mark_task_as_epic(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task: &Task,
) -> Result<()> {
    if task.is_epic {
        return Ok(());
    }
    let field = TaskField::IsEpic.as_str();
    let base = field_version(conn, &task.id, field).await?;
    let ts = now();
    sqlx::query("UPDATE tasks SET is_epic = 1, updated_at = ? WHERE workspace_id = ? AND id = ?")
        .bind(&ts)
        .bind(&task.workspace_id)
        .bind(&task.id)
        .execute(&mut *conn)
        .await?;
    let change_id = insert_change(
        conn,
        ChangeEntity::Task.as_str(),
        &task.id,
        Some(field),
        op_type::SET_FIELD,
        TaskField::IsEpic.scalar_payload(&workspace.id, &workspace.key, "1")?,
        base.as_deref(),
    )
    .await?;
    set_field_version(conn, &task.id, field, &change_id).await?;
    Ok(())
}

#[cfg(any(test, feature = "test-support"))]
pub async fn add_task_to_epic(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    child_id: &crate::ids::TaskId,
    epic_id: &crate::ids::TaskId,
) -> Result<EpicLinkOutcome> {
    let mut tx = begin_immediate(conn).await?;
    let outcome = add_task_to_epic_in_transaction(&mut tx, workspace, child_id, epic_id).await?;
    tx.commit().await?;
    Ok(outcome)
}

pub(crate) async fn add_task_to_epic_in_transaction(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    child_id: &crate::ids::TaskId,
    epic_id: &crate::ids::TaskId,
) -> Result<EpicLinkOutcome> {
    crate::operations::route_recurrence_task_field(
        conn,
        workspace,
        child_id,
        "epic_membership",
        "",
    )
    .await?;
    crate::operations::route_recurrence_task_field(conn, workspace, epic_id, "epic_membership", "")
        .await?;
    let pair = load_epic_pair(conn, workspace, child_id, epic_id).await?;
    if pair.child.project_id != pair.epic.project_id {
        bail!("error epic-cross-project child_task_id={child_id} epic_task_id={epic_id}");
    }
    if pair.child.is_epic {
        bail!("error epic-child-is-epic child_task_id={child_id}");
    }
    if pair.child.deleted {
        bail!("error epic-child-deleted child_task_id={child_id}");
    }
    if pair.epic.deleted {
        bail!("error epic-parent-deleted epic_task_id={epic_id}");
    }
    let ts = now();
    let promoted = !pair.epic.is_epic;
    if promoted {
        mark_task_as_epic(conn, workspace, &pair.epic).await?;
    }
    let changed = insert_epic_link_if_absent(conn, &pair, &ts).await?;

    if changed {
        record_epic_change(conn, workspace, &pair, op_type::EPIC_LINK_ADD).await?;
    }

    Ok(EpicLinkOutcome {
        epic: get_task_in_workspace(conn, workspace, &pair.epic.id).await?,
        child: pair.child,
        changed,
        promoted,
    })
}

pub(crate) async fn restore_task_to_epic_in_transaction(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    child_id: &crate::ids::TaskId,
    epic_id: &crate::ids::TaskId,
) -> Result<EpicLinkOutcome> {
    let pair = load_epic_pair(conn, workspace, child_id, epic_id).await?;
    let ts = now();
    let changed = insert_epic_link_if_absent(conn, &pair, &ts).await?;
    if changed {
        record_epic_change(conn, workspace, &pair, op_type::EPIC_LINK_ADD).await?;
    }
    Ok(EpicLinkOutcome {
        epic: pair.epic,
        child: pair.child,
        changed,
        promoted: false,
    })
}

pub(crate) async fn remove_task_from_epic_in_transaction(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    child_id: &crate::ids::TaskId,
    epic_id: &crate::ids::TaskId,
) -> Result<EpicLinkOutcome> {
    crate::operations::route_recurrence_task_field(
        conn,
        workspace,
        child_id,
        "epic_membership",
        "",
    )
    .await?;
    crate::operations::route_recurrence_task_field(conn, workspace, epic_id, "epic_membership", "")
        .await?;
    let pair = load_epic_pair(conn, workspace, child_id, epic_id).await?;
    let changed = sqlx::query(
        "DELETE FROM task_epic_links
         WHERE workspace_id = ? AND epic_task_id = ? AND child_task_id = ?",
    )
    .bind(&pair.child.workspace_id)
    .bind(&pair.epic.id)
    .bind(&pair.child.id)
    .execute(&mut *conn)
    .await?
    .rows_affected()
        > 0;

    if changed {
        record_epic_change(conn, workspace, &pair, op_type::EPIC_LINK_REMOVE).await?;
    }

    Ok(EpicLinkOutcome {
        epic: pair.epic,
        child: pair.child,
        changed,
        promoted: false,
    })
}

pub async fn task_has_epic_children(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_id: &crate::ids::TaskId,
) -> Result<bool> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM task_epic_links WHERE workspace_id = ? AND epic_task_id = ? LIMIT 1",
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_one(&mut *conn)
    .await?
        > 0)
}
