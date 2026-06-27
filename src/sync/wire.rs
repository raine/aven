use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(crate) const SYNC_PROTOCOL_VERSION: u32 = 4;
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
