//! Bootstrap control messages and bounded staging batches.
use serde::{Deserialize, Serialize};
pub mod batch;

/// Refusal limits for the single-vault staging storage profile.
pub const MAX_STORAGE_BYTES: u64 = 600 * 1_048_576;
pub const MAX_CHUNKS: u64 = 4096;
pub const MAX_CANDIDATES: i64 = 1024;
pub const MAX_REQUEST_BYTES: usize = 1_048_576 + 222;
pub const STAGING_TTL_SECONDS: i64 = 24 * 60 * 60;

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
    pub fn is_catalog(self) -> bool {
        self.catalog().is_some()
    }

    pub fn catalog(self) -> Option<usize> {
        match self {
            Self::DataCatalog => Some(0),
            Self::PrefixCatalog => Some(1),
            Self::ImageCatalog => Some(2),
            _ => None,
        }
    }

    pub fn key(self) -> Vec<u8> {
        match self {
            Self::DataCatalog => vec![0],
            Self::PrefixCatalog => vec![1],
            Self::ImageCatalog => vec![2],
            Self::Manifest => vec![3],
            Self::State => vec![4],
            Self::Image(id) => std::iter::once(5).chain(id).collect(),
        }
    }

    /// The component whose [`Self::key`] is `key`.
    pub fn from_key(key: &[u8]) -> Option<Self> {
        Some(match key {
            [0] => Self::DataCatalog,
            [1] => Self::PrefixCatalog,
            [2] => Self::ImageCatalog,
            [3] => Self::Manifest,
            [4] => Self::State,
            [5, id @ ..] => Self::Image(id.try_into().ok()?),
            _ => return None,
        })
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
    /// Unix seconds. Expiry denies PUT but does not cancel or erase verified bytes.
    pub expires_at: i64,
    pub budget: Budget,
    /// Bounded by MAX_CHUNKS. Data/image components appear only after their
    /// catalogs verify; absent describing catalogs are not empty data sets.
    pub components: Vec<ComponentStatus>,
}

pub const PATH: &str = "/e2ee/bootstrap/v1";
pub const REQUEST_LIMIT: usize = crate::base64_bytes::encoded_len(MAX_REQUEST_BYTES) + 4096;
pub const RESPONSE_LIMIT: usize = 1_048_576;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub vault: [u8; 32],
    pub genesis: [u8; 32],
    pub operation: Operation,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Operation {
    ClaimSetup {
        #[serde(with = "crate::base64_bytes")]
        bytes: Vec<u8>,
    },
    ClaimBearer {
        #[serde(with = "crate::base64_bytes")]
        bytes: Vec<u8>,
    },
    Declare {
        #[serde(with = "crate::base64_bytes")]
        descriptor: Vec<u8>,
        budget: Budget,
    },
    Status {
        bootstrap: [u8; 32],
    },
    Ensure {
        bootstrap: [u8; 32],
        commitment: [u8; 32],
    },
    Cancel {
        bootstrap: [u8; 32],
    },
    Publish {
        bootstrap: [u8; 32],
        commitment: [u8; 32],
        #[serde(with = "crate::base64_bytes")]
        record: Vec<u8>,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Reply {
    Claimed {
        vault: [u8; 32],
        claim: [u8; 32],
        genesis: [u8; 32],
    },
    Missing,
    Canceled,
    Staging(StagingStatus),
    Stored,
    Published(#[serde(with = "crate::base64_bytes")] Vec<u8>),
}
