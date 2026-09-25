use super::super::peer::{PeerAuthority, open, request_parts, seal};
use super::*;

/// Borrowed installation keys. Membership matching is mandatory before signing.
/// This value does not assert protected storage, installation or server readiness.
pub struct Device<'a> {
    pub(super) device: Hash,
    pub(super) signing: &'a Secret,
    pub(super) recipient: &'a Secret,
    bearer: &'a Secret,
}
impl<'a> Device<'a> {
    pub fn seed(seed: &'a SeedAuthority) -> Self {
        Self {
            device: seed.genesis.device,
            signing: &seed.signing,
            recipient: &seed.recipient,
            bearer: &seed.bearer,
        }
    }
    pub(super) fn active<'m>(&self, membership: &'m Membership) -> Result<&'m Member> {
        let member = membership.member(&self.device)?;
        let private = HpkePrivate::from_bytes(self.recipient.expose())
            .map_err(|_| anyhow::anyhow!("error membership-recipient"))?;
        check(
            member.sign
                == SigningKey::from_bytes(self.signing.expose())
                    .verifying_key()
                    .to_bytes()
                && member.hpke.as_slice()
                    == <Kem as hpke::Kem>::sk_to_pk(&private).to_bytes().as_slice()
                && bool::from(member.verifier.ct_eq(&credential_verifier(
                    membership.genesis.context.vault_id,
                    self.device,
                    self.bearer,
                ))),
        )?;
        Ok(member)
    }
    pub fn validate(&self, membership: &Membership) -> Result<()> {
        self.active(membership).map(|_| ())
    }
    pub fn prepare_invitation(
        &self,
        membership: &Membership,
        expiry: u64,
    ) -> Result<(Invitation, Declaration)> {
        self.invitation_with_psk(membership, expiry, Secret::generate()?)
    }
    pub(super) fn invitation_with_psk(
        &self,
        membership: &Membership,
        expiry: u64,
        psk: Secret,
    ) -> Result<(Invitation, Declaration)> {
        let member = self.active(membership)?;
        check(membership.device_count() < MAX_DEVICES)?;
        membership.ensure_change_capacity()?;
        let inv = Invitation {
            vault: membership.genesis.context.vault_id,
            inviter: member.hpke,
            psk,
        };
        let mut body = b"AVID\0\x02\x01".to_vec();
        for field in [
            inv.vault,
            membership.genesis.commitment(),
            membership.head(),
            self.device,
            inv.inviter,
            inv.handle(),
        ] {
            body.extend(field);
        }
        body.extend(expiry.to_be_bytes());
        let sig = SigningKey::from_bytes(self.signing.expose())
            .sign(&cce("aven-e2ee/v1/pairing/declaration", &[&body]));
        let mut raw = vec![1];
        bytes(&mut raw, &body);
        bytes(&mut raw, &sig.to_bytes());
        Ok((inv, Declaration::from_record(membership, &raw)?))
    }
    /// Prepare once and retain the exact returned bytes before any dispatch.
    /// Repreparation is a different candidate, not an exact retry.
    pub fn prepare_admission(
        &self,
        membership: &Membership,
        declaration: &Declaration,
        invitation: &Invitation,
        request: &[u8],
        keys: &VerifiedKeys,
    ) -> Result<Vec<u8>> {
        self.active(membership)?;
        let d = Declaration::from_record(membership, declaration.record())?;
        check(
            d.inviter == self.device
                && invitation.vault == membership.genesis.context.vault_id
                && invitation.inviter == d.hpke
                && invitation.handle() == d.handle,
        )?;
        keys.validate(membership)?;
        let handle = invitation.handle();
        let (enc, cipher) = request_parts(request, &handle)?;
        let plain = open(
            invitation,
            self.recipient,
            &cce(
                "aven-e2ee/v1/pairing/request",
                &[&invitation.vault, &handle],
            ),
            &cce(
                "aven-e2ee/v1/pairing/request-aad",
                &[&invitation.vault, &handle, &invitation.inviter],
            ),
            enc,
            cipher,
        )?;
        let recipient = Recipient::parse(&plain)?;
        recipient.verify(&invitation.vault, &handle, &invitation.inviter)?;
        membership.unique(&recipient, &handle)?;
        let (state, core) = admission::state_core(membership, &d, request, &recipient);
        let plain = admission::grant_plaintext(membership, &d, request, &recipient, keys);
        let info = cce(
            "aven-e2ee/v1/pairing/grant",
            &[&invitation.vault, &handle, &hash(request)],
        );
        let (enc, cipher) = seal(invitation, &recipient.hpke, &info, &core, &plain)?;
        let mut attachments = encoding::packages(1);
        encoding::package(
            &mut attachments,
            1,
            &recipient.device,
            &recipient.hpke,
            &enc,
            &cipher,
        );
        let raw = admission::signed(self.signing, &core, &state, &attachments);
        membership.append(d.record(), request, &raw)?;
        Ok(raw)
    }
}

