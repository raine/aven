use super::{NoteDeleteOutcome, NoteEditOutcome, NoteOutcome};
use crate::change_log::{ChangeEntity, ChangePayload, append_change, op_type};
use crate::db::{Database, begin_immediate};
use crate::ids::{TaskId, new_id, now};
use crate::operations::{RecurrenceStructuralMutation, RecurrenceTaskMutation};
use crate::undo::{UndoCommand, UndoPayload, record_tui_undo};
use crate::workspaces::Workspace;
use anyhow::Result;
use sqlx::SqliteConnection;
use tracing::info;

impl Database {
    pub async fn add_note(
        &self,
        workspace: &Workspace,
        task_id: &TaskId,
        body: String,
    ) -> Result<NoteOutcome> {
        let mut conn = self.acquire_writer().await?;
        add_note_operation(&mut conn, workspace, task_id, body, false).await
    }

    pub async fn add_note_with_tui_undo(
        &self,
        workspace: &Workspace,
        task_id: &TaskId,
        body: String,
    ) -> Result<NoteOutcome> {
        let mut conn = self.acquire_writer().await?;
        add_note_operation(&mut conn, workspace, task_id, body, true).await
    }

    pub async fn edit_note(
        &self,
        workspace: &Workspace,
        task_id: &TaskId,
        note_id: &str,
        body: String,
    ) -> Result<NoteEditOutcome> {
        let mut conn = self.acquire_writer().await?;
        edit_note_operation(&mut conn, workspace, task_id, note_id, body, false).await
    }

    pub async fn edit_note_with_tui_undo(
        &self,
        workspace: &Workspace,
        task_id: &TaskId,
        note_id: &str,
        body: String,
    ) -> Result<NoteEditOutcome> {
        let mut conn = self.acquire_writer().await?;
        edit_note_operation(&mut conn, workspace, task_id, note_id, body, true).await
    }

    pub async fn delete_note(
        &self,
        workspace: &Workspace,
        task_id: &TaskId,
        note_id: &str,
    ) -> Result<NoteDeleteOutcome> {
        let mut conn = self.acquire_writer().await?;
        delete_note_operation(&mut conn, workspace, task_id, note_id, false).await
    }

    pub async fn delete_note_with_tui_undo(
        &self,
        workspace: &Workspace,
        task_id: &TaskId,
        note_id: &str,
    ) -> Result<NoteDeleteOutcome> {
        let mut conn = self.acquire_writer().await?;
        delete_note_operation(&mut conn, workspace, task_id, note_id, true).await
    }
}

pub(in crate::operations) async fn add_note_operation(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &crate::ids::TaskId,
    body: String,
    tui_undo: bool,
) -> Result<NoteOutcome> {
    let mut tx = begin_immediate(conn).await?;
    let outcome = add_note_in_transaction(&mut tx, workspace, task_id, body).await?;
    if tui_undo {
        record_tui_undo(
            &mut tx,
            &workspace.id,
            UndoPayload {
                commands: vec![UndoCommand::DeleteCreatedNote {
                    task_id: task_id.clone(),
                    note_id: outcome.note_id.clone(),
                    note_add_change_id: outcome.change_id.clone(),
                    restoration_change_ids: Vec::new(),
                }],
            },
        )
        .await?;
    }
    tx.commit().await?;
    info!(task_id = %task_id, note_id = %outcome.note_id, "note added");
    Ok(outcome)
}

