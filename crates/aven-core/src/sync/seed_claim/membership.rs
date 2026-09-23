//! Pure signed AddDevice chain and PSK trust handoff for one published generation.
//!
//! Verified signatures establish client intent, not server acceptance, current
//! authorization, protected persistence or dispatch readiness. Unsupported state
//! transitions and enrollment versions fail closed.
mod admission;
mod pairing;
#[cfg(test)]
mod tests;

pub use super::peer::Invitation;
use super::peer::{Recipient, verify};
use super::*;
pub use pairing::{Declaration, Device, Joiner, ProvisionalGrant, VerifiedEnrollment};

pub const MAX_DEVICES: usize = 32;
pub const MAX_RECORD_BYTES: usize = 8192;
pub const MAX_CHAIN_BYTES: usize = 262144;
pub const DECLARATION_BYTES: usize = 280;
pub const REQUEST_BYTES: usize = 314;
const CORE_BYTES: usize = 461;
const ATTACHMENT_BYTES: usize = 608;
const GRANT_BYTES: usize = 523;
const GRANT_PLAINTEXT_BYTES: usize = 466;
const DOMAIN_VERSION: u32 = crate::sync::bootstrap_format::DOMAIN_VERSION;

type Hash = [u8; 32];
fn check(condition: bool) -> Result<()> {
    ensure!(condition, "error membership-invalid");
    Ok(())
}

#[derive(Clone, PartialEq, Eq)]
struct Member {
    device: Hash,
    sign: Hash,
    hpke: Hash,
    verifier: Hash,
    admission: Hash,
    admitted_at: u64,
}
impl Member {
    fn write(&self, out: &mut Vec<u8>) {
        for field in [self.device, self.sign, self.hpke, self.verifier] {
            out.extend(field);
        }
        out.extend(1_u32.to_be_bytes());
        out.extend(self.admission);
    }
}

/// A chain derived exclusively from verified signed predecessors. No server
/// observation or resulting-state DTO can construct this value directly.
#[derive(Clone)]
pub struct Membership {
    genesis: Genesis,
    publication: Publication,
    members: Vec<Member>,
    heads: Vec<Hash>,
    handles: Vec<Hash>,
    evidence_bytes: usize,
}
impl Membership {
    pub fn from_publication(
        genesis: &Genesis,
        descriptor: &[u8],
        publication: &[u8],
    ) -> Result<Self> {
        let publication = Publication::from_record(genesis, descriptor, publication)?;
        Ok(Self {
            genesis: genesis.clone(),
            heads: vec![publication.commitment()],
            publication,
            members: vec![Member {
                device: genesis.device,
                sign: genesis.signing_public,
                hpke: genesis.hpke_public,
                verifier: genesis.verifier,
                admission: genesis.claim,
                admitted_at: 0,
            }],
            handles: vec![],
            evidence_bytes: GENESIS_BYTES + PUBLICATION_BYTES + descriptor.len(),
        })
    }
    pub fn head(&self) -> Hash {
        self.heads[self.heads.len() - 1]
    }
    pub fn sequence(&self) -> u64 {
        self.heads.len() as u64
    }
    pub fn device_count(&self) -> usize {
        self.members.len()
    }
    pub fn genesis(&self) -> &Genesis {
        &self.genesis
    }
    pub fn publication(&self) -> &Publication {
        &self.publication
    }
    /// Ancestry, not a claim that either checkpoint is the latest server head.
    pub fn extends(&self, previous: &Self) -> bool {
        self.genesis == previous.genesis && self.heads.starts_with(&previous.heads)
    }
    fn member(&self, device: &Hash) -> Result<&Member> {
        self.members
            .iter()
            .find(|m| &m.device == device)
            .context("error membership-signer")
    }
    fn unique(&self, recipient: &Recipient, handle: &Hash) -> Result<()> {
        check(self.members.len() < MAX_DEVICES && !self.handles.contains(handle))?;
        check(self.members.iter().all(|m| {
            m.device != recipient.device && m.sign != recipient.sign && m.hpke != recipient.hpke
        }))
    }
    /// Validate one exact successor without mutating the trusted predecessor.
    /// Expiry, current bearer authorization and atomic server CAS are separate.
    pub fn append(&self, declaration: &[u8], request: &[u8], record: &[u8]) -> Result<Self> {
        check(record.len() <= MAX_RECORD_BYTES)?;
        let evidence_bytes = self
            .evidence_bytes
            .checked_add(declaration.len())
            .and_then(|n| n.checked_add(request.len()))
            .and_then(|n| n.checked_add(record.len()))
            .context("error membership-limit")?;
        check(evidence_bytes <= MAX_CHAIN_BYTES)?;
        let declaration = Declaration::from_record(self, declaration)?;
        let recipient = admission::validate(self, &declaration, request, record)?;
        let mut next = self.clone();
        next.members.push(Member {
            device: recipient.device,
            sign: recipient.sign,
            hpke: recipient.hpke,
            verifier: recipient.verifier,
            admission: declaration.handle,
            admitted_at: self.sequence() + 1,
        });
        next.members.sort_by_key(|m| m.device);
        next.handles.push(declaration.handle);
        next.heads.push(hash(record));
        next.evidence_bytes = evidence_bytes;
        Ok(next)
    }
}
