//! Bounded public transition evidence, without server or protected-store authority.
use super::*;
use crate::sync::base64_bytes;
use serde::{Deserialize, Serialize};

/// Base64 of the whole chain, up to one padding group per byte field, and
/// JSON framing for every transition.
pub const MAX_EVIDENCE_JSON_BYTES: usize =
    base64_bytes::encoded_len(MAX_CHAIN_BYTES) + 4 * (3 * MAX_TRANSITIONS + 3) + 16384;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRecord {
    #[serde(
        serialize_with = "base64_bytes::serialize",
        deserialize_with = "declaration_bytes"
    )]
    pub declaration: Vec<u8>,
    #[serde(
        serialize_with = "base64_bytes::serialize",
        deserialize_with = "request_bytes"
    )]
    pub request: Vec<u8>,
    #[serde(
        serialize_with = "base64_bytes::serialize",
        deserialize_with = "record_bytes"
    )]
    pub record: Vec<u8>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    #[serde(
        serialize_with = "base64_bytes::serialize",
        deserialize_with = "genesis_bytes"
    )]
    pub genesis: Vec<u8>,
    #[serde(
        serialize_with = "base64_bytes::serialize",
        deserialize_with = "publication_bytes"
    )]
    pub publication: Vec<u8>,
    #[serde(
        serialize_with = "base64_bytes::serialize",
        deserialize_with = "descriptor_bytes"
    )]
    pub descriptor: Vec<u8>,
    #[serde(deserialize_with = "records")]
    pub transitions: Vec<EvidenceRecord>,
}
fn record_bytes<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Vec<u8>, D::Error> {
    base64_bytes::bounded::<D, MAX_RECORD_BYTES>(d)
}
fn declaration_bytes<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Vec<u8>, D::Error> {
    base64_bytes::bounded::<D, DECLARATION_BYTES>(d)
}
fn request_bytes<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Vec<u8>, D::Error> {
    base64_bytes::bounded::<D, REQUEST_BYTES>(d)
}
fn genesis_bytes<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Vec<u8>, D::Error> {
    base64_bytes::bounded::<D, GENESIS_BYTES>(d)
}
fn publication_bytes<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Vec<u8>, D::Error> {
    base64_bytes::bounded::<D, PUBLICATION_BYTES>(d)
}
fn descriptor_bytes<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Vec<u8>, D::Error> {
    base64_bytes::bounded::<D, { crate::sync::bootstrap_format::MAX_DESCRIPTOR_BYTES }>(d)
}
fn records<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Vec<EvidenceRecord>, D::Error> {
    struct Records;
    impl<'de> serde::de::Visitor<'de> for Records {
        type Value = Vec<EvidenceRecord>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("bounded transition evidence")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> std::result::Result<Self::Value, A::Error> {
            if seq.size_hint().is_some_and(|n| n > MAX_TRANSITIONS) {
                return Err(serde::de::Error::custom("membership-limit"));
            }
            let mut result = Vec::new();
            let mut total = 0usize;
            while let Some(record) = seq.next_element::<EvidenceRecord>()? {
                total += record.declaration.len() + record.request.len() + record.record.len();
                if result.len() == MAX_TRANSITIONS || total > MAX_CHAIN_BYTES {
                    return Err(serde::de::Error::custom("membership-limit"));
                }
                result.push(record);
            }
            Ok(result)
        }
    }
    d.deserialize_seq(Records)
}
impl Evidence {
    /// Verify the complete chain while retaining the exact original enrollment.
    pub fn enrollment(&self, peer: &Joiner, outcome: Hash) -> Result<VerifiedEnrollment> {
        let current = self.verify()?;
        let g = Genesis::from_record(&self.genesis)?;
        let mut before = Membership::from_publication(&g, &self.descriptor, &self.publication)?;
        for a in &self.transitions {
            if hash(&a.record) == outcome {
                let verified = peer.verify_enrollment(&before, &a.declaration, &a.record)?;
                ensure!(
                    current.extends(verified.membership()),
                    "error membership-fork"
                );
                return Ok(verified);
            }
            before = before.append(&a.declaration, &a.request, &a.record)?;
        }
        anyhow::bail!("error enrollment-outcome-missing")
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() <= MAX_EVIDENCE_JSON_BYTES,
            "error membership-limit"
        );
        let evidence: Self = serde_json::from_slice(bytes)
            .map_err(|_| anyhow::anyhow!("error membership-evidence"))?;
        evidence.verify()?;
        Ok(evidence)
    }
    pub fn verify(&self) -> Result<Membership> {
        ensure!(
            self.transitions.len() <= MAX_TRANSITIONS,
            "error membership-limit"
        );
        let mut size = self
            .genesis
            .len()
            .checked_add(self.publication.len())
            .and_then(|n| n.checked_add(self.descriptor.len()))
            .context("error membership-limit")?;
        for a in &self.transitions {
            ensure!(
                (a.declaration.len() == DECLARATION_BYTES && a.request.len() == REQUEST_BYTES
                    || a.declaration.is_empty() && a.request.is_empty())
                    && a.record.len() <= MAX_RECORD_BYTES,
                "error membership-limit"
            );
            size = size
                .checked_add(a.declaration.len())
                .and_then(|n| n.checked_add(a.request.len()))
                .and_then(|n| n.checked_add(a.record.len()))
                .context("error membership-limit")?;
        }
        ensure!(size <= MAX_CHAIN_BYTES, "error membership-limit");
        let genesis = Genesis::from_record(&self.genesis)?;
        let mut m = Membership::from_publication(&genesis, &self.descriptor, &self.publication)?;
        for a in &self.transitions {
            m = m.append(&a.declaration, &a.request, &a.record)?;
        }
        Ok(m)
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mailbox {
    #[serde(
        serialize_with = "base64_bytes::serialize",
        deserialize_with = "declaration_bytes"
    )]
    pub declaration: Vec<u8>,
    #[serde(with = "base64_bytes::option")]
    pub request: Option<Vec<u8>>,
    #[serde(with = "base64_bytes::option")]
    pub admission: Option<Vec<u8>>,
}
