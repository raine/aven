use std::collections::HashSet;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::attachments::validation::{
    MAX_BLOB_BYTES, validate_alt_text, validate_blob_size, validate_dimensions, validate_filename,
    validate_media_type,
};
use crate::change_log::op_type;
use crate::ids::{BASE32, MetadataFieldId, ProjectId, WorkspaceId};
use crate::task_fields::TaskField;

mod changes;
#[cfg(any(test, feature = "test-support"))]
mod envelope;
mod recurrence;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;

pub const SYNC_PROTOCOL_VERSION: u32 = 18;
const MAX_CHANGE_PAYLOAD_BYTES: usize = 64 * 1024;

pub(crate) fn serialize_change_payload(payload: &Value) -> Result<String> {
    let serialized = serde_json::to_string(payload)?;
    if serialized.len() > MAX_CHANGE_PAYLOAD_BYTES {
        return Err(crate::error::CoreError::validation(format!(
            "error invalid-sync-change payload-too-large limit={MAX_CHANGE_PAYLOAD_BYTES}"
        ))
        .into());
    }
    Ok(serialized)
}

pub fn sync_server_url_is_valid(server: &str) -> bool {
    url::Url::parse(server)
        .as_ref()
        .is_ok_and(sync_server_url_is_valid_url)
}

