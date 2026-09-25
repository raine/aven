//! Sequence-zero membership and one-vault claim, without transport.
//!
//! The fixed genesis profile uses strict Ed25519 and an HPKE base-mode self grant.
//! Operator setup authority, not the self signature, admits the first device.
//! Public validation cannot prove the encrypted self grant is usable. Hosts must
//! verify it and persist exact authority outside replaceable databases before use.
//! A fixed signed publication successor preserves genesis authority. Neither
//! preparation nor server acceptance enables client sync or advances a local
//! protected checkpoint. Exact profiles remain provisional format boundaries.

mod codec;
pub mod membership;
pub mod peer;
mod persistence;
mod publication;

pub use publication::{PUBLICATION_BYTES, Publication, PublicationBinding, PublicationOutcome};

use std::fmt;

use anyhow::{Context, Result, ensure};
use chacha20::ChaCha20Rng;
use ed25519_dalek::{Signer, SigningKey};
use hpke::{Deserializable, OpModeR, OpModeS, Serializable};
use rand_core::SeedableRng;
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

use super::{LocalSharedStatePackageContext, LocalSharedStatePackageKey};
use codec::{Reader, bytes, cce, hash, valid};

type Kem = hpke::kem::X25519HkdfSha256;
type Aead = hpke::aead::ChaCha20Poly1305;
type Kdf = hpke::kdf::HkdfSha256;
type HpkePrivate = <Kem as hpke::Kem>::PrivateKey;
type Enc = <Kem as hpke::Kem>::EncappedKey;

pub const GENESIS_BYTES: usize = 902;
pub const CLAIM_BYTES: usize = 912;
/// Signing seed, HPKE private key, token and exact public genesis. Host-only storage.
pub const SEED_STORAGE_BYTES: usize = 96 + GENESIS_BYTES;

/// A high-entropy secret with redacted diagnostics. Never serialize into task data.
pub struct Secret(Zeroizing<[u8; 32]>);

impl Secret {
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(Zeroizing::new(bytes))
    }

    pub fn generate() -> Result<Self> {
        let mut bytes = Zeroizing::new([0; 32]);
        getrandom::fill(bytes.as_mut()).context("error seed-claim-entropy-unavailable")?;
        Ok(Self(bytes))
    }

    /// For protected persistence or authenticated transport only, never logs.
    pub fn expose(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([REDACTED])")
    }
}

/// Immutable, publicly verified sequence-zero record. Not proof of admission.
#[derive(Clone, PartialEq, Eq)]
pub struct Genesis {
    record: [u8; GENESIS_BYTES],
    context: LocalSharedStatePackageContext,
    setup: [u8; 32],
    claim: [u8; 32],
    device: [u8; 32],
    signing_public: [u8; 32],
    hpke_public: [u8; 32],
    verifier: [u8; 32],
    generation_commitment: [u8; 32],
}

impl fmt::Debug for Genesis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Genesis")
            .field("commitment", &self.commitment())
            .finish()
    }
}

impl Genesis {
    pub fn from_claim(bytes: &[u8]) -> Result<Self> {
        Self::from_record(codec::claim_record(bytes)?)
    }

    pub fn from_record(record: &[u8]) -> Result<Self> {
        codec::parse(record)
    }

    pub fn record(&self) -> &[u8; GENESIS_BYTES] {
        &self.record
    }

    pub fn commitment(&self) -> [u8; 32] {
        hash(&self.record)
    }

    pub(crate) fn authorizes_bearer(&self, token: &Secret) -> bool {
        bool::from(
            credential_verifier(self.context.vault_id, self.device, token).ct_eq(&self.verifier),
        )
    }

    pub fn context(&self) -> LocalSharedStatePackageContext {
        self.context
    }

    pub fn setup_id(&self) -> [u8; 32] {
        self.setup
    }

    pub fn claim_id(&self) -> [u8; 32] {
        self.claim
    }

