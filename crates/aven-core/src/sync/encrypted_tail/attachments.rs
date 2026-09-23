//! Opaque image representations, separate from local plaintext content addressing.
pub(crate) mod client;
pub(crate) mod codec;
pub(crate) mod server;
pub use client::{Download, Upload};
pub use codec::{HTTP_LIMIT, TRANSFER_BYTES};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ticket {
    pub reservation: [u8; 32],
    pub epoch: i64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Operation {
    Declare {
        workspace: String,
        descriptor: Vec<u8>,
    },
    Status {
        workspace: String,
        object: [u8; 32],
        descriptor_commitment: [u8; 32],
    },
    Ensure {
        workspace: String,
        object: [u8; 32],
        descriptor_commitment: [u8; 32],
        expected_epoch: i64,
    },
    Put {
        workspace: String,
        object: [u8; 32],
        descriptor_commitment: [u8; 32],
        epoch: i64,
        reservation: [u8; 32],
        index: usize,
        record: Vec<u8>,
    },
    Complete {
        workspace: String,
        object: [u8; 32],
        descriptor_commitment: [u8; 32],
        epoch: i64,
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
        epoch: i64,
        reservation: [u8; 32],
    },
    Prune {
        limit: usize,
    },
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Status {
    pub epoch: i64,
    pub complete: bool,
    pub missing: Vec<usize>,
    pub reservation: Option<[u8; 32]>,
    pub expires_at: Option<i64>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Reply {
    Status(Status),
    Chunk(Vec<u8>),
    Unavailable,
    Done,
    Pruned(usize),
}
