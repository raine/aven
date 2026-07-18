use anyhow::{Context, Result};
use serde_json::Value;

use crate::ids::ProjectId;
use crate::sync::wire::ChangeWire;

use super::shared::{optional_str_payload, str_payload};

/// Extracted fields from an `attachment_add` change payload.
pub(crate) struct AttachmentAddPayload {
    pub(crate) attachment_id: String,
    pub(crate) sha256: String,
    pub(crate) byte_size: i64,
    pub(crate) media_type: String,
    pub(crate) filename: Option<String>,
    pub(crate) alt_text: Option<String>,
    pub(crate) width: Option<i64>,
    pub(crate) height: Option<i64>,
    pub(crate) created_at: String,
}

impl AttachmentAddPayload {
    pub(crate) fn from_change(change: &ChangeWire) -> Result<Self> {
        let payload = &change.payload;
        Ok(Self {
            attachment_id: str_payload(payload, "attachment_id")?,
            sha256: str_payload(payload, "sha256")?,
            byte_size: payload
                .get("byte_size")
                .and_then(Value::as_i64)
                .context("payload missing byte_size")?,
            media_type: str_payload(payload, "media_type")?,
            filename: optional_str_payload(payload, "filename"),
            alt_text: optional_str_payload(payload, "alt_text"),
            width: payload.get("width").and_then(Value::as_i64),
            height: payload.get("height").and_then(Value::as_i64),
            created_at: str_payload(payload, "created_at")?,
        })
    }
}

/// Extracted fields from an `attachment_delete` change payload.
pub(crate) struct AttachmentDeletePayload {
    pub(crate) attachment_id: String,
    pub(crate) deleted_at: String,
}

impl AttachmentDeletePayload {
    pub(crate) fn from_change(change: &ChangeWire) -> Result<Self> {
        Ok(Self {
            attachment_id: str_payload(&change.payload, "attachment_id")?,
            deleted_at: str_payload(&change.payload, "deleted_at")?,
        })
    }
}

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