#[derive(Clone)]
pub struct Declaration {
    record: Vec<u8>,
    pub(super) handle: Hash,
    pub(super) inviter: Hash,
    pub(super) hpke: Hash,
    expiry: u64,
}
impl Declaration {
    pub fn from_record(membership: &Membership, raw: &[u8]) -> Result<Self> {
        check(raw.len() == DECLARATION_BYTES)?;
        let mut r = Reader(raw);
        check(r.take(1)? == [1])?;
        let body = r.blob(207)?;
        let signature = r.blob(64)?;
        r.end()?;
        let mut b = Reader(body);
        check(b.take(7)? == b"AVID\0\x02\x01")?;
        check(
            b.array::<32>()? == membership.genesis.context.vault_id
                && b.array::<32>()? == membership.genesis.commitment(),
        )?;
        let anchor = b.array::<32>()?;
        let anchor_sequence = membership
            .heads
            .iter()
            .position(|head| *head == anchor)
            .context("error membership-anchor")? as u64
            + 1;
        let inviter = b.array()?;
        let hpke = b.array()?;
        let member = membership.member(&inviter)?;
        check(member.admitted_at <= anchor_sequence && member.hpke == hpke)?;
        let handle = b.array()?;
        let expiry = u64::from_be_bytes(b.array()?);
        b.end()?;
        verify(
            &member.sign,
            "aven-e2ee/v1/pairing/declaration",
            &[body],
            signature,
        )?;
        Ok(Self {
            record: raw.to_vec(),
            handle,
            inviter,
            hpke,
            expiry,
        })
    }
    pub fn record(&self) -> &[u8] {
        &self.record
    }
    pub fn commitment(&self) -> Hash {
        hash(&self.record)
    }
    pub fn handle(&self) -> Hash {
        self.handle
    }
    pub fn expiry(&self) -> u64 {
        self.expiry
    }
}

/// Device ID and signing, recipient and bearer secrets in protected storage.
const ATTEMPT_KEY_BYTES: usize = 128;

