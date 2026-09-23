//! Internal ordinary encrypted task stream. Membership remains chain-owned.
mod client;
mod codec;
mod domain;
mod labels;
mod notes;
mod server;

use super::{LocalSharedStatePackageKey, seed_claim::peer};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const RECORD_LIMIT: usize = 132328;
pub const CONTROL_LIMIT: usize = 16384;
pub const APPEND_LIMIT: usize = 545696;
pub const RESPONSE_LIMIT: usize = 4259840;
pub const PAGE_BYTES: usize = 1048576;
pub const PAGE_COUNT: usize = 16;

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
    pub generation: [u8; 32],
    pub key: LocalSharedStatePackageKey,
    pub prefix: i64,
    pub association: String,
    pub sync_generation: i64,
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
    pub record: Vec<u8>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Operation {
    Append {
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