fn sync_server_url_is_valid_url(url: &url::Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
}
#[cfg(any(test, feature = "test-support"))]
pub const MAX_PUSH_BATCH: usize = 256;
/// Full decoded JSON request allowance for the in-process page simulator.
#[cfg(any(test, feature = "test-support"))]
pub const MAX_SYNC_REQUEST_BYTES: usize = 2 * 1024 * 1024;
#[cfg(any(test, feature = "test-support"))]
pub const MAX_PULL_BATCH: u32 = 512;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeWire {
    pub change_id: String,
    pub client_id: String,
    pub local_seq: i64,
    pub entity_type: String,
    pub entity_id: String,
    pub field: Option<String>,
    pub op_type: String,
    pub payload: Value,
    pub base_version: Option<String>,
    pub created_at: String,
    pub server_seq: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AttachmentAddPayload {
    pub(crate) workspace_id: String,
    pub(crate) workspace_key: String,
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
        validate_attachment_change_envelope(change, op_type::ATTACHMENT_ADD)?;
        required_workspace_payload(&change.payload)?;
        let attachment_id = required_string_payload("attachment_id", &change.payload)?;
        ensure_sync_id("attachment_id", &attachment_id)?;
        let sha256 = required_string_payload("sha256", &change.payload)?;
        validate_sha256_for_sync(&sha256)?;
        let byte_size = required_i64_payload("byte_size", &change.payload)?;
        validate_blob_size_for_sync(byte_size)?;
        let media_type = required_string_payload("media_type", &change.payload)?;
        map_attachment_validation(validate_media_type(&media_type))?;
        let filename = optional_string_payload("filename", &change.payload)?;
        map_attachment_validation(validate_filename(filename.as_deref()))?;
        let alt_text = optional_string_payload("alt_text", &change.payload)?;
        map_attachment_validation(validate_alt_text(alt_text.as_deref()))?;
        let width = optional_i64_payload("width", &change.payload)?;
        let height = optional_i64_payload("height", &change.payload)?;
        map_attachment_validation(validate_dimensions(width, height))?;
        required_timestamp_payload("created_at", &change.payload)?;
        deserialize_attachment_payload(&change.payload)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AttachmentDeletePayload {
    pub(crate) workspace_id: String,
    pub(crate) workspace_key: String,
    pub(crate) attachment_id: String,
    pub(crate) deleted_at: String,
}

impl AttachmentDeletePayload {
    pub(crate) fn from_change(change: &ChangeWire) -> Result<Self> {
        validate_attachment_change_envelope(change, op_type::ATTACHMENT_DELETE)?;
        required_workspace_payload(&change.payload)?;
        let attachment_id = required_string_payload("attachment_id", &change.payload)?;
        ensure_sync_id("attachment_id", &attachment_id)?;
        required_timestamp_payload("deleted_at", &change.payload)?;
        deserialize_attachment_payload(&change.payload)
    }
}

fn deserialize_attachment_payload<T>(payload: &Value) -> Result<T>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_value(payload.clone())
        .map_err(|error| anyhow::anyhow!("error invalid-sync-change payload {error}"))
}

fn validate_attachment_change_envelope(change: &ChangeWire, expected_op_type: &str) -> Result<()> {
    if change.op_type != expected_op_type {
        bail!("error invalid-sync-change op_type={}", change.op_type);
    }
    ensure_entity_type(change, "task")?;
    ensure_sync_id("entity_id", &change.entity_id)?;
    if change.field.as_deref() != Some("attachments") {
        bail!("error invalid-sync-change field=attachments");
    }
    if !change.payload.is_object() {
        bail!("error invalid-sync-change payload expected-object");
    }
    serialize_change_payload(&change.payload)?;
    Ok(())
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushAck {
    pub change_id: String,
    pub server_seq: i64,
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Clone, Copy)]
pub struct ValidatedSyncRequestEnvelope {
    pub after: i64,
    pub pull_limit: u32,
    pub push_count: usize,
}

#[cfg(any(test, feature = "test-support"))]
pub(crate) fn validate_request_at_protocol(
    request: &SyncRequest,
    protocol: u32,
) -> Result<ValidatedSyncRequestEnvelope> {
    envelope::validate_request_at_protocol(request, protocol)
}

#[cfg(any(test, feature = "test-support"))]
pub(crate) fn validate_response_at_protocol(
    protocol: u32,
    after: i64,
    pull_limit: u32,
    request_change_ids: &[String],
    response: &SyncResponse,
) -> Result<()> {
    envelope::validate_response_at_protocol(
        protocol,
        after,
        pull_limit,
        request_change_ids,
        response,
    )
}

#[derive(Debug)]
pub struct ChangeRow {
    pub change_id: String,
    pub client_id: String,
    pub local_seq: i64,
    pub entity_type: String,
    pub entity_id: String,
    pub field: Option<String>,
    pub op_type: String,
    pub payload: String,
    pub base_version: Option<String>,
    pub created_at: String,
    pub server_seq: Option<i64>,
}

impl ChangeRow {
    pub fn into_wire(self) -> ChangeWire {
        ChangeWire {
            change_id: self.change_id,
            client_id: self.client_id,
            local_seq: self.local_seq,
            entity_type: self.entity_type,
            entity_id: self.entity_id,
            field: self.field,
            op_type: self.op_type,
            payload: serde_json::from_str(&self.payload).unwrap_or(Value::Null),
            base_version: self.base_version,
            created_at: self.created_at,
            server_seq: self.server_seq,
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncRequest {
    #[serde(default)]
    pub protocol_version: Option<u32>,
    pub client_id: String,
    pub after: i64,
    #[serde(default)]
    pub pull_limit: Option<u32>,
    pub changes: Vec<ChangeWire>,
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Serialize, Deserialize)]
pub struct SyncResponse {
    pub protocol_version: u32,
    pub cursor: i64,
    pub has_more: bool,
    #[serde(default)]
    pub push_acks: Vec<PushAck>,
    pub changes: Vec<ChangeWire>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChangeDirection {
    Pushed,
    #[cfg(any(test, feature = "test-support"))]
    Pulled,
}

#[cfg(any(test, feature = "test-support"))]
pub fn validate_pushed_change(change: &ChangeWire) -> Result<()> {
    validate_local_change_shape(change)?;
    super::protocol::validate_change(SYNC_PROTOCOL_VERSION, change)
}

pub(crate) fn validate_local_change_shape(change: &ChangeWire) -> Result<()> {
    validate_change_shape(change, ChangeDirection::Pushed)
}

#[cfg(any(test, feature = "test-support"))]
fn validate_pulled_change(change: &ChangeWire) -> Result<()> {
    validate_change_shape(change, ChangeDirection::Pulled)
}

fn validate_change_shape(change: &ChangeWire, direction: ChangeDirection) -> Result<()> {
    ensure_non_empty("change_id", &change.change_id)?;
    ensure_non_empty("client_id", &change.client_id)?;
    ensure_non_empty("entity_id", &change.entity_id)?;
    ensure_non_empty("op_type", &change.op_type)?;
    ensure_non_empty("entity_type", &change.entity_type)?;
    if direction == ChangeDirection::Pushed {
        ensure_sync_id("change_id", &change.change_id)?;
    }
    validate_change_server_seq(change, direction)?;
    if !change.payload.is_object() {
        bail!("error invalid-sync-change payload expected-object");
    }
    serialize_change_payload(&change.payload)?;

    match change.op_type.as_str() {
        op_type::CREATE_WORKSPACE => changes::validate_create_workspace(change)?,
        op_type::SET_WORKSPACE_FIELD => changes::validate_set_workspace_field(change)?,
        op_type::CREATE_PROJECT => changes::validate_create_project(change)?,
        op_type::SET_PROJECT_METADATA => changes::validate_set_project_metadata(change)?,
        op_type::CREATE_LABEL => changes::validate_create_label(change)?,
        op_type::CREATE_METADATA_FIELD => changes::validate_create_metadata_field(change)?,
        op_type::SET_METADATA_FIELD => changes::validate_set_metadata_field(change)?,
        op_type::SET_TASK_METADATA | op_type::REMOVE_TASK_METADATA => {
            changes::validate_task_metadata(change)?
        }
        op_type::CREATE_TASK => changes::validate_create_task(change)?,
        op_type::SET_FIELD | op_type::RESOLVE_FIELD => changes::validate_task_field(change)?,
        op_type::LABEL_ADD | op_type::LABEL_REMOVE => changes::validate_label_change(change)?,
        op_type::NOTE_ADD => changes::validate_note_add(change)?,
        op_type::DEPENDENCY_ADD | op_type::DEPENDENCY_REMOVE => {
            changes::validate_dependency_change(change)?
        }
        op_type::RELATED_ADD | op_type::RELATED_REMOVE => changes::validate_related_change(change)?,
        op_type::EPIC_LINK_ADD | op_type::EPIC_LINK_REMOVE => {
            changes::validate_epic_link_change(change)?
        }
        op_type::PROJECT_DELETE => changes::validate_project_delete(change)?,
        op_type::SET_LABEL_NAME => changes::validate_set_label_name(change)?,
        op_type::LABEL_DELETE => changes::validate_label_delete(change)?,
        op_type::NOTE_EDIT => changes::validate_note_edit(change)?,
        op_type::LABEL_RESTORE => changes::validate_label_restore(change)?,
        op_type::NOTE_DELETE => changes::validate_note_delete(change)?,
        op_type::SET_RECURRENCE_METADATA | op_type::REMOVE_RECURRENCE_METADATA => {
            changes::validate_recurrence_metadata(change)?
        }
        op_type::CREATE_RECURRENCE_SERIES => recurrence::validate_recurrence_create(change)?,
        op_type::UPDATE_RECURRENCE_TEMPLATE => recurrence::validate_recurrence_template(change)?,
        op_type::PROJECT_RECURRENCE_OCCURRENCE => {
            recurrence::validate_recurrence_projection(change)?
        }
        op_type::RESOLVE_RECURRENCE_OCCURRENCE => recurrence::validate_recurrence_outcome(change)?,
        op_type::SET_RECURRENCE_STATE | op_type::STOP_RECURRENCE_SERIES => {
            recurrence::validate_recurrence_state(change)?
        }
        op_type::OPEN_RECURRENCE_PAUSE => recurrence::validate_recurrence_pause(change, true)?,
        op_type::CLOSE_RECURRENCE_PAUSE => recurrence::validate_recurrence_pause(change, false)?,
        op_type::ATTACHMENT_ADD => {
            AttachmentAddPayload::from_change(change)?;
        }
        op_type::ATTACHMENT_DELETE => {
            AttachmentDeletePayload::from_change(change)?;
        }
        _ => bail!("error invalid-sync-change op_type={}", change.op_type),
    }
    Ok(())
}

fn validate_sha256_for_sync(value: &str) -> Result<()> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        Ok(())
    } else {
        bail!("error invalid-sync-change invalid-sha256");
    }
}

fn validate_blob_size_for_sync(byte_size: i64) -> Result<()> {
    let Ok(bytes) = usize::try_from(byte_size) else {
        bail!("error invalid-sync-change error invalid-attachment-size bytes={byte_size}");
    };
    if bytes == 0 || bytes > MAX_BLOB_BYTES {
        bail!("error invalid-sync-change error invalid-attachment-size bytes={byte_size}");
    }
    map_attachment_validation(validate_blob_size(bytes))
}

fn map_attachment_validation(result: Result<()>) -> Result<()> {
    result.map_err(|err| anyhow::anyhow!("error invalid-sync-change {err}"))
}

fn validate_change_server_seq(change: &ChangeWire, direction: ChangeDirection) -> Result<()> {
    match direction {
        ChangeDirection::Pushed if change.server_seq.is_some() => {
            bail!("error invalid-sync-change server_seq client-supplied");
        }
        #[cfg(any(test, feature = "test-support"))]
        ChangeDirection::Pulled => match change.server_seq {
            Some(server_seq) if server_seq > 0 => {}
            Some(server_seq) => {
                bail!("error invalid-sync-change server_seq={server_seq}");
            }
            None => bail!("error invalid-sync-change server_seq missing"),
        },
        ChangeDirection::Pushed => {}
    }
    Ok(())
}

fn validate_sync_task_field_value(field: TaskField, value: &str) -> Result<()> {
    field
        .validate_value(value)
        .map_err(|err| anyhow::anyhow!("error invalid-sync-change {err}"))
}

fn ensure_entity_type(change: &ChangeWire, expected: &str) -> Result<()> {
    if change.entity_type == expected {
        Ok(())
    } else {
        bail!(
            "error invalid-sync-change op_type={} entity_type={} expected={}",
            change.op_type,
            change.entity_type,
            expected
        )
    }
}

fn ensure_non_empty(name: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("error invalid-sync-change {name} empty");
    }
    Ok(())
}

fn ensure_sync_id(name: &str, value: &str) -> Result<()> {
    if value.len() == 16 && value.bytes().all(|byte| BASE32.contains(&byte)) {
        Ok(())
    } else {
        bail!("error invalid-sync-change {name} invalid-id");
    }
}

fn ensure_project_id(name: &str, value: &str) -> Result<()> {
    if value.parse::<ProjectId>().is_ok() {
        Ok(())
    } else {
        bail!("error invalid-sync-change {name} invalid-id");
    }
}

fn required_string_payload(key: &str, payload: &Value) -> Result<String> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .with_context(|| format!("error invalid-sync-change payload.{key} missing"))
}

