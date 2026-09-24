#[cfg(any(test, feature = "test-support"))]
use super::wire::{ChangeWire, PushAck, SyncRequest, SyncResponse};

mod blobs;
pub(crate) mod changes;
#[cfg(any(test, feature = "test-support"))]
mod client;
pub(crate) mod parent_liveness;
#[cfg(any(test, feature = "test-support"))]
mod server;
mod status;
pub(in crate::sync) use blobs::collect_attachment_liveness_hashes;
pub(in crate::sync) use changes::{
    epic_change_workspace, insert_wire_change, is_epic_change, reconcile_epic_change,
    update_change_server_seq,
};
#[cfg(any(test, feature = "test-support"))]
pub(in crate::sync) use changes::{
    load_existing_change_ids, reconcile_acknowledged_epic_memberships,
    update_change_server_seqs_if_missing, verify_existing_change,
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncPersistenceStatus {
    pub pending_changes: i64,
    pub conflicts: i64,
    pub sync_cursor: Option<String>,
    pub local_sequence: Option<String>,
}

/// In-process stand-in for the unencrypted page exchange; test fixtures use
/// it to produce accepted history and remote changes.
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Clone)]
pub struct ClientSyncPage {
    pub request: SyncRequest,
    pub pending: usize,
    pub sync_generation: i64,
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Debug)]
pub struct ApplySyncPage {
    pub request: SyncRequest,
    pub sync_generation: i64,
    pub response: SyncResponse,
    /// Clock for recurrence reconciliation inside the applied page.
    pub attempted_at: String,
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Debug)]
pub struct ServerSyncPage {
    pub request: SyncRequest,
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Debug)]
pub struct ServerSyncResult {
    pub accepted_count: i64,
    pub push_acks: Vec<PushAck>,
    pub changes: Vec<ChangeWire>,
    pub has_more: bool,
}