    pub fn device_id(&self) -> [u8; 32] {
        self.device
    }

    pub fn claim_bytes(&self) -> Vec<u8> {
        let mut out = b"AVCL\0\x01".to_vec();
        bytes(&mut out, &self.record);
        out
    }

    fn info(&self) -> Vec<u8> {
        cce(
            "aven-e2ee/v1/genesis/self",
            &[
                &1_u16.to_be_bytes(),
                &[1],
                &self.context.vault_id,
                &self.setup,
                &self.claim,
                &self.device,
                &self.hpke_public,
                &self.context.generation_id,
            ],
        )
    }

    fn state(&self) -> Vec<u8> {
        let mut out = b"AVGS\0\x01\x01".to_vec();
        out.extend(self.context.vault_id);
        out.extend([0, 0, 0, 1]);
        out.extend(self.device);
        out.extend(self.signing_public);
        out.extend(self.hpke_public);
        out.extend(self.verifier);
        out.extend(1_u32.to_be_bytes());
        out.extend(self.claim);
        out.push(1);
        out.extend(self.context.generation_id);
        out.extend(self.generation_commitment);
        out.extend(0_u64.to_be_bytes());
        out
    }

    fn core(&self, state: &[u8]) -> Vec<u8> {
        let mut body = b"AVGC\0\x01".to_vec();
        body.extend(self.setup);
        body.extend(self.claim);
        let mut out = vec![1];
        bytes(&mut out, &self.context.vault_id);
        out.extend(0_u64.to_be_bytes());
        bytes(&mut out, &[0; 32]);
        out.push(1);
        bytes(&mut out, &self.device);
        out.push(1);
        bytes(&mut out, &body);
        bytes(
            &mut out,
            &hash(&cce("aven-e2ee/v1/membership/state", &[state])),
        );
        out
    }
}

/// Private seed material and pinned exact genesis, validated against package authority.
/// Construction alone is not durable. The host must save before making a request.
pub struct SeedAuthority {
    signing: Secret,
    recipient: Secret,
    bearer: Secret,
    genesis: Genesis,
}

impl fmt::Debug for SeedAuthority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SeedAuthority([REDACTED])")
    }
}

