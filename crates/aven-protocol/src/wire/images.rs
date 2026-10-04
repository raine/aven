//! Ordinary encrypted image transport.
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
        #[serde(with = "crate::base64_bytes")]
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
        #[serde(with = "crate::base64_bytes")]
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
    Chunk(#[serde(with = "crate::base64_bytes")] Vec<u8>),
    Unavailable,
    Done,
}

pub const PATH: &str = "/e2ee/images/v1";
pub const DESCRIPTOR_LIMIT: usize = 1984;
pub const IMAGE_BYTES: usize = 25 * 1048576;
pub const CHUNK_BYTES: usize = 1048576 + 222;
pub const TRANSFER_BYTES: usize = IMAGE_BYTES + 25 * 222;
pub const HTTP_LIMIT: usize = crate::base64_bytes::encoded_len(CHUNK_BYTES) + 16384;
