mod attachment;
mod conflict;
mod dependency;
mod epic;
mod label;
mod metadata;
mod note;
mod payload;
mod project;
mod recurrence;
mod shared;
mod task;
mod workspace;

use anyhow::{Result, bail};
use sqlx::SqliteConnection;
use tracing::debug;

use crate::change_log::op_type;
use crate::sync::wire::{AttachmentAddPayload, AttachmentDeletePayload, ChangeWire};

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
        op_type::CREATE_METADATA_FIELD => metadata::create_field(conn, change).await?,
        op_type::SET_METADATA_FIELD => metadata::set_field(conn, change).await?,
        op_type::SET_TASK_METADATA => metadata::set_task_value(conn, change).await?,
        op_type::REMOVE_TASK_METADATA => metadata::remove_task_value(conn, change).await?,
        op_type::SET_RECURRENCE_METADATA => metadata::set_recurrence_value(conn, change).await?,
        op_type::REMOVE_RECURRENCE_METADATA => {
            metadata::remove_recurrence_value(conn, change).await?
        }
        op_type::SET_LABEL_NAME => label::set_label_name(conn, change).await?,
        op_type::LABEL_RESTORE => label::restore_label(conn, change).await?,
        op_type::CREATE_TASK => task::create_task(conn, change).await?,
        op_type::SET_FIELD => task::set_field(conn, change, false).await?,
        op_type::RESOLVE_FIELD => task::set_field(conn, change, true).await?,
        op_type::LABEL_ADD => label::add_label(conn, change).await?,
        op_type::LABEL_REMOVE => label::remove_label(conn, change).await?,
        op_type::NOTE_ADD => note::add_note(conn, change).await?,
        op_type::NOTE_EDIT => note::edit_note(conn, change).await?,
        op_type::NOTE_DELETE => note::delete_note(conn, change).await?,
        op_type::DEPENDENCY_ADD => dependency::add_dependency(conn, change).await?,
        op_type::DEPENDENCY_REMOVE => dependency::remove_dependency(conn, change).await?,
        op_type::EPIC_LINK_ADD => epic::add_epic_link(conn, change).await?,
        op_type::EPIC_LINK_REMOVE => epic::remove_epic_link(conn, change).await?,
        op_type::PROJECT_DELETE => project::delete_project(conn, change).await?,
        op_type::LABEL_DELETE => label::delete_label(conn, change).await?,
        op_type::ATTACHMENT_ADD => {
            let payload = AttachmentAddPayload::from_change(change)?;
            attachment::add_attachment(conn, change, &payload).await?
        }
        op_type::ATTACHMENT_DELETE => {
            let payload = AttachmentDeletePayload::from_change(change)?;
            attachment::delete_attachment(conn, change, &payload).await?
        }
        op_type::CREATE_RECURRENCE_SERIES => recurrence::create_series(conn, change).await?,
        op_type::UPDATE_RECURRENCE_TEMPLATE => recurrence::update_template(conn, change).await?,
        op_type::PROJECT_RECURRENCE_OCCURRENCE => {
            recurrence::project_occurrence(conn, change).await?
        }
        op_type::RESOLVE_RECURRENCE_OCCURRENCE => {
            recurrence::resolve_occurrence(conn, change).await?
        }
        op_type::SET_RECURRENCE_STATE | op_type::STOP_RECURRENCE_SERIES => {
            recurrence::set_state(conn, change).await?
        }
        op_type::OPEN_RECURRENCE_PAUSE => recurrence::open_pause(conn, change).await?,
        op_type::CLOSE_RECURRENCE_PAUSE => recurrence::close_pause(conn, change).await?,
        _ => bail!("error unsupported-remote-change op_type={}", change.op_type),
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

    async fn seed_attachment_task(conn: &mut SqliteConnection) -> Workspace {
        let workspace = Workspace::default();
        let project = create_project(conn, &workspace, "app").await.unwrap();
        sqlx::query(
            "INSERT INTO tasks(id, workspace_id, title, description, project_id, status, priority, created_at, updated_at)
             VALUES ('BBBBBBBBBBBBBBBB', ?, 'task', '', ?, 'inbox', 'none', 't', 't')",
        )
        .bind(&workspace.id)
        .bind(project.id)
        .execute(&mut *conn)
        .await
        .unwrap();
        workspace
    }

    fn attachment_add_change(workspace: &Workspace) -> ChangeWire {
        ChangeWire {
            change_id: "AAAAAAAAAAAAAAA0".to_string(),
            client_id: "remote".to_string(),
            local_seq: 1,
            entity_type: "task".to_string(),
            entity_id: "BBBBBBBBBBBBBBBB".to_string(),
            field: Some("attachments".to_string()),
            op_type: op_type::ATTACHMENT_ADD.to_string(),
            payload: json!({
                "workspace_id": workspace.id,
                "workspace_key": workspace.key,
                "attachment_id": "7KQ9A1X4MV2P8D6R",
                "sha256": "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
                "byte_size": 12,
                "media_type": "image/png",
                "filename": "photo.png",
                "alt_text": null,
                "width": 320,
                "height": 240,
                "created_at": "2026-06-01T00:00:00Z"
            }),
            base_version: None,
            created_at: "2026-06-01T00:00:00Z".to_string(),
            server_seq: Some(1),
        }
    }

    #[tokio::test]
    async fn label_administration_changes_preserve_task_references() {
        let (_temp, mut conn) = test_conn().await;
        let workspace = Workspace::default();
        let project = create_project(&mut conn, &workspace, "app").await.unwrap();
        sqlx::query(
            "INSERT INTO tasks(id, workspace_id, title, description, project_id, status, priority, created_at, updated_at)
             VALUES ('7KQ9A1X4MV2P8D6R', ?, 'local', '', ?, 'inbox', 'none', 't', 't')",
        )
        .bind(&workspace.id)
        .bind(project.id)
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query("INSERT INTO labels(workspace_id, name, created_at) VALUES (?, 'old', 't')")
            .bind(&workspace.id)
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO task_labels(workspace_id, task_id, label)
             VALUES (?, '7KQ9A1X4MV2P8D6R', 'old')",
        )
        .bind(&workspace.id)
        .execute(&mut *conn)
        .await
        .unwrap();
        let change = |op_type: &str, entity_id: &str, payload| ChangeWire {
            change_id: format!("{op_type}change"),
            client_id: "remote".to_string(),
            local_seq: 1,
            entity_type: "label".to_string(),
            entity_id: entity_id.to_string(),
            field: None,
            op_type: op_type.to_string(),
            payload,
            base_version: None,
            created_at: "t".to_string(),
            server_seq: Some(1),
        };

        label::set_label_name(
            &mut conn,
            &change(
                op_type::SET_LABEL_NAME,
                "old",
                json!({
                    "workspace_id": workspace.id.clone(),
                    "workspace_key": workspace.key.clone(),
                    "name": "old",
                    "new_name": "new",
                    "renamed_at": "2026-08-01T00:00:00Z"
                }),
            ),
        )
        .await
        .unwrap();
        let labels: Vec<String> = sqlx::query_scalar(
            "SELECT label FROM task_labels WHERE workspace_id = ? AND task_id = '7KQ9A1X4MV2P8D6R'",
        )
        .bind(&workspace.id)
        .fetch_all(&mut *conn)
        .await
        .unwrap();
        assert_eq!(labels, vec!["new"]);

        label::delete_label(
            &mut conn,
            &change(
                op_type::LABEL_DELETE,
                "new",
                json!({
                    "workspace_id": workspace.id.clone(),
                    "workspace_key": workspace.key.clone(),
                    "name": "new",
                    "deleted_at": "2026-08-01T00:00:01Z"
                }),
            ),
        )
        .await
        .unwrap();
        label::restore_label(
            &mut conn,
            &change(
                op_type::LABEL_RESTORE,
                "new",
                json!({
                    "workspace_id": workspace.id.clone(),
                    "workspace_key": workspace.key.clone(),
                    "name": "new",
                    "created_at": "t",
                    "task_ids": ["7KQ9A1X4MV2P8D6R"],
                    "series_ids": [],
                    "restored_at": "2026-08-01T00:00:02Z"
                }),
            ),
        )
        .await
        .unwrap();
        let labels: Vec<String> = sqlx::query_scalar(
            "SELECT label FROM task_labels WHERE workspace_id = ? AND task_id = '7KQ9A1X4MV2P8D6R'",
        )
        .bind(&workspace.id)
        .fetch_all(&mut *conn)
        .await
        .unwrap();
        assert_eq!(labels, vec!["new"]);
    }

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

    #[tokio::test]
    async fn attachment_apply_rejects_malformed_payload_before_database_writes() {
        let (_temp, mut conn) = test_conn().await;
        let workspace = seed_attachment_task(&mut conn).await;
        let mut change = attachment_add_change(&workspace);
        change.payload["filename"] = json!("bad/name.png");

        let error = apply_remote_change(&mut conn, &change).await.unwrap_err();
        assert!(error.to_string().contains("invalid-attachment-filename"));
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM task_attachments")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn attachment_apply_accepts_identical_duplicate() {
        let (_temp, mut conn) = test_conn().await;
        let workspace = seed_attachment_task(&mut conn).await;
        let change = attachment_add_change(&workspace);

        apply_remote_change(&mut conn, &change).await.unwrap();
        apply_remote_change(&mut conn, &change).await.unwrap();

        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM task_attachments")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn attachment_apply_rejects_conflicting_duplicate_identity() {
        let (_temp, mut conn) = test_conn().await;
        let workspace = seed_attachment_task(&mut conn).await;
        let change = attachment_add_change(&workspace);
        apply_remote_change(&mut conn, &change).await.unwrap();

        let mut conflicting = change;
        conflicting.change_id = "AAAAAAAAAAAAAAA1".to_string();
        conflicting.payload["filename"] = json!("other.png");
        let error = apply_remote_change(&mut conn, &conflicting)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("attachment-identity-conflict"));
        let filename: Option<String> = sqlx::query_scalar("SELECT filename FROM task_attachments")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(filename.as_deref(), Some("photo.png"));
    }
}
