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
//! carry its descriptor commitment and current epoch; exact retries are idempotent.
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

mod persistence;

pub use crate::sync::seed_claim::{Publication, PublicationOutcome};
#[cfg(test)]
mod tests;

use super::seed_claim::Secret;
use serde::{Deserialize, Serialize};

/// Refusal limits for the single-vault staging storage profile.
pub const MAX_STORAGE_BYTES: u64 = 600 * 1_048_576;
pub const MAX_CHUNKS: u64 = 4096;
pub const MAX_CANDIDATES: i64 = 1024;
pub const MAX_REQUEST_BYTES: usize = 1_048_576 + 222;
pub const STAGING_TTL_SECONDS: i64 = 24 * 60 * 60;

#[derive(Debug)]
pub struct Authentication<'a> {
    pub vault_id: [u8; 32],
    pub genesis_commitment: [u8; 32],
    pub bearer: &'a Secret,
}

/// Budgets include catalog slices, encrypted manifest, state and selected images.
/// They cannot change on a declaration retry, even when staging has expired.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budget {
    pub bytes: u64,
    pub chunks: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Component {
    DataCatalog,
    PrefixCatalog,
    ImageCatalog,
    Manifest,
    State,
    Image([u8; 32]),
}

impl Component {
    fn catalog(self) -> Option<usize> {
        match self {
            Self::DataCatalog => Some(0),
            Self::PrefixCatalog => Some(1),
            Self::ImageCatalog => Some(2),
            _ => None,
        }
    }

    pub(crate) fn key(self) -> Vec<u8> {
        match self {
            Self::DataCatalog => vec![0],
            Self::PrefixCatalog => vec![1],
            Self::ImageCatalog => vec![2],
            Self::Manifest => vec![3],
            Self::State => vec![4],
            Self::Image(id) => std::iter::once(5).chain(id).collect(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Presence {
    Missing,
    Verified,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentStatus {
    pub component: Component,
    pub chunks: Vec<Presence>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StagingStatus {
    pub descriptor_commitment: [u8; 32],
    pub stream_id: [u8; 32],
    pub epoch: u64,
    /// Unix seconds. Expiry denies PUT but does not cancel or erase verified bytes.
    pub expires_at: i64,
    pub budget: Budget,
    /// Bounded by MAX_CHUNKS. Data/image components appear only after their
    /// catalogs verify; absent describing catalogs are not empty data sets.
    pub components: Vec<ComponentStatus>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Missing,
    Canceled,
    /// Immutable accepted intent. Callers validate it against pinned expectations.
    Published(PublicationOutcome),
    Staging(StagingStatus),
}

pub struct PutChunk<'a> {
    pub bootstrap_id: [u8; 32],
    pub descriptor_commitment: [u8; 32],
    pub epoch: u64,
    pub component: Component,
    pub index: u64,
    pub bytes: &'a [u8],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reclaim {
    /// Fence outstanding PUTs, preserving stored bytes.
    Fence,
    /// Actually remove all blobs. The descriptor remains frozen and resumable.
    All,
}

/// First admission preconditions. Record bytes are validated against the exact
/// staged descriptor and trusted predecessor inside the transaction.
pub struct PublishBootstrap<'a> {
    pub bootstrap_id: [u8; 32],
    pub descriptor_commitment: [u8; 32],
    pub epoch: u64,
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
