//! Canonical bounded framing shared by membership actions.
use super::*;

pub(super) type Components<'a> = (&'a [u8], &'a [u8], &'a [u8], &'a [u8]);
pub(super) fn blob<'a>(r: &mut Reader<'a>, limit: usize) -> Result<&'a [u8]> {
    let n = u32::from_be_bytes(r.array()?) as usize;
    check(n <= limit)?;
    r.take(n)
}
pub(super) fn components(raw: &[u8]) -> Result<Components<'_>> {
    check(raw.len() <= MAX_RECORD_BYTES)?;
    let mut r = Reader(raw);
    check(r.take(1)? == [1])?;
    let parts = (
        blob(&mut r, MAX_RECORD_BYTES)?,
        blob(&mut r, MAX_RECORD_BYTES)?,
        blob(&mut r, MAX_RECORD_BYTES)?,
        r.blob(64)?,
    );
    r.end()?;
    Ok(parts)
}
pub(super) fn action(core: &[u8]) -> Result<u8> {
    let mut r = Reader(core);
    check(r.take(1)? == [1])?;
    r.blob(32)?;
    r.take(8)?;
    r.blob(32)?;
    check(r.take(1)? == [1])?;
    r.blob(32)?;
    Ok(r.take(1)?[0])
}
pub(super) fn state(m: &Membership) -> Vec<u8> {
    let mut out = b"AVGS\0\x05\x01".to_vec();
    out.extend(m.genesis.context.vault_id);
    out.push(1);
    out.extend(m.publication.binding().tuple());
    out.extend([u8::from(m.pending), 0]);
    out.extend((m.members.len() as u16).to_be_bytes());
    for member in &m.members {
        member.write(&mut out);
    }
    out.extend((m.generations.len() as u16).to_be_bytes());
    for g in &m.generations {
        out.extend(g.id);
        out.extend(g.commitment);
        out.extend(g.starts_after.to_be_bytes());
    }
    out
}
pub(super) fn core(m: &Membership, signer: Hash, action: u8, body: &[u8], state: &[u8]) -> Vec<u8> {
    let mut out = vec![1];
    bytes(&mut out, &m.genesis.context.vault_id);
    out.extend((m.sequence() + 1).to_be_bytes());
    bytes(&mut out, &m.head());
    out.push(1);
    bytes(&mut out, &signer);
    out.push(action);
    bytes(&mut out, body);
    bytes(
        &mut out,
        &hash(&cce("aven-e2ee/v1/membership/state", &[state])),
    );
    out
}
pub(super) fn packages(count: usize) -> Vec<u8> {
    let mut out = b"AVGA\0\x05".to_vec();
    out.extend((count as u16).to_be_bytes());
    out
}
pub(super) fn package(
    out: &mut Vec<u8>,
    mode: u8,
    device: &Hash,
    public: &Hash,
    enc: &[u8],
    cipher: &[u8],
) {
    out.extend([1, mode]);
    for field in [device.as_slice(), public, enc, cipher] {
        bytes(out, field);
    }
}
pub(super) struct Package<'a> {
    pub device: Hash,
    pub public: Hash,
    pub enc: &'a [u8],
    pub cipher: &'a [u8],
}
pub(super) fn read_packages(
    raw: &[u8],
    mode: u8,
    count: usize,
    plaintext_len: usize,
) -> Result<Vec<Package<'_>>> {
    check(count <= MAX_DEVICES && plaintext_len <= MAX_KEY_PLAINTEXT_BYTES)?;
    let mut r = Reader(raw);
    check(r.take(6)? == b"AVGA\0\x05" && u16::from_be_bytes(r.array()?) as usize == count)?;
    let mut result = Vec::with_capacity(count);
    let mut previous = None;
    for _ in 0..count {
        check(r.take(2)? == [1, mode])?;
        let device: Hash = r.blob(32)?.try_into()?;
        check(previous.is_none_or(|p| p < device))?;
        previous = Some(device);
        result.push(Package {
            device,
            public: r.blob(32)?.try_into()?,
            enc: r.blob(32)?,
            cipher: r.blob(plaintext_len + 16)?,
        });
    }
    r.end()?;
    Ok(result)
}
