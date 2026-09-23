//! Fixed genesis-to-PublishBootstrap signed profile. These exact format versions
//! are provisional protocol boundaries, not a released interoperability contract.
//! Signing prepares intent only; it neither persists dispatch ownership nor adopts
//! membership. Hosts must not dispatch from a never-dispatched capture journal.
//!
//! # Fixed publication profile
//!
//! Membership byte strings use U32 big-endian lengths, not catalog U64 framing.
//! T is bootstrap[32], stream[32], SHA256(exact descriptor)[32], manifest ordered
//! ciphertext aggregate[32], U64(N). The 416-byte resulting state is genesis state
//! with AVGS version 2, bootstrap-present=1 and T immediately after that flag. All
//! remaining genesis fields are unchanged. Action is AVPB, U16(1), T (142 bytes).
//! The 301-byte core retains genesis framing with sequence=1, predecessor=genesis
//! commitment, device signer=seed, action=2 and the resulting state commitment.
//! Attachments are exactly AVGA, U16(2), U8(0 count), seven bytes with no grants.
//! Outer record version 1 frames core, state, attachments and signature (805 bytes).
//! State/signature CCE labels and whole-record SHA256 are unchanged from genesis.
//! No other successors, counts, state flags or grants are supported. Epoch is an
//! unsigned first-admission precondition, never part of immutable signed intent.

use super::*;
use crate::sync::bootstrap_format::{self, staging::DeclarationView};

pub const PUBLICATION_BYTES: usize = 805;

/// Client-authorized immutable bootstrap binding. Not proof of server acceptance.
#[derive(Clone, PartialEq, Eq)]
pub struct Publication {
    record: Box<[u8; PUBLICATION_BYTES]>,
    pub(crate) binding: PublicationBinding,
}

impl fmt::Debug for Publication {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Publication")
            .field("commitment", &self.commitment())
            .field("binding", &self.binding)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PublicationBinding {
    pub vault_id: [u8; 32],
    pub genesis_commitment: [u8; 32],
    pub bootstrap_id: [u8; 32],
    pub stream_id: [u8; 32],
    pub descriptor_commitment: [u8; 32],
    pub manifest_commitment: [u8; 32],
    pub prefix_count: u64,
}

impl PublicationBinding {
    pub(crate) fn from_descriptor(genesis: &Genesis, descriptor: &[u8]) -> Result<Self> {
        let d = DeclarationView::decode(descriptor)?;
        let b = d.binding();
        ensure!(
            b.vault == genesis.context.vault_id
                && b.generation == genesis.context.generation_id
                && b.membership == genesis.commitment(),
            "error bootstrap-publication-context"
        );
        Ok(Self {
            vault_id: b.vault,
            genesis_commitment: b.membership,
            bootstrap_id: b.bootstrap,
            stream_id: b.stream,
            descriptor_commitment: hash(descriptor),
            manifest_commitment: d.manifest_commitment(),
            prefix_count: d.prefix_count(),
        })
    }

    fn tuple(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(136);
        for value in [
            self.bootstrap_id,
            self.stream_id,
            self.descriptor_commitment,
            self.manifest_commitment,
        ] {
            out.extend(value);
        }
        out.extend(self.prefix_count.to_be_bytes());
        out
    }
}

fn components(genesis: &Genesis, binding: &PublicationBinding) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let original = genesis.state();
    let mut state = b"AVGS\0\x02\x01".to_vec();
    state.extend(genesis.context.vault_id);
    state.push(1);
    state.extend(binding.tuple());
    // The remaining authority and generation fields retain their genesis bytes.
    state.extend_from_slice(&original[40..]);
    let mut body = b"AVPB\0\x01".to_vec();
    body.extend(binding.tuple());
    let mut core = vec![1];
    bytes(&mut core, &genesis.context.vault_id);
    core.extend(1_u64.to_be_bytes());
    bytes(&mut core, &genesis.commitment());
    core.push(1);
    bytes(&mut core, &genesis.device);
    core.push(2);
    bytes(&mut core, &body);
    bytes(
        &mut core,
        &hash(&cce("aven-e2ee/v1/membership/state", &[&state])),
    );
    (core, state, b"AVGA\0\x02\0".to_vec())
}

impl Publication {
    /// Verifies exact successor semantics against a trusted genesis and descriptor.
    /// A server must additionally authenticate current membership and admission.
    pub fn from_record(genesis: &Genesis, descriptor: &[u8], record: &[u8]) -> Result<Self> {
        ensure!(
            record.len() == PUBLICATION_BYTES,
            "error bootstrap-publication-invalid"
        );
        let binding = PublicationBinding::from_descriptor(genesis, descriptor)?;
        let (core, state, attachments) = components(genesis, &binding);
        let mut r = Reader(record);
        valid(r.take(1)? == [1])?;
        valid(r.blob(301)? == core)?;
        valid(r.blob(416)? == state)?;
        valid(r.blob(7)? == attachments)?;
        let signature = ed25519_dalek::Signature::from_slice(r.blob(64)?)?;
        r.end()?;
        ed25519_dalek::VerifyingKey::from_bytes(&genesis.signing_public)?
            .verify_strict(
                &cce("aven-e2ee/v1/membership/sign", &[&core, &attachments]),
                &signature,
            )
            .map_err(|_| anyhow::anyhow!("error bootstrap-publication-signature"))?;
        Ok(Self {
            record: Box::new(record.try_into()?),
            binding,
        })
    }

    pub fn record(&self) -> &[u8; PUBLICATION_BYTES] {
        &self.record
    }
    pub fn commitment(&self) -> [u8; 32] {
        hash(self.record.as_slice())
    }
    pub fn binding(&self) -> &PublicationBinding {
        &self.binding
    }
}

impl SeedAuthority {
    /// Authenticates the frozen package before signing bounded publication intent.
    /// No persistence, dispatch, checkpoint advancement or local adoption occurs.
    pub fn prepare_bootstrap_publication(
        &self,
        package: &bootstrap_format::Package,
        key: &LocalSharedStatePackageKey,
    ) -> Result<Publication> {
        self.validate(self.genesis.context, key)?;
        let binding = PublicationBinding::from_descriptor(&self.genesis, &package.descriptor)?;
        bootstrap_format::authenticate(
            package,
            key,
            self.genesis.context,
            binding.stream_id,
            binding.bootstrap_id,
            self.genesis.commitment(),
        )?;
        let (core, state, attachments) = components(&self.genesis, &binding);
        let signature = SigningKey::from_bytes(self.signing.expose())
            .sign(&cce("aven-e2ee/v1/membership/sign", &[&core, &attachments]));
        let mut record = vec![1];
        for part in [&core[..], &state, &attachments, &signature.to_bytes()] {
            bytes(&mut record, part);
        }
        Publication::from_record(&self.genesis, &package.descriptor, &record)
    }
}

/// Retained server acceptance of exact signed intent. Not physical durability,
/// current membership, or permission to adopt a local database.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicationOutcome {
    pub(crate) publication: Publication,
}

impl PublicationOutcome {
    pub fn publication(&self) -> &Publication {
        &self.publication
    }

    /// Authenticate the signed binding against pinned genesis and local expectation.
    /// Never replace that expectation with a descriptor supplied by the server.
    pub fn validate_expected(&self, genesis: &Genesis, expected_descriptor: &[u8]) -> Result<()> {
        let expected =
            Publication::from_record(genesis, expected_descriptor, self.publication.record())?;
        ensure!(
            expected == self.publication,
            "error bootstrap-publication-conflict"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests;
