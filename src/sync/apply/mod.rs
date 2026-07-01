mod conflict;
mod dependency;
mod epic;
mod label;
mod note;
mod payload;
mod project;
mod shared;
mod task;
mod workspace;

use anyhow::Result;
use sqlx::SqliteConnection;
use tracing::debug;

use crate::change_log::op_type;
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
        op_type::CREATE_WORKSPACE => workspace::create_workspace(conn, change).await?,
        op_type::SET_WORKSPACE_FIELD => workspace::set_workspace_field(conn, change).await?,
        op_type::CREATE_PROJECT => project::create_project(conn, change).await?,
        op_type::SET_PROJECT_METADATA => project::set_project_metadata(conn, change).await?,
        op_type::CREATE_LABEL => label::create_label(conn, change).await?,
        op_type::CREATE_TASK => task::create_task(conn, change).await?,
        op_type::SET_FIELD => task::set_field(conn, change, false).await?,
        op_type::RESOLVE_FIELD => task::set_field(conn, change, true).await?,
        op_type::LABEL_ADD => label::add_label(conn, change).await?,
        op_type::LABEL_REMOVE => label::remove_label(conn, change).await?,
        op_type::NOTE_ADD => note::add_note(conn, change).await?,
        op_type::NOTE_DELETE => note::delete_note(conn, change).await?,
        op_type::DEPENDENCY_ADD => dependency::add_dependency(conn, change).await?,
        op_type::DEPENDENCY_REMOVE => dependency::remove_dependency(conn, change).await?,
        op_type::EPIC_LINK_ADD => epic::add_epic_link(conn, change).await?,
        op_type::EPIC_LINK_REMOVE => epic::remove_epic_link(conn, change).await?,
        op_type::PROJECT_DELETE => project::delete_project(conn, change).await?,
        op_type::LABEL_DELETE => label::delete_label(conn, change).await?,
        _ => {}
    }
    Ok(())
}

pub(crate) use task::set_field as apply_remote_set_field;
