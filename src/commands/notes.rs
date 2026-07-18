use anyhow::Result;
use aven_core::db::Database;

use crate::cli::{NoteArgs, NoteDeleteArgs};
use crate::input::read_required_text;
use crate::render::changed_text;
use crate::workspaces::Workspace;

pub(crate) async fn cmd_note(
    database: &Database,
    workspace: &Workspace,
    args: NoteArgs,
) -> Result<()> {
    let task = database.resolve_task_ref(workspace, &args.task_ref).await?;
    let body = read_required_text(args.text, args.file.as_deref(), args.stdin, "note")?;
    let outcome = database.add_note(workspace, &task.id, body).await?;
    let display_refs = database.display_ref_context(&workspace.id).await?;
    println!(
        "noted {} note={}",
        display_refs.display_ref(&task),
        outcome.note_id
    );
    Ok(())
}

pub(crate) async fn cmd_note_delete(
    database: &Database,
    workspace: &Workspace,
    args: NoteDeleteArgs,
) -> Result<()> {
    let task = database.resolve_task_ref(workspace, &args.task_ref).await?;
    let outcome = database
        .delete_note(workspace, &task.id, &args.note_id)
        .await?;
    let display_refs = database.display_ref_context(&workspace.id).await?;
    println!(
        "deleted-note {} note={} changed={}",
        display_refs.display_ref(&task),
        outcome.note_id,
        changed_text(outcome.changed),
    );
    Ok(())
}
