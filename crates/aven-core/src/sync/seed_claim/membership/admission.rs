use super::*;

type Components<'a> = (&'a [u8], &'a [u8], &'a [u8], &'a [u8]);

pub(super) fn components(raw: &[u8], count: usize) -> Result<Components<'_>> {
    check((2..=MAX_DEVICES).contains(&count) && raw.len() == 1403 + 164 * count)?;
    let mut r = Reader(raw);
    check(r.take(1)? == [1])?;
    let parts = (
        r.blob(CORE_BYTES)?,
        r.blob(253 + 164 * count)?,
        r.blob(ATTACHMENT_BYTES)?,
        r.blob(64)?,
    );
    r.end()?;
    Ok(parts)
}

pub(super) fn state_core(
    m: &Membership,
    d: &Declaration,
    request: &[u8],
    recipient: &Recipient,
) -> (Vec<u8>, Vec<u8>) {
    let mut state = b"AVGS\0\x04\x01".to_vec();
    state.extend(m.genesis.context.vault_id);
    state.push(1);
    state.extend(m.publication.binding().tuple());
    state.extend([0, 0]);
    state.extend(((m.members.len() + 1) as u16).to_be_bytes());
    let mut rows: Vec<Vec<u8>> = m
        .members
        .iter()
        .map(|member| {
            let mut row = Vec::new();
            member.write(&mut row);
            row
        })
        .collect();
    rows.push(recipient.row(d.handle));
    rows.sort();
    for row in rows {
        state.extend(row);
    }
    state.push(1);
    state.extend(m.genesis.context.generation_id);
    state.extend(m.genesis.generation_commitment);
    state.extend(0_u64.to_be_bytes());
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
    let mut core = vec![1];
    bytes(&mut core, &m.genesis.context.vault_id);
    core.extend((m.sequence() + 1).to_be_bytes());
    bytes(&mut core, &m.head());
    core.push(1);
    bytes(&mut core, &d.inviter);
    core.push(3);
    bytes(&mut core, &action);
    bytes(
        &mut core,
        &hash(&cce("aven-e2ee/v1/membership/state", &[&state])),
    );
    (state, core)
}

pub(super) fn grant_parts<'a>(
    attachments: &'a [u8],
    recipient: &Recipient,
) -> Result<(&'a [u8], &'a [u8])> {
    let mut r = Reader(attachments);
    check(r.take(9)? == b"AVGA\0\x04\x01\x01\x02")?;
    check(r.blob(32)? == recipient.device && r.blob(32)? == recipient.hpke)?;
    let mut grant = Reader(r.blob(GRANT_BYTES)?);
    r.end()?;
    check(grant.take(1)? == [2])?;
    let parts = (grant.blob(32)?, grant.blob(GRANT_PLAINTEXT_BYTES + 16)?);
    grant.end()?;
    Ok(parts)
}

pub(super) fn validate(
    m: &Membership,
    d: &Declaration,
    request: &[u8],
    raw: &[u8],
) -> Result<Recipient> {
    super::super::peer::request_parts(request, &d.handle)?;
    let (core, state, attachments, signature) = components(raw, m.members.len() + 1)?;
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
    grant_parts(attachments, &recipient)?;
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
    key: &LocalSharedStatePackageKey,
) -> Zeroizing<Vec<u8>> {
    let b = m.publication.binding();
    let mut out = Zeroizing::new(vec![2]);
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
    out.push(1);
    out.extend(m.genesis.context.generation_id);
    out.extend(key.protected_storage_bytes());
    out.extend(0_u64.to_be_bytes());
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