fn required_i64_payload(key: &str, payload: &Value) -> Result<i64> {
    match payload.get(key) {
        Some(Value::Number(value)) => value
            .as_i64()
            .with_context(|| format!("error invalid-sync-change payload.{key} invalid")),
        Some(Value::Null) | None => bail!("error invalid-sync-change payload.{key} missing"),
        Some(_) => bail!("error invalid-sync-change payload.{key} invalid"),
    }
}

fn required_timestamp_payload(key: &str, payload: &Value) -> Result<String> {
    let value = required_string_payload(key, payload)?;
    validate_timestamp_value(&format!("payload.{key}"), &value)?;
    Ok(value)
}

fn validate_timestamp_value(label: &str, value: &str) -> Result<()> {
    if value.ends_with('Z') && chrono::DateTime::parse_from_rfc3339(value).is_ok() {
        Ok(())
    } else {
        bail!("error invalid-sync-change {label} invalid-timestamp");
    }
}

fn required_workspace_payload(payload: &Value) -> Result<()> {
    let id = required_string_payload("workspace_id", payload)?;
    if id.parse::<WorkspaceId>().is_err() {
        bail!("error invalid-sync-change workspace_id invalid-id");
    }
    required_string_payload("workspace_key", payload)?;
    Ok(())
}

