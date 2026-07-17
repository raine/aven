use anyhow::Result;
use sqlx::SqliteConnection;

use crate::cli::{NoteArgs, NoteDeleteArgs};
use crate::input::read_required_text;
use crate::operations::{add_note, delete_note};
use crate::refs::{DisplayRefContext, resolve_task_ref_in_workspace};
use crate::render::changed_text;
use crate::workspaces::Workspace;

pub(crate) async fn cmd_note(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    args: NoteArgs,
) -> Result<()> {
    let task = resolve_task_ref_in_workspace(conn, workspace, &args.task_ref).await?;
    let body = read_required_text(args.text, args.file.as_deref(), args.stdin, "note")?;
    let outcome = add_note(conn, workspace, &task.id, body).await?;
    let display_refs = DisplayRefContext::for_workspace(conn, &workspace.id).await?;
    println!(
        "noted {} note={}",
        display_refs.display_ref(&task),
        outcome.note_id
    );
    Ok(())
}

pub(crate) async fn cmd_note_delete(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    args: NoteDeleteArgs,
) -> Result<()> {
    let task = resolve_task_ref_in_workspace(conn, workspace, &args.task_ref).await?;
    let outcome = delete_note(conn, workspace, &task.id, &args.note_id).await?;
    let display_refs = DisplayRefContext::for_workspace(conn, &workspace.id).await?;
    println!(
        "deleted-note {} note={} changed={}",
        display_refs.display_ref(&task),
        outcome.note_id,
        changed_text(outcome.changed),
    );
    Ok(())
}
