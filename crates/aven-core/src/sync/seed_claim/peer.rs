//! Shared PSK invitation, request PoP and independent installation key primitives.
pub(crate) mod persistence;
pub use persistence::{Authentication, RegistrationStatus};

use super::*;
use crate::sync::bootstrap_format::DOMAIN_VERSION;
use ed25519_dalek::{Signature, VerifyingKey};
use hpke::PskBundle;
use serde::{Deserialize, Serialize};
use sha2::Sha256;

pub const REQUEST_BYTES: usize = 314;
pub const PEER_STORAGE_BYTES: usize = 538;
pub const CONTROL_LIMIT: usize = 16384;

fn check(ok: bool) -> Result<()> {
    ensure!(ok, "error enrollment-invalid");
    Ok(())
}

pub(super) fn verify(pk: &[u8; 32], label: &str, fields: &[&[u8]], signature: &[u8]) -> Result<()> {
    VerifyingKey::from_bytes(pk)
        .and_then(|key| key.verify_strict(&cce(label, fields), &Signature::from_slice(signature)?))
        .map_err(|_| anyhow::anyhow!("error enrollment-signature"))
}

/// Secret-bearing out-of-band handoff. Locator ownership remains with the host.
pub struct Invitation {
    pub(super) vault: [u8; 32],
    pub(super) inviter: [u8; 32],
    pub(super) psk: Secret,
}

impl fmt::Debug for Invitation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Invitation([REDACTED])")
    }
}

