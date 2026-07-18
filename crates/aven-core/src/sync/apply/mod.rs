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

pub async fn apply_remote_change(conn: &mut SqliteConnection, change: &ChangeWire) -> Result<()> {
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::db::{conflict_exists, set_field_version};
    use crate::projects::create_project;
    use crate::test_support::test_conn;
    use crate::workspaces::Workspace;

    #[tokio::test]
    async fn creates_conflict_on_same_field_version_mismatch() {
        let (_temp, mut conn) = test_conn().await;
        let project = create_project(&mut conn, &Workspace::default(), "app")
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO tasks(id, title, description, project_id, status, priority, created_at, updated_at)
             VALUES ('7KQ9A1X4MV2P8D6R', 'local', '', ?, 'inbox', 'none', 't', 't')",
        )
        .bind(project.id)
        .execute(&mut *conn)
        .await
        .unwrap();
        set_field_version(&mut conn, "7KQ9A1X4MV2P8D6R", "title", "localchange")
            .await
            .unwrap();
        let change = ChangeWire {
            change_id: "remotechange1234".to_string(),
            client_id: "remote".to_string(),
            local_seq: 1,
            entity_type: "task".to_string(),
            entity_id: "7KQ9A1X4MV2P8D6R".to_string(),
            field: Some("title".to_string()),
            op_type: "set_field".to_string(),
            payload: json!({ "value": "remote" }),
            base_version: Some("base".to_string()),
            created_at: "t".to_string(),
            server_seq: Some(1),
        };
        task::set_field(&mut conn, &change, false).await.unwrap();
        let task_id = "7KQ9A1X4MV2P8D6R".parse().unwrap();
        assert!(
            conflict_exists(
                &mut conn,
                &crate::workspaces::default_workspace_id(),
                &task_id,
                "title"
            )
            .await
            .unwrap()
        );
    }
}
