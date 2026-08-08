use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, bail};
use chrono::{NaiveDate, NaiveTime};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::attachments::validation::{
    MAX_BLOB_BYTES, validate_alt_text, validate_blob_size, validate_dimensions, validate_filename,
    validate_media_type,
};
use crate::change_log::op_type;
use crate::choices::TaskSource;
use crate::ids::{BASE32, MetadataFieldId, ProjectId, WorkspaceId};
use crate::recurrence::{
    RecurrenceDuePolicy, RecurrenceFrequency, RecurrenceOutcome, RecurrenceRule,
    RecurrenceSchedule, RecurrenceSeriesId, RecurrenceSeriesState, TimeZoneId, WeekdaySet,
    derive_occurrence_identity, is_slot, next_slot_after,
};
use crate::task_fields::TaskField;

pub const SYNC_PROTOCOL_VERSION: u32 = 13;
const MAX_CHANGE_PAYLOAD_BYTES: usize = 64 * 1024;
pub fn sync_server_url_is_valid(server: &str) -> bool {
    let Ok(url) = url::Url::parse(server) else {
        return false;
    };
    matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
}
pub const MAX_PUSH_BATCH: usize = 256;
pub const MAX_PULL_BATCH: u32 = 512;
pub const MAX_BLOB_TRANSFER_OBJECTS: usize = 16;
pub const MAX_BLOB_TRANSFER_BYTES: u64 = 64 * 1024 * 1024;
pub const DAEMON_SYNC_PAGE_BUDGET: usize = 8;
pub const DAEMON_INCOMPLETE_RESCHEDULE_MS: u64 = 100;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushAck {
    pub change_id: String,
    pub server_seq: i64,
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

#[derive(Debug, Serialize, Deserialize)]
pub struct SyncResponse {
    pub protocol_version: u32,
    pub cursor: i64,
    pub has_more: bool,
    #[serde(default)]
    pub push_acks: Vec<PushAck>,
    pub changes: Vec<ChangeWire>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobUploadContract {
    pub workspace_id: String,
    pub sha256: String,
    pub byte_size: i64,
    pub media_type: String,
    pub width: i64,
    pub height: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MissingBlobsRequest {
    pub blobs: Vec<BlobUploadContract>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MissingBlobsResponse {
    pub missing: Vec<String>,
}

pub fn validate_blob_contracts(blobs: &[BlobUploadContract]) -> Result<()> {
    if blobs.len() > MAX_PUSH_BATCH {
        bail!(
            "error blob-batch-too-large limit={} got={}",
            MAX_PUSH_BATCH,
            blobs.len()
        );
    }
    let mut seen = HashSet::with_capacity(blobs.len());
    let mut hashes = HashSet::with_capacity(blobs.len());
    for blob in blobs {
        ensure_sync_id("workspace_id", &blob.workspace_id)?;
        validate_sha256_for_sync(&blob.sha256)?;
        validate_blob_size_for_sync(blob.byte_size)?;
        map_attachment_validation(validate_media_type(&blob.media_type))?;
        map_attachment_validation(validate_dimensions(Some(blob.width), Some(blob.height)))?;
        if !seen.insert((blob.workspace_id.as_str(), blob.sha256.as_str())) {
            bail!("error duplicate-blob-contract");
        }
        hashes.insert(blob.sha256.as_str());
    }
    if hashes.len() > MAX_BLOB_TRANSFER_OBJECTS {
        bail!(
            "error blob-batch-too-large limit={} got={}",
            MAX_BLOB_TRANSFER_OBJECTS,
            hashes.len()
        );
    }
    Ok(())
}

pub fn validate_blob_hashes(hashes: &[String]) -> Result<()> {
    if hashes.len() > MAX_BLOB_TRANSFER_OBJECTS {
        bail!(
            "error blob-batch-too-large limit={} got={}",
            MAX_BLOB_TRANSFER_OBJECTS,
            hashes.len()
        );
    }
    let mut seen = HashSet::with_capacity(hashes.len());
    for hash in hashes {
        validate_sha256_for_sync(hash)?;
        if !seen.insert(hash.as_str()) {
            bail!("error duplicate-blob-hash");
        }
    }
    Ok(())
}

#[derive(Debug)]
struct SyncProtocolError {
    client: u32,
    server: u32,
}

impl std::fmt::Display for SyncProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "error sync-protocol-unsupported client={} server={}",
            self.client, self.server
        )
    }
}

impl std::error::Error for SyncProtocolError {}

fn sync_protocol_error(client: u32, server: u32) -> anyhow::Error {
    anyhow::Error::new(SyncProtocolError { client, server })
}

pub fn validate_sync_protocol_version(client: u32, server: u32) -> Result<()> {
    if client != server {
        return Err(sync_protocol_error(client, server));
    }
    Ok(())
}

pub fn validate_sync_request_protocol_version(client: Option<u32>) -> Result<()> {
    validate_sync_protocol_version(client.unwrap_or(0), SYNC_PROTOCOL_VERSION)
}

pub fn request_pull_limit(requested: Option<u32>) -> Result<u32> {
    match requested {
        None => Ok(MAX_PULL_BATCH),
        Some(limit @ 1..=MAX_PULL_BATCH) => Ok(limit),
        Some(limit) => {
            bail!("error sync-pull-limit-out-of-range min=1 max={MAX_PULL_BATCH} got={limit}")
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ValidatedSyncRequestEnvelope {
    pub after: i64,
    pub pull_limit: u32,
    pub push_count: usize,
}

pub fn validate_sync_request_envelope(
    request: &SyncRequest,
) -> Result<ValidatedSyncRequestEnvelope> {
    validate_sync_request_protocol_version(request.protocol_version)?;
    validate_request_cursor(request.after)?;
    validate_push_batch_size(request.changes.len())?;
    Ok(ValidatedSyncRequestEnvelope {
        after: request.after,
        pull_limit: request_pull_limit(request.pull_limit)?,
        push_count: request.changes.len(),
    })
}

fn validate_request_cursor(after: i64) -> Result<()> {
    if after < 0 {
        bail!("error sync-after-out-of-range min=0 got={after}");
    }
    Ok(())
}

fn validate_push_batch_size(len: usize) -> Result<()> {
    if len > MAX_PUSH_BATCH {
        bail!("error sync-push-too-large limit={MAX_PUSH_BATCH} got={len}");
    }
    Ok(())
}

pub fn validate_sync_response_for_request(
    after: i64,
    pull_limit: u32,
    request_change_ids: &[String],
    response: &SyncResponse,
) -> Result<()> {
    validate_sync_protocol_version(SYNC_PROTOCOL_VERSION, response.protocol_version)?;
    if response.changes.len() > pull_limit as usize {
        bail!(
            "error invalid-sync-response pull-too-large limit={} got={}",
            pull_limit,
            response.changes.len()
        );
    }
    if response.cursor < after {
        bail!(
            "error invalid-sync-response cursor-regressed after={} cursor={}",
            after,
            response.cursor
        );
    }
    validate_push_acks(request_change_ids, response)?;
    validate_pull_page(after, pull_limit, response)?;
    validate_push_pull_overlap(response)?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChangeDirection {
    Pushed,
    Pulled,
}

pub fn validate_pushed_change(change: &ChangeWire) -> Result<()> {
    validate_change_shape(change, ChangeDirection::Pushed)
}

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
    if serde_json::to_vec(&change.payload)?.len() > MAX_CHANGE_PAYLOAD_BYTES {
        bail!("error invalid-sync-change payload-too-large limit={MAX_CHANGE_PAYLOAD_BYTES}");
    }

    match change.op_type.as_str() {
        op_type::CREATE_WORKSPACE => {
            ensure_entity_type(change, "workspace")?;
            ensure_sync_id("entity_id", &change.entity_id)?;
            required_string_payload("key", &change.payload)?;
            required_string_payload("name", &change.payload)?;
            required_string_payload("created_at", &change.payload)?;
        }
        op_type::SET_WORKSPACE_FIELD => {
            ensure_entity_type(change, "workspace")?;
            ensure_sync_id("entity_id", &change.entity_id)?;
            let field = change
                .field
                .as_deref()
                .filter(|field| !field.trim().is_empty())
                .context("error invalid-sync-change field missing")?;
            if !matches!(field, "name" | "key") {
                bail!("error invalid-sync-change field={field}");
            }
            required_string_payload("value", &change.payload)?;
        }
        op_type::CREATE_PROJECT => {
            ensure_entity_type(change, "project")?;
            ensure_project_id("entity_id", &change.entity_id)?;
            optional_workspace_payload(&change.payload)?;
            required_string_payload("key", &change.payload)?;
            required_string_payload("name", &change.payload)?;
            required_string_payload("prefix", &change.payload)?;
            required_string_payload("created_at", &change.payload)?;
        }
        op_type::SET_PROJECT_METADATA => {
            ensure_entity_type(change, "project")?;
            ensure_project_id("entity_id", &change.entity_id)?;
            optional_workspace_payload(&change.payload)?;
            required_string_payload("key", &change.payload)?;
            required_string_payload("name", &change.payload)?;
            required_string_payload("prefix", &change.payload)?;
            required_string_payload("updated_at", &change.payload)?;
        }
        op_type::CREATE_LABEL => {
            ensure_entity_type(change, "label")?;
            optional_workspace_payload(&change.payload)?;
            required_string_payload("name", &change.payload)?;
            required_string_payload("created_at", &change.payload)?;
        }
        op_type::CREATE_METADATA_FIELD => {
            ensure_entity_type(change, "metadata_field")?;
            ensure_metadata_field_id("entity_id", &change.entity_id)?;
            required_workspace_payload(&change.payload)?;
            validate_metadata_key_payload("key", &change.payload)?;
            required_timestamp_payload("created_at", &change.payload)?;
        }
        op_type::SET_METADATA_FIELD => {
            ensure_entity_type(change, "metadata_field")?;
            ensure_metadata_field_id("entity_id", &change.entity_id)?;
            required_workspace_payload(&change.payload)?;
            if change.field.as_deref() != Some("key") {
                bail!("error invalid-sync-change field=key");
            }
            validate_metadata_key_payload("key", &change.payload)?;
            optional_bool_payload("conflict_resolution", &change.payload)?;
        }
        op_type::SET_TASK_METADATA | op_type::REMOVE_TASK_METADATA => {
            ensure_entity_type(change, "task")?;
            ensure_sync_id("entity_id", &change.entity_id)?;
            required_workspace_payload(&change.payload)?;
            let field_id = required_string_payload("field_id", &change.payload)?;
            ensure_metadata_field_id("field_id", &field_id)?;
            let expected_field = format!("metadata:{field_id}");
            if change.field.as_deref() != Some(expected_field.as_str()) {
                bail!("error invalid-sync-change metadata-field-mismatch");
            }
            validate_metadata_key_payload("key", &change.payload)?;
            optional_bool_payload("conflict_resolution", &change.payload)?;
            if change.op_type == op_type::SET_TASK_METADATA {
                validate_metadata_value_payload("value", &change.payload)?;
            }
        }
        op_type::CREATE_TASK => {
            ensure_entity_type(change, "task")?;
            ensure_sync_id("entity_id", &change.entity_id)?;
            optional_workspace_payload(&change.payload)?;
            required_string_payload("title", &change.payload)?;
            let project_id = required_string_payload("project_id", &change.payload)?;
            ensure_project_id("project_id", &project_id)?;
            optional_string_payload("description", &change.payload)?;
            let recurrence_task = change
                .payload
                .get("series_id")
                .and_then(Value::as_str)
                .is_some();
            if recurrence_task {
                let series_id = required_string_payload("series_id", &change.payload)?;
                series_id.parse::<RecurrenceSeriesId>().map_err(|_| {
                    anyhow::anyhow!("error invalid-sync-change series_id invalid-id")
                })?;
                validate_recurrence_task(change)?;
            } else {
                required_string_payload("project_key", &change.payload)?;
                required_string_payload("project_name", &change.payload)?;
                required_string_payload("project_prefix", &change.payload)?;
            }
            if let Some(status) = optional_string_payload("status", &change.payload)? {
                validate_sync_task_field_value(TaskField::Status, &status)?;
            }
            if let Some(priority) = optional_string_payload("priority", &change.payload)? {
                validate_sync_task_field_value(TaskField::Priority, &priority)?;
            }
            if let Some(source) = optional_string_payload("source", &change.payload)? {
                TaskSource::parse(&source)
                    .context("error invalid-sync-change invalid-task-source")?;
            }
            if let Some(available_at) = optional_string_payload("available_at", &change.payload)? {
                validate_sync_task_field_value(TaskField::AvailableAt, &available_at)?;
            }
            if let Some(due_on) = optional_string_payload("due_on", &change.payload)? {
                validate_sync_task_field_value(TaskField::DueOn, &due_on)?;
            }
            if let Some(is_epic) = optional_string_payload("is_epic", &change.payload)? {
                validate_sync_task_field_value(TaskField::IsEpic, &is_epic)?;
            }
            optional_string_array_payload("labels", &change.payload)?;
            validate_metadata_array_payload(&change.payload)?;
            optional_string_payload("created_at", &change.payload)?;
        }
        op_type::SET_FIELD | op_type::RESOLVE_FIELD => {
            ensure_entity_type(change, "task")?;
            ensure_sync_id("entity_id", &change.entity_id)?;
            optional_workspace_payload(&change.payload)?;
            let field = change
                .field
                .as_deref()
                .filter(|field| !field.trim().is_empty())
                .context("error invalid-sync-change field missing")?;
            let task_field = TaskField::parse_for_sync(field)?;
            let value = required_string_payload("value", &change.payload)?;
            validate_sync_task_field_value(task_field, &value)?;
            if task_field == TaskField::Project {
                let project_id = required_string_payload("project_id", &change.payload)?;
                ensure_project_id("project_id", &project_id)?;
                if value != project_id {
                    bail!("error invalid-sync-change project-value-mismatch");
                }
                required_string_payload("project_key", &change.payload)?;
                required_string_payload("project_name", &change.payload)?;
                required_string_payload("project_prefix", &change.payload)?;
            }
        }
        op_type::LABEL_ADD | op_type::LABEL_REMOVE => {
            ensure_entity_type(change, "task")?;
            ensure_sync_id("entity_id", &change.entity_id)?;
            optional_workspace_payload(&change.payload)?;
            required_string_payload("label", &change.payload)?;
        }
        op_type::NOTE_ADD => {
            ensure_entity_type(change, "task")?;
            ensure_sync_id("entity_id", &change.entity_id)?;
            optional_workspace_payload(&change.payload)?;
            let note_id = required_string_payload("note_id", &change.payload)?;
            ensure_sync_id("note_id", &note_id)?;
            required_string_payload("body", &change.payload)?;
            required_string_payload("created_at", &change.payload)?;
        }
        op_type::DEPENDENCY_ADD | op_type::DEPENDENCY_REMOVE => {
            ensure_entity_type(change, "task")?;
            ensure_sync_id("entity_id", &change.entity_id)?;
            required_workspace_payload(&change.payload)?;
            let depends_on_task_id =
                required_string_payload("depends_on_task_id", &change.payload)?;
            ensure_sync_id("depends_on_task_id", &depends_on_task_id)?;
            if change.entity_id == depends_on_task_id {
                bail!("error invalid-sync-change dependency-self");
            }
        }
        op_type::EPIC_LINK_ADD | op_type::EPIC_LINK_REMOVE => {
            ensure_entity_type(change, "task")?;
            ensure_sync_id("entity_id", &change.entity_id)?;
            required_workspace_payload(&change.payload)?;
            let epic_task_id = required_string_payload("epic_task_id", &change.payload)?;
            ensure_sync_id("epic_task_id", &epic_task_id)?;
            if change.entity_id == epic_task_id {
                bail!("error invalid-sync-change epic-self");
            }
            if change.op_type == op_type::EPIC_LINK_ADD {
                required_timestamp_payload("created_at", &change.payload)?;
            }
        }
        op_type::PROJECT_DELETE => {
            ensure_entity_type(change, "project")?;
            ensure_project_id("entity_id", &change.entity_id)?;
            required_workspace_payload(&change.payload)?;
            required_timestamp_payload("deleted_at", &change.payload)?;
        }
        op_type::SET_LABEL_NAME => {
            ensure_entity_type(change, "label")?;
            required_workspace_payload(&change.payload)?;
            let name = required_string_payload("name", &change.payload)?;
            if name != change.entity_id {
                bail!("error invalid-sync-change label-value-mismatch");
            }
            required_string_payload("new_name", &change.payload)?;
            required_timestamp_payload("renamed_at", &change.payload)?;
        }
        op_type::LABEL_DELETE => {
            ensure_entity_type(change, "label")?;
            required_workspace_payload(&change.payload)?;
            let name = required_string_payload("name", &change.payload)?;
            if name != change.entity_id {
                bail!("error invalid-sync-change label-value-mismatch");
            }
            required_timestamp_payload("deleted_at", &change.payload)?;
        }
        op_type::NOTE_EDIT => {
            ensure_entity_type(change, "task")?;
            ensure_sync_id("entity_id", &change.entity_id)?;
            if change.field.as_deref() != Some("notes") {
                bail!("error invalid-sync-change field=notes");
            }
            required_workspace_payload(&change.payload)?;
            let note_id = required_string_payload("note_id", &change.payload)?;
            ensure_sync_id("note_id", &note_id)?;
            required_string_payload("body", &change.payload)?;
            required_timestamp_payload("edited_at", &change.payload)?;
        }
        op_type::LABEL_RESTORE => {
            ensure_entity_type(change, "label")?;
            required_workspace_payload(&change.payload)?;
            let name = required_string_payload("name", &change.payload)?;
            if name != change.entity_id {
                bail!("error invalid-sync-change label-value-mismatch");
            }
            required_timestamp_payload("created_at", &change.payload)?;
            required_timestamp_payload("restored_at", &change.payload)?;
            for key in ["task_ids", "series_ids"] {
                optional_string_array_payload(key, &change.payload)?;
                if change.payload.get(key).is_none() {
                    bail!("error invalid-sync-change payload.{key} missing");
                }
            }
            for task_id in string_array_payload("task_ids", &change.payload)? {
                ensure_sync_id("task_ids", &task_id)?;
            }
            for series_id in string_array_payload("series_ids", &change.payload)? {
                ensure_sync_id("series_ids", &series_id)?;
            }
        }
        op_type::NOTE_DELETE => {
            ensure_entity_type(change, "task")?;
            ensure_sync_id("entity_id", &change.entity_id)?;
            if change.field.as_deref() != Some("notes") {
                bail!("error invalid-sync-change field=notes");
            }
            required_workspace_payload(&change.payload)?;
            let note_id = required_string_payload("note_id", &change.payload)?;
            ensure_sync_id("note_id", &note_id)?;
            required_timestamp_payload("deleted_at", &change.payload)?;
        }
        op_type::SET_RECURRENCE_METADATA | op_type::REMOVE_RECURRENCE_METADATA => {
            ensure_entity_type(change, "recurrence_series")?;
            change
                .entity_id
                .parse::<RecurrenceSeriesId>()
                .map_err(|_| anyhow::anyhow!("error invalid-sync-change entity_id invalid-id"))?;
            required_workspace_payload(&change.payload)?;
            let field_id = required_string_payload("field_id", &change.payload)?;
            ensure_metadata_field_id("field_id", &field_id)?;
            let expected_field = format!("metadata:{field_id}");
            if change.field.as_deref() != Some(expected_field.as_str()) {
                bail!("error invalid-sync-change metadata-field-mismatch");
            }
            validate_metadata_key_payload("key", &change.payload)?;
            optional_bool_payload("conflict_resolution", &change.payload)?;
            if change.op_type == op_type::SET_RECURRENCE_METADATA {
                validate_metadata_value_payload("value", &change.payload)?;
            }
        }
        op_type::CREATE_RECURRENCE_SERIES => validate_recurrence_create(change)?,
        op_type::UPDATE_RECURRENCE_TEMPLATE => validate_recurrence_template(change)?,
        op_type::PROJECT_RECURRENCE_OCCURRENCE => validate_recurrence_projection(change)?,
        op_type::RESOLVE_RECURRENCE_OCCURRENCE => validate_recurrence_outcome(change)?,
        op_type::SET_RECURRENCE_STATE | op_type::STOP_RECURRENCE_SERIES => {
            validate_recurrence_state(change)?
        }
        op_type::OPEN_RECURRENCE_PAUSE => validate_recurrence_pause(change, true)?,
        op_type::CLOSE_RECURRENCE_PAUSE => validate_recurrence_pause(change, false)?,
        op_type::ATTACHMENT_ADD => validate_attachment_add_change(change)?,
        op_type::ATTACHMENT_DELETE => validate_attachment_delete_change(change)?,
        _ => bail!("error invalid-sync-change op_type={}", change.op_type),
    }
    Ok(())
}

fn validate_recurrence_task(change: &ChangeWire) -> Result<()> {
    let workspace_id: WorkspaceId = required_string_payload("workspace_id", &change.payload)?
        .parse()
        .map_err(|_| anyhow::anyhow!("error invalid-sync-change workspace_id invalid-id"))?;
    let series_id: RecurrenceSeriesId = required_string_payload("series_id", &change.payload)?
        .parse()
        .map_err(|_| anyhow::anyhow!("error invalid-sync-change series_id invalid-id"))?;
    let slot_on = recurrence_date_payload("slot_on", &change.payload)?;
    let schedule = recurrence_schedule_payload(&change.payload)?;
    if !is_slot(&schedule.rule, schedule.start_on, slot_on) {
        bail!("error invalid-sync-change recurrence-slot-off-lattice");
    }
    let identity = derive_occurrence_identity(&workspace_id, &series_id, &schedule, slot_on)
        .map_err(|err| anyhow::anyhow!("error invalid-sync-change {err}"))?;
    let slot = crate::recurrence::slot_values(&schedule, slot_on)
        .map_err(|err| anyhow::anyhow!("error invalid-sync-change {err}"))?;
    for (key, expected) in [
        ("task_id", identity.task_id.as_str()),
        ("created_at", identity.created_at.as_str()),
        ("updated_at", identity.updated_at.as_str()),
        ("available_at", slot.available_at.as_str()),
        ("due_on", slot.due_on.as_deref().unwrap_or("")),
        ("task_change_id", identity.task_change_id.as_str()),
        (
            "occurrence_change_id",
            identity.occurrence_change_id.as_str(),
        ),
        (
            "task_field_version_seed",
            identity.field_version_seeds.task.as_str(),
        ),
        (
            "occurrence_field_version_seed",
            identity.field_version_seeds.occurrence.as_str(),
        ),
    ] {
        if required_string_payload(key, &change.payload)? != expected {
            bail!("error invalid-sync-change recurrence-deterministic-mismatch field={key}");
        }
    }
    if change.entity_id != identity.task_id.as_str()
        || change.change_id != identity.task_change_id
        || change.created_at != identity.created_at
    {
        bail!("error invalid-sync-change recurrence-deterministic-mismatch field=change_identity");
    }
    Ok(())
}

fn validate_recurrence_create(change: &ChangeWire) -> Result<()> {
    ensure_entity_type(change, "recurrence_series")?;
    let series_id: RecurrenceSeriesId = change
        .entity_id
        .parse()
        .map_err(|_| anyhow::anyhow!("error invalid-sync-change entity_id invalid-id"))?;
    required_workspace_payload(&change.payload)?;
    if required_string_payload("series_id", &change.payload)? != series_id.as_str() {
        bail!("error invalid-sync-change recurrence-series-id-mismatch");
    }
    required_string_payload("title", &change.payload)?;
    required_string_payload("description", &change.payload)?;
    let project_id = required_string_payload("project_id", &change.payload)?;
    ensure_project_id("project_id", &project_id)?;
    required_string_payload("project_key", &change.payload)?;
    required_string_payload("project_name", &change.payload)?;
    required_string_payload("project_prefix", &change.payload)?;
    validate_sync_task_field_value(
        TaskField::Priority,
        &required_string_payload("priority", &change.payload)?,
    )?;
    let status = required_string_payload("initial_status", &change.payload)?;
    validate_sync_task_field_value(TaskField::Status, &status)?;
    if matches!(status.as_str(), "done" | "canceled") {
        bail!("error invalid-sync-change recurrence-terminal-template");
    }
    recurrence_schedule_payload(&change.payload)?;
    optional_string_array_payload("labels", &change.payload)?;
    validate_metadata_array_payload(&change.payload)?;
    if required_string_payload("state", &change.payload)? != "active"
        || !required_string_payload("stopped_at", &change.payload)?.is_empty()
    {
        bail!("error invalid-sync-change recurrence-create-state");
    }
    required_timestamp_payload("created_at", &change.payload)?;
    required_timestamp_payload("updated_at", &change.payload)?;
    Ok(())
}

fn validate_recurrence_template(change: &ChangeWire) -> Result<()> {
    validate_recurrence_entity(change)?;
    let fields = change
        .payload
        .get("fields")
        .and_then(Value::as_array)
        .context("error invalid-sync-change payload.fields missing")?;
    if fields.len() > 8 {
        bail!("error invalid-sync-change recurrence-template-fields-too-large");
    }
    let base_versions = change
        .payload
        .get("base_versions")
        .and_then(Value::as_object)
        .context("error invalid-sync-change payload.base_versions missing")?;
    let mut seen = HashSet::new();
    for pair in fields {
        let pair = pair
            .as_array()
            .filter(|pair| pair.len() == 2)
            .context("error invalid-sync-change recurrence-template-field-pair")?;
        let field = pair[0]
            .as_str()
            .context("error invalid-sync-change recurrence-template-field")?;
        let value = pair[1]
            .as_str()
            .context("error invalid-sync-change recurrence-template-value")?;
        if !seen.insert(field) {
            bail!("error invalid-sync-change recurrence-template-duplicate-field");
        }
        if !matches!(
            base_versions.get(field),
            Some(Value::String(_)) | Some(Value::Null)
        ) {
            bail!("error invalid-sync-change recurrence-template-base-version field={field}");
        }
        match field {
            "title" | "description" => {}
            "project" => ensure_project_id("project_id", value)?,
            "priority" => validate_sync_task_field_value(TaskField::Priority, value)?,
            "initial_status" => {
                validate_sync_task_field_value(TaskField::Status, value)?;
                if matches!(value, "done" | "canceled") {
                    bail!("error invalid-sync-change recurrence-terminal-template");
                }
            }
            "available_local_time" => {
                validate_local_time(value)?;
            }
            "due_policy" => {
                RecurrenceDuePolicy::parse(value)
                    .map_err(|err| anyhow::anyhow!("error invalid-sync-change {err}"))?;
            }
            _ => bail!("error invalid-sync-change recurrence-template-field={field}"),
        }
    }
    optional_string_array_payload("labels", &change.payload)?;
    let labels_changed = change
        .payload
        .get("labels_changed")
        .and_then(Value::as_bool)
        .context("error invalid-sync-change payload.labels_changed missing")?;
    if labels_changed
        && !matches!(
            base_versions.get("labels"),
            Some(Value::String(_)) | Some(Value::Null)
        )
    {
        bail!("error invalid-sync-change recurrence-template-base-version field=labels");
    }
    required_timestamp_payload("updated_at", &change.payload)?;
    Ok(())
}

fn validate_recurrence_projection(change: &ChangeWire) -> Result<()> {
    validate_recurrence_entity(change)?;
    if change.field.as_deref() != Some("projection") {
        bail!("error invalid-sync-change field=projection");
    }
    let workspace_id: WorkspaceId = required_string_payload("workspace_id", &change.payload)?
        .parse()
        .map_err(|_| anyhow::anyhow!("error invalid-sync-change workspace_id invalid-id"))?;
    let series_id: RecurrenceSeriesId = change.entity_id.parse().unwrap();
    let slot_on = recurrence_date_payload("slot_on", &change.payload)?;
    let schedule = recurrence_schedule_payload(&change.payload)?;
    if !is_slot(&schedule.rule, schedule.start_on, slot_on) {
        bail!("error invalid-sync-change recurrence-slot-off-lattice");
    }
    let identity = derive_occurrence_identity(&workspace_id, &series_id, &schedule, slot_on)
        .map_err(|err| anyhow::anyhow!("error invalid-sync-change {err}"))?;
    for (key, expected) in [
        ("task_id", identity.task_id.as_str()),
        (
            "projected_at",
            identity.occurrence_link.projected_at.as_str(),
        ),
        ("task_change_id", identity.task_change_id.as_str()),
        (
            "occurrence_change_id",
            identity.occurrence_change_id.as_str(),
        ),
        (
            "task_field_version_seed",
            identity.field_version_seeds.task.as_str(),
        ),
        (
            "occurrence_field_version_seed",
            identity.field_version_seeds.occurrence.as_str(),
        ),
    ] {
        if required_string_payload(key, &change.payload)? != expected {
            bail!("error invalid-sync-change recurrence-deterministic-mismatch field={key}");
        }
    }
    if change.change_id != identity.occurrence_change_id
        || change.created_at != identity.occurrence_link.projected_at
    {
        bail!("error invalid-sync-change recurrence-deterministic-mismatch field=change_id");
    }
    Ok(())
}

fn validate_recurrence_outcome(change: &ChangeWire) -> Result<()> {
    validate_recurrence_entity(change)?;
    validate_timestamp_value("created_at", &change.created_at)?;
    if change.field.as_deref() != Some("outcome") {
        bail!("error invalid-sync-change field=outcome");
    }
    let slot_on = recurrence_date_payload("slot_on", &change.payload)?;
    let schedule = recurrence_schedule_payload(&change.payload)?;
    if !is_slot(&schedule.rule, schedule.start_on, slot_on) {
        bail!("error invalid-sync-change recurrence-slot-off-lattice");
    }
    let outcome = RecurrenceOutcome::parse(&required_string_payload("outcome", &change.payload)?)
        .map_err(|err| anyhow::anyhow!("error invalid-sync-change {err}"))?;
    let resolved_at = required_timestamp_payload("resolved_at", &change.payload)?;
    if resolved_at < crate::recurrence::slot_values(&schedule, slot_on)?.boundary_at {
        bail!("error invalid-sync-change recurrence-resolution-before-slot");
    }
    let conflict_resolution = change
        .payload
        .get("conflict_resolution")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let task_id = required_string_payload("task_id", &change.payload)?;
    ensure_sync_id("task_id", &task_id)?;
    let workspace_id: WorkspaceId =
        required_string_payload("workspace_id", &change.payload)?.parse()?;
    let series_id: RecurrenceSeriesId = change.entity_id.parse().unwrap();
    let identity = derive_occurrence_identity(&workspace_id, &series_id, &schedule, slot_on)?;
    if task_id != identity.task_id.as_str() {
        bail!("error invalid-sync-change recurrence-deterministic-mismatch field=task_id");
    }
    let expected_status = match outcome {
        RecurrenceOutcome::Completed => "done",
        RecurrenceOutcome::Skipped => "canceled",
    };
    if required_string_payload("task_status", &change.payload)? != expected_status {
        bail!("error invalid-sync-change recurrence-outcome-status-mismatch");
    }
    let status_change_id = required_string_payload("task_status_change_id", &change.payload)?;
    if !conflict_resolution || !status_change_id.is_empty() {
        ensure_sync_id("task_status_change_id", &status_change_id)?;
    }
    let successor = required_string_payload("successor_task_id", &change.payload)?;
    if !successor.is_empty() {
        ensure_sync_id("successor_task_id", &successor)?;
        let successor_slot = next_slot_after(&schedule.rule, schedule.start_on, slot_on)
            .context("error invalid-sync-change recurrence-successor-out-of-range")?;
        let successor_identity =
            derive_occurrence_identity(&workspace_id, &series_id, &schedule, successor_slot)?;
        if successor != successor_identity.task_id.as_str() {
            bail!(
                "error invalid-sync-change recurrence-deterministic-mismatch field=successor_task_id"
            );
        }
    }
    Ok(())
}

fn validate_recurrence_state(change: &ChangeWire) -> Result<()> {
    validate_recurrence_entity(change)?;
    if change.field.as_deref() != Some("state") {
        bail!("error invalid-sync-change field=state");
    }
    let conflict_resolution = change
        .payload
        .get("conflict_resolution")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if change.base_version.is_none() && !conflict_resolution {
        bail!("error invalid-sync-change recurrence-lifecycle-base-version-missing");
    }
    let state = RecurrenceSeriesState::parse(&required_string_payload("state", &change.payload)?)
        .map_err(|err| anyhow::anyhow!("error invalid-sync-change {err}"))?;
    let stopped_at = required_string_payload("stopped_at", &change.payload)?;
    if matches!(state, RecurrenceSeriesState::Stopped) {
        required_timestamp_payload("stopped_at", &change.payload)?;
    } else if !stopped_at.is_empty() {
        bail!("error invalid-sync-change recurrence-stop-state-mismatch");
    }
    required_timestamp_payload("changed_at", &change.payload)?;
    Ok(())
}

fn validate_recurrence_pause(change: &ChangeWire, open: bool) -> Result<()> {
    validate_recurrence_entity(change)?;
    if change.field.as_deref() != Some("pause") {
        bail!("error invalid-sync-change field=pause");
    }
    if open {
        let interval_id = required_string_payload("interval_id", &change.payload)?;
        ensure_sync_id("interval_id", &interval_id)?;
        required_timestamp_payload("paused_at", &change.payload)?;
        let slot = required_string_payload("suspended_slot_on", &change.payload)?;
        if !slot.is_empty() {
            slot.parse::<NaiveDate>()
                .context("error invalid-sync-change suspended_slot_on")?;
            let task_id = required_string_payload("suspended_task_id", &change.payload)?;
            ensure_sync_id("suspended_task_id", &task_id)?;
        }
    } else {
        required_timestamp_payload("resumed_at", &change.payload)?;
        if let Some(interval_id) = optional_string_payload("interval_id", &change.payload)? {
            ensure_sync_id("interval_id", &interval_id)?;
        }
    }
    Ok(())
}

fn validate_recurrence_entity(change: &ChangeWire) -> Result<()> {
    ensure_entity_type(change, "recurrence_series")?;
    change
        .entity_id
        .parse::<RecurrenceSeriesId>()
        .map_err(|_| anyhow::anyhow!("error invalid-sync-change entity_id invalid-id"))?;
    required_workspace_payload(&change.payload)?;
    Ok(())
}

fn recurrence_schedule_payload(payload: &Value) -> Result<RecurrenceSchedule> {
    let frequency = RecurrenceFrequency::parse(&required_string_payload("frequency", payload)?)
        .map_err(|err| anyhow::anyhow!("error invalid-sync-change {err}"))?;
    let interval = required_i64_payload("interval", payload)?;
    let interval = u32::try_from(interval)
        .context("error invalid-sync-change recurrence-interval-out-of-range")?;
    let weekdays_text = required_string_payload("weekdays", payload)?;
    let weekdays = weekdays_text
        .parse::<WeekdaySet>()
        .map_err(|err| anyhow::anyhow!("error invalid-sync-change {err}"))?;
    if weekdays.to_string() != weekdays_text {
        bail!("error invalid-sync-change recurrence-weekdays-noncanonical");
    }
    let rule = RecurrenceRule::new(frequency, interval, weekdays)
        .map_err(|err| anyhow::anyhow!("error invalid-sync-change {err}"))?;
    let timezone_text = required_string_payload("timezone", payload)?;
    let timezone = timezone_text
        .parse::<TimeZoneId>()
        .map_err(|err| anyhow::anyhow!("error invalid-sync-change {err}"))?;
    if timezone_text.parse::<chrono_tz::Tz>()?.to_string() != timezone_text {
        bail!("error invalid-sync-change recurrence-timezone-noncanonical");
    }
    let start_on = recurrence_date_payload("start_on", payload)?;
    let available = required_string_payload("available_local_time", payload)?;
    let available_local_time = if available.is_empty() {
        None
    } else {
        Some(validate_local_time(&available)?)
    };
    let due_policy = RecurrenceDuePolicy::parse(&required_string_payload("due_policy", payload)?)
        .map_err(|err| anyhow::anyhow!("error invalid-sync-change {err}"))?;
    Ok(RecurrenceSchedule::new(
        rule,
        timezone,
        start_on,
        available_local_time,
        due_policy,
    ))
}

fn recurrence_date_payload(key: &str, payload: &Value) -> Result<NaiveDate> {
    required_string_payload(key, payload)?
        .parse()
        .with_context(|| format!("error invalid-sync-change payload.{key} invalid-date"))
}

fn validate_local_time(value: &str) -> Result<NaiveTime> {
    NaiveTime::parse_from_str(value, "%H:%M:%S")
        .context("error invalid-sync-change recurrence-local-time")
}

fn validate_attachment_add_change(change: &ChangeWire) -> Result<()> {
    ensure_entity_type(change, "task")?;
    ensure_sync_id("entity_id", &change.entity_id)?;
    ensure_attachment_field(change)?;
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
    Ok(())
}

fn validate_attachment_delete_change(change: &ChangeWire) -> Result<()> {
    ensure_entity_type(change, "task")?;
    ensure_sync_id("entity_id", &change.entity_id)?;
    ensure_attachment_field(change)?;
    required_workspace_payload(&change.payload)?;
    let attachment_id = required_string_payload("attachment_id", &change.payload)?;
    ensure_sync_id("attachment_id", &attachment_id)?;
    required_timestamp_payload("deleted_at", &change.payload)?;
    Ok(())
}

fn ensure_attachment_field(change: &ChangeWire) -> Result<()> {
    if change.field.as_deref() != Some("attachments") {
        bail!("error invalid-sync-change field=attachments");
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

fn validate_push_acks(request_change_ids: &[String], response: &SyncResponse) -> Result<()> {
    if response.push_acks.len() != request_change_ids.len() {
        bail!(
            "error invalid-sync-response push-ack-count expected={} got={}",
            request_change_ids.len(),
            response.push_acks.len()
        );
    }
    let expected = request_change_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let mut seen = HashSet::with_capacity(response.push_acks.len());
    for ack in &response.push_acks {
        if !expected.contains(ack.change_id.as_str()) {
            bail!(
                "error invalid-sync-response unexpected-push-ack change_id={}",
                ack.change_id
            );
        }
        if ack.server_seq <= 0 {
            bail!(
                "error invalid-sync-response push-ack-server-seq change_id={} server_seq={}",
                ack.change_id,
                ack.server_seq
            );
        }
        if !seen.insert(ack.change_id.as_str()) {
            bail!(
                "error invalid-sync-response duplicate-push-ack change_id={}",
                ack.change_id
            );
        }
    }
    Ok(())
}

fn validate_pull_page(after: i64, pull_limit: u32, response: &SyncResponse) -> Result<()> {
    let mut previous = after;
    let mut change_ids = HashSet::with_capacity(response.changes.len());
    for change in &response.changes {
        validate_pulled_change(change)?;
        if !change_ids.insert(&change.change_id) {
            bail!(
                "error invalid-sync-response duplicate-pull-change change_id={}",
                change.change_id
            );
        }
        let server_seq = change.server_seq.with_context(|| {
            format!(
                "error invalid-sync-response missing-server-seq change_id={}",
                change.change_id
            )
        })?;
        if server_seq <= previous {
            bail!(
                "error invalid-sync-response server-seq-order previous={} server_seq={}",
                previous,
                server_seq
            );
        }
        previous = server_seq;
    }
    let expected_cursor = response
        .changes
        .last()
        .and_then(|change| change.server_seq)
        .unwrap_or(after);
    if response.cursor != expected_cursor {
        bail!(
            "error invalid-sync-response cursor-mismatch expected={} got={}",
            expected_cursor,
            response.cursor
        );
    }
    if response.has_more && response.changes.len() < pull_limit as usize {
        bail!(
            "error invalid-sync-response has-more-short-page returned={} limit={}",
            response.changes.len(),
            pull_limit
        );
    }
    Ok(())
}

fn validate_push_pull_overlap(response: &SyncResponse) -> Result<()> {
    let acked = response
        .push_acks
        .iter()
        .map(|ack| (ack.change_id.as_str(), ack.server_seq))
        .collect::<HashMap<_, _>>();
    for change in &response.changes {
        if let Some(acked_server_seq) = acked.get(change.change_id.as_str()) {
            let Some(pull_server_seq) = change.server_seq else {
                continue;
            };
            if *acked_server_seq != pull_server_seq {
                bail!(
                    "error invalid-sync-response push-pull-server-seq-mismatch change_id={} ack={} pull={}",
                    change.change_id,
                    acked_server_seq,
                    pull_server_seq
                );
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change_log::{ChangePayload, op_type};
    use crate::workspaces::Workspace;

    fn test_workspace() -> Workspace {
        Workspace {
            id: "0000000000000000".parse().unwrap(),
            key: "default".to_string(),
            name: "default".to_string(),
        }
    }

    fn make_change_wire(
        op_type: &str,
        entity_type: &str,
        entity_id: &str,
        payload: serde_json::Value,
    ) -> ChangeWire {
        ChangeWire {
            change_id: "AAAAAAAAAAAAAAA0".to_string(),
            client_id: "client".to_string(),
            local_seq: 1,
            entity_type: entity_type.to_string(),
            entity_id: entity_id.to_string(),
            field: None,
            op_type: op_type.to_string(),
            payload,
            base_version: None,
            created_at: "2026-06-01T00:00:00Z".to_string(),
            server_seq: None,
        }
    }

    #[test]
    fn sync_timestamps_accept_fractional_utc_precision() {
        validate_timestamp_value("created_at", "2026-07-28T06:41:44Z").unwrap();
        validate_timestamp_value("created_at", "2026-07-28T06:41:44.000000Z").unwrap();
        validate_timestamp_value("created_at", "2026-07-28T06:41:44.123456789Z").unwrap();
    }

    #[test]
    fn sync_timestamps_reject_offsets_and_invalid_calendar_values() {
        for value in ["2026-07-28T08:41:44+02:00", "2026-13-28T06:41:44Z", "today"] {
            assert!(validate_timestamp_value("created_at", value).is_err());
        }
    }

    #[test]
    fn recurrence_projection_rejects_nondeterministic_change_timestamp() {
        let workspace = test_workspace();
        let series_id: RecurrenceSeriesId = "AAAAAAAAAAAAAAAA".parse().unwrap();
        let schedule = RecurrenceSchedule::new(
            RecurrenceRule::daily(),
            "UTC".parse().unwrap(),
            "2026-07-20".parse().unwrap(),
            None,
            RecurrenceDuePolicy::SameDay,
        );
        let slot_on: NaiveDate = "2026-07-20".parse().unwrap();
        let identity =
            derive_occurrence_identity(&workspace.id, &series_id, &schedule, slot_on).unwrap();
        let payload = ChangePayload::workspace(&workspace)
            .set("series_id", series_id.as_str())
            .set("slot_on", slot_on.to_string())
            .set("task_id", identity.task_id.as_str())
            .set("projected_at", &identity.occurrence_link.projected_at)
            .set("task_change_id", &identity.task_change_id)
            .set("occurrence_change_id", &identity.occurrence_change_id)
            .set(
                "task_field_version_seed",
                &identity.field_version_seeds.task,
            )
            .set(
                "occurrence_field_version_seed",
                &identity.field_version_seeds.occurrence,
            )
            .set("frequency", "daily")
            .set("interval", 1)
            .set("weekdays", "")
            .set("timezone", "UTC")
            .set("start_on", "2026-07-20")
            .set("available_local_time", "")
            .set("due_policy", "same_day")
            .into_value();
        let mut change = make_change_wire(
            op_type::PROJECT_RECURRENCE_OCCURRENCE,
            "recurrence_series",
            series_id.as_str(),
            payload,
        );
        change.change_id = identity.occurrence_change_id;
        change.field = Some("projection".to_string());
        change.created_at = identity.occurrence_link.projected_at;
        validate_pushed_change(&change).unwrap();

        change.created_at = "2026-07-20T00:00:01Z".to_string();
        assert!(
            validate_pushed_change(&change)
                .unwrap_err()
                .to_string()
                .contains("recurrence-deterministic-mismatch")
        );
    }

    #[test]
    fn request_pull_limit_has_default_and_bounds() {
        assert_eq!(request_pull_limit(None).unwrap(), MAX_PULL_BATCH);
        assert!(request_pull_limit(Some(MAX_PULL_BATCH)).is_ok());
        assert_eq!(
            request_pull_limit(Some(0)).unwrap_err().to_string(),
            "error sync-pull-limit-out-of-range min=1 max=512 got=0"
        );
        assert_eq!(
            request_pull_limit(Some(MAX_PULL_BATCH + 1))
                .unwrap_err()
                .to_string(),
            "error sync-pull-limit-out-of-range min=1 max=512 got=513"
        );
    }

    #[test]
    fn request_envelope_rejects_negative_cursor_and_oversized_push_batch() {
        let request = SyncRequest {
            protocol_version: Some(SYNC_PROTOCOL_VERSION),
            client_id: "test-client".to_string(),
            after: -1,
            pull_limit: Some(MAX_PULL_BATCH),
            changes: Vec::new(),
        };
        assert_eq!(
            validate_sync_request_envelope(&request)
                .unwrap_err()
                .to_string(),
            "error sync-after-out-of-range min=0 got=-1"
        );

        let request = SyncRequest {
            protocol_version: Some(SYNC_PROTOCOL_VERSION),
            client_id: "test-client".to_string(),
            after: 0,
            pull_limit: Some(MAX_PULL_BATCH),
            changes: vec![
                ChangeWire {
                    change_id: "AAAAAAAAAAAAAAA0".to_string(),
                    client_id: "client".to_string(),
                    local_seq: 1,
                    entity_type: "task".to_string(),
                    entity_id: "BBBBBBBBBBBBBBBB".to_string(),
                    field: None,
                    op_type: "create_task".to_string(),
                    payload: serde_json::json!({"title":"oops","project_id":"0000000000000000","project_key":"app","project_name":"app","project_prefix":"APP","workspace_id":"0000000000000000","workspace_key":"default","created_at":"2026-01-01T00:00:00Z"}),
                    base_version: None,
                    created_at: "2026-01-01T00:00:00Z".to_string(),
                    server_seq: None,
                };
                MAX_PUSH_BATCH + 1
            ],
        };
        assert_eq!(
            validate_sync_request_envelope(&request)
                .unwrap_err()
                .to_string(),
            "error sync-push-too-large limit=256 got=257"
        );
    }

    #[test]
    fn response_validation_respects_request_pull_limit() {
        let response = SyncResponse {
            protocol_version: SYNC_PROTOCOL_VERSION,
            cursor: 1,
            has_more: false,
            push_acks: vec![],
            changes: vec![
                ChangeWire {
                    change_id: "AAAAAAAAAAAAAAA1".to_string(),
                    client_id: "client".to_string(),
                    local_seq: 1,
                    entity_type: "task".to_string(),
                    entity_id: "BBBBBBBBBBBBBBBB".to_string(),
                    field: None,
                    op_type: "create_task".to_string(),
                    payload: serde_json::json!({
                        "title":"one",
                        "project_id":"0000000000000000",
                        "project_key":"app",
                        "project_name":"app",
                        "project_prefix":"APP",
                    }),
                    base_version: None,
                    created_at: "2026-01-01T00:00:00Z".to_string(),
                    server_seq: Some(1),
                };
                MAX_PULL_BATCH as usize + 1
            ],
        };
        assert_eq!(
            validate_sync_response_for_request(0, MAX_PULL_BATCH, &[], &response)
                .unwrap_err()
                .to_string(),
            "error invalid-sync-response pull-too-large limit=512 got=513"
        );
    }

    #[test]
    fn constructed_create_task_payload_passes_wire_validation() {
        let ws = test_workspace();
        let payload = ChangePayload::workspace(&ws)
            .set("title", "test task")
            .set("description", "a description")
            .set("project_id", "1111111111111111")
            .set("project_key", "app")
            .set("project_name", "App")
            .set("project_prefix", "APP")
            .set("status", "inbox")
            .set("priority", "none")
            .set("created_at", "2026-06-01T00:00:00Z")
            .into_value();
        let change = make_change_wire(op_type::CREATE_TASK, "task", "BBBBBBBBBBBBBBBB", payload);
        validate_pushed_change(&change)
            .expect("create_task payload built with ChangePayload should be wire-valid");
    }

    #[test]
    fn constructed_create_project_payload_passes_wire_validation() {
        let ws = test_workspace();
        let payload = ChangePayload::workspace(&ws)
            .set("key", "app")
            .set("name", "App")
            .set("prefix", "APP")
            .set("created_at", "2026-06-01T00:00:00Z")
            .into_value();
        let change = make_change_wire(
            op_type::CREATE_PROJECT,
            "project",
            "1111111111111111",
            payload,
        );
        validate_pushed_change(&change)
            .expect("create_project payload built with ChangePayload should be wire-valid");
    }

    #[test]
    fn project_changes_reject_invalid_project_ids() {
        let ws = test_workspace();
        let project_payload = ChangePayload::workspace(&ws)
            .set("key", "app")
            .set("name", "App")
            .set("prefix", "APP")
            .set("created_at", "2026-06-01T00:00:00Z")
            .into_value();
        let create_project = make_change_wire(
            op_type::CREATE_PROJECT,
            "project",
            "invalid",
            project_payload,
        );
        assert_eq!(
            validate_pushed_change(&create_project)
                .unwrap_err()
                .to_string(),
            "error invalid-sync-change entity_id invalid-id"
        );

        let task_payload = ChangePayload::workspace(&ws)
            .set("title", "test task")
            .set("project_id", "invalid")
            .set("project_key", "app")
            .set("project_name", "App")
            .set("project_prefix", "APP")
            .set("created_at", "2026-06-01T00:00:00Z")
            .into_value();
        let create_task = make_change_wire(
            op_type::CREATE_TASK,
            "task",
            "BBBBBBBBBBBBBBBB",
            task_payload,
        );
        assert_eq!(
            validate_pushed_change(&create_task)
                .unwrap_err()
                .to_string(),
            "error invalid-sync-change project_id invalid-id"
        );

        let field_payload = ChangePayload::workspace(&ws)
            .set("value", "invalid")
            .set("project_id", "invalid")
            .set("project_key", "app")
            .set("project_name", "App")
            .set("project_prefix", "APP")
            .into_value();
        let mut set_field = make_change_wire(
            op_type::SET_FIELD,
            "task",
            "BBBBBBBBBBBBBBBB",
            field_payload,
        );
        set_field.field = Some("project".to_string());
        assert_eq!(
            validate_pushed_change(&set_field).unwrap_err().to_string(),
            "error invalid-sync-change project_id invalid-id"
        );
    }

    #[test]
    fn constructed_label_add_payload_passes_wire_validation() {
        let ws = test_workspace();
        let payload = ChangePayload::workspace(&ws)
            .set("label", "bug")
            .into_value();
        let mut change = make_change_wire(op_type::LABEL_ADD, "task", "BBBBBBBBBBBBBBBB", payload);
        change.field = Some("labels".to_string());
        validate_pushed_change(&change)
            .expect("label_add payload built with ChangePayload should be wire-valid");
    }

    #[test]
    fn constructed_label_administration_payloads_pass_wire_validation() {
        let ws = test_workspace();
        let rename = make_change_wire(
            op_type::SET_LABEL_NAME,
            "label",
            "old",
            ChangePayload::workspace(&ws)
                .set("name", "old")
                .set("new_name", "new")
                .set("renamed_at", "2026-06-01T00:00:00Z")
                .into_value(),
        );
        validate_pushed_change(&rename).unwrap();

        let restore = make_change_wire(
            op_type::LABEL_RESTORE,
            "label",
            "new",
            ChangePayload::workspace(&ws)
                .set("name", "new")
                .set("created_at", "2026-06-01T00:00:00Z")
                .set("task_ids", ["BBBBBBBBBBBBBBBB"])
                .set("series_ids", ["CCCCCCCCCCCCCCCC"])
                .set("restored_at", "2026-06-01T00:00:01Z")
                .into_value(),
        );
        validate_pushed_change(&restore).unwrap();
    }

    #[test]
    fn constructed_dependency_add_payload_passes_wire_validation() {
        let ws = test_workspace();
        let payload = ChangePayload::workspace(&ws)
            .set("depends_on_task_id", "CCCCCCCCCCCCCCCC")
            .into_value();
        let mut change =
            make_change_wire(op_type::DEPENDENCY_ADD, "task", "BBBBBBBBBBBBBBBB", payload);
        change.field = Some("dependencies".to_string());
        validate_pushed_change(&change)
            .expect("dependency_add payload built with ChangePayload should be wire-valid");
    }

    #[test]
    fn constructed_note_edit_payload_passes_wire_validation() {
        let ws = test_workspace();
        let payload = ChangePayload::workspace(&ws)
            .set("note_id", "DDDDDDDDDDDDDDDD")
            .set("body", "corrected note body")
            .set("edited_at", "2026-06-01T01:00:00Z")
            .into_value();
        let mut change = make_change_wire(op_type::NOTE_EDIT, "task", "BBBBBBBBBBBBBBBB", payload);
        change.field = Some("notes".to_string());
        validate_pushed_change(&change)
            .expect("note_edit payload built with ChangePayload should be wire-valid");
    }

    #[test]
    fn constructed_note_add_payload_passes_wire_validation() {
        let ws = test_workspace();
        let payload = ChangePayload::workspace(&ws)
            .set("note_id", "DDDDDDDDDDDDDDDD")
            .set("body", "note body")
            .set("created_at", "2026-06-01T00:00:00Z")
            .into_value();
        let mut change = make_change_wire(op_type::NOTE_ADD, "task", "BBBBBBBBBBBBBBBB", payload);
        change.field = Some("notes".to_string());
        validate_pushed_change(&change)
            .expect("note_add payload built with ChangePayload should be wire-valid");
    }
}
