//! Bounded public transition evidence, without server or protected-store authority.
use super::*;
use serde::{Deserialize, Serialize};

pub const MAX_EVIDENCE_JSON_BYTES: usize = 4 * MAX_CHAIN_BYTES + 16384;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRecord {
    #[serde(deserialize_with = "declaration_bytes")]
    pub declaration: Vec<u8>,
    #[serde(deserialize_with = "request_bytes")]
    pub request: Vec<u8>,
    #[serde(deserialize_with = "record_bytes")]
    pub record: Vec<u8>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    #[serde(deserialize_with = "genesis_bytes")]
    pub genesis: Vec<u8>,
    #[serde(deserialize_with = "publication_bytes")]
    pub publication: Vec<u8>,
    #[serde(deserialize_with = "descriptor_bytes")]
    pub descriptor: Vec<u8>,
    #[serde(deserialize_with = "records")]
    pub transitions: Vec<EvidenceRecord>,
}
fn bounded<'de, D, T, const N: usize>(d: D) -> std::result::Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Visitor<T, const N: usize>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>, const N: usize> serde::de::Visitor<'de> for Visitor<T, N> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("bounded membership evidence")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> std::result::Result<Self::Value, A::Error> {
            if seq.size_hint().is_some_and(|n| n > N) {
                return Err(serde::de::Error::custom("membership-limit"));
            }
            let mut values = Vec::new();
            while values.len() < N {
                let Some(value) = seq.next_element()? else {
                    return Ok(values);
                };
                values.push(value);
            }
            if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                return Err(serde::de::Error::custom("membership-limit"));
            }
            Ok(values)
        }
    }
    d.deserialize_seq(Visitor::<T, N>(std::marker::PhantomData))
}
fn record_bytes<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Vec<u8>, D::Error> {
    bounded::<D, u8, MAX_RECORD_BYTES>(d)
}
fn declaration_bytes<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Vec<u8>, D::Error> {
    bounded::<D, u8, DECLARATION_BYTES>(d)
}
fn request_bytes<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Vec<u8>, D::Error> {
    bounded::<D, u8, REQUEST_BYTES>(d)
}
fn genesis_bytes<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Vec<u8>, D::Error> {
    bounded::<D, u8, GENESIS_BYTES>(d)
}
fn publication_bytes<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Vec<u8>, D::Error> {
    bounded::<D, u8, PUBLICATION_BYTES>(d)
}
fn descriptor_bytes<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Vec<u8>, D::Error> {
    bounded::<D, u8, 1024>(d)
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
    #[serde(deserialize_with = "declaration_bytes")]
    pub declaration: Vec<u8>,
    pub request: Option<Vec<u8>>,
    pub admission: Option<Vec<u8>>,
}