pub(super) async fn add_note_in_transaction(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &TaskId,
    body: String,
) -> Result<NoteOutcome> {
    let ts = now();
    let note_id = new_id();
    crate::operations::route_recurrence_task_mutation(
        conn,
        workspace,
        task_id,
        RecurrenceTaskMutation::Structural(RecurrenceStructuralMutation::Notes),
        &ts,
    )
    .await?;
    let change_id = append_change(
        conn,
        ChangeEntity::Task,
        task_id,
        Some("notes"),
        op_type::NOTE_ADD,
        ChangePayload::workspace(workspace)
            .set("note_id", &note_id)
            .set("body", &body)
            .set("created_at", &ts),
    )
    .await?;
    sqlx::query(
        "INSERT INTO notes(workspace_id, id, task_id, body, created_at, change_id) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&workspace.id)
    .bind(&note_id)
    .bind(task_id)
    .bind(&body)
    .bind(&ts)
    .bind(&change_id)
    .execute(&mut *conn)
    .await?;
    sqlx::query("UPDATE tasks SET queue_activity_at = ? WHERE workspace_id = ? AND id = ?")
        .bind(&ts)
        .bind(&workspace.id)
        .bind(task_id)
        .execute(&mut *conn)
        .await?;
    Ok(NoteOutcome {
        task_id: task_id.clone(),
        note_id,
        change_id,
    })
}

async fn edit_note_operation(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &crate::ids::TaskId,
    note_id: &str,
    body: String,
    tui_undo: bool,
) -> Result<NoteEditOutcome> {
    let edited_at = now();
    let mut tx = begin_immediate(conn).await?;
    crate::operations::route_recurrence_task_mutation(
        &mut tx,
        workspace,
        task_id,
        RecurrenceTaskMutation::Structural(RecurrenceStructuralMutation::Notes),
        &edited_at,
    )
    .await?;
    let before = sqlx::query_scalar::<_, String>(
        "SELECT body FROM notes WHERE workspace_id = ? AND task_id = ? AND id = ?",
    )
    .bind(&workspace.id)
    .bind(task_id)
    .bind(note_id)
    .fetch_optional(&mut *tx)
    .await?;
    let found = before.is_some();
    let changed = before.as_ref().is_some_and(|before| before != &body);
    if changed {
        sqlx::query("UPDATE notes SET body = ? WHERE workspace_id = ? AND task_id = ? AND id = ?")
            .bind(&body)
            .bind(&workspace.id)
            .bind(task_id)
            .bind(note_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE tasks SET queue_activity_at = ? WHERE workspace_id = ? AND id = ?")
            .bind(&edited_at)
            .bind(&workspace.id)
            .bind(task_id)
            .execute(&mut *tx)
            .await?;
        append_change(
            &mut tx,
            ChangeEntity::Task,
            task_id,
            Some("notes"),
            op_type::NOTE_EDIT,
            ChangePayload::workspace(workspace)
                .set("note_id", note_id)
                .set("body", &body)
                .set("edited_at", edited_at),
        )
        .await?;
        if tui_undo {
            record_tui_undo(
                &mut tx,
                &workspace.id,
                UndoPayload {
                    commands: vec![UndoCommand::SetNoteBody {
                        task_id: task_id.clone(),
                        note_id: note_id.to_string(),
                        before: before.expect("changed note has before body"),
                        after: body,
                    }],
                },
            )
            .await?;
        }
    }
    tx.commit().await?;
    if changed {
        info!(task_id = %task_id, note_id = %note_id, "note edited");
    }
    Ok(NoteEditOutcome {
        task_id: task_id.clone(),
        note_id: note_id.to_string(),
        found,
        changed,
    })
}

async fn delete_note_operation(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_id: &crate::ids::TaskId,
    note_id: &str,
    tui_undo: bool,
) -> Result<NoteDeleteOutcome> {
    let deleted_at = now();
    let mut tx = begin_immediate(conn).await?;
    crate::operations::route_recurrence_task_mutation(
        &mut tx,
        workspace,
        task_id,
        RecurrenceTaskMutation::Structural(RecurrenceStructuralMutation::Notes),
        &deleted_at,
    )
    .await?;
    let before = sqlx::query_as::<_, (String, String, String)>(
        "SELECT body, created_at, change_id FROM notes WHERE workspace_id = ? AND task_id = ? AND id = ?",
    )
    .bind(&workspace.id)
    .bind(task_id)
    .bind(note_id)
    .fetch_optional(&mut *tx)
    .await?;
    let deleted =
        sqlx::query("DELETE FROM notes WHERE workspace_id = ? AND task_id = ? AND id = ?")
            .bind(&workspace.id)
            .bind(task_id)
            .bind(note_id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
    if deleted > 0 {
        sqlx::query("UPDATE tasks SET queue_activity_at = ? WHERE workspace_id = ? AND id = ?")
            .bind(&deleted_at)
            .bind(&workspace.id)
            .bind(task_id)
            .execute(&mut *tx)
            .await?;
        append_change(
            &mut tx,
            ChangeEntity::Task,
            task_id,
            Some("notes"),
            op_type::NOTE_DELETE,
            ChangePayload::workspace(workspace)
                .set("note_id", note_id)
                .set(
                    "body",
                    &before.as_ref().expect("deleted note has before state").0,
                )
                .set("deleted_at", deleted_at),
        )
        .await?;
        if tui_undo {
            let (body, created_at, note_add_change_id) =
                before.expect("deleted note has before state");
            record_tui_undo(
                &mut tx,
                &workspace.id,
                UndoPayload {
                    commands: vec![UndoCommand::RestoreDeletedNote {
                        task_id: task_id.clone(),
                        note_id: note_id.to_string(),
                        body,
                        created_at,
                        note_add_change_id,
                    }],
                },
            )
            .await?;
        }
    }
    tx.commit().await?;
    if deleted > 0 {
        info!(task_id = %task_id, note_id = %note_id, "note deleted");
    }
    Ok(NoteDeleteOutcome {
        task_id: task_id.clone(),
        note_id: note_id.to_string(),
        changed: deleted > 0,
    })
}