impl SeedAuthority {
    pub fn generate(
        context: LocalSharedStatePackageContext,
        key: &LocalSharedStatePackageKey,
        setup: [u8; 32],
    ) -> Result<Self> {
        let signing = Secret::generate()?;
        let ikm = Secret::generate()?;
        let (recipient, _) = <Kem as hpke::Kem>::derive_keypair(ikm.expose());
        let bearer = Secret::generate()?;
        let claim = *Secret::generate()?.expose();
        let device = *Secret::generate()?.expose();
        let entropy = Secret::generate()?;
        let mut rng = ChaCha20Rng::from_seed(*entropy.expose());
        Self::build(
            context, key, setup, claim, device, signing, recipient, bearer, &mut rng,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        context: LocalSharedStatePackageContext,
        key: &LocalSharedStatePackageKey,
        setup: [u8; 32],
        claim: [u8; 32],
        device: [u8; 32],
        signing: Secret,
        recipient: HpkePrivate,
        bearer: Secret,
        rng: &mut impl rand_core::CryptoRng,
    ) -> Result<Self> {
        let signer = SigningKey::from_bytes(signing.expose());
        let hpke_public = <Kem as hpke::Kem>::sk_to_pk(&recipient);
        let mut genesis = Genesis {
            record: [0; GENESIS_BYTES],
            context,
            setup,
            claim,
            device,
            signing_public: signer.verifying_key().to_bytes(),
            hpke_public: hpke_public.to_bytes().into(),
            verifier: credential_verifier(context.vault_id, device, &bearer),
            generation_commitment: generation_commitment(context, key.protected_storage_bytes()),
        };
        let state = genesis.state();
        let core = genesis.core(&state);
        let plaintext = self_plaintext(&genesis, key.protected_storage_bytes());
        let (enc, ciphertext) = hpke::single_shot_seal_with_rng::<Aead, Kdf, Kem>(
            &OpModeS::Base,
            &hpke_public,
            &genesis.info(),
            &plaintext,
            &core,
            rng,
        )
        .map_err(|_| anyhow::anyhow!("error seed-claim-self-seal"))?;
        let mut attachments = b"AVGA\0\x01\x01\x01\x01".to_vec();
        bytes(&mut attachments, &device);
        bytes(&mut attachments, &genesis.hpke_public);
        bytes(&mut attachments, &enc.to_bytes());
        bytes(&mut attachments, &ciphertext);
        let signature = signer.sign(&cce("aven-e2ee/v1/membership/sign", &[&core, &attachments]));
        let mut record = vec![1];
        for component in [&core[..], &state, &attachments, &signature.to_bytes()] {
            bytes(&mut record, component);
        }
        genesis = Genesis::from_record(&record)?;
        let authority = Self {
            signing,
            recipient: Secret::new(recipient.to_bytes().into()),
            bearer,
            genesis,
        };
        authority.validate(context, key)?;
        Ok(authority)
    }

    pub fn genesis(&self) -> &Genesis {
        &self.genesis
    }

    pub fn bearer(&self) -> &Secret {
        &self.bearer
    }

    /// Exact protected-storage representation, never SQLite or ordinary backups.
    pub fn protected_storage_bytes(&self) -> Zeroizing<Vec<u8>> {
        let mut out = Zeroizing::new(Vec::with_capacity(SEED_STORAGE_BYTES));
        out.extend_from_slice(self.signing.expose());
        out.extend_from_slice(self.recipient.expose());
        out.extend_from_slice(self.bearer.expose());
        out.extend_from_slice(self.genesis.record());
        out
    }

    pub fn from_protected_storage(
        raw: &[u8],
        context: LocalSharedStatePackageContext,
        key: &LocalSharedStatePackageKey,
    ) -> Result<Self> {
        valid(raw.len() == SEED_STORAGE_BYTES)?;
        let mut reader = Reader(raw);
        let signing = Secret::new(reader.array()?);
        let recipient = Secret::new(reader.array()?);
        let bearer = Secret::new(reader.array()?);
        let genesis = Genesis::from_record(reader.take(GENESIS_BYTES)?)?;
        reader.end()?;
        let authority = Self {
            signing,
            recipient,
            bearer,
            genesis,
        };
        authority.validate(context, key)?;
        Ok(authority)
    }

    fn validate(
        &self,
        context: LocalSharedStatePackageContext,
        key: &LocalSharedStatePackageKey,
    ) -> Result<()> {
        let g = &self.genesis;
        valid(g.context == context)?;
        valid(
            SigningKey::from_bytes(self.signing.expose())
                .verifying_key()
                .to_bytes()
                == g.signing_public,
        )?;
        valid(bool::from(
            credential_verifier(context.vault_id, g.device, &self.bearer).ct_eq(&g.verifier),
        ))?;
        valid(
            generation_commitment(context, key.protected_storage_bytes())
                == g.generation_commitment,
        )?;
        let private = HpkePrivate::from_bytes(self.recipient.expose())
            .map_err(|_| anyhow::anyhow!("error seed-claim-recipient"))?;
        valid(<Kem as hpke::Kem>::sk_to_pk(&private).to_bytes().as_slice() == g.hpke_public)?;
        let (core, _, attachments, _) = codec::components(&g.record)?;
        let mut reader = Reader(attachments);
        reader.take(9)?;
        reader.blob(32)?;
        reader.blob(32)?;
        let enc = Enc::from_bytes(reader.blob(32)?)
            .map_err(|_| anyhow::anyhow!("error seed-claim-encapsulation"))?;
        let ciphertext = reader.blob(191)?;
        reader.end()?;
        let plaintext = Zeroizing::new(
            hpke::single_shot_open::<Aead, Kdf, Kem>(
                &OpModeR::Base,
                &private,
                &enc,
                &g.info(),
                ciphertext,
                core,
            )
            .map_err(|_| anyhow::anyhow!("error seed-claim-self-open"))?,
        );
        valid(bool::from(
            plaintext
                .as_slice()
                .ct_eq(&self_plaintext(g, key.protected_storage_bytes())),
        ))
    }
}

fn self_plaintext(g: &Genesis, secret: &[u8; 32]) -> Zeroizing<Vec<u8>> {
    let mut out = Zeroizing::new(b"AVGK\0\x01\x01".to_vec());
    for value in [
        &g.context.vault_id,
        &g.claim,
        &g.device,
        &g.context.generation_id,
        secret,
    ] {
        out.extend_from_slice(value);
    }
    out.extend_from_slice(&0_u64.to_be_bytes());
    out
}

fn generation_commitment(context: LocalSharedStatePackageContext, secret: &[u8; 32]) -> [u8; 32] {
    let message = Zeroizing::new(cce(
        "aven-e2ee/v1/generation/commit",
        &[&context.vault_id, &context.generation_id, secret],
    ));
    hash(&message)
}

fn credential_verifier(vault: [u8; 32], device: [u8; 32], token: &Secret) -> [u8; 32] {
    let message = Zeroizing::new(cce(
        "aven-e2ee/v1/credential/verifier",
        &[&vault, &device, &1_u32.to_be_bytes(), token.expose()],
    ));
    hash(&message)
}

/// Operator-configured public verifier. Never accept this value from a claimant.
#[derive(Clone, Debug)]
pub struct SetupAuthority {
    id: [u8; 32],
    verifier: [u8; 32],
}

impl SetupAuthority {
    pub fn from_verifier(id: [u8; 32], verifier: [u8; 32]) -> Self {
        Self { id, verifier }
    }

