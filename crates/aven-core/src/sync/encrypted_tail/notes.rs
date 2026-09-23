use anyhow::{Context, Result};
use serde_json::Value;
use sqlx::SqliteConnection;

use crate::change_log::op_type;
use crate::sync::wire::ChangeWire;

// The published materialization is the baseline, not a replay of prefix history.
// Tail edits supply complete bodies; only an add can recreate a deleted note.
enum NoteState {
    Baseline(Option<String>),
    Present {
        body: String,
        created_at: String,
        change_id: String,
    },
    Absent,
}

pub(super) async fn reconcile(
    conn: &mut SqliteConnection,
    prefix: i64,
    change: &ChangeWire,
) -> Result<()> {
    if !matches!(
        change.op_type.as_str(),
        op_type::NOTE_ADD | op_type::NOTE_EDIT | op_type::NOTE_DELETE
    ) {
        return Ok(());
    }
    let workspace = text(&change.payload, "workspace_id")?;
    let note = text(&change.payload, "note_id")?;
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT op_type, payload, change_id FROM changes
         WHERE entity_type = 'task' AND entity_id = ?
           AND op_type IN ('note_add', 'note_edit', 'note_delete')
           AND json_extract(payload, '$.workspace_id') = ?
           AND json_extract(payload, '$.note_id') = ?
           AND (server_seq > ? OR server_seq IS NULL)
         ORDER BY server_seq IS NULL, server_seq, local_seq, created_at, change_id",
    )
    .bind(&change.entity_id)
    .bind(workspace)
    .bind(note)
    .bind(prefix)
    .fetch_all(&mut *conn)
    .await?;
    let mut state = NoteState::Baseline(None);
    for (op, payload, change_id) in rows {
        let payload: Value = serde_json::from_str(&payload)?;
        match op.as_str() {
            op_type::NOTE_ADD if !matches!(state, NoteState::Present { .. }) => {
                state = NoteState::Present {
                    body: text(&payload, "body")?.to_owned(),
                    created_at: text(&payload, "created_at")?.to_owned(),
                    change_id,
                };
            }
            op_type::NOTE_EDIT => match &mut state {
                NoteState::Baseline(body) => *body = Some(text(&payload, "body")?.to_owned()),
                NoteState::Present { body, .. } => *body = text(&payload, "body")?.to_owned(),
                NoteState::Absent => {}
            },
            op_type::NOTE_DELETE => state = NoteState::Absent,
            _ => {}
        }
    }
    match state {
        NoteState::Baseline(Some(body)) => {
            sqlx::query(
                "UPDATE notes SET body = ? WHERE workspace_id = ? AND task_id = ? AND id = ?",
            )
            .bind(body)
            .bind(workspace)
            .bind(&change.entity_id)
            .bind(note)
            .execute(conn)
            .await?;
        }
        NoteState::Present {
            body,
            created_at,
            change_id,
        } => {
            sqlx::query(
                "INSERT INTO notes(workspace_id, task_id, id, body, created_at, change_id)
                 VALUES (?, ?, ?, ?, ?, ?)
                 ON CONFLICT(id) DO UPDATE SET
                   body = excluded.body, created_at = excluded.created_at,
                   change_id = excluded.change_id
                 WHERE notes.workspace_id = excluded.workspace_id
                   AND notes.task_id = excluded.task_id",
            )
            .bind(workspace)
            .bind(&change.entity_id)
            .bind(note)
            .bind(body)
            .bind(created_at)
            .bind(change_id)
            .execute(conn)
            .await?;
        }
        NoteState::Absent => {
            sqlx::query("DELETE FROM notes WHERE workspace_id = ? AND task_id = ? AND id = ?")
                .bind(workspace)
                .bind(&change.entity_id)
                .bind(note)
                .execute(conn)
                .await?;
        }
        NoteState::Baseline(None) => {}
    }
    Ok(())
}

fn text<'a>(payload: &'a Value, key: &str) -> Result<&'a str> {
    payload[key]
        .as_str()
        .context("error encrypted-tail-note-history")
}

#[cfg(test)]
mod tests;
