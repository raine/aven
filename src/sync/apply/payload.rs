use anyhow::Result;
use serde_json::Value;

use crate::sync::wire::ChangeWire;

use super::shared::{optional_str_payload, str_payload};

/// Extracted fields from a `create_task` change payload.
///
/// Keeps extraction centralized so field keys and optionality are defined
/// in one place. Apply handlers stay responsible for DB resolution and
/// domain parsing (status/priority parsing, project lookup, etc.).
pub(crate) struct CreateTaskPayload {
    pub(crate) title: String,
    pub(crate) description: Option<String>,
    pub(crate) project_id: String,
    pub(crate) status: Option<String>,
    pub(crate) priority: Option<String>,
    pub(crate) is_epic: Option<String>,
    pub(crate) created_at: Option<String>,
}

impl CreateTaskPayload {
    pub(crate) fn from_change(change: &ChangeWire) -> Result<Self> {
        let payload: &Value = &change.payload;
        Ok(Self {
            title: str_payload(payload, "title")?,
            description: optional_str_payload(payload, "description"),
            project_id: str_payload(payload, "project_id")?,
            status: optional_str_payload(payload, "status"),
            priority: optional_str_payload(payload, "priority"),
            is_epic: optional_str_payload(payload, "is_epic"),
            created_at: optional_str_payload(payload, "created_at"),
        })
    }
}

/// Extracted fields from a `create_project` change payload.
pub(crate) struct CreateProjectPayload {
    pub(crate) key: String,
    pub(crate) name: String,
    pub(crate) prefix: String,
    pub(crate) created_at: Option<String>,
}

impl CreateProjectPayload {
    pub(crate) fn from_change(change: &ChangeWire) -> Result<Self> {
        let payload: &Value = &change.payload;
        Ok(Self {
            key: str_payload(payload, "key")?,
            name: str_payload(payload, "name")?,
            prefix: str_payload(payload, "prefix")?,
            created_at: optional_str_payload(payload, "created_at"),
        })
    }
}

/// Extracted fields from a `set_project_metadata` change payload.
pub(crate) struct SetProjectMetadataPayload {
    pub(crate) key: String,
    pub(crate) name: String,
    pub(crate) prefix: String,
}

impl SetProjectMetadataPayload {
    pub(crate) fn from_change(change: &ChangeWire) -> Result<Self> {
        let payload: &Value = &change.payload;
        Ok(Self {
            key: str_payload(payload, "key")?,
            name: str_payload(payload, "name")?,
            prefix: str_payload(payload, "prefix")?,
        })
    }
}
