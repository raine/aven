//! Revoke/freeze and signed generation cutoffs with base-mode recipient packages.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Generation {
    pub id: Hash,
    pub commitment: Hash,
    pub starts_after: u64,
}
const ROTATION_PLAINTEXT_BYTES: usize = 176;
const ROTATION_KEY_OFFSET: usize = 6 + 3 * 32 + 2 + 32;

/// Fresh per-candidate material, owned in protected storage before package creation.
pub struct RotationMaterial {
    generation: Hash,
    key: LocalSharedStatePackageKey,
    entropy: Secret,
}
impl RotationMaterial {
    pub fn generate() -> Result<Self> {
        Ok(Self {
            generation: *Secret::generate()?.expose(),
            key: LocalSharedStatePackageKey::new(*Secret::generate()?.expose()),
            entropy: Secret::generate()?,
        })
    }
    pub fn protected_storage_bytes(&self) -> Zeroizing<Vec<u8>> {
        let mut bytes = Zeroizing::new(b"AVRM\0\x01".to_vec());
        bytes.extend(self.generation);
        bytes.extend(self.key.protected_storage_bytes());
        bytes.extend(self.entropy.expose());
        bytes
    }
    pub fn validate_generation(&self, membership: &Membership) -> Result<()> {
        let generation = membership.current_generation();
        check(
            generation.id == self.generation
                && generation.commitment
                    == generation_commitment(
                        LocalSharedStatePackageContext {
                            vault_id: membership.genesis.context.vault_id,
                            generation_id: self.generation,
                        },
                        self.key.protected_storage_bytes(),
                    ),
        )
    }
    pub fn from_protected_storage(bytes: &[u8]) -> Result<Self> {
        check(bytes.len() == 102)?;
        let mut r = Reader(bytes);
        check(r.take(6)? == b"AVRM\0\x01")?;
        Ok(Self {
            generation: r.array()?,
            key: LocalSharedStatePackageKey::new(r.array()?),
            entropy: Secret::new(r.array()?),
        })
    }
}
fn revoke(m: &Membership, targets: &[Hash]) -> Result<Membership> {
    check(targets.len() < m.members.len() && targets.windows(2).all(|w| w[0] < w[1]))?;
    let mut next = m.clone();
    for target in targets {
        let member = m.member(target)?;
        next.retired.push(member.clone());
    }
    next.members
        .retain(|member| !targets.contains(&member.device));
    next.pending = true;
    Ok(next)
}
fn rotate(m: &Membership, generation: Generation) -> Result<Membership> {
    check(
        m.pending
            && m.generations.len() < MAX_GENERATIONS
            && !m.generations.iter().any(|g| g.id == generation.id)
            && generation.starts_after >= m.publication.binding().prefix_count
            && generation.starts_after >= m.current_generation().starts_after,
    )?;
    let mut next = m.clone();
    next.generations.push(generation);
    next.pending = false;
    Ok(next)
}
pub(super) fn validate(m: &Membership, raw: &[u8]) -> Result<Membership> {
    let (core, state, attachments, signature) = encoding::components(raw)?;
    let mut r = Reader(core);
    check(r.take(1)? == [1] && r.blob(32)? == m.genesis.context.vault_id)?;
    check(u64::from_be_bytes(r.array()?) == m.sequence() + 1 && r.blob(32)? == m.head())?;
    check(r.take(1)? == [1])?;
    let signer: Hash = r.blob(32)?.try_into()?;
    let member = m.member(&signer)?;
    let action = r.take(1)?[0];
    let body = encoding::blob(&mut r, 8 + MAX_DEVICES * 32)?;
    r.blob(32)?;
    r.end()?;
    verify(
        &member.sign,
        "aven-e2ee/v1/membership/sign",
        &[core, attachments],
        signature,
    )?;
    let mut b = Reader(body);
    let next = match action {
        4 => {
            check(b.take(6)? == b"AVRV\0\x01")?;
            let count = u16::from_be_bytes(b.array()?) as usize;
            check(count <= MAX_DEVICES)?;
            let targets = (0..count)
                .map(|_| b.array())
                .collect::<Result<Vec<Hash>>>()?;
            encoding::read_packages(attachments, 0, 0, 0)?;
            revoke(m, &targets)?
        }
        5 => {
            check(
                b.take(6)? == b"AVRT\0\x01"
                    && b.array::<32>()? == m.publication.binding().stream_id,
            )?;
            let next = rotate(
                m,
                Generation {
                    id: b.array()?,
                    commitment: b.array()?,
                    starts_after: u64::from_be_bytes(b.array()?),
                },
            )?;
            let packages = encoding::read_packages(
                attachments,
                0,
                next.members.len(),
                ROTATION_PLAINTEXT_BYTES,
            )?;
            for (p, member) in packages.iter().zip(&next.members) {
                check(p.device == member.device && p.public == member.hpke)?;
            }
            next
        }
        _ => anyhow::bail!("error membership-action"),
    };
    b.end()?;
    check(
        state == encoding::state(&next) && core == encoding::core(m, signer, action, body, state),
    )?;
    Ok(next)
}
fn info(m: &Membership, member: &Member) -> Vec<u8> {
    cce(
        "aven-e2ee/v1/membership/rotation-key",
        &[
            &m.genesis.context.vault_id,
            &[1],
            &member.device,
            &member.hpke,
        ],
    )
}
fn plaintext(
    m: &Membership,
    member: &Member,
    g: &Generation,
    key: &LocalSharedStatePackageKey,
) -> Zeroizing<Vec<u8>> {
    let mut out = Zeroizing::new(b"AVRK\0\x01".to_vec());
    out.extend(m.genesis.context.vault_id);
    out.extend(m.genesis.commitment());
    out.extend(member.device);
    out.extend(1_u16.to_be_bytes());
    out.extend(g.id);
    out.extend(key.protected_storage_bytes());
    out.extend(g.starts_after.to_be_bytes());
    out
}
impl Device<'_> {
    /// Sorted active targets; an empty list requests rotation without removal.
    /// Exact bytes must be retained before dispatch. No server commit is implied.
    pub fn prepare_revoke(&self, m: &Membership, targets: &[Hash]) -> Result<Vec<u8>> {
        self.active(m)?;
        let next = revoke(m, targets)?;
        let mut body = b"AVRV\0\x01".to_vec();
        body.extend((targets.len() as u16).to_be_bytes());
        for target in targets {
            body.extend(target);
        }
        let state = encoding::state(&next);
        let core = encoding::core(m, self.device, 4, &body, &state);
        let raw = admission::signed(self.signing, &core, &state, &encoding::packages(0));
        m.append(&[], &[], &raw)?;
        Ok(raw)
    }
    /// A new candidate always generates independent key and generation identities.
    /// The cutoff is a proposal; server admission must compare its frozen allocator.
    pub fn prepare_rotation(
        &self,
        m: &Membership,
        keys: &VerifiedKeys,
        cutoff: u64,
    ) -> Result<Vec<u8>> {
        self.prepare_rotation_with(m, keys, cutoff, &RotationMaterial::generate()?)
    }
    /// Replay one protected candidate. Replacements must use fresh material;
    /// callers own predecessor/cutoff binding and must not reuse losing secrets.
    pub fn prepare_rotation_with(
        &self,
        m: &Membership,
        keys: &VerifiedKeys,
        cutoff: u64,
        material: &RotationMaterial,
    ) -> Result<Vec<u8>> {
        self.rotation_with(
            m,
            keys,
            material.generation,
            &material.key,
            cutoff,
            &mut ChaCha20Rng::from_seed(*material.entropy.expose()),
        )
    }
    pub(super) fn rotation_with(
        &self,
        m: &Membership,
        keys: &VerifiedKeys,
        id: Hash,
        key: &LocalSharedStatePackageKey,
        cutoff: u64,
        rng: &mut impl rand_core::CryptoRng,
    ) -> Result<Vec<u8>> {
        self.active(m)?;
        keys.validate(m)?;
        let g = Generation {
            id,
            commitment: generation_commitment(
                LocalSharedStatePackageContext {
                    vault_id: m.genesis.context.vault_id,
                    generation_id: id,
                },
                key.protected_storage_bytes(),
            ),
            starts_after: cutoff,
        };
        let next = rotate(m, g.clone())?;
        let mut body = b"AVRT\0\x01".to_vec();
        body.extend(m.publication.binding().stream_id);
        body.extend(g.id);
        body.extend(g.commitment);
        body.extend(cutoff.to_be_bytes());
        let state = encoding::state(&next);
        let core = encoding::core(m, self.device, 5, &body, &state);
        let mut attachments = encoding::packages(next.members.len());
        for member in &next.members {
            let public = <Kem as hpke::Kem>::PublicKey::from_bytes(&member.hpke)
                .map_err(|_| anyhow::anyhow!("error membership-recipient"))?;
            let (enc, cipher) = hpke::single_shot_seal_with_rng::<Aead, Kdf, Kem>(
                &OpModeS::Base,
                &public,
                &info(m, member),
                &plaintext(m, member, &g, key),
                &core,
                rng,
            )
            .map_err(|_| anyhow::anyhow!("error membership-package-seal"))?;
            encoding::package(
                &mut attachments,
                0,
                &member.device,
                &member.hpke,
                &enc.to_bytes(),
                &cipher,
            );
        }
        let raw = admission::signed(self.signing, &core, &state, &attachments);
        self.receive_rotation(m, &raw, keys)?;
        Ok(raw)
    }
    /// Validates the entire transition and own package before yielding full coverage.
    /// Other recipients' encrypted plaintext cannot be validated by this device.
    pub fn receive_rotation(
        &self,
        before: &Membership,
        raw: &[u8],
        keys: &VerifiedKeys,
    ) -> Result<VerifiedKeys> {
        self.active(before)?;
        keys.validate(before)?;
        let next = before.append(&[], &[], raw)?;
        check(next.generations.len() == before.generations.len() + 1)?;
        let member = self.active(&next)?;
        let (core, _, attachments, _) = encoding::components(raw)?;
        let packages =
            encoding::read_packages(attachments, 0, next.members.len(), ROTATION_PLAINTEXT_BYTES)?;
        let p = packages
            .iter()
            .find(|p| p.device == self.device)
            .context("error membership-package-missing")?;
        let private = HpkePrivate::from_bytes(self.recipient.expose())
            .map_err(|_| anyhow::anyhow!("error membership-recipient"))?;
        let enc =
            Enc::from_bytes(p.enc).map_err(|_| anyhow::anyhow!("error membership-package-enc"))?;
        let plain = Zeroizing::new(
            hpke::single_shot_open::<Aead, Kdf, Kem>(
                &OpModeR::Base,
                &private,
                &enc,
                &info(before, member),
                p.cipher,
                core,
            )
            .map_err(|_| anyhow::anyhow!("error membership-package-open"))?,
        );
        check(plain.len() == ROTATION_PLAINTEXT_BYTES)?;
        let key = LocalSharedStatePackageKey::new(
            plain[ROTATION_KEY_OFFSET..ROTATION_KEY_OFFSET + 32].try_into()?,
        );
        check(bool::from(plain.as_slice().ct_eq(&plaintext(
            before,
            member,
            next.current_generation(),
            &key,
        ))))?;
        keys.extended(&next, key)
    }
}
