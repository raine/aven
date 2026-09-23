use super::*;

pub(super) fn state_core(
    m: &Membership,
    d: &Declaration,
    request: &[u8],
    recipient: &Recipient,
) -> (Vec<u8>, Vec<u8>) {
    let mut next = m.clone();
    next.members.push(Member {
        device: recipient.device,
        sign: recipient.sign,
        hpke: recipient.hpke,
        verifier: recipient.verifier,
        admission: d.handle,
        admitted_at: m.sequence() + 1,
    });
    next.members.sort_by_key(|member| member.device);
    let state = encoding::state(&next);
    let mut action = b"AVAD\0\x02".to_vec();
    for field in [
        d.handle,
        d.commitment(),
        hash(request),
        recipient.device,
        recipient.sign,
        recipient.hpke,
        recipient.verifier,
    ] {
        action.extend(field);
    }
    action.extend(1_u32.to_be_bytes());
    action.extend(DOMAIN_VERSION.to_be_bytes());
    action.extend(recipient.pop);
    let core = encoding::core(m, d.inviter, 3, &action, &state);
    (state, core)
}

pub(super) fn grant_parts<'a>(
    attachments: &'a [u8],
    recipient: &Recipient,
    count: usize,
) -> Result<(&'a [u8], &'a [u8])> {
    let packages = encoding::read_packages(attachments, 1, 1, GRANT_PREFIX_BYTES + 2 + 72 * count)?;
    let p = &packages[0];
    check(p.device == recipient.device && p.public == recipient.hpke)?;
    Ok((p.enc, p.cipher))
}

pub(super) fn validate(
    m: &Membership,
    d: &Declaration,
    request: &[u8],
    raw: &[u8],
) -> Result<Recipient> {
    super::super::peer::request_parts(request, &d.handle)?;
    let (core, state, attachments, signature) = encoding::components(raw)?;
    check(core.len() == CORE_BYTES)?;
    let mut r = Reader(core);
    check(r.take(1)? == [1] && r.blob(32)? == m.genesis.context.vault_id)?;
    check(u64::from_be_bytes(r.array()?) == m.sequence() + 1 && r.blob(32)? == m.head())?;
    check(r.take(1)? == [1] && r.blob(32)? == d.inviter && r.take(1)? == [3])?;
    let mut a = Reader(r.blob(302)?);
    check(a.take(6)? == b"AVAD\0\x02")?;
    check(
        a.array::<32>()? == d.handle
            && a.array::<32>()? == d.commitment()
            && a.array::<32>()? == hash(request),
    )?;
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
    m.unique(&recipient, &d.handle)?;
    recipient.verify(&m.genesis.context.vault_id, &d.handle, &d.hpke)?;
    let (expected_state, expected_core) = state_core(m, d, request, &recipient);
    check(state == expected_state && core == expected_core)?;
    grant_parts(attachments, &recipient, m.generations.len())?;
    verify(
        &m.member(&d.inviter)?.sign,
        "aven-e2ee/v1/membership/sign",
        &[core, attachments],
        signature,
    )?;
    Ok(recipient)
}

pub(super) fn grant_plaintext(
    m: &Membership,
    d: &Declaration,
    request: &[u8],
    recipient: &Recipient,
    keys: &VerifiedKeys,
) -> Zeroizing<Vec<u8>> {
    let b = m.publication.binding();
    let mut out = Zeroizing::new(vec![3]);
    for field in [
        m.genesis.context.vault_id,
        d.handle,
        d.commitment(),
        hash(request),
        recipient.device,
        m.genesis.commitment(),
        m.publication.commitment(),
        m.head(),
        b.bootstrap_id,
        b.stream_id,
        b.descriptor_commitment,
        b.manifest_commitment,
    ] {
        out.extend(field);
    }
    out.extend(b.prefix_count.to_be_bytes());
    keys.write(&mut out);
    out
}

pub(super) fn signed(signing: &Secret, core: &[u8], state: &[u8], attachments: &[u8]) -> Vec<u8> {
    let sig = SigningKey::from_bytes(signing.expose())
        .sign(&cce("aven-e2ee/v1/membership/sign", &[core, attachments]));
    let mut raw = vec![1];
    for part in [core, state, attachments, &sig.to_bytes()] {
        bytes(&mut raw, part);
    }
    raw
}
