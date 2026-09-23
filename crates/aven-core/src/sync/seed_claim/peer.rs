//! Fixed first invitation admission in the existing membership chain.
//!
//! Sequence two is an internal supported subset, not a two-device product policy.
//! Pure preparation is not durable dispatch authority. Hosts retain exact bytes,
//! independent credentials and disclosure fences outside replaceable databases.
pub(crate) mod persistence;
pub use persistence::{Authentication, Mailbox, RegistrationStatus};
#[cfg(test)]
pub(crate) mod tests;

use super::*;
use crate::sync::bootstrap_format::DOMAIN_VERSION;
use ed25519_dalek::{Signature, VerifyingKey};
use hpke::PskBundle;
use serde::{Deserialize, Serialize};
use sha2::Sha256;

pub const DECLARATION_BYTES: usize = 280;
pub const REQUEST_BYTES: usize = 314;
pub const ADMISSION_BYTES: usize = 1698;
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
pub struct Declaration {
    bytes: Vec<u8>,
    pub(crate) handle: [u8; 32],
    pub(crate) expiry: u64,
}
impl Declaration {
    pub fn record(&self) -> &[u8] {
        &self.bytes
    }
    pub fn commitment(&self) -> [u8; 32] {
        hash(&self.bytes)
    }
    pub fn handle(&self) -> [u8; 32] {
        self.handle
    }
    pub fn from_record(g: &Genesis, p: &Publication, raw: &[u8]) -> Result<Self> {
        check(p.binding().genesis_commitment == g.commitment())?;
        check(raw.len() == DECLARATION_BYTES)?;
        let mut r = Reader(raw);
        check(r.take(1)? == [1])?;
        let body = r.blob(207)?;
        let sig = r.blob(64)?;
        r.end()?;
        let mut b = Reader(body);
        check(b.take(7)? == b"AVID\0\x01\x01")?;
        check(b.array::<32>()? == g.context.vault_id)?;
        check(b.array::<32>()? == g.commitment())?;
        check(b.array::<32>()? == p.commitment())?;
        check(b.array::<32>()? == g.device)?;
        check(b.array::<32>()? == g.hpke_public)?;
        let handle = b.array()?;
        let expiry = u64::from_be_bytes(b.array()?);
        b.end()?;
        verify(
            &g.signing_public,
            "aven-e2ee/v1/pairing/declaration",
            &[body],
            sig,
        )?;
        Ok(Self {
            bytes: raw.to_vec(),
            handle,
            expiry,
        })
    }
}

