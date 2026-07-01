use anyhow::Result;
use serde_json::Value;
use sqlx::SqliteConnection;

use crate::db::insert_change;
use crate::workspaces::Workspace;

pub(crate) mod op_type {
    pub(crate) const CREATE_TASK: &str = "create_task";
    pub(crate) const SET_FIELD: &str = "set_field";
    pub(crate) const RESOLVE_FIELD: &str = "resolve_field";
    pub(crate) const LABEL_ADD: &str = "label_add";
    pub(crate) const LABEL_REMOVE: &str = "label_remove";
    pub(crate) const NOTE_ADD: &str = "note_add";
    pub(crate) const NOTE_DELETE: &str = "note_delete";
    pub(crate) const DEPENDENCY_ADD: &str = "dependency_add";
    pub(crate) const DEPENDENCY_REMOVE: &str = "dependency_remove";
    pub(crate) const EPIC_LINK_ADD: &str = "epic_link_add";
    pub(crate) const EPIC_LINK_REMOVE: &str = "epic_link_remove";
    pub(crate) const CREATE_PROJECT: &str = "create_project";
    pub(crate) const SET_PROJECT_METADATA: &str = "set_project_metadata";
    pub(crate) const PROJECT_DELETE: &str = "project_delete";
    pub(crate) const CREATE_LABEL: &str = "create_label";
    pub(crate) const LABEL_DELETE: &str = "label_delete";
    pub(crate) const CREATE_WORKSPACE: &str = "create_workspace";
    pub(crate) const SET_WORKSPACE_FIELD: &str = "set_workspace_field";
}

pub(crate) enum ChangeEntity {
    Task,
    Project,
    Label,
    #[allow(dead_code)]
    Workspace,
}

impl ChangeEntity {
    pub(crate) const fn as_str(&self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Project => "project",
            Self::Label => "label",
            Self::Workspace => "workspace",
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
pub(crate) struct ChangePayload {
    map: serde_json::Map<String, serde_json::Value>,
}

impl ChangePayload {
    pub(crate) fn workspace(workspace: &Workspace) -> Self {
        let mut map = serde_json::Map::new();
        map.insert(
            "workspace_id".to_string(),
            Value::String(workspace.id.clone()),
        );
        map.insert(
            "workspace_key".to_string(),
            Value::String(workspace.key.clone()),
        );
        Self { map }
    }

    pub(crate) fn set(mut self, key: &str, value: impl serde::Serialize) -> Self {
        self.map.insert(
            key.to_string(),
            serde_json::to_value(value).expect("change payload value serialization"),
        );
        self
    }

    pub(crate) fn into_value(self) -> Value {
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
