//! Pure signed membership transitions and recipient-verified generation coverage.
//!
//! Verified signatures establish client intent, not server acceptance, current
//! authorization, protected persistence or dispatch readiness. Unsupported state
//! transitions and enrollment versions fail closed.
mod admission;
mod encoding;
mod evidence;
mod pairing;
pub use evidence::{Evidence, EvidenceRecord, MAX_EVIDENCE_JSON_BYTES, Mailbox};
mod keys;
mod rotation;
pub use keys::VerifiedKeys;
pub use rotation::Generation;
pub(crate) mod persistence;
pub use persistence::{MAX_CANDIDATES, MAX_INVITATIONS};
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;

pub use super::peer::Invitation;
use super::peer::{Recipient, verify};
use super::*;
pub use pairing::{Declaration, Device, Joiner, ProvisionalGrant, VerifiedEnrollment};

pub const MAX_DEVICES: usize = 32;
pub const MAX_RECORD_BYTES: usize = 32768;
pub const MAX_CHAIN_BYTES: usize = 1048576;
pub const MAX_TRANSITIONS: usize = 128;
pub const MAX_GENERATIONS: usize = 32;
pub const MAX_KEY_PLAINTEXT_BYTES: usize = 4096;
pub const DECLARATION_BYTES: usize = 280;
pub const REQUEST_BYTES: usize = 314;
const CORE_BYTES: usize = 461;
const GRANT_PREFIX_BYTES: usize = 1 + 12 * 32 + 8;
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
    retired: Vec<Member>,
    generations: Vec<Generation>,
    pending: bool,
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
            retired: vec![],
            generations: vec![Generation {
                id: genesis.context.generation_id,
                commitment: genesis.generation_commitment,
                starts_after: 0,
            }],
            pending: false,
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
    /// Authenticate the actual credential before examining its requested head.
    pub fn authenticate(
        &self,
        auth: &super::peer::Authentication<'_>,
        ancestor: bool,
    ) -> Result<()> {
        ensure!(
            auth.credential_version == 1
                && auth.vault == self.genesis.context.vault_id
                && auth.genesis == self.genesis.commitment(),
            "error enrollment-unauthorized"
        );
        let member = self.member(&auth.device)?;
        ensure!(
            bool::from(member.verifier.ct_eq(&credential_verifier(
                auth.vault,
                auth.device,
                auth.bearer
            ))),
            "error enrollment-unauthorized"
        );
        ensure!(
            self.heads.contains(&auth.head),
            "error membership-context-unknown"
        );
        if !ancestor && auth.head != self.head() {
            anyhow::bail!(StaleContext);
        }
        Ok(())
    }
    pub fn head_at(&self, sequence: u64) -> Option<Hash> {
        sequence
            .checked_sub(1)
            .and_then(|n| self.heads.get(n as usize))
            .copied()
    }
    pub fn validate_key(&self, key: &LocalSharedStatePackageKey) -> Result<()> {
        check(self.generations.len() == 1)?;
        check(
            generation_commitment(self.genesis.context, key.protected_storage_bytes())
                == self.genesis.generation_commitment,
        )
    }
    pub fn contains_head(&self, head: &Hash) -> bool {
        self.heads.contains(head)
    }
    fn member(&self, device: &Hash) -> Result<&Member> {
        self.members
            .iter()
            .find(|m| &m.device == device)
            .context("error membership-signer")
    }
    fn unique(&self, recipient: &Recipient, handle: &Hash) -> Result<()> {
        check(self.members.len() < MAX_DEVICES && !self.handles.contains(handle))?;
        check(self.members.iter().chain(&self.retired).all(|m| {
            m.device != recipient.device && m.sign != recipient.sign && m.hpke != recipient.hpke
        }))
    }
    /// Validate one exact successor without mutating the trusted predecessor.
    /// Expiry, current bearer authorization and atomic server CAS are separate.
    pub fn append(&self, declaration: &[u8], request: &[u8], record: &[u8]) -> Result<Self> {
        let (core, _, _, _) = encoding::components(record)?;
        let action = encoding::action(core)?;
        let mut next = match action {
            3 => {
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
                next
            }
            4 | 5 => {
                check(declaration.is_empty() && request.is_empty())?;
                rotation::validate(self, record)?
            }
            _ => anyhow::bail!("error membership-action"),
        };
        next.heads.push(hash(record));
        next.evidence_bytes = self
            .evidence_bytes
            .checked_add(declaration.len())
            .and_then(|n| n.checked_add(request.len()))
            .and_then(|n| n.checked_add(record.len()))
            .context("error membership-limit")?;
        next.capacity()?;
        Ok(next)
    }
    fn capacity(&self) -> Result<()> {
        let reserve = usize::from(self.pending);
        check(self.heads.len() - 1 + reserve <= MAX_TRANSITIONS)?;
        check(self.generations.len() + reserve <= MAX_GENERATIONS)?;
        check(self.evidence_bytes <= MAX_CHAIN_BYTES - reserve * MAX_RECORD_BYTES)
    }
    pub fn rotation_pending(&self) -> bool {
        self.pending
    }
    pub fn generations(&self) -> &[Generation] {
        &self.generations
    }
    pub fn current_generation(&self) -> &Generation {
        self.generations.last().expect("genesis generation")
    }
    /// Signed interval eligibility, not proof of server acceptance or completeness.
    pub fn generation_allows(&self, generation: Hash, sequence: u64) -> bool {
        self.generations
            .iter()
            .position(|g| g.id == generation)
            .is_some_and(|i| {
                sequence > self.generations[i].starts_after
                    && self
                        .generations
                        .get(i + 1)
                        .is_none_or(|g| sequence <= g.starts_after)
            })
    }
}

/// A verified ancestor context may be refreshed once; it never authorizes content.
#[derive(Debug)]
pub struct StaleContext;
impl fmt::Display for StaleContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("error enrollment-context-stale")
    }
}
impl std::error::Error for StaleContext {}
