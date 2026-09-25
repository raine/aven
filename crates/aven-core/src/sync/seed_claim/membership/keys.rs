//! Complete commitment-checked key coverage, independent of host readiness.
use super::*;

/// Construction requires every generation in authenticated ancestry order.
/// This value does not assert protected persistence, server commit or freshness.
/// Cloning preserves the verified coverage and zeroizes each owned copy on drop.
#[derive(Clone)]
pub struct VerifiedKeys {
    vault: Hash,
    generations: Vec<Generation>,
    keys: Vec<LocalSharedStatePackageKey>,
}
impl VerifiedKeys {
    pub fn key(&self, generation: Hash) -> Result<&LocalSharedStatePackageKey> {
        let index = self
            .generations
            .iter()
            .position(|g| g.id == generation)
            .context("error membership-key-missing")?;
        Ok(&self.keys[index])
    }
    pub fn validate(&self, m: &Membership) -> Result<()> {
        check(self.vault == m.genesis.context.vault_id && self.generations == m.generations)
    }
    pub(super) fn write(&self, out: &mut Vec<u8>) {
        out.extend((self.keys.len() as u16).to_be_bytes());
        for (g, key) in self.generations.iter().zip(&self.keys) {
            out.extend(g.id);
            out.extend(key.protected_storage_bytes());
            out.extend(g.starts_after.to_be_bytes());
        }
    }
    pub(super) fn read(m: &Membership, r: &mut Reader<'_>) -> Result<Self> {
        check(u16::from_be_bytes(r.array()?) as usize == m.generations.len())?;
        let mut keys = Vec::with_capacity(m.generations.len());
        for g in &m.generations {
            check(r.array::<32>()? == g.id)?;
            let key = LocalSharedStatePackageKey::new(r.array()?);
            check(r.array::<8>()? == g.starts_after.to_be_bytes())?;
            check(
                generation_commitment(
                    LocalSharedStatePackageContext {
                        vault_id: m.genesis.context.vault_id,
                        generation_id: g.id,
                    },
                    key.protected_storage_bytes(),
                ) == g.commitment,
            )?;
            keys.push(key);
        }
        check(r.0.is_empty())?;
        Ok(Self {
            vault: m.genesis.context.vault_id,
            generations: m.generations.clone(),
            keys,
        })
    }
    pub(super) fn extended(&self, m: &Membership, key: LocalSharedStatePackageKey) -> Result<Self> {
        check(
            self.vault == m.genesis.context.vault_id
                && m.generations.len() == self.generations.len() + 1
                && m.generations.starts_with(&self.generations),
        )?;
        let mut bytes = Zeroizing::new(Vec::new());
        bytes.extend((m.generations.len() as u16).to_be_bytes());
        for (g, key) in self
            .generations
            .iter()
            .zip(&self.keys)
            .chain(std::iter::once((m.current_generation(), &key)))
        {
            bytes.extend(g.id);
            bytes.extend(key.protected_storage_bytes());
            bytes.extend(g.starts_after.to_be_bytes());
        }
        Self::read(m, &mut Reader(&bytes))
    }
}
impl Membership {
    /// Initial-generation-only construction for existing protected seed authority.
    pub fn verify_initial_key(&self, key: &LocalSharedStatePackageKey) -> Result<VerifiedKeys> {
        self.validate_key(key)?;
        Ok(VerifiedKeys {
            vault: self.genesis.context.vault_id,
            generations: self.generations.clone(),
            keys: vec![key.clone()],
        })
    }
}

/// Protected coverage holding every generation key.
pub const MAX_COVERAGE_BYTES: usize = 40 + GENERATION_BYTES * MAX_GENERATIONS;
impl VerifiedKeys {
    /// Installation-bound protected storage only, never public evidence or SQLite.
    pub fn protected_storage_bytes(&self) -> Zeroizing<Vec<u8>> {
        let mut out = Zeroizing::new(b"AVKC\0\x01".to_vec());
        out.extend(self.vault);
        self.write(&mut out);
        out
    }
    /// Revalidate complete commitments against authenticated membership on every load.
    pub fn from_protected_storage(m: &Membership, bytes: &[u8]) -> Result<Self> {
        check(bytes.len() <= MAX_COVERAGE_BYTES)?;
        let mut r = Reader(bytes);
        check(r.take(6)? == b"AVKC\0\x01" && r.array::<32>()? == m.genesis.context.vault_id)?;
        Self::read(m, &mut r)
    }
}
