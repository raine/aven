//! Opaque image representations, separate from local plaintext content addressing.
pub(crate) mod client;
pub(crate) mod codec;
pub(crate) mod server;
pub use client::{Download, ImageSourceUnavailable, Upload};
pub use codec::{CHUNK_BYTES, HTTP_LIMIT, TRANSFER_BYTES};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ticket {
    pub reservation: [u8; 32],
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Operation {
    Declare {
        workspace: String,
        #[serde(with = "crate::sync::base64_bytes")]
        descriptor: Vec<u8>,
    },
    Status {
        workspace: String,
        object: [u8; 32],
        descriptor_commitment: [u8; 32],
    },
    Put {
        workspace: String,
        object: [u8; 32],
        descriptor_commitment: [u8; 32],
        reservation: [u8; 32],
        index: usize,
        #[serde(with = "crate::sync::base64_bytes")]
        record: Vec<u8>,
    },
    Complete {
        workspace: String,
        object: [u8; 32],
        descriptor_commitment: [u8; 32],
        reservation: [u8; 32],
    },
    Read {
        workspace: String,
        object: [u8; 32],
        descriptor_commitment: [u8; 32],
        index: usize,
    },
    Release {
        workspace: String,
        object: [u8; 32],
        descriptor_commitment: [u8; 32],
        reservation: [u8; 32],
    },
    Prune {
        limit: usize,
    },
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Status {
    pub complete: bool,
    pub missing: Vec<usize>,
    pub reservation: Option<[u8; 32]>,
    pub expires_at: Option<i64>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Reply {
    Status(Status),
    Chunk(#[serde(with = "crate::sync::base64_bytes")] Vec<u8>),
    Unavailable,
    Done,
    Pruned(usize),
}
