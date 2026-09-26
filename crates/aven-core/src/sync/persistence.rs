mod blobs;
pub(crate) mod changes;
pub(crate) mod parent_liveness;
mod status;
pub(in crate::sync) use blobs::collect_attachment_liveness_hashes;
pub(in crate::sync) use changes::{
    epic_change_workspace, insert_wire_change, is_epic_change, reconcile_epic_change,
    update_change_server_seq,
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncPersistenceStatus {
    pub pending_changes: i64,
    pub conflicts: i64,
    pub sync_cursor: Option<String>,
    pub local_sequence: Option<String>,
}