impl SeedAuthority {
    pub fn prepare_peer_invitation(
        &self,
        publication: &Publication,
        expires: u64,
    ) -> Result<(Invitation, Declaration)> {
        check(publication.binding.genesis_commitment == self.genesis.commitment())?;
        let invitation = Invitation {
            vault: self.genesis.context.vault_id,
            inviter: self.genesis.hpke_public,
            psk: Secret::generate()?,
        };
        let mut body = b"AVID\0\x01\x01".to_vec();
        for field in [
            invitation.vault,
            self.genesis.commitment(),
            publication.commitment(),
            self.genesis.device,
            invitation.inviter,
            invitation.handle(),
        ] {
            body.extend(field);
        }
        body.extend(expires.to_be_bytes());
        let signature = SigningKey::from_bytes(self.signing.expose())
            .sign(&cce("aven-e2ee/v1/pairing/declaration", &[&body]));
        let mut raw = vec![1];
        bytes(&mut raw, &body);
        bytes(&mut raw, &signature.to_bytes());
        Ok((
            invitation,
            Declaration::from_record(&self.genesis, publication, &raw)?,
        ))
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
    pub(super) fn row(&self, handle: [u8; 32]) -> Vec<u8> {
        let mut out = Vec::new();
        for v in [self.device, self.sign, self.hpke, self.verifier] {
            out.extend(v);
        }
        out.extend(1_u32.to_be_bytes());
        out.extend(handle);
        out
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

type RecordComponents<'a> = (&'a [u8], &'a [u8], &'a [u8], &'a [u8]);
fn components(raw: &[u8]) -> Result<RecordComponents<'_>> {
    check(raw.len() == ADMISSION_BYTES)?;
    let mut r = Reader(raw);
    check(r.take(1)? == [1])?;
    let result = (r.blob(461)?, r.blob(580)?, r.blob(576)?, r.blob(64)?);
    r.end()?;
    Ok(result)
}
fn action(
    core: &[u8],
    g: &Genesis,
    p: &Publication,
    d: &Declaration,
    request: &[u8],
) -> Result<Recipient> {
    let mut r = Reader(core);
    check(r.take(1)? == [1])?;
    check(r.blob(32)? == g.context.vault_id)?;
    check(r.take(8)? == 2_u64.to_be_bytes())?;
    check(r.blob(32)? == p.commitment())?;
    check(r.take(1)? == [1])?;
    check(r.blob(32)? == g.device)?;
    check(r.take(1)? == [3])?;
    let mut a = Reader(r.blob(302)?);
    check(a.take(6)? == b"AVAD\0\x01")?;
    check(a.array::<32>()? == d.handle)?;
    check(a.array::<32>()? == d.commitment())?;
    check(a.array::<32>()? == hash(request))?;
    let device = a.array()?;
    let sign = a.array()?;
    let hpke = a.array()?;
    let verifier = a.array()?;
    check(u32::from_be_bytes(a.array()?) == 1 && u32::from_be_bytes(a.array()?) == DOMAIN_VERSION)?;
    let pop = a.array()?;
    a.end()?;
    r.blob(32)?;
    r.end()?;
    let recipient = Recipient {
        device,
        sign,
        hpke,
        verifier,
        pop,
    };
    check(device != g.device && sign != g.signing_public && hpke != g.hpke_public)?;
    recipient.verify(&g.context.vault_id, &d.handle, &g.hpke_public)?;
    Ok(recipient)
}
fn state_core(
    g: &Genesis,
    p: &Publication,
    d: &Declaration,
    request: &[u8],
    peer: &Recipient,
) -> (Vec<u8>, Vec<u8>) {
    let (_, published, _) = super::publication::components(g, p.binding());
    let seed_row = g.state()[43..207].to_vec();
    let peer_row = peer.row(d.handle);
    let mut rows = [seed_row, peer_row];
    rows.sort();
    let mut state = published[..179].to_vec();
    state[4..6].copy_from_slice(&3_u16.to_be_bytes());
    state[178] = 2;
    for row in rows {
        state.extend(row);
    }
    state.extend_from_slice(&published[343..]);
    let mut body = b"AVAD\0\x01".to_vec();
    for v in [
        d.handle,
        d.commitment(),
        hash(request),
        peer.device,
        peer.sign,
        peer.hpke,
        peer.verifier,
    ] {
        body.extend(v);
    }
    body.extend(1_u32.to_be_bytes());
    body.extend(DOMAIN_VERSION.to_be_bytes());
    body.extend(peer.pop);
    let mut core = vec![1];
    bytes(&mut core, &g.context.vault_id);
    core.extend(2_u64.to_be_bytes());
    bytes(&mut core, &p.commitment());
    core.push(1);
    bytes(&mut core, &g.device);
    core.push(3);
    bytes(&mut core, &body);
    bytes(
        &mut core,
        &hash(&cce("aven-e2ee/v1/membership/state", &[&state])),
    );
    (state, core)
}
fn grant_parts<'a>(attachments: &'a [u8], peer: &Recipient) -> Result<(&'a [u8], &'a [u8])> {
    let mut r = Reader(attachments);
    check(r.take(9)? == b"AVGA\0\x03\x01\x01\x02")?;
    check(r.blob(32)? == peer.device)?;
    check(r.blob(32)? == peer.hpke)?;
    let mut grant = Reader(r.blob(491)?);
    r.end()?;
    check(grant.take(1)? == [1])?;
    let enc = grant.blob(32)?;
    let ciphertext = grant.blob(450)?;
    grant.end()?;
    Ok((enc, ciphertext))
}

#[derive(Clone)]
pub struct Admission {
    record: Vec<u8>,
    recipient: Recipient,
}
impl Admission {
    pub fn record(&self) -> &[u8] {
        &self.record
    }
    pub fn commitment(&self) -> [u8; 32] {
        hash(&self.record)
    }
    pub fn peer_device(&self) -> [u8; 32] {
        self.recipient.device
    }
    pub fn from_record(
        g: &Genesis,
        p: &Publication,
        d: &Declaration,
        request: &[u8],
        raw: &[u8],
    ) -> Result<Self> {
        Declaration::from_record(g, p, d.record())?;
        request_parts(request, &d.handle)?;
        let (core, state, attachments, sig) = components(raw)?;
        let peer = action(core, g, p, d, request)?;
        let (expected_state, expected_core) = state_core(g, p, d, request, &peer);
        check(state == expected_state && core == expected_core)?;
        grant_parts(attachments, &peer)?;
        verify(
            &g.signing_public,
            "aven-e2ee/v1/membership/sign",
            &[core, attachments],
            sig,
        )?;
        Ok(Self {
            record: raw.to_vec(),
            recipient: peer,
        })
    }
}
impl SeedAuthority {
    fn peer_request_recipient(&self, inv: &Invitation, request: &[u8]) -> Result<Recipient> {
        check(
            inv.vault == self.genesis.context.vault_id && inv.inviter == self.genesis.hpke_public,
        )?;
        let handle = inv.handle();
        let (enc, cipher) = request_parts(request, &handle)?;
        let info = cce("aven-e2ee/v1/pairing/request", &[&inv.vault, &handle]);
        let aad = cce(
            "aven-e2ee/v1/pairing/request-aad",
            &[&inv.vault, &handle, &inv.inviter],
        );
        let plaintext = open(inv, &self.recipient, &info, &aad, enc, cipher)?;
        let peer = Recipient::parse(&plaintext)?;
        peer.verify(&inv.vault, &handle, &inv.inviter)?;
        check(
            peer.device != self.genesis.device
                && peer.sign != self.genesis.signing_public
                && peer.hpke != self.genesis.hpke_public,
        )?;
        Ok(peer)
    }
    /// Validates the request before the host commits the one-recipient binding.
    pub fn validate_peer_request(&self, inv: &Invitation, request: &[u8]) -> Result<()> {
        self.peer_request_recipient(inv, request).map(|_| ())
    }
    pub fn prepare_peer_admission(
        &self,
        p: &Publication,
        d: &Declaration,
        inv: &Invitation,
        request: &[u8],
        key: &LocalSharedStatePackageKey,
    ) -> Result<Admission> {
        let g = &self.genesis;
        check(
            inv.vault == g.context.vault_id
                && inv.inviter == g.hpke_public
                && inv.handle() == d.handle,
        )?;
        Declaration::from_record(g, p, d.record())?;
        check(
            generation_commitment(g.context, key.protected_storage_bytes())
                == g.generation_commitment,
        )?;
        let peer = self.peer_request_recipient(inv, request)?;
        let (state, core) = state_core(g, p, d, request, &peer);
        let b = p.binding();
        let mut plain = Zeroizing::new(vec![1]);
        for v in [
            inv.vault,
            d.handle,
            d.commitment(),
            hash(request),
            peer.device,
            g.commitment(),
            p.commitment(),
            b.bootstrap_id,
            b.stream_id,
            b.descriptor_commitment,
            b.manifest_commitment,
        ] {
            plain.extend(v);
        }
        plain.extend(b.prefix_count.to_be_bytes());
        plain.push(1);
        plain.extend(g.context.generation_id);
        plain.extend(key.protected_storage_bytes());
        plain.extend(0_u64.to_be_bytes());
        let info = cce(
            "aven-e2ee/v1/pairing/grant",
            &[&inv.vault, &d.handle, &hash(request)],
        );
        let (enc, cipher) = seal(inv, &peer.hpke, &info, &core, &plain)?;
        let mut grant = vec![1];
        bytes(&mut grant, &enc);
        bytes(&mut grant, &cipher);
        let mut attachments = b"AVGA\0\x03\x01\x01\x02".to_vec();
        bytes(&mut attachments, &peer.device);
        bytes(&mut attachments, &peer.hpke);
        bytes(&mut attachments, &grant);
        let signature = SigningKey::from_bytes(self.signing.expose())
            .sign(&cce("aven-e2ee/v1/membership/sign", &[&core, &attachments]));
        let mut raw = vec![1];
        for part in [&core[..], &state, &attachments, &signature.to_bytes()] {
            bytes(&mut raw, part);
        }
        Admission::from_record(g, p, d, request, &raw)
    }
}

/// Public mailbox evidence, not current authorization or installation readiness.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub genesis: Vec<u8>,
    pub publication: Vec<u8>,
    pub declaration: Vec<u8>,
    pub request: Vec<u8>,
    pub admission: Vec<u8>,
}
impl Evidence {
    pub fn validate_lengths(&self) -> Result<()> {
        check(
            self.genesis.len() == GENESIS_BYTES
                && self.publication.len() == PUBLICATION_BYTES
                && self.declaration.len() == DECLARATION_BYTES
                && self.request.len() == REQUEST_BYTES
                && self.admission.len() == ADMISSION_BYTES,
        )
    }
}

