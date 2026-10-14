//! Internal ordinary encrypted task stream. Membership remains chain-owned.
pub mod attachments;
pub mod batch;
mod client;
pub use client::Preflight;
mod codec;
pub(crate) mod dependencies;
mod domain;
#[cfg(feature = "test-support")]
pub(crate) mod fuzz;
pub(crate) mod graphs;
mod labels;
mod moves;
mod notes;
mod recurrence;
mod server;

use super::LocalSharedStatePackageKey;
use anyhow::{Result, ensure};
use serde::Serialize;
use sha2::Sha256;

pub use aven_protocol::wire::tail::{
    APPEND_LIMIT, BATCH_APPEND_LIMIT, BATCH_BYTES, BATCH_CONTROL_LIMIT, BATCH_COUNT, CONTROL_LIMIT,
    PAGE_BYTES, PAGE_COUNT, RECORD_LIMIT, RESPONSE_LIMIT,
};

/// An appended record's identity collides with a published prefix record.
#[derive(Debug)]
pub struct PrefixIdentityCollision;

impl std::fmt::Display for PrefixIdentityCollision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("error encrypted-tail-prefix-identity-collision")
    }
}

impl std::error::Error for PrefixIdentityCollision {}

pub use aven_protocol::wire::tail::Context;
/// Protected host inputs. The host retains installation and authority exclusion.
pub struct Authority {
    pub context: Context,
    pub membership: super::seed_claim::membership::Membership,
    pub keys: super::seed_claim::membership::VerifiedKeys,
    pub prefix: i64,
    pub association: String,
    pub sync_generation: i64,
}
/// Context-bound lookup absence. Only atomic replacement consumes this observation.
pub struct AbsentOperation {
    context: Context,
    record: Vec<u8>,
}
/// Exact frozen bytes for one dispatch. Image heads include their staged upload.
pub struct Push {
    pub record: Vec<u8>,
    pub upload: Option<attachments::Upload>,
}
/// Local round state read in one validated transaction.
pub struct RoundState {
    pub cursor: i64,
    /// Finite page target captured while initial image catch-up is pending.
    pub initial_watermark: Option<i64>,
    /// No frozen or unacknowledged local metadata.
    pub idle: bool,
    pub upload_pending: bool,
    /// Image demand, known only after initial image catch-up.
    pub downloads: Option<Downloads>,
}
#[derive(Clone, Copy)]
pub struct Downloads {
    pub pending: bool,
    pub unavailable: bool,
}
impl Authority {
    pub fn generation(&self) -> [u8; 32] {
        self.membership.current_generation().id
    }
    pub fn rotation_pending(&self) -> bool {
        self.membership.rotation_pending()
    }
    pub fn key(&self, generation: [u8; 32]) -> Result<&LocalSharedStatePackageKey> {
        self.keys.key(generation)
    }
    pub fn record_is_closed(&self, record: &[u8]) -> Result<bool> {
        let e = codec::parse(record)?;
        self.key(e.generation)?;
        valid(e.vault == self.context.vault && e.stream == self.context.stream)?;
        Ok(e.generation != self.generation())
    }
    /// Caller binds the lookup response to this authority and the parsed operation ID.
    pub fn confirm_absent(&self, record: &[u8], response: &Reply) -> Result<AbsentOperation> {
        self.validate()?;
        valid(matches!(response, Reply::Absent))?;
        codec::open(self, record)?;
        Ok(AbsentOperation {
            context: self.context.clone(),
            record: record.to_vec(),
        })
    }
    fn validate(&self) -> Result<()> {
        self.keys.validate(&self.membership)?;
        let b = self.membership.publication().binding();
        valid(
            self.context.vault == b.vault_id
                && self.context.genesis == self.membership.genesis().commitment()
                && self.context.head == self.membership.head()
                && self.context.stream == b.stream_id
                && self.context.descriptor == b.descriptor_commitment
                && self.prefix == i64::try_from(b.prefix_count)?,
        )
    }
}
pub(crate) use crate::sync::codec::hash;
pub use aven_protocol::wire::tail::{
    Accepted, BatchFeatures, BatchOperation, BatchRecord, BatchReply, Mapping, Operation, Page,
    Reply, Resolution,
};
pub(crate) fn valid(ok: bool) -> Result<()> {
    ensure!(ok, "error encrypted-tail-invalid");
    Ok(())
}

#[cfg(test)]
mod tests;

/// An authority over the fixed publication fixture and its initial key.
#[cfg(any(test, feature = "test-support"))]
pub(crate) fn fixture_authority() -> Authority {
    let (membership, keys) = crate::sync::seed_claim::membership::test_support::content_authority();
    let b = membership.publication().binding();
    Authority {
        context: Context {
            vault: b.vault_id,
            genesis: membership.genesis().commitment(),
            device: [3; 32],
            head: membership.head(),
            stream: b.stream_id,
            descriptor: b.descriptor_commitment,
        },
        prefix: b.prefix_count as i64,
        membership,
        keys,
        association: "test".into(),
        sync_generation: 1,
    }
}

#[cfg(any(test, feature = "test-support"))]
pub fn attachment_integrity_scan_metrics() -> (u64, u64) {
    attachments::client::integrity_scan_metrics()
}

#[cfg(any(test, feature = "test-support"))]
pub(crate) fn open_record(authority: &Authority, record: &[u8]) -> Result<super::wire::ChangeWire> {
    codec::open(authority, record)
}

/// Re-seals an accepted record after `edit`, keeping its sequence.
#[cfg(any(test, feature = "test-support"))]
pub(crate) fn reseal(
    authority: &Authority,
    accepted: &Accepted,
    edit: impl FnOnce(&mut super::wire::ChangeWire),
) -> Result<Accepted> {
    let mut change = codec::open(authority, &accepted.record)?;
    edit(&mut change);
    let record = codec::seal(authority, &change)?;
    Ok(Accepted {
        mapping: Mapping {
            operation_id: change.change_id,
            sequence: accepted.mapping.sequence,
            commitment: hash(&record),
        },
        record,
    })
}
