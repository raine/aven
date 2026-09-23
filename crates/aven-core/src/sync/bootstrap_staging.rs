//! Authenticated bootstrap staging for a claimed seed.
//! Publication and membership admission are handled separately.
//!
//! All operations authenticate the stored genesis bearer and exact genesis context
//! inside the transaction. Successor membership must set `genesis_only = 0` in
//! its authorization transaction, retiring both staging and first-claim retries.
//! A descriptor, device ID, setup secret or signature alone grants no staging access.
//!
//! Declaration freezes the exact profile-1 descriptor and explicit total byte and
//! chunk budgets, including all three catalogs. One candidate may be active. PUTs
//! carry its descriptor commitment and current epoch; exact retries are idempotent.
//! Catalog slices are quarantined until their ordered aggregate and structure
//! verify. Data requires its verified describing catalog. Manifest descriptors
//! are inline. Verified means public framing/commitments, never AEAD/domain validity.
//!
//! SQLite blobs and presence commit together under the core writer gate. No staged
//! files, HTTP, READY, signed PublishBootstrap, adoption or power-loss guarantee.
//! Limits bound logical retained payload, not SQLite/WAL/temp files, process memory
//! or total disk use. Validation can materialize one bounded artifact (256 MiB)
//! plus framing and one catalog (16 MiB); callers must separately bound concurrent
//! requests.

mod persistence;
#[cfg(test)]
mod tests;

use super::seed_claim::Secret;

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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    pub bytes: u64,
    pub chunks: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

    fn key(self) -> Vec<u8> {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presence {
    Missing,
    Quarantined,
    Verified,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComponentStatus {
    pub component: Component,
    pub chunks: Vec<Presence>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogFailureReason {
    Invalid,
    ResourceLimit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CatalogFailure {
    pub component: Component,
    pub reason: CatalogFailureReason,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagingStatus {
    pub descriptor_commitment: [u8; 32],
    pub stream_id: [u8; 32],
    pub epoch: u64,
    /// Unix seconds. Expiry denies PUT but does not cancel or erase verified bytes.
    pub expires_at: i64,
    pub budget: Budget,
    /// Last failed catalog, cleared when that catalog verifies successfully.
    pub catalog_failure: Option<CatalogFailure>,
    /// Bounded by MAX_CHUNKS. Data/image components appear only after their
    /// catalogs verify; absent describing catalogs are not empty data sets.
    pub components: Vec<ComponentStatus>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Missing,
    Canceled,
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
pub enum PutOutcome {
    Quarantined,
    Verified,
    /// The transaction discarded quarantine and fenced outstanding writers.
    /// Read status or ensure staging to obtain the new epoch before retrying.
    CatalogRejected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reclaim {
    /// Fence outstanding PUTs and discard quarantine, preserving verified blobs.
    Quarantine,
    /// Actually remove all blobs. The descriptor remains frozen and resumable.
    All,
}
