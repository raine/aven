mod conflict;
mod dependency;
mod label;
mod note;
mod project;
mod shared;
mod task;
mod workspace;

use anyhow::Result;
use sqlx::SqliteConnection;
use tracing::debug;

use crate::sync::wire::ChangeWire;

pub(super) async fn apply_remote_change(
    conn: &mut SqliteConnection,
    change: &ChangeWire,
) -> Result<()> {
    debug!(
        change_id = %change.change_id,
        op_type = %change.op_type,
        entity_type = %change.entity_type,
        entity_id = shared::safe_entity_id(change),
        field = change.field.as_deref().unwrap_or(""),
        "applying remote change"
    );
    match change.op_type.as_str() {
        "create_workspace" => workspace::create_workspace(conn, change).await?,
        "set_workspace_field" => workspace::set_workspace_field(conn, change).await?,
        "create_project" => project::create_project(conn, change).await?,
        "set_project_metadata" => project::set_project_metadata(conn, change).await?,
        "create_label" => label::create_label(conn, change).await?,
        "create_task" => task::create_task(conn, change).await?,
        "set_field" => task::set_field(conn, change, false).await?,
        "resolve_field" => task::set_field(conn, change, true).await?,
        "label_add" => label::add_label(conn, change).await?,
        "label_remove" => label::remove_label(conn, change).await?,
        "note_add" => note::add_note(conn, change).await?,
        "note_delete" => note::delete_note(conn, change).await?,
        "dependency_add" => dependency::add_dependency(conn, change).await?,
        "dependency_remove" => dependency::remove_dependency(conn, change).await?,
        "project_delete" => project::delete_project(conn, change).await?,
        "label_delete" => label::delete_label(conn, change).await?,
        _ => {}
    }
    Ok(())
}

pub(crate) use task::set_field as apply_remote_set_field;
