//! Internal ordinary encrypted task stream. Membership remains chain-owned.
pub mod attachments;
mod client;
mod codec;
pub(crate) mod dependencies;
mod domain;
mod labels;
mod notes;
mod recurrence;
mod server;

use super::{LocalSharedStatePackageKey, seed_claim::peer};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const RECORD_LIMIT: usize = 135640;
/// Bodies without a record; also the framing allowance around records.
pub const CONTROL_LIMIT: usize = 16384;
/// One record: an append request or a lookup response.
pub const APPEND_LIMIT: usize = super::base64_bytes::encoded_len(RECORD_LIMIT) + CONTROL_LIMIT;
/// Serialized size of the records in one pull page.
pub const PAGE_BYTES: usize = 2 * 1048576;
pub const PAGE_COUNT: usize = 256;
pub const RESPONSE_LIMIT: usize = PAGE_BYTES + CONTROL_LIMIT;

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Context {
    pub vault: [u8; 32],
    pub genesis: [u8; 32],
    pub device: [u8; 32],
    pub credential_version: u32,
    pub head: [u8; 32],
    pub stream: [u8; 32],
    pub descriptor: [u8; 32],
}
impl Context {
    pub fn authentication<'a>(
        &self,
        bearer: &'a super::seed_claim::Secret,
    ) -> peer::Authentication<'a> {
        peer::Authentication {
            vault: self.vault,
            genesis: self.genesis,
            device: self.device,
            credential_version: self.credential_version,
            head: self.head,
            bearer,
        }
    }
}
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
    #[serde(with = "crate::sync::base64_bytes")]
    pub record: Vec<u8>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Operation {
    Append {
        ticket: Option<attachments::Ticket>,
        #[serde(with = "crate::sync::base64_bytes")]
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
#[serde(deny_unknown_fields)]
pub enum Reply {
    Appended(Mapping),
    Found(Accepted),
    Absent,
    Bootstrap,
    Page(Page),
}
pub(crate) fn hash(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
pub(crate) fn valid(ok: bool) -> Result<()> {
    ensure!(ok, "error encrypted-tail-invalid");
    Ok(())
}

#[cfg(test)]
mod tests;

#[cfg(any(test, feature = "test-support"))]
fn crash_at(stage: &str) {
    if std::env::var("AVEN_TAIL_CRASH").as_deref() == Ok(stage) {
        std::process::exit(84);
    }
}
