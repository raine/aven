use super::wire::{ChangeWire, PushAck, SyncRequest, SyncResponse};

mod blobs;
mod changes;
mod client;
mod parent_liveness;
mod server;
mod status;
use blobs::{
    apply_server_blob_reference, collect_attachment_liveness_hashes,
    ensure_attachment_blobs_admitted, prepare_server_blobs,
};
use changes::{
    epic_change_workspace, insert_wire_change, is_epic_change, load_assigned_change_ids,
    load_existing_change_ids, reconcile_acknowledged_epic_memberships, reconcile_epic_change,
    update_change_server_seq, update_change_server_seqs_if_missing, verify_existing_change,
};
use server::assign_server_sequences;
use status::sync_persistence_status;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncPersistenceStatus {
    pub pinned_server: Option<String>,
    pub established_protocol: u32,
    pub blocked_protocol: Option<u32>,
    pub pending_changes: i64,
    pub pending_attachment_uploads: i64,
    pub pending_attachment_upload_bytes: i64,
    pub conflicts: i64,
    pub sync_cursor: Option<String>,
    pub local_sequence: Option<String>,
    pub last_attempt: Option<String>,
    pub last_success: Option<String>,
    pub last_error: Option<String>,
    pub last_pushed: Option<String>,
    pub last_pulled: Option<String>,
    pub last_cursor: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ClientSyncPage {
    pub request: SyncRequest,
    pub pending: usize,
    pub(crate) behavior_protocol: u32,
    pub sync_generation: i64,
}

#[derive(Debug)]
pub struct ApplySyncPage {
    pub request: SyncRequest,
    pub sync_generation: i64,
    pub response: SyncResponse,
    pub attempted_at: String,
    pub previous_pushed: i64,
    pub previous_pulled: usize,
}

#[derive(Debug)]
pub struct ServerSyncPage {
    pub request: SyncRequest,
}

#[derive(Debug)]
pub struct ServerSyncResult {
    pub accepted_count: i64,
    pub push_acks: Vec<PushAck>,
    pub changes: Vec<ChangeWire>,
    pub has_more: bool,
    pub blob_prepare_ms: u128,
    pub assign_ms: u128,
    pub pull_query_ms: u128,
}
