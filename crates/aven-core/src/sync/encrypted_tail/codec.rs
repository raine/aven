use super::domain::Projection;
use super::*;
use crate::sync::wire::ChangeWire;
use anyhow::Context as _;
use chacha20poly1305::{
    KeyInit, XChaCha20Poly1305, XNonce,
    aead::{Aead, Payload},
};
use zeroize::Zeroizing;

use crate::sync::codec::bytes;
struct Reader<'a>(&'a [u8]);
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        valid(n <= self.0.len())?;
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.take(N)?.try_into()?)
    }
    fn blob(&mut self, max: usize) -> Result<&'a [u8]> {
        let n = u32::from_be_bytes(self.array()?) as usize;
        valid(n <= max)?;
        self.take(n)
    }
}
pub(super) struct Envelope<'a> {
    pub vault: [u8; 32],
    pub stream: [u8; 32],
    pub generation: [u8; 32],
    pub id: String,
    pub projection: Projection,
    header: &'a [u8],
    body: &'a [u8],
    nonce: [u8; 24],
}
pub(super) fn parse(record: &[u8]) -> Result<Envelope<'_>> {
    valid(record.len() <= RECORD_LIMIT)?;
    let mut r = Reader(record);
    let header = r.blob(4544)?;
    let body = r.blob(131072 + 16)?;
    valid(r.0.is_empty() && body.len() >= 16)?;
    let mut r = Reader(header);
    valid(r.take(8)? == b"AVEN\x01\x00\x01\x01")?;
    let vault = r.blob(32)?.try_into()?;
    let stream = r.blob(32)?.try_into()?;
    let generation = r.blob(32)?.try_into()?;
    valid(r.blob(32)?.len() == 32)?;
    let id = std::str::from_utf8(r.blob(256)?)?.to_owned();
    valid(!id.is_empty())?;
    valid(r.array::<4>()? == 18u32.to_be_bytes())?;
    let projection = Projection::decode(r.blob(4096)?)?;
    let nonce = r.blob(24)?.try_into()?;
    valid(r.0.is_empty())?;
    Ok(Envelope {
        vault,
        stream,
        generation,
        id,
        projection,
        header,
        body,
        nonce,
    })
}
fn key(a: &Authority, generation: [u8; 32]) -> Result<Zeroizing<[u8; 32]>> {
    let kdf = hkdf::Hkdf::<Sha256>::new(
        Some(b"aven-e2ee/v1/generation"),
        a.key(generation)?.protected_storage_bytes(),
    );
    let mut info = Vec::new();
    bytes(&mut info, b"aven-e2ee/v1/key/operation");
    bytes(&mut info, &a.context.vault);
    bytes(&mut info, &generation);
    let mut result = Zeroizing::new([0; 32]);
    kdf.expand(&info, result.as_mut())
        .expect("fixed HKDF length");
    Ok(result)
}
#[cfg(any(test, feature = "test-support"))]
pub(super) fn seal(a: &Authority, change: &ChangeWire) -> Result<Vec<u8>> {
    let projection = domain::validate(change)?;
    seal_projection(a, change, &projection)
}
pub(super) fn seal_projection(
    a: &Authority,
    change: &ChangeWire,
    projection: &Projection,
) -> Result<Vec<u8>> {
    a.validate()?;
    ensure!(!a.rotation_pending(), "error membership-rotation-pending");
    domain::validate_projection(change, projection)?;
    let plain = Zeroizing::new(serde_json::to_vec(change)?);
    valid(plain.len() <= 131072)?;
    let mut random = [0; 56];
    getrandom::fill(&mut random).context("error encrypted-tail-entropy")?;
    let mut header = b"AVEN\x01\x00\x01\x01".to_vec();
    for value in [
        &a.context.vault[..],
        &a.context.stream,
        &a.generation(),
        &random[..32],
        change.change_id.as_bytes(),
    ] {
        bytes(&mut header, value);
    }
    header.extend(18u32.to_be_bytes());
    bytes(&mut header, &projection.encode());
    bytes(&mut header, &random[32..]);
    let k = key(a, a.generation())?;
    let cipher = XChaCha20Poly1305::new_from_slice(k.as_ref()).expect("fixed key");
    let body = cipher
        .encrypt(
            &XNonce::try_from(&random[32..]).expect("fixed nonce"),
            Payload {
                msg: &plain,
                aad: &header,
            },
        )
        .map_err(|_| anyhow::anyhow!("error encrypted-tail-encrypt"))?;
    let mut record = Vec::new();
    bytes(&mut record, &header);
    bytes(&mut record, &body);
    parse(&record)?;
    Ok(record)
}
pub(super) fn open(a: &Authority, record: &[u8]) -> Result<ChangeWire> {
    a.validate()?;
    let e = parse(record)?;
    valid(e.vault == a.context.vault && e.stream == a.context.stream)?;
    let k = key(a, e.generation)?;
    let cipher = XChaCha20Poly1305::new_from_slice(k.as_ref()).expect("fixed key");
    let plain = Zeroizing::new(
        cipher
            .decrypt(
                &XNonce::from(e.nonce),
                Payload {
                    msg: e.body,
                    aad: e.header,
                },
            )
            .map_err(|_| anyhow::anyhow!("error encrypted-tail-authentication"))?,
    );
    let change = domain::decode(&plain)?;
    valid(change.change_id == e.id)?;
    domain::validate_projection(&change, &e.projection)?;
    if let Projection::Ref { descriptor, .. } = &e.projection {
        let d = super::attachments::codec::Descriptor::decode(descriptor)?;
        valid(d.vault == e.vault && d.stream == e.stream)?;
        a.key(d.generation)?;
        let generations = a.membership.generations();
        let object_generation = generations
            .iter()
            .position(|g| g.id == d.generation)
            .context("error encrypted-image-generation")?;
        let envelope_generation = generations
            .iter()
            .position(|g| g.id == e.generation)
            .context("error encrypted-tail-generation")?;
        valid(object_generation <= envelope_generation)?;
    }
    Ok(change)
}
pub(super) fn encode_text(out: &mut Vec<u8>, s: &str) {
    bytes(out, s.as_bytes());
}
pub(super) fn decode_projection(input: &[u8]) -> Result<Projection> {
    let mut r = Reader(input);
    let result = match r.take(1)?[0] {
        0 => Projection::None,
        1 => {
            let action = r.take(1)?[0];
            valid(action <= 2)?;
            let workspace = std::str::from_utf8(r.blob(256)?)?.to_owned();
            let task = std::str::from_utf8(r.blob(256)?)?.to_owned();
            let deleted = r.take(1)?[0];
            valid(deleted <= 1)?;
            let version = match r.take(1)?[0] {
                0 => None,
                1 => Some(std::str::from_utf8(r.blob(256)?)?.to_owned()),
                _ => anyhow::bail!("error encrypted-tail-projection"),
            };
            valid(
                !workspace.is_empty()
                    && !task.is_empty()
                    && version.as_ref().is_none_or(|s| !s.is_empty()),
            )?;
            valid(action != 0 || (deleted == 0 && version.is_some()))?;
            Projection::Parent {
                action,
                workspace,
                task,
                deleted: deleted == 1,
                version,
            }
        }
        kind @ (2 | 3) => {
            let mut text = || -> Result<String> {
                let s = std::str::from_utf8(r.blob(256)?)?.to_owned();
                valid(!s.is_empty())?;
                Ok(s)
            };
            let workspace = text()?;
            let task = text()?;
            let reference = text()?;
            if kind == 3 {
                Projection::Unref {
                    workspace,
                    task,
                    reference,
                }
            } else {
                let descriptor = r.blob(1984)?.to_vec();
                super::attachments::codec::Descriptor::decode(&descriptor)?;
                let deleted = r.take(1)?[0];
                valid(deleted <= 1)?;
                let version = match r.take(1)?[0] {
                    0 => None,
                    1 => {
                        let s = std::str::from_utf8(r.blob(256)?)?.to_owned();
                        valid(!s.is_empty())?;
                        Some(s)
                    }
                    _ => anyhow::bail!("error encrypted-image-hint"),
                };
                Projection::Ref {
                    workspace,
                    task,
                    reference,
                    descriptor,
                    deleted: deleted == 1,
                    version,
                }
            }
        }
        _ => anyhow::bail!("error encrypted-tail-projection"),
    };
    valid(r.0.is_empty())?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn valid_aead_cannot_hide_projection_domain_disagreement() {
        let a = super::super::tests::authority();
        let c = super::super::tests::change();
        let record = seal(&a, &c).unwrap();
        let e = parse(&record).unwrap();
        let mut header = e.header.to_vec();
        // Fixed profile: projection starts after the 16-byte operation ID.
        let projection_start = 192 + 16 - 28;
        assert_eq!(header[projection_start], 1);
        header[projection_start + 1] = 2;
        let k = key(&a, a.generation()).unwrap();
        let cipher = XChaCha20Poly1305::new_from_slice(k.as_ref()).unwrap();
        let plain = serde_json::to_vec(&c).unwrap();
        let body = cipher
            .encrypt(
                &XNonce::from(e.nonce),
                Payload {
                    msg: &plain,
                    aad: &header,
                },
            )
            .unwrap();
        let mut forged = Vec::new();
        bytes(&mut forged, &header);
        bytes(&mut forged, &body);
        assert!(parse(&forged).is_ok());
        assert!(open(&a, &forged).is_err());
    }
}
