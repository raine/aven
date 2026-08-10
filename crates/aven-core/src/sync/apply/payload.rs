use anyhow::Result;
use serde_json::Value;

use crate::choices::TaskSource;
use crate::ids::ProjectId;
use crate::sync::wire::ChangeWire;

use super::shared::{optional_str_payload, str_payload};

/// Extracted fields from a `create_task` change payload.
///
/// Keeps extraction centralized so field keys and optionality are defined
/// in one place. Apply handlers stay responsible for DB resolution and
/// domain parsing (status/priority parsing, project lookup, etc.).
pub struct CreateTaskPayload {
    pub title: String,
    pub description: Option<String>,
    pub project_id: ProjectId,
    pub status: Option<String>,
    pub priority: Option<String>,
    pub source: TaskSource,
    pub available_at: Option<String>,
    pub due_on: Option<String>,
    pub is_epic: Option<String>,
    pub created_at: Option<String>,
}

impl CreateTaskPayload {
    pub fn from_change(change: &ChangeWire) -> Result<Self> {
        let payload: &Value = &change.payload;
        Ok(Self {
            title: str_payload(payload, "title")?,
            description: optional_str_payload(payload, "description"),
            project_id: str_payload(payload, "project_id")?.parse()?,
            status: optional_str_payload(payload, "status"),
            priority: optional_str_payload(payload, "priority"),
            source: optional_str_payload(payload, "source")
                .map(|value| TaskSource::parse(&value))
                .transpose()?
                .unwrap_or_default(),
            available_at: optional_str_payload(payload, "available_at"),
            due_on: optional_str_payload(payload, "due_on"),
            is_epic: optional_str_payload(payload, "is_epic"),
            created_at: optional_str_payload(payload, "created_at"),
        })
    }
}

/// Extracted fields from a `create_project` change payload.
pub struct CreateProjectPayload {
    pub key: String,
    pub name: String,
    pub prefix: String,
    pub created_at: Option<String>,
}

impl CreateProjectPayload {
    pub fn from_change(change: &ChangeWire) -> Result<Self> {
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
pub struct SetProjectMetadataPayload {
    pub key: String,
    pub name: String,
    pub prefix: String,
}

impl SetProjectMetadataPayload {
    pub fn from_change(change: &ChangeWire) -> Result<Self> {
        let payload: &Value = &change.payload;
        Ok(Self {
            key: str_payload(payload, "key")?,
            name: str_payload(payload, "name")?,
            prefix: str_payload(payload, "prefix")?,
        })
    }
}
