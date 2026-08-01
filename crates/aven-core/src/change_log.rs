use anyhow::Result;
use serde_json::Value;
use sqlx::SqliteConnection;

use crate::db::insert_change;
use crate::workspaces::Workspace;

pub mod op_type {
    pub const CREATE_TASK: &str = "create_task";
    pub const SET_FIELD: &str = "set_field";
    pub const RESOLVE_FIELD: &str = "resolve_field";
    pub const LABEL_ADD: &str = "label_add";
    pub const LABEL_REMOVE: &str = "label_remove";
    pub const NOTE_ADD: &str = "note_add";
    pub const NOTE_EDIT: &str = "note_edit";
    pub const NOTE_DELETE: &str = "note_delete";
    pub const DEPENDENCY_ADD: &str = "dependency_add";
    pub const DEPENDENCY_REMOVE: &str = "dependency_remove";
    pub const EPIC_LINK_ADD: &str = "epic_link_add";
    pub const EPIC_LINK_REMOVE: &str = "epic_link_remove";
    pub const ATTACHMENT_ADD: &str = "attachment_add";
    pub const ATTACHMENT_DELETE: &str = "attachment_delete";
    pub const CREATE_PROJECT: &str = "create_project";
    pub const SET_PROJECT_METADATA: &str = "set_project_metadata";
    pub const PROJECT_DELETE: &str = "project_delete";
    pub const CREATE_LABEL: &str = "create_label";
    pub const SET_LABEL_NAME: &str = "set_label_name";
    pub const LABEL_DELETE: &str = "label_delete";
    pub const LABEL_RESTORE: &str = "label_restore";
    pub const CREATE_WORKSPACE: &str = "create_workspace";
    pub const SET_WORKSPACE_FIELD: &str = "set_workspace_field";
    pub const CREATE_RECURRENCE_SERIES: &str = "create_recurrence_series";
    pub const UPDATE_RECURRENCE_TEMPLATE: &str = "update_recurrence_template";
    pub const PROJECT_RECURRENCE_OCCURRENCE: &str = "project_recurrence_occurrence";
    pub const RESOLVE_RECURRENCE_OCCURRENCE: &str = "resolve_recurrence_occurrence";
    pub const SET_RECURRENCE_STATE: &str = "set_recurrence_state";
    pub const OPEN_RECURRENCE_PAUSE: &str = "open_recurrence_pause";
    pub const CLOSE_RECURRENCE_PAUSE: &str = "close_recurrence_pause";
    pub const STOP_RECURRENCE_SERIES: &str = "stop_recurrence_series";
}

pub enum ChangeEntity {
    Task,
    Project,
    Label,
    #[allow(dead_code)]
    Workspace,
    RecurrenceSeries,
}

impl ChangeEntity {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Project => "project",
            Self::Label => "label",
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
