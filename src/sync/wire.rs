use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ids::BASE32;
use crate::task_fields::TaskField;

pub(crate) const SYNC_PROTOCOL_VERSION: u32 = 5;
pub(crate) fn sync_server_url_is_valid(server: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(server) else {
        return false;
    };
    matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
}
pub(crate) const MAX_PUSH_BATCH: usize = 256;
pub(crate) const MAX_PULL_BATCH: u32 = 512;
pub(crate) const DAEMON_SYNC_PAGE_BUDGET: usize = 8;
pub(crate) const DAEMON_INCOMPLETE_RESCHEDULE_MS: u64 = 100;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ChangeWire {
    pub(crate) change_id: String,
    pub(crate) client_id: String,
    pub(crate) local_seq: i64,
    pub(crate) entity_type: String,
    pub(crate) entity_id: String,
    pub(crate) field: Option<String>,
    pub(crate) op_type: String,
    pub(crate) payload: Value,
    pub(crate) base_version: Option<String>,
    pub(crate) created_at: String,
    pub(crate) server_seq: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PushAck {
    pub(crate) change_id: String,
    pub(crate) server_seq: i64,
}

#[derive(Debug)]
pub(super) struct ChangeRow {
    pub(super) change_id: String,
    pub(super) client_id: String,
    pub(super) local_seq: i64,
    pub(super) entity_type: String,
    pub(super) entity_id: String,
    pub(super) field: Option<String>,
    pub(super) op_type: String,
    pub(super) payload: String,
    pub(super) base_version: Option<String>,
    pub(super) created_at: String,
    pub(super) server_seq: Option<i64>,
}

impl ChangeRow {
    pub(super) fn into_wire(self) -> ChangeWire {
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

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct SyncRequest {
    #[serde(default)]
    pub(super) protocol_version: Option<u32>,
    pub(super) client_id: String,
    pub(super) after: i64,
    #[serde(default)]
    pub(super) pull_limit: Option<u32>,
    pub(super) changes: Vec<ChangeWire>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct SyncResponse {
    pub(super) protocol_version: u32,
    pub(super) cursor: i64,
    pub(super) has_more: bool,
    #[serde(default)]
    pub(super) push_acks: Vec<PushAck>,
    pub(super) changes: Vec<ChangeWire>,
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

pub(super) fn validate_sync_protocol_version(client: u32, server: u32) -> Result<()> {
    if client != server {
        return Err(sync_protocol_error(client, server));
    }
    Ok(())
}

pub(super) fn validate_sync_request_protocol_version(client: Option<u32>) -> Result<()> {
    validate_sync_protocol_version(client.unwrap_or(0), SYNC_PROTOCOL_VERSION)
}

pub(super) fn request_pull_limit(requested: Option<u32>) -> Result<u32> {
    match requested {
        None => Ok(MAX_PULL_BATCH),
        Some(limit @ 1..=MAX_PULL_BATCH) => Ok(limit),
        Some(limit) => {
            bail!("error sync-pull-limit-out-of-range min=1 max={MAX_PULL_BATCH} got={limit}")
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ValidatedSyncRequestEnvelope {
    pub(super) after: i64,
    pub(super) pull_limit: u32,
    pub(super) push_count: usize,
}

pub(super) fn validate_sync_request_envelope(
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

pub(super) fn validate_sync_response_for_request(
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

pub(super) fn validate_pushed_change(change: &ChangeWire) -> Result<()> {
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

    match change.op_type.as_str() {
        "create_workspace" => {
            ensure_entity_type(change, "workspace")?;
            ensure_sync_id("entity_id", &change.entity_id)?;
            required_string_payload("key", &change.payload)?;
            required_string_payload("name", &change.payload)?;
            required_string_payload("created_at", &change.payload)?;
        }
        "set_workspace_field" => {
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
        "create_project" => {
            ensure_entity_type(change, "project")?;
            ensure_sync_id("entity_id", &change.entity_id)?;
            optional_workspace_payload(&change.payload)?;
            required_string_payload("key", &change.payload)?;
            required_string_payload("name", &change.payload)?;
            required_string_payload("prefix", &change.payload)?;
            required_string_payload("created_at", &change.payload)?;
        }
        "set_project_metadata" => {
            ensure_entity_type(change, "project")?;
            ensure_sync_id("entity_id", &change.entity_id)?;
            optional_workspace_payload(&change.payload)?;
            required_string_payload("key", &change.payload)?;
            required_string_payload("name", &change.payload)?;
            required_string_payload("prefix", &change.payload)?;
            required_string_payload("updated_at", &change.payload)?;
        }
        "create_label" => {
            ensure_entity_type(change, "label")?;
            optional_workspace_payload(&change.payload)?;
            required_string_payload("name", &change.payload)?;
            required_string_payload("created_at", &change.payload)?;
        }
        "create_task" => {
            ensure_entity_type(change, "task")?;
            ensure_sync_id("entity_id", &change.entity_id)?;
            optional_workspace_payload(&change.payload)?;
            required_string_payload("title", &change.payload)?;
            let project_id = required_string_payload("project_id", &change.payload)?;
            ensure_sync_id("project_id", &project_id)?;
            required_string_payload("project_key", &change.payload)?;
            optional_string_payload("description", &change.payload)?;
            required_string_payload("project_name", &change.payload)?;
            required_string_payload("project_prefix", &change.payload)?;
            if let Some(status) = optional_string_payload("status", &change.payload)? {
                validate_sync_task_field_value(TaskField::Status, &status)?;
            }
            if let Some(priority) = optional_string_payload("priority", &change.payload)? {
                validate_sync_task_field_value(TaskField::Priority, &priority)?;
            }
            optional_string_array_payload("labels", &change.payload)?;
            optional_string_payload("created_at", &change.payload)?;
        }
        "set_field" | "resolve_field" => {
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
                ensure_sync_id("project_id", &project_id)?;
                if value != project_id {
                    bail!("error invalid-sync-change project-value-mismatch");
                }
                required_string_payload("project_key", &change.payload)?;
                required_string_payload("project_name", &change.payload)?;
                required_string_payload("project_prefix", &change.payload)?;
            }
        }
        "label_add" | "label_remove" => {
            ensure_entity_type(change, "task")?;
            ensure_sync_id("entity_id", &change.entity_id)?;
            optional_workspace_payload(&change.payload)?;
            required_string_payload("label", &change.payload)?;
        }
        "note_add" => {
            ensure_entity_type(change, "task")?;
            ensure_sync_id("entity_id", &change.entity_id)?;
            optional_workspace_payload(&change.payload)?;
            let note_id = required_string_payload("note_id", &change.payload)?;
            ensure_sync_id("note_id", &note_id)?;
            required_string_payload("body", &change.payload)?;
            required_string_payload("created_at", &change.payload)?;
        }
        "dependency_add" | "dependency_remove" => {
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
        "project_delete" => {
            ensure_entity_type(change, "project")?;
            ensure_sync_id("entity_id", &change.entity_id)?;
            required_workspace_payload(&change.payload)?;
            required_timestamp_payload("deleted_at", &change.payload)?;
        }
        "label_delete" => {
            ensure_entity_type(change, "label")?;
            required_workspace_payload(&change.payload)?;
            let name = required_string_payload("name", &change.payload)?;
            if name != change.entity_id {
                bail!("error invalid-sync-change label-value-mismatch");
            }
            required_timestamp_payload("deleted_at", &change.payload)?;
        }
        "note_delete" => {
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
        _ => bail!("error invalid-sync-change op_type={}", change.op_type),
    }
    Ok(())
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

fn required_string_payload(key: &str, payload: &Value) -> Result<String> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .with_context(|| format!("error invalid-sync-change payload.{key} missing"))
}

fn required_timestamp_payload(key: &str, payload: &Value) -> Result<String> {
    let value = required_string_payload(key, payload)?;
    if value.len() == 20
        && value.as_bytes()[4] == b'-'
        && value.as_bytes()[7] == b'-'
        && value.as_bytes()[10] == b'T'
        && value.as_bytes()[13] == b':'
        && value.as_bytes()[16] == b':'
        && value.as_bytes()[19] == b'Z'
        && value
            .bytes()
            .enumerate()
            .all(|(idx, byte)| matches!(idx, 4 | 7 | 10 | 13 | 16 | 19) || byte.is_ascii_digit())
    {
        Ok(value)
    } else {
        bail!("error invalid-sync-change payload.{key} invalid-timestamp");
    }
}

fn required_workspace_payload(payload: &Value) -> Result<()> {
    required_string_payload("workspace_id", payload)
        .and_then(|id| ensure_sync_id("workspace_id", &id))?;
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
