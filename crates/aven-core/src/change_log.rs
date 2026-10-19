use anyhow::Result;
use serde_json::Value;
use sqlx::SqliteConnection;

use crate::db::insert_change;
use crate::workspaces::Workspace;

pub mod op_type {
    macro_rules! operations {
        ($($name:ident = $value:literal;)*) => {
            $(pub const $name: &str = $value;)*

            /// Every registered operation, in released vocabulary order.
            pub const ALL: &[&str] = &[$($name),*];
        };
    }

    operations! {
        CREATE_TASK = "create_task";
        MOVE_TASKS = "move_tasks";
        SET_FIELD = "set_field";
        RESOLVE_FIELD = "resolve_field";
        LABEL_ADD = "label_add";
        LABEL_REMOVE = "label_remove";
        NOTE_ADD = "note_add";
        NOTE_EDIT = "note_edit";
        NOTE_DELETE = "note_delete";
        DEPENDENCY_ADD = "dependency_add";
        DEPENDENCY_REMOVE = "dependency_remove";
        RELATED_ADD = "related_add";
        RELATED_REMOVE = "related_remove";
        EPIC_LINK_ADD = "epic_link_add";
        EPIC_LINK_REMOVE = "epic_link_remove";
        ATTACHMENT_ADD = "attachment_add";
        ATTACHMENT_DELETE = "attachment_delete";
        CREATE_PROJECT = "create_project";
        SET_PROJECT_METADATA = "set_project_metadata";
        PROJECT_DELETE = "project_delete";
        CREATE_LABEL = "create_label";
        SET_LABEL_NAME = "set_label_name";
        LABEL_DELETE = "label_delete";
        LABEL_RESTORE = "label_restore";
        CREATE_WORKSPACE = "create_workspace";
        SET_WORKSPACE_FIELD = "set_workspace_field";
        CREATE_RECURRENCE_SERIES = "create_recurrence_series";
        UPDATE_RECURRENCE_TEMPLATE = "update_recurrence_template";
        PROJECT_RECURRENCE_OCCURRENCE = "project_recurrence_occurrence";
        RESOLVE_RECURRENCE_OCCURRENCE = "resolve_recurrence_occurrence";
        SET_RECURRENCE_STATE = "set_recurrence_state";
        CREATE_METADATA_FIELD = "create_metadata_field";
        SET_METADATA_FIELD = "set_metadata_field";
        SET_TASK_METADATA = "set_task_metadata";
        REMOVE_TASK_METADATA = "remove_task_metadata";
        SET_RECURRENCE_METADATA = "set_recurrence_metadata";
        REMOVE_RECURRENCE_METADATA = "remove_recurrence_metadata";
        OPEN_RECURRENCE_PAUSE = "open_recurrence_pause";
        CLOSE_RECURRENCE_PAUSE = "close_recurrence_pause";
        STOP_RECURRENCE_SERIES = "stop_recurrence_series";
        PUBLISH_DEVICE_LABEL = "publish_device_label";
        DELETE_WORKSPACE = "delete_workspace";
    }
}

pub enum ChangeEntity {
    Task,
    TaskMove,
    Project,
    Label,
    MetadataField,
    #[allow(dead_code)]
    Workspace,
    RecurrenceSeries,
}

impl ChangeEntity {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::TaskMove => "task_move",
            Self::Project => "project",
            Self::Label => "label",
            Self::MetadataField => "metadata_field",
            Self::Workspace => "workspace",
            Self::RecurrenceSeries => "recurrence_series",
        }
    }
}

/// Builder for change payload JSON produced by operations.
///
/// Use `.workspace(&workspace)` to seed workspace_id and workspace_key,
/// then chain `.set(key, value)` for each payload field.
///
/// Example:
/// ```ignore
/// ChangePayload::workspace(&workspace)
///     .set("title", draft.title)
///     .set("project_id", project.id)
///     .into_value()
/// ```
pub struct ChangePayload {
    map: serde_json::Map<String, serde_json::Value>,
}

impl ChangePayload {
    pub fn workspace(workspace: &Workspace) -> Self {
        let mut map = serde_json::Map::new();
        map.insert(
            "workspace_id".to_string(),
            Value::String(workspace.id.to_string()),
        );
        map.insert(
            "workspace_key".to_string(),
            Value::String(workspace.key.clone()),
        );
        Self { map }
    }

    pub fn set(mut self, key: &str, value: impl serde::Serialize) -> Self {
        self.map.insert(
            key.to_string(),
            serde_json::to_value(value).expect("change payload value serialization"),
        );
        self
    }

    pub fn into_value(self) -> Value {
        Value::Object(self.map)
    }
}

/// Insert a change-log row using a `ChangeEntity` and pre-built `ChangePayload`.
///
/// This wrapper always passes `None` for `base_version`. Task field-level
/// operations that need version tracking for conflict detection use
/// `insert_change` directly through `mutation.rs`.
pub(crate) async fn append_change(
    conn: &mut SqliteConnection,
    entity: ChangeEntity,
    entity_id: &str,
    field: Option<&str>,
    op_type: &'static str,
    payload: ChangePayload,
) -> Result<String> {
    insert_change(
        conn,
        entity.as_str(),
        entity_id,
        field,
        op_type,
        payload.into_value(),
        None,
    )
    .await
}
