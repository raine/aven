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
pub(crate) struct CreateTaskPayload {
    pub(crate) title: String,
    pub(crate) description: Option<String>,
    pub(crate) project_id: ProjectId,
    pub(crate) status: Option<String>,
    pub(crate) priority: Option<String>,
    pub(crate) available_at: Option<String>,
    pub(crate) due_on: Option<String>,
    pub(crate) is_epic: Option<String>,
    pub(crate) created_at: Option<String>,
}

impl CreateTaskPayload {
    pub(crate) fn from_change(change: &ChangeWire) -> Result<Self> {
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
