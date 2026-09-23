use anyhow::{Result, ensure};
use ed25519_dalek::{Signature, VerifyingKey};
use sha2::{Digest, Sha256};

use super::{CLAIM_BYTES, GENESIS_BYTES, Genesis, LocalSharedStatePackageContext};

pub(super) fn valid(condition: bool) -> Result<()> {
    ensure!(condition, "error seed-claim-invalid");
    Ok(())
}

pub(super) fn bytes(out: &mut Vec<u8>, value: &[u8]) {
    // All callers encode bounded fixed-profile fields.
    out.extend_from_slice(
        &u32::try_from(value.len())
            .expect("bounded seed profile")
            .to_be_bytes(),
    );
    out.extend_from_slice(value);
}

pub(super) fn cce(label: &str, fields: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    bytes(&mut out, label.as_bytes());
    for field in fields {
        bytes(&mut out, field);
    }
    out
}

pub(super) fn hash(value: &[u8]) -> [u8; 32] {
    Sha256::digest(value).into()
}

pub(super) struct Reader<'a>(pub &'a [u8]);

impl<'a> Reader<'a> {
    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        valid(n <= self.0.len())?;
        let (value, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(value)
    }

    pub fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.take(N)?.try_into()?)
    }

    pub fn blob(&mut self, n: usize) -> Result<&'a [u8]> {
        valid(u32::from_be_bytes(self.array()?) as u64 == n as u64)?;
        self.take(n)
    }

    pub fn end(self) -> Result<()> {
        valid(self.0.is_empty())
    }
}

pub(super) fn claim_record(request: &[u8]) -> Result<&[u8]> {
    valid(request.len() == CLAIM_BYTES)?;
    let mut reader = Reader(request);
    valid(reader.take(6)? == b"AVCL\0\x01")?;
    let record = reader.blob(GENESIS_BYTES)?;
    reader.end()?;
    Ok(record)
}

type Components<'a> = (&'a [u8], &'a [u8], &'a [u8], &'a [u8]);

pub(super) fn components(record: &[u8]) -> Result<Components<'_>> {
    valid(record.len() == GENESIS_BYTES)?;
    let mut reader = Reader(record);
    valid(reader.take(1)? == [1])?;
    let parts = (
        reader.blob(229)?,
        reader.blob(280)?,
        reader.blob(312)?,
        reader.blob(64)?,
    );
    reader.end()?;
    Ok(parts)
}

pub(super) fn parse(record: &[u8]) -> Result<Genesis> {
    let (core, state, attachments, signature) = components(record)?;
    let mut r = Reader(state);
    valid(r.take(7)? == b"AVGS\0\x01\x01")?;
    let vault_id = r.array()?;
    valid(r.take(4)? == [0, 0, 0, 1])?;
    let device = r.array()?;
    let signing_public = r.array()?;
    let hpke_public = r.array()?;
    let verifier = r.array()?;
    valid(r.take(4)? == 1_u32.to_be_bytes())?;
    let claim = r.array()?;
    valid(r.take(1)? == [1])?;
    let generation_id = r.array()?;
    let generation_commitment = r.array()?;
    valid(r.take(8)? == [0; 8])?;
    r.end()?;

    let mut r = Reader(core);
    valid(r.take(1)? == [1])?;
    valid(r.blob(32)? == vault_id)?;
    valid(r.take(8)? == [0; 8])?;
    valid(r.blob(32)? == [0; 32])?;
    valid(r.take(1)? == [1])?;
    valid(r.blob(32)? == device)?;
    valid(r.take(1)? == [1])?;
    let mut body = Reader(r.blob(70)?);
    valid(body.take(6)? == b"AVGC\0\x01")?;
    let setup = body.array()?;
    valid(body.take(32)? == claim)?;
    body.end()?;
    valid(r.blob(32)? == hash(&cce("aven-e2ee/v1/membership/state", &[state])))?;
    r.end()?;

    let mut r = Reader(attachments);
    valid(r.take(9)? == b"AVGA\0\x01\x01\x01\x01")?;
    valid(r.blob(32)? == device)?;
    valid(r.blob(32)? == hpke_public)?;
    r.blob(32)?;
    r.blob(191)?;
    r.end()?;

    let public = VerifyingKey::from_bytes(&signing_public)
        .map_err(|_| anyhow::anyhow!("error seed-claim-signature"))?;
    let signature = Signature::from_slice(signature)
        .map_err(|_| anyhow::anyhow!("error seed-claim-signature"))?;
    public
        .verify_strict(
            &cce("aven-e2ee/v1/membership/sign", &[core, attachments]),
            &signature,
        )
        .map_err(|_| anyhow::anyhow!("error seed-claim-signature"))?;
    Ok(Genesis {
        record: record.try_into()?,
        context: LocalSharedStatePackageContext {
            vault_id,
            generation_id,
        },
        setup,
        claim,
        device,
        signing_public,
        hpke_public,
        verifier,
        generation_commitment,
    })
}