/// Independently generated installation keys and one immutable PSK request.
/// Hosts must durably own this material before sending anything.
pub struct Joiner(pub(super) PeerAuthority);
impl Joiner {
    pub fn generate(invitation: Invitation) -> Result<Self> {
        Ok(Self(PeerAuthority::generate(invitation)?))
    }
    pub fn protected_storage_bytes(&self) -> Zeroizing<Vec<u8>> {
        self.0.protected_storage_bytes()
    }
    pub fn from_protected_storage(bytes: &[u8]) -> Result<Self> {
        Ok(Self(PeerAuthority::from_protected_storage(bytes)?))
    }
    pub fn matches_invitation(&self, invitation: &Invitation) -> bool {
        self.0.matches_invitation(invitation)
    }
    /// Another request from these unchanged installation keys, for a
    /// replacement invitation to the same vault from the same inviter.
    pub fn retry(&self, invitation: Invitation) -> Result<Self> {
        self.same_context(&invitation)?;
        ensure!(
            !self.matches_invitation(&invitation),
            "error enrollment-retry-context"
        );
        Ok(Self(self.0.retry(invitation)?))
    }
    /// The invitation and exact request of this attempt, without the keys.
    pub fn attempt_bytes(&self) -> Zeroizing<Vec<u8>> {
        Zeroizing::new(self.protected_storage_bytes()[ATTEMPT_KEY_BYTES..].to_vec())
    }
    /// Rebuilds a retained attempt from `attempt_bytes` over these keys, so
    /// every attempt shares this installation authority by construction.
    pub fn attempt(&self, bytes: &[u8]) -> Result<Self> {
        let mut raw = Zeroizing::new(self.protected_storage_bytes()[..ATTEMPT_KEY_BYTES].to_vec());
        raw.extend_from_slice(bytes);
        let attempt = Self::from_protected_storage(&raw)?;
        self.same_context(&attempt.0.invitation)?;
        Ok(attempt)
    }
    fn same_context(&self, invitation: &Invitation) -> Result<()> {
        ensure!(
            invitation.vault == self.0.invitation.vault
                && invitation.inviter == self.0.invitation.inviter,
            "error enrollment-retry-context"
        );
        Ok(())
    }
    pub fn vault(&self) -> Hash {
        self.0.vault()
    }
    pub fn handle(&self) -> Hash {
        self.0.handle()
    }
    pub fn device(&self) -> Hash {
        self.0.device()
    }
    pub fn bearer(&self) -> &Secret {
        self.0.bearer()
    }
    pub fn request(&self) -> &[u8] {
        self.0.request()
    }
    pub fn authority(&self) -> Device<'_> {
        Device {
            device: self.0.device,
            signing: &self.0.signing,
            recipient: &self.0.recipient,
            bearer: &self.0.bearer,
        }
    }
    /// PSK-authenticated expectations only. No membership or content authority.
    pub fn open_provisional(&self, declaration: &[u8], record: &[u8]) -> Result<ProvisionalGrant> {
        check(declaration.len() == DECLARATION_BYTES && record.len() <= MAX_RECORD_BYTES)?;
        let (core, _, attachments, _) = encoding::components(record)?;
        let recipient = self.0.recipient()?;
        // Public framing bounds ciphertext; trust comes only from the PSK opening.
        let mut r = Reader(attachments);
        check(r.take(8)? == b"AVGA\0\x05\0\x01" && r.take(2)? == [1, 1])?;
        check(r.blob(32)? == recipient.device && r.blob(32)? == recipient.hpke)?;
        let enc = r.blob(32)?;
        let cipher = encoding::blob(&mut r, MAX_KEY_PLAINTEXT_BYTES + 16)?;
        r.end()?;
        let info = cce(
            "aven-e2ee/v1/pairing/grant",
            &[&self.0.vault(), &self.0.handle(), &hash(self.request())],
        );
        let plain = open(
            &self.0.invitation,
            &self.0.recipient,
            &info,
            core,
            enc,
            cipher,
        )?;
        check(plain.len() >= GRANT_PREFIX_BYTES + 2 && plain.len() <= MAX_KEY_PLAINTEXT_BYTES)?;
        let mut r = Reader(&plain);
        check(
            r.take(1)? == [3]
                && r.array::<32>()? == self.0.vault()
                && r.array::<32>()? == self.0.handle()
                && r.array::<32>()? == hash(declaration)
                && r.array::<32>()? == hash(self.request())
                && r.array::<32>()? == self.device(),
        )?;
        let genesis = r.array()?;
        let publication = r.array()?;
        let predecessor = r.array()?;
        r.take(64)?;
        let descriptor = r.array()?;
        Ok(ProvisionalGrant {
            plaintext: plain,
            genesis,
            publication,
            predecessor,
            descriptor,
            outcome: hash(record),
        })
    }
    pub fn verify_enrollment(
        &self,
        predecessor: &Membership,
        declaration: &[u8],
        record: &[u8],
    ) -> Result<VerifiedEnrollment> {
        let grant = self.open_provisional(declaration, record)?;
        let membership = predecessor.append(declaration, self.request(), record)?;
        let d = Declaration::from_record(predecessor, declaration)?;
        check(d.hpke == self.0.invitation.inviter)?;
        let recipient = self.0.recipient()?;
        let member = membership.member(&self.device())?;
        check(
            member.sign == recipient.sign
                && member.hpke == recipient.hpke
                && member.verifier == recipient.verifier
                && member.admission == d.handle,
        )?;
        let mut r = Reader(&grant.plaintext[GRANT_PREFIX_BYTES..]);
        let keys = VerifiedKeys::read(predecessor, &mut r)?;
        let expected =
            admission::grant_plaintext(predecessor, &d, self.request(), &recipient, &keys);
        check(bool::from(
            grant.plaintext.as_slice().ct_eq(expected.as_slice()),
        ))?;
        Ok(VerifiedEnrollment { membership, keys })
    }
}

/// Grant pins cannot prove relay commit, current authorization or freshness.
pub struct ProvisionalGrant {
    plaintext: Zeroizing<Vec<u8>>,
    pub genesis: Hash,
    pub publication: Hash,
    pub predecessor: Hash,
    pub descriptor: Hash,
    pub outcome: Hash,
}
/// Verified historical enrollment and complete key coverage, not install readiness.
pub struct VerifiedEnrollment {
    membership: Membership,
    keys: VerifiedKeys,
}
impl VerifiedEnrollment {
    pub fn genesis(&self) -> &Genesis {
        self.membership.genesis()
    }
    pub fn publication(&self) -> &Publication {
        self.membership.publication()
    }
    pub fn checkpoint(&self) -> Hash {
        self.membership.head()
    }
    pub fn membership(&self) -> &Membership {
        &self.membership
    }
    pub fn keys(&self) -> &VerifiedKeys {
        &self.keys
    }
    pub fn key(&self) -> &LocalSharedStatePackageKey {
        self.keys
            .key(self.membership.genesis.context.generation_id)
            .expect("verified bootstrap coverage")
    }
}
