use anyhow::Result;
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
