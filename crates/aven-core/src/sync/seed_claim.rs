//! Sequence-zero membership and one-vault claim, without transport.
//!
//! The fixed genesis profile uses strict Ed25519 and an HPKE base-mode self grant.
//! Operator setup authority, not the self signature, admits the first device.
//! Public validation cannot prove the encrypted self grant is usable. Hosts must
//! verify it and persist exact authority outside replaceable databases before use.
//! A fixed signed publication successor preserves genesis authority. Neither
//! preparation nor server acceptance enables client sync or advances a local
//! protected checkpoint.

mod codec;
pub mod membership;
pub mod peer;
mod persistence;
mod publication;

pub use publication::{PUBLICATION_BYTES, Publication, PublicationBinding, PublicationOutcome};

use std::fmt;

use anyhow::{Context, Result, ensure};
#[cfg(test)]
use ed25519_dalek::{Signer, SigningKey};
use zeroize::Zeroizing;

use super::{LocalSharedStatePackageContext, LocalSharedStatePackageKey};
#[cfg(test)]
use codec::{bytes, cce, hash};

pub use aven_protocol::claim::{
    CLAIM_BYTES, ClaimAuthentication, ClaimRefusal, ClaimResult, GENESIS_BYTES, Genesis,
    SEED_STORAGE_BYTES, Secret, SetupAuthority, StorageNotEmpty,
};

/// Protected seed authority with local package publication operations.
/// Construction alone is not durable; hosts must save before making a request.
pub struct SeedAuthority(aven_protocol::claim::SeedAuthority);

impl fmt::Debug for SeedAuthority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl std::ops::Deref for SeedAuthority {
    type Target = aven_protocol::claim::SeedAuthority;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl SeedAuthority {
    pub fn genesis(&self) -> &Genesis {
        self.0.genesis()
    }

    pub fn bearer(&self) -> &Secret {
        self.0.bearer()
    }

    /// Exact protected-storage representation, never SQLite or ordinary backups.
    pub fn protected_storage_bytes(&self) -> Zeroizing<Vec<u8>> {
        self.0.protected_storage_bytes()
    }

    fn validate(
        &self,
        context: LocalSharedStatePackageContext,
        key: &LocalSharedStatePackageKey,
    ) -> Result<()> {
        self.0.validate(context, key)
    }

    pub fn generate(
        context: LocalSharedStatePackageContext,
        key: &LocalSharedStatePackageKey,
        setup: [u8; 32],
    ) -> Result<Self> {
        aven_protocol::claim::SeedAuthority::generate(context, key, setup).map(Self)
    }

    pub fn from_protected_storage(
        raw: &[u8],
        context: LocalSharedStatePackageContext,
        key: &LocalSharedStatePackageKey,
    ) -> Result<Self> {
        aven_protocol::claim::SeedAuthority::from_protected_storage(raw, context, key).map(Self)
    }
}

#[cfg(test)]
mod tests;