/// PSK-authenticated expectations. Not proof of admission or latest membership.
pub struct ProvisionalGrant {
    plaintext: Zeroizing<Vec<u8>>,
    pub head: [u8; 32],
    pub genesis: [u8; 32],
    pub descriptor: [u8; 32],
}
/// Authenticated key coverage, still not a domain installation receipt.
pub struct VerifiedEnrollment {
    genesis: Genesis,
    publication: Publication,
    admission: Admission,
    key: LocalSharedStatePackageKey,
}
impl VerifiedEnrollment {
    pub fn genesis(&self) -> &Genesis {
        &self.genesis
    }
    pub fn publication(&self) -> &Publication {
        &self.publication
    }
    pub fn admission(&self) -> &Admission {
        &self.admission
    }
    pub fn key(&self) -> &LocalSharedStatePackageKey {
        &self.key
    }
}
impl PeerAuthority {
    pub fn open_provisional(&self, evidence: &Evidence) -> Result<ProvisionalGrant> {
        evidence.validate_lengths()?;
        check(evidence.request == self.request)?;
        let (core, _, attachments, _) = components(&evidence.admission)?;
        let peer = self.recipient()?;
        let (enc, cipher) = grant_parts(attachments, &peer)?;
        let info = cce(
            "aven-e2ee/v1/pairing/grant",
            &[&self.vault(), &self.handle(), &hash(&self.request)],
        );
        let plain = open(&self.invitation, &self.recipient, &info, core, enc, cipher)?;
        check(plain.len() == 434)?;
        let mut r = Reader(&plain);
        check(r.take(1)? == [1])?;
        check(r.array::<32>()? == self.vault())?;
        check(r.array::<32>()? == self.handle())?;
        check(r.array::<32>()? == hash(&evidence.declaration))?;
        check(r.array::<32>()? == hash(&self.request))?;
        check(r.array::<32>()? == self.device)?;
        let genesis = r.array()?;
        check(genesis == hash(&evidence.genesis))?;
        check(r.array::<32>()? == hash(&evidence.publication))?;
        r.take(64)?;
        let descriptor = r.array()?;
        Ok(ProvisionalGrant {
            plaintext: plain,
            head: hash(&evidence.admission),
            genesis,
            descriptor,
        })
    }
    pub fn verify_enrollment(
        &self,
        evidence: &Evidence,
        descriptor: &[u8],
    ) -> Result<VerifiedEnrollment> {
        let grant = self.open_provisional(evidence)?;
        check(hash(descriptor) == grant.descriptor)?;
        let g = Genesis::from_record(&evidence.genesis)?;
        check(g.context.vault_id == self.vault() && g.hpke_public == self.invitation.inviter)?;
        let p = Publication::from_record(&g, descriptor, &evidence.publication)?;
        let d = Declaration::from_record(&g, &p, &evidence.declaration)?;
        let a = Admission::from_record(&g, &p, &d, &self.request, &evidence.admission)?;
        check(a.recipient == self.recipient()?)?;
        let mut r = Reader(&grant.plaintext[225..]);
        let b = p.binding();
        check(r.array::<32>()? == b.bootstrap_id)?;
        check(r.array::<32>()? == b.stream_id)?;
        check(r.array::<32>()? == b.descriptor_commitment)?;
        check(r.array::<32>()? == b.manifest_commitment)?;
        check(u64::from_be_bytes(r.array()?) == b.prefix_count)?;
        check(r.take(1)? == [1])?;
        check(r.array::<32>()? == g.context.generation_id)?;
        let secret = r.array()?;
        check(r.take(8)? == [0; 8])?;
        r.end()?;
        check(generation_commitment(g.context, &secret) == g.generation_commitment)?;
        Ok(VerifiedEnrollment {
            genesis: g,
            publication: p,
            admission: a,
            key: LocalSharedStatePackageKey::new(secret),
        })
    }
}