    pub fn verifier(id: [u8; 32], secret: &Secret) -> [u8; 32] {
        let message = Zeroizing::new(cce("aven-e2ee/v1/setup/verifier", &[&id, secret.expose()]));
        hash(&message)
    }

    fn authorizes(&self, id: [u8; 32], secret: &Secret) -> bool {
        self.id == id && bool::from(Self::verifier(id, secret).ct_eq(&self.verifier))
    }
}

#[derive(Debug)]
pub enum ClaimAuthentication<'a> {
    SetupSecret(&'a Secret),
    SeedBearer(&'a Secret),
}

/// A claim refusal decided inside the claim transaction. Any other claim
/// error leaves the outcome unknown to the claimant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaimRefusal {
    /// The credential does not authorize this claim; `claimed` records
    /// whether storage already held a claim.
    Unauthorized { claimed: bool },
    /// Storage holds a different claim.
    Conflict,
    /// The stored claim was retired by publication.
    Retired,
}

impl fmt::Display for ClaimRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unauthorized { .. } => "error seed-claim-unauthorized",
            Self::Conflict => "error seed-claim-conflict",
            Self::Retired => "error seed-claim-retired",
        })
    }
}

impl std::error::Error for ClaimRefusal {}

/// Equality result, not proof of READY, active membership or physical durability.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimResult {
    pub vault_id: [u8; 32],
    pub claim_id: [u8; 32],
    pub genesis_commitment: [u8; 32],
}

impl ClaimResult {
    fn from_genesis(g: &Genesis) -> Self {
        Self {
            vault_id: g.context.vault_id,
            claim_id: g.claim,
            genesis_commitment: g.commitment(),
        }
    }

    pub fn validate_pinned(&self, pinned: &Genesis) -> Result<()> {
        ensure!(
            *self == Self::from_genesis(pinned),
            "error seed-claim-result-mismatch"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests;
