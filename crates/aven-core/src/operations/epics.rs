use crate::ids::WorkspaceId;
use anyhow::{Result, bail};
use sqlx::SqliteConnection;

use crate::change_log::{ChangeEntity, ChangePayload, append_change, op_type};
use crate::db::{Database, begin_immediate, field_version, insert_change, set_field_version};
use crate::ids::{TaskId, now};
use crate::refs::get_task_in_workspace;
use crate::task_fields::TaskField;
use crate::types::Task;
use crate::workspaces::Workspace;

impl Database {
    pub async fn add_task_to_epic(
        &self,
        workspace: &Workspace,
        child_id: &TaskId,
        epic_id: &TaskId,
    ) -> Result<EpicLinkOutcome> {
        let mut conn = self.acquire().await?;
        add_task_to_epic(&mut conn, workspace, child_id, epic_id).await
    }

    pub async fn remove_task_from_epic(
        &self,
        workspace: &Workspace,
        child_id: &TaskId,
        epic_id: &TaskId,
    ) -> Result<EpicLinkOutcome> {
        let mut conn = self.acquire().await?;
        remove_task_from_epic(&mut conn, workspace, child_id, epic_id).await
    }
}

pub struct EpicLinkOutcome {
    pub epic: Task,
    pub child: Task,
    pub changed: bool,
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
    if child.project_id != epic.project_id {
        bail!("error epic-cross-project child_task_id={child_id} epic_task_id={epic_id}");
    }
    if child.is_epic {
        bail!("error epic-child-is-epic child_task_id={child_id}");
    }

    Ok(EpicPair { epic, child })
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

pub async fn add_task_to_epic(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    child_id: &crate::ids::TaskId,
    epic_id: &crate::ids::TaskId,
) -> Result<EpicLinkOutcome> {
    let mut tx = begin_immediate(conn).await?;
    let pair = load_epic_pair(&mut tx, workspace, child_id, epic_id).await?;
    let ts = now();
    if !pair.epic.is_epic {
        mark_task_as_epic(&mut tx, workspace, &pair.epic).await?;
    }
    let existing_epic_id = sqlx::query_scalar::<_, crate::ids::TaskId>(
        "SELECT epic_task_id FROM task_epic_links WHERE workspace_id = ? AND child_task_id = ?",
    )
    .bind(&pair.child.workspace_id)
    .bind(&pair.child.id)
    .fetch_optional(&mut *tx)
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
    let changed = sqlx::query(
        "INSERT OR IGNORE INTO task_epic_links(workspace_id, epic_task_id, child_task_id, created_at)
         VALUES (?, ?, ?, ?)",
    )
    .bind(&pair.child.workspace_id)
    .bind(&pair.epic.id)
    .bind(&pair.child.id)
    .bind(&ts)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        > 0;

    if changed {
        record_epic_change(&mut tx, workspace, &pair, op_type::EPIC_LINK_ADD).await?;
    }

    tx.commit().await?;
    Ok(EpicLinkOutcome {
        epic: get_task_in_workspace(conn, workspace, &pair.epic.id).await?,
        child: pair.child,
        changed,
    })
}

pub async fn remove_task_from_epic(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    child_id: &crate::ids::TaskId,
    epic_id: &crate::ids::TaskId,
) -> Result<EpicLinkOutcome> {
    let mut tx = begin_immediate(conn).await?;
    let pair = load_epic_pair(&mut tx, workspace, child_id, epic_id).await?;
    let changed = sqlx::query(
        "DELETE FROM task_epic_links
         WHERE workspace_id = ? AND epic_task_id = ? AND child_task_id = ?",
    )
    .bind(&pair.child.workspace_id)
    .bind(&pair.epic.id)
    .bind(&pair.child.id)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        > 0;

    if changed {
        record_epic_change(&mut tx, workspace, &pair, op_type::EPIC_LINK_REMOVE).await?;
    }

    tx.commit().await?;
    Ok(EpicLinkOutcome {
        epic: pair.epic,
        child: pair.child,
        changed,
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