fn optional_workspace_payload(payload: &Value) -> Result<()> {
    if payload.get("workspace_id").is_none() && payload.get("workspace_key").is_none() {
        return Ok(());
    }
    required_workspace_payload(payload)
}

fn optional_string_payload(key: &str, payload: &Value) -> Result<Option<String>> {
    match payload.get(key) {
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(Value::Null) | None => Ok(None),
        Some(_) => bail!("error invalid-sync-change payload.{key} invalid"),
    }
}

fn optional_i64_payload(key: &str, payload: &Value) -> Result<Option<i64>> {
    match payload.get(key) {
        Some(Value::Number(value)) => value
            .as_i64()
            .map(Some)
            .with_context(|| format!("error invalid-sync-change payload.{key} invalid")),
        Some(Value::Null) | None => Ok(None),
        Some(_) => bail!("error invalid-sync-change payload.{key} invalid"),
    }
}

fn optional_bool_payload(key: &str, payload: &Value) -> Result<Option<bool>> {
    match payload.get(key) {
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(Value::Null) | None => Ok(None),
        Some(_) => bail!("error invalid-sync-change payload.{key} invalid"),
    }
}

fn string_array_payload(key: &str, payload: &Value) -> Result<Vec<String>> {
    payload
        .get(key)
        .and_then(Value::as_array)
        .with_context(|| format!("error invalid-sync-change payload.{key} invalid"))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .with_context(|| format!("error invalid-sync-change payload.{key} invalid"))
        })
        .collect()
}

