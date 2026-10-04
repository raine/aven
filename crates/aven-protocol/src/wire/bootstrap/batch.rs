//! Binary framing of one staging batch:
//! `b"AVBU" | u16 version | u32 header_len | JSON header | record bytes`.
//!
//! Integers are big-endian. The header names each record's slot and length;
//! record bytes follow back to back in header order with nothing after them.
//! Decoding checks every bound and length before handing out any record, so a
//! server can refuse a malformed batch without touching storage.
use anyhow::{Result, ensure};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{Component, MAX_REQUEST_BYTES};

pub const CONTENT_TYPE: &str = "application/vnd.aven.bootstrap-batch";
pub const MAGIC: &[u8; 4] = b"AVBU";
pub const VERSION: u16 = 1;
/// Upper bound on the sum of record lengths in one batch.
pub const MAX_PAYLOAD: usize = 4 * 1_048_576;
pub const MAX_RECORDS: usize = 64;
pub const MAX_HEADER: usize = 4096;
const PREFIX: usize = MAGIC.len() + 2 + 4;
/// Upper bound on one encoded batch. A single record of at most
/// [`MAX_REQUEST_BYTES`] always fits.
pub const MAX_BYTES: usize = PREFIX + MAX_HEADER + MAX_PAYLOAD;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Header {
    #[serde(with = "hex32")]
    pub vault: [u8; 32],
    #[serde(with = "hex32")]
    pub genesis: [u8; 32],
    #[serde(with = "hex32")]
    pub bootstrap: [u8; 32],
    #[serde(with = "hex32")]
    pub commitment: [u8; 32],
    pub records: Vec<Slot>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Slot {
    #[serde(serialize_with = "component_out", deserialize_with = "component_in")]
    pub component: Component,
    pub index: u64,
    pub len: u64,
}

/// A decoded batch whose records borrow the request body.
pub struct Batch<'a> {
    pub header: Header,
    pub records: Vec<&'a [u8]>,
}

/// Checks the header's shape: record count, per-record length, the payload
/// total, unique slots, and that catalog slices never share a batch with
/// the records a catalog describes.
fn check(header: &Header) -> Result<usize> {
    let records = &header.records;
    ensure!(
        !records.is_empty() && records.len() <= MAX_RECORDS,
        "error bootstrap-batch-shape"
    );
    let mut payload = 0_usize;
    for slot in records {
        let len = usize::try_from(slot.len)
            .ok()
            .filter(|len| *len > 0 && *len <= MAX_REQUEST_BYTES);
        let len = len.ok_or_else(|| anyhow::anyhow!("error bootstrap-batch-shape"))?;
        payload = payload
            .checked_add(len)
            .filter(|total| *total <= MAX_PAYLOAD)
            .ok_or_else(|| anyhow::anyhow!("error bootstrap-batch-shape"))?;
    }
    let catalogs = records
        .iter()
        .filter(|slot| slot.component.catalog().is_some())
        .count();
    ensure!(
        catalogs == 0 || catalogs == records.len(),
        "error bootstrap-batch-mixed"
    );
    for (position, slot) in records.iter().enumerate() {
        ensure!(
            records[..position]
                .iter()
                .all(|other| (other.component, other.index) != (slot.component, slot.index)),
            "error bootstrap-batch-shape"
        );
    }
    Ok(payload)
}

/// The encoded header length, or `None` if it exceeds [`MAX_HEADER`].
pub fn header_len(header: &Header) -> Option<usize> {
    serde_json::to_vec(header)
        .ok()
        .map(|bytes| bytes.len())
        .filter(|len| *len <= MAX_HEADER)
}

pub fn encode(header: &Header, records: &[&[u8]]) -> Result<Vec<u8>> {
    let payload = check(header)?;
    ensure!(
        records.len() == header.records.len()
            && records
                .iter()
                .zip(&header.records)
                .all(|(bytes, slot)| bytes.len() as u64 == slot.len),
        "error bootstrap-batch-shape"
    );
    let json = serde_json::to_vec(header)?;
    ensure!(json.len() <= MAX_HEADER, "error bootstrap-batch-shape");
    let mut out = Vec::with_capacity(PREFIX + json.len() + payload);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_be_bytes());
    out.extend_from_slice(&(json.len() as u32).to_be_bytes());
    out.extend_from_slice(&json);
    for record in records {
        out.extend_from_slice(record);
    }
    Ok(out)
}

pub fn decode(bytes: &[u8]) -> Result<Batch<'_>> {
    ensure!(
        bytes.len() >= PREFIX && bytes.len() <= MAX_BYTES,
        "error bootstrap-batch-framing"
    );
    let (magic, rest) = bytes.split_at(MAGIC.len());
    ensure!(magic == MAGIC, "error bootstrap-batch-framing");
    let (version, rest) = rest.split_at(2);
    ensure!(
        u16::from_be_bytes([version[0], version[1]]) == VERSION,
        "error bootstrap-batch-version"
    );
    let (len, rest) = rest.split_at(4);
    let len = u32::from_be_bytes([len[0], len[1], len[2], len[3]]) as usize;
    ensure!(
        len <= MAX_HEADER && len <= rest.len(),
        "error bootstrap-batch-framing"
    );
    let (json, mut body) = rest.split_at(len);
    let header: Header = serde_json::from_slice(json)
        .map_err(|_| anyhow::anyhow!("error bootstrap-batch-framing"))?;
    let payload = check(&header)?;
    ensure!(body.len() == payload, "error bootstrap-batch-framing");
    let mut records = Vec::with_capacity(header.records.len());
    for slot in &header.records {
        let (record, rest) = body.split_at(slot.len as usize);
        records.push(record);
        body = rest;
    }
    Ok(Batch { header, records })
}

fn component_out<S: Serializer>(component: &Component, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&hex::encode(component.key()))
}

fn component_in<'de, D: Deserializer<'de>>(d: D) -> Result<Component, D::Error> {
    let text = String::deserialize(d)?;
    hex::decode(&text)
        .ok()
        .and_then(|key| Component::from_key(&key))
        .ok_or_else(|| serde::de::Error::custom("invalid component"))
}

mod hex32 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S: Serializer>(value: &[u8; 32], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(value))
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 32], D::Error> {
        let text = String::deserialize(d)?;
        let mut out = [0; 32];
        hex::decode_to_slice(&text, &mut out)
            .map_err(|_| serde::de::Error::custom("invalid 32-byte hex"))?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests;