impl Invitation {
    pub fn vault(&self) -> [u8; 32] {
        self.vault
    }
    pub fn handle(&self) -> [u8; 32] {
        let kdf = hkdf::Hkdf::<Sha256>::new(Some(b"aven-e2ee/v1/invitation"), self.psk.expose());
        let mut out = [0; 32];
        kdf.expand(
            &cce("aven-e2ee/v1/invitation/handle", &[&self.vault]),
            &mut out,
        )
        .expect("fixed HKDF length");
        out
    }
    pub fn protected_storage_bytes(&self) -> Zeroizing<Vec<u8>> {
        let mut out = Zeroizing::new(Vec::with_capacity(96));
        out.extend(self.vault);
        out.extend(self.inviter);
        out.extend(self.psk.expose());
        out
    }
    pub fn from_protected_storage(bytes: &[u8]) -> Result<Self> {
        check(bytes.len() == 96)?;
        let mut r = Reader(bytes);
        Ok(Self {
            vault: r.array()?,
            inviter: r.array()?,
            psk: Secret::new(r.array()?),
        })
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Recipient {
    pub(super) device: [u8; 32],
    pub(super) sign: [u8; 32],
    pub(super) hpke: [u8; 32],
    pub(super) verifier: [u8; 32],
    pub(super) pop: [u8; 64],
}
impl Recipient {
    pub(super) fn pop_message(
        &self,
        vault: &[u8; 32],
        handle: &[u8; 32],
        inviter: &[u8; 32],
    ) -> Vec<u8> {
        cce(
            "aven-e2ee/v1/pairing/request-pop",
            &[
                vault,
                handle,
                inviter,
                &self.device,
                &self.sign,
                &self.hpke,
                &self.verifier,
                &1_u32.to_be_bytes(),
                &DOMAIN_VERSION.to_be_bytes(),
            ],
        )
    }
    pub(super) fn verify(
        &self,
        vault: &[u8; 32],
        handle: &[u8; 32],
        inviter: &[u8; 32],
    ) -> Result<()> {
        VerifyingKey::from_bytes(&self.sign)
            .and_then(|key| {
                key.verify_strict(
                    &self.pop_message(vault, handle, inviter),
                    &Signature::from_bytes(&self.pop),
                )
            })
            .map_err(|_| anyhow::anyhow!("error enrollment-pop"))
    }
    pub(super) fn plaintext(&self) -> Vec<u8> {
        let mut out = vec![1];
        for v in [self.device, self.sign, self.hpke, self.verifier] {
            bytes(&mut out, &v);
        }
        out.extend(1_u32.to_be_bytes());
        out.extend(DOMAIN_VERSION.to_be_bytes());
        bytes(&mut out, &self.pop);
        out
    }
    pub(super) fn parse(raw: &[u8]) -> Result<Self> {
        check(raw.len() == 221)?;
        let mut r = Reader(raw);
        check(r.take(1)? == [1])?;
        let device = r.blob(32)?.try_into()?;
        let sign = r.blob(32)?.try_into()?;
        let hpke = r.blob(32)?.try_into()?;
        let verifier = r.blob(32)?.try_into()?;
        check(
            u32::from_be_bytes(r.array()?) == 1 && u32::from_be_bytes(r.array()?) == DOMAIN_VERSION,
        )?;
        let pop = r.blob(64)?.try_into()?;
        r.end()?;
        Ok(Self {
            device,
            sign,
            hpke,
            verifier,
            pop,
        })
    }
}

pub(super) fn seal(
    inv: &Invitation,
    public: &[u8; 32],
    info: &[u8],
    aad: &[u8],
    plaintext: &[u8],
) -> Result<(Vec<u8>, Vec<u8>)> {
    let entropy = Secret::generate()?;
    let mut rng = ChaCha20Rng::from_seed(*entropy.expose());
    let pk = <Kem as hpke::Kem>::PublicKey::from_bytes(public)
        .map_err(|_| anyhow::anyhow!("error enrollment-recipient"))?;
    let handle = inv.handle();
    let mode = OpModeS::Psk(
        PskBundle::new(inv.psk.expose(), &handle)
            .map_err(|_| anyhow::anyhow!("error enrollment-psk"))?,
    );
    let (enc, cipher) = hpke::single_shot_seal_with_rng::<Aead, Kdf, Kem>(
        &mode, &pk, info, plaintext, aad, &mut rng,
    )
    .map_err(|_| anyhow::anyhow!("error enrollment-seal"))?;
    Ok((enc.to_bytes().to_vec(), cipher))
}
pub(super) fn open(
    inv: &Invitation,
    private: &Secret,
    info: &[u8],
    aad: &[u8],
    enc: &[u8],
    cipher: &[u8],
) -> Result<Zeroizing<Vec<u8>>> {
    let private = HpkePrivate::from_bytes(private.expose())
        .map_err(|_| anyhow::anyhow!("error enrollment-recipient"))?;
    let enc =
        Enc::from_bytes(enc).map_err(|_| anyhow::anyhow!("error enrollment-encapsulation"))?;
    let handle = inv.handle();
    let mode = OpModeR::Psk(
        PskBundle::new(inv.psk.expose(), &handle)
            .map_err(|_| anyhow::anyhow!("error enrollment-psk"))?,
    );
    hpke::single_shot_open::<Aead, Kdf, Kem>(&mode, &private, &enc, info, cipher, aad)
        .map(Zeroizing::new)
        .map_err(|_| anyhow::anyhow!("error enrollment-open"))
}
pub(super) fn request_parts<'a>(raw: &'a [u8], handle: &[u8; 32]) -> Result<(&'a [u8], &'a [u8])> {
    check(raw.len() == REQUEST_BYTES)?;
    let mut r = Reader(raw);
    check(r.take(1)? == [1])?;
    check(r.blob(32)? == handle)?;
    let enc = r.blob(32)?;
    let cipher = r.blob(237)?;
    r.end()?;
    Ok((enc, cipher))
}

/// Independent peer authority. Exact storage includes its one frozen request.
pub struct PeerAuthority {
    pub(super) device: [u8; 32],
    pub(super) signing: Secret,
    pub(super) recipient: Secret,
    pub(super) bearer: Secret,
    pub(super) invitation: Invitation,
    pub(super) request: Vec<u8>,
}
impl fmt::Debug for PeerAuthority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PeerAuthority([REDACTED])")
    }
}
impl PeerAuthority {
    pub fn generate(invitation: Invitation) -> Result<Self> {
        let ikm = Secret::generate()?;
        let (recipient, _) = <Kem as hpke::Kem>::derive_keypair(ikm.expose());
        let mut peer = Self {
            device: *Secret::generate()?.expose(),
            signing: Secret::generate()?,
            recipient: Secret::new(recipient.to_bytes().into()),
            bearer: Secret::generate()?,
            invitation,
            request: Vec::new(),
        };
        let row = peer.recipient()?;
        let handle = peer.invitation.handle();
        let info = cce(
            "aven-e2ee/v1/pairing/request",
            &[&peer.invitation.vault, &handle],
        );
        let aad = cce(
            "aven-e2ee/v1/pairing/request-aad",
            &[&peer.invitation.vault, &handle, &peer.invitation.inviter],
        );
        let (enc, cipher) = seal(
            &peer.invitation,
            &peer.invitation.inviter,
            &info,
            &aad,
            &row.plaintext(),
        )?;
        let mut request = vec![1];
        bytes(&mut request, &handle);
        bytes(&mut request, &enc);
        bytes(&mut request, &cipher);
        peer.request = request;
        Ok(peer)
    }
    pub(super) fn recipient(&self) -> Result<Recipient> {
        let private = HpkePrivate::from_bytes(self.recipient.expose())
            .map_err(|_| anyhow::anyhow!("error enrollment-recipient"))?;
        let signer = SigningKey::from_bytes(self.signing.expose());
        let mut row = Recipient {
            device: self.device,
            sign: signer.verifying_key().to_bytes(),
            hpke: <Kem as hpke::Kem>::sk_to_pk(&private).to_bytes().into(),
            verifier: credential_verifier(self.invitation.vault, self.device, &self.bearer),
            pop: [0; 64],
        };
        row.pop = signer
            .sign(&row.pop_message(
                &self.invitation.vault,
                &self.invitation.handle(),
                &self.invitation.inviter,
            ))
            .to_bytes();
        Ok(row)
    }
    pub fn matches_invitation(&self, invitation: &Invitation) -> bool {
        bool::from(
            self.invitation
                .protected_storage_bytes()
                .as_slice()
                .ct_eq(invitation.protected_storage_bytes().as_slice()),
        )
    }
    pub fn device(&self) -> [u8; 32] {
        self.device
    }
    pub fn vault(&self) -> [u8; 32] {
        self.invitation.vault
    }
    pub fn handle(&self) -> [u8; 32] {
        self.invitation.handle()
    }
    pub fn bearer(&self) -> &Secret {
        &self.bearer
    }
    pub fn request(&self) -> &[u8] {
        &self.request
    }
    pub fn protected_storage_bytes(&self) -> Zeroizing<Vec<u8>> {
        let mut out = Zeroizing::new(Vec::new());
        out.extend(self.device);
        for secret in [&self.signing, &self.recipient, &self.bearer] {
            out.extend(secret.expose());
        }
        out.extend(self.invitation.protected_storage_bytes().iter());
        out.extend(&self.request);
        out
    }
    pub fn from_protected_storage(raw: &[u8]) -> Result<Self> {
        check(raw.len() == PEER_STORAGE_BYTES)?;
        let mut r = Reader(raw);
        let peer = Self {
            device: r.array()?,
            signing: Secret::new(r.array()?),
            recipient: Secret::new(r.array()?),
            bearer: Secret::new(r.array()?),
            invitation: Invitation::from_protected_storage(r.take(96)?)?,
            request: r.take(REQUEST_BYTES)?.to_vec(),
        };
        request_parts(&peer.request, &peer.handle())?;
        peer.recipient()?;
        Ok(peer)
    }
}