fn optional_string_array_payload(key: &str, payload: &Value) -> Result<()> {
    match payload.get(key) {
        Some(Value::Array(values))
            if values
                .iter()
                .all(|value| value.as_str().is_some_and(|value| !value.trim().is_empty())) =>
        {
            Ok(())
        }
        Some(Value::Null) | None => Ok(()),
        Some(_) => bail!("error invalid-sync-change payload.{key} invalid"),
    }
}

fn ensure_metadata_field_id(name: &str, value: &str) -> Result<()> {
    value
        .parse::<MetadataFieldId>()
        .map(|_| ())
        .map_err(|_| anyhow::anyhow!("error invalid-sync-change {name} invalid-id"))
}

fn validate_metadata_key_payload(key: &str, payload: &Value) -> Result<String> {
    let value = required_string_payload(key, payload)?;
    let normalized = crate::metadata::normalize_metadata_key(&value)
        .map_err(|_| anyhow::anyhow!("error invalid-sync-change payload.{key} invalid"))?;
    if normalized != value {
        bail!("error invalid-sync-change payload.{key} noncanonical");
    }
    Ok(value)
}

fn validate_metadata_value_payload(key: &str, payload: &Value) -> Result<String> {
    let value = required_string_payload(key, payload)?;
    if value.len() > crate::metadata::MAX_METADATA_VALUE_BYTES {
        bail!("error invalid-sync-change payload.{key} too-large");
    }
    Ok(value)
}

fn validate_metadata_array_payload(payload: &Value) -> Result<()> {
    let Some(values) = payload.get("metadata") else {
        return Ok(());
    };
    let values = values
        .as_array()
        .context("error invalid-sync-change payload.metadata invalid")?;
    if values.len() > crate::metadata::MAX_METADATA_VALUES {
        bail!("error invalid-sync-change payload.metadata too-many");
    }
    let mut ids = HashSet::new();
    let mut keys = HashSet::new();
    let mut total_bytes = 0usize;
    for value in values {
        let object = value
            .as_object()
            .context("error invalid-sync-change payload.metadata invalid")?;
        let field_id = required_string_payload("field_id", value)?;
        ensure_metadata_field_id("metadata.field_id", &field_id)?;
        let key = validate_metadata_key_payload("key", value)?;
        let metadata_value = validate_metadata_value_payload("value", value)?;
        if !ids.insert(field_id) || !keys.insert(key) {
            bail!("error invalid-sync-change payload.metadata duplicate");
        }
        if object.len() != 3 {
            bail!("error invalid-sync-change payload.metadata invalid");
        }
        total_bytes = total_bytes.saturating_add(metadata_value.len());
    }
    if total_bytes > crate::metadata::MAX_METADATA_TOTAL_BYTES {
        bail!("error invalid-sync-change payload.metadata too-large");
    }
    Ok(())
}
