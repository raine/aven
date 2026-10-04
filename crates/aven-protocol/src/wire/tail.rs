//! Ordinary encrypted tail messages. Context authentication is backend-owned.
use serde::{Deserialize, Serialize};
pub mod batch;
pub const RECORD_LIMIT: usize = 135640;
/// Bodies without a record; also the framing allowance around records.
pub const CONTROL_LIMIT: usize = 16384;
/// One record: an append request or a lookup response.
pub const APPEND_LIMIT: usize = crate::base64_bytes::encoded_len(RECORD_LIMIT) + CONTROL_LIMIT;
/// Serialized size of the records in one pull page.
pub const PAGE_BYTES: usize = 2 * 1048576;
pub const PAGE_COUNT: usize = 256;
pub const RESPONSE_LIMIT: usize = PAGE_BYTES + CONTROL_LIMIT;
pub const BATCH_COUNT: usize = 128;
pub const BATCH_BYTES: usize = 1048576;
pub const BATCH_APPEND_LIMIT: usize = crate::base64_bytes::encoded_len(BATCH_BYTES) + CONTROL_LIMIT;
pub const BATCH_CONTROL_LIMIT: usize = 256 * 1024;

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Mapping {
    pub operation_id: String,
    pub sequence: i64,
    pub commitment: [u8; 32],
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Accepted {
    pub mapping: Mapping,
    #[serde(with = "crate::base64_bytes")]
    pub record: Vec<u8>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchFeatures {
    pub count: usize,
    pub bytes: usize,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Operation {
    Features,
    Append {
        ticket: Option<super::images::Ticket>,
        #[serde(with = "crate::base64_bytes")]
        record: Vec<u8>,
    },
    Lookup {
        operation_id: String,
        expected: Option<Mapping>,
    },
    Pull {
        after: i64,
        limit: usize,
        watermark: Option<i64>,
    },
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Page {
    pub after: i64,
    pub watermark: i64,
    pub cursor: i64,
    pub has_more: bool,
    pub records: Vec<Accepted>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BatchRecord(
    #[serde(
        serialize_with = "crate::base64_bytes::serialize",
        deserialize_with = "crate::base64_bytes::bounded::<_, RECORD_LIMIT>"
    )]
    pub Vec<u8>,
);

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum BatchOperation {
    Append {
        #[serde(deserialize_with = "batch::bounded_items")]
        records: Vec<BatchRecord>,
    },
    Resolve {
        #[serde(deserialize_with = "batch::bounded_items")]
        operation_ids: Vec<String>,
    },
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Resolution {
    Found(batch::CompactMapping),
    Absent { operation_id: String },
    Bootstrap { operation_id: String },
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum BatchReply {
    Appended(#[serde(deserialize_with = "batch::bounded_items")] Vec<batch::CompactMapping>),
    Resolved(#[serde(deserialize_with = "batch::bounded_items")] Vec<Resolution>),
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Reply {
    Features(BatchFeatures),
    Appended(Mapping),
    Found(Accepted),
    Absent,
    Bootstrap,
    Page(Page),
}

pub const PATH: &str = "/e2ee/tail/v1";
pub const BATCH_PATH: &str = "/e2ee/tail/batch/v1";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope<T> {
    pub context: Context,
    pub correlation: [u8; 32],
    pub operation: T,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Context {
    pub vault: [u8; 32],
    pub genesis: [u8; 32],
    pub device: [u8; 32],
    pub head: [u8; 32],
    pub stream: [u8; 32],
    pub descriptor: [u8; 32],
}

impl Context {
    pub fn authentication<'a>(
        &self,
        bearer: &'a crate::claim::Secret,
    ) -> super::enrollment::Authentication<'a> {
        super::enrollment::Authentication {
            vault: self.vault,
            genesis: self.genesis,
            device: self.device,
            head: self.head,
            bearer,
        }
    }
}
