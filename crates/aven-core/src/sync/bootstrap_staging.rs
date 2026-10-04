//! Authenticated staging and atomic initial snapshot publication for a claimed seed.
//!
//! All operations authenticate current supported membership inside the transaction.
//! Publication installs its explicit membership head and retires genesis-only
//! admission atomically. Published status, exact retry and cancel-to-outcome use
//! that head, never historical genesis credentials across unsupported successors.
//! A descriptor, device ID, setup secret or signature alone grants no staging access.
//!
//! Declaration freezes the exact profile-1 descriptor and explicit total byte and
//! chunk budgets, including all three catalogs. One candidate may be active. PUTs
//! carry its descriptor commitment; exact retries are idempotent. A batch PUT
//! stores all of its records or none, and never mixes catalog slices with data.
//! Every PUT is checked against its descriptor slot before it is stored, and a
//! slice that completes a catalog is stored only if the whole catalog verifies.
//! Data requires its complete describing catalog. Manifest descriptors are
//! inline. Verified means public framing/commitments, never AEAD/domain validity.
//!
//! SQLite blobs and presence commit together under the core writer gate. Signed
//! PublishBootstrap couples complete staged bytes, immutable outcome, active prefix
//! and image ownership, allocator=N and READY in one immediate transaction. Image
//! bytes move to ordinary lifecycle ownership; immutable catalogs are not byte pins.
//! Transport and client adoption live outside this module. There are no staged
//! files, ordinary encrypted tail or power-loss guarantee.
//! Limits bound logical retained payload, not SQLite/WAL/temp files, process memory
//! or total disk use. Validation can materialize one bounded artifact (256 MiB)
//! plus framing and one catalog (16 MiB); callers must separately bound concurrent
//! requests.

pub mod batch;
mod persistence;

pub use crate::sync::seed_claim::{Publication, PublicationOutcome};
#[cfg(test)]
mod tests;

use super::seed_claim::Secret;

/// The credential does not authenticate current membership for staging.
#[derive(Debug)]
pub struct Unauthorized;

impl std::fmt::Display for Unauthorized {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("error bootstrap-unauthorized")
    }
}

impl std::error::Error for Unauthorized {}

pub use aven_protocol::wire::bootstrap::{
    MAX_CANDIDATES, MAX_CHUNKS, MAX_REQUEST_BYTES, MAX_STORAGE_BYTES, STAGING_TTL_SECONDS,
};

#[derive(Debug)]
pub struct Authentication<'a> {
    pub vault_id: [u8; 32],
    pub genesis_commitment: [u8; 32],
    pub bearer: &'a Secret,
}

pub use aven_protocol::wire::bootstrap::{
    Budget, Component, ComponentStatus, Presence, StagingStatus,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Missing,
    Canceled,
    /// Immutable accepted intent. Callers validate it against pinned expectations.
    Published(PublicationOutcome),
    Staging(StagingStatus),
}

/// Records for several slots of one candidate, stored all or nothing.
pub struct PutBatch<'a> {
    pub bootstrap_id: [u8; 32],
    pub descriptor_commitment: [u8; 32],
    pub records: Vec<(Component, u64, &'a [u8])>,
}

pub struct PutChunk<'a> {
    pub bootstrap_id: [u8; 32],
    pub descriptor_commitment: [u8; 32],
    pub component: Component,
    pub index: u64,
    pub bytes: &'a [u8],
}

/// First admission preconditions. Record bytes are validated against the exact
/// staged descriptor and trusted predecessor inside the transaction.
pub struct PublishBootstrap<'a> {
    pub bootstrap_id: [u8; 32],
    pub descriptor_commitment: [u8; 32],
    pub record: &'a [u8],
}

/// Operator-owned admission policy, not a value accepted from a remote requester.
#[derive(Clone, Copy, Debug)]
pub struct PublicationPolicy {
    pub workspace_quota_bytes: u64,
}

impl Default for PublicationPolicy {
    fn default() -> Self {
        Self {
            workspace_quota_bytes: crate::attachments::lifecycle::DEFAULT_ORIGINAL_QUOTA_BYTES
                as u64,
        }
    }
}
