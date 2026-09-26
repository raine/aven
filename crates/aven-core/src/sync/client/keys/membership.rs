//! Append-only protected floors bind separately stored public chain evidence.
use super::*;
use crate::sync::seed_claim::membership::{
    Device, Evidence, MAX_COVERAGE_BYTES, MAX_EVIDENCE_JSON_BYTES, MAX_TRANSITIONS, Membership,
    VerifiedKeys,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
type Hash = [u8; 32];
pub(super) const FLOOR_LIMIT: usize = 1024;
pub(super) const COVERAGE_LIMIT: usize = 44 + MAX_COVERAGE_BYTES;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EvidenceRef {
    pub digest: Hash,
    pub length: usize,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Floor {
    sequence: u64,
    head: Hash,
    previous: u64,
    evidence: EvidenceRef,
    coverage: Hash,
}

impl ProtectedLocalKeyStore {
    pub(super) fn read_owned(
        &self,
        kind: &str,
        limit: usize,
        required: bool,
    ) -> Result<Option<Zeroizing<Vec<u8>>>> {
        if self.with_index(|index| index.listing.is_none()) == Some(true) {
            let listing = self.list_kinds()?;
            self.with_index(|index| index.listing = Some(listing));
        }
        let known = self.with_index(|index| {
            let listing = index.listing.as_ref()?;
            if let Some(value) = index.values.get(kind) {
                return Some(Ok(Some(value.clone())));
            }
            if listing.items.contains(kind) {
                return None;
            }
            Some(if required || listing.markers.contains(kind) {
                Err(anyhow::anyhow!("error enrollment-protected-missing"))
            } else {
                Ok(None)
            })
        });
        if let Some(Some(known)) = known {
            return known;
        }
        // A listed item that fails to load has vanished, never absent.
        let listed = known.is_some();
        let marker_name = format!("{kind}-authority");
        let marker = self.read_record(&marker_name, 32)?;
        let Some(frame) = self.load_secret(kind, limit)? else {
            ensure!(
                !required && !listed && marker.is_none(),
                "error enrollment-protected-missing"
            );
            return Ok(None);
        };
        ensure!(
            frame.len() == limit
                && limit >= 44
                && &frame[..8] == b"AVENPER2"
                && frame[8..40] == hex::decode(&self.account)?,
            "error enrollment-store-unsupported"
        );
        let len = u32::from_be_bytes(frame[40..44].try_into()?) as usize;
        ensure!(
            len <= limit - 44 && frame[44 + len..].iter().all(|b| *b == 0),
            "error enrollment-protected-framing"
        );
        let digest = Sha256::digest(&frame);
        ensure!(
            marker
                .as_ref()
                .is_none_or(|m| m.as_slice() == digest.as_slice()),
            "error enrollment-protected-corrupt"
        );
        if marker.is_none() {
            self.write_record(&marker_name, &digest)?;
        }
        let value = Zeroizing::new(frame[44..44 + len].to_vec());
        self.with_index(|index| {
            if let Some(listing) = &mut index.listing {
                listing.items.insert(kind.to_string());
                listing.markers.insert(kind.to_string());
            }
            index.values.insert(kind.to_string(), value.clone());
        });
        Ok(Some(value))
    }
    pub(super) fn write_owned(&self, kind: &str, limit: usize, bytes: &[u8]) -> Result<()> {
        ensure!(
            bytes.len() <= limit - 44,
            "error enrollment-protected-limit"
        );
        if let Some(saved) = self.read_owned(kind, limit, false)? {
            ensure!(
                saved.as_slice() == bytes,
                "error enrollment-protected-conflict"
            );
            return Ok(());
        }
        let mut frame = Zeroizing::new(vec![0; limit]);
        frame[..8].copy_from_slice(b"AVENPER2");
        frame[8..40].copy_from_slice(&hex::decode(&self.account)?);
        frame[40..44].copy_from_slice(&(bytes.len() as u32).to_be_bytes());
        frame[44..44 + bytes.len()].copy_from_slice(bytes);
        self.create_secret(kind, &frame)?;
        self.with_index(|index| {
            if let Some(listing) = &mut index.listing {
                listing.items.insert(kind.to_string());
            }
        });
        ensure!(
            self.read_owned(kind, limit, true)?
                .as_ref()
                .map(|v| v.as_slice())
                == Some(bytes),
            "error enrollment-protected-write"
        );
        #[cfg(any(test, feature = "test-support"))]
        if let Ok(requested) = std::env::var("AVEN_PEER_CRASH_KIND") {
            let matches = requested == kind
                || (kind.starts_with("invite-")
                    && match requested.as_str() {
                        "peer-bound" => kind.ends_with("-bound"),
                        "peer-candidate" => kind.ends_with("-candidate-0"),
                        "peer-sent" => kind.ends_with("-sent-0"),
                        "peer-ready" => kind.ends_with("-ready"),
                        _ => false,
                    });
            if matches {
                std::process::exit(79);
            }
        }
        Ok(())
    }
    fn evidence_record(digest: &Hash) -> String {
        format!("membership-evidence-{}", hex::encode(digest))
    }
    /// Callers pass evidence they have already verified; every load verifies
    /// it again before use.
    pub(super) fn save_evidence(&self, evidence: &Evidence) -> Result<EvidenceRef> {
        let bytes = serde_json::to_vec(evidence)?;
        ensure!(
            bytes.len() <= MAX_EVIDENCE_JSON_BYTES,
            "error membership-limit"
        );
        let reference = EvidenceRef {
            digest: Sha256::digest(&bytes).into(),
            length: bytes.len(),
        };
        let name = Self::evidence_record(&reference.digest);
        let saved = match self.read_record(&name, bytes.len())? {
            Some(saved) => saved,
            None => {
                self.write_record(&name, &bytes)?;
                self.read_record(&name, bytes.len())?
                    .context("error membership-evidence-missing")?
            }
        };
        ensure!(saved == bytes, "error membership-evidence-corrupt");
        #[cfg(any(test, feature = "test-support"))]
        if std::env::var("AVEN_PEER_CRASH_KIND").as_deref() == Ok("membership-evidence") {
            std::process::exit(79);
        }
        Ok(reference)
    }
    pub(super) fn load_evidence(&self, reference: &EvidenceRef) -> Result<(Evidence, Membership)> {
        Evidence::decode(&self.load_evidence_bytes(reference)?)
    }
    /// Digest-checked but unverified; the caller must replay it before use.
    pub(super) fn load_evidence_bytes(&self, reference: &EvidenceRef) -> Result<Vec<u8>> {
        ensure!(
            reference.length <= MAX_EVIDENCE_JSON_BYTES,
            "error membership-limit"
        );
        let bytes = self
            .read_record(&Self::evidence_record(&reference.digest), reference.length)?
            .context("error membership-evidence-missing")?;
        ensure!(
            Sha256::digest(&bytes).as_slice() == reference.digest,
            "error membership-evidence-corrupt"
        );
        Ok(bytes)
    }
    /// Caller retains both installation and store exclusion across read and use.
    pub(super) async fn membership_floor(
        &self,
        db: &Database,
        identity: Hash,
    ) -> Result<Option<(Membership, Evidence, VerifiedKeys)>> {
        let mut floors: Vec<Floor> = Vec::new();
        for sequence in 1..=MAX_TRANSITIONS + 1 {
            let kind = format!("membership-floor-{sequence}");
            let Some(bytes) = self.read_owned(&kind, FLOOR_LIMIT, false)? else {
                continue;
            };
            let floor: Floor = serde_json::from_slice(&bytes)
                .map_err(|_| anyhow::anyhow!("error membership-floor-corrupt"))?;
            ensure!(
                floor.sequence == sequence as u64,
                "error membership-floor-corrupt"
            );
            if let Some(before) = floors.last() {
                ensure!(
                    floor.previous == before.sequence,
                    "error membership-floor-fork"
                );
            } else {
                ensure!(floor.previous == 0, "error membership-floor-missing");
            }
            floors.push(floor);
        }
        // Only the latest chain is replayed; its hash-linked heads prove every
        // earlier floor is an ancestor, so loads stay linear in the chain length.
        let latest = match floors.last() {
            Some(floor) => {
                let (evidence, m) = self.load_evidence(&floor.evidence)?;
                ensure!(
                    m.sequence() == floor.sequence && m.head() == floor.head,
                    "error membership-floor-corrupt"
                );
                ensure!(
                    floors
                        .iter()
                        .all(|earlier| m.head_at(earlier.sequence) == Some(earlier.head)),
                    "error membership-floor-fork"
                );
                let coverage = self
                    .read_owned(
                        &format!("membership-coverage-{}", floor.sequence),
                        COVERAGE_LIMIT,
                        true,
                    )?
                    .context("error membership-coverage-missing")?;
                ensure!(
                    Sha256::digest(&coverage).as_slice() == floor.coverage,
                    "error membership-coverage-corrupt"
                );
                let keys = VerifiedKeys::from_protected_storage(&m, &coverage)?;
                Some((m, evidence, keys, floor.evidence.digest))
            }
            None => None,
        };
        if let Some((id, sequence, head, digest)) = db.membership_checkpoint_mirror().await? {
            let (m, _, _, _) = latest.as_ref().context("error membership-floor-missing")?;
            ensure!(
                id == identity && m.head_at(sequence) == Some(head),
                "error membership-mirror"
            );
            let bytes = self
                .read_owned(&format!("membership-floor-{sequence}"), FLOOR_LIMIT, true)?
                .context("error membership-floor-missing")?;
            let floor: Floor = serde_json::from_slice(&bytes)
                .map_err(|_| anyhow::anyhow!("error membership-floor-corrupt"))?;
            ensure!(floor.evidence.digest == digest, "error membership-mirror");
        }
        if let Some((m, _, _, digest)) = &latest {
            db.mirror_membership_checkpoint(identity, m, *digest)
                .await?;
        }
        Ok(latest.map(|(m, evidence, keys, _)| (m, evidence, keys)))
    }
    /// `m` must be what `evidence` verifies to. Key coverage and own identity
    /// must be verified by the caller first.
    pub(super) async fn adopt_membership(
        &self,
        db: &Database,
        identity: Hash,
        evidence: &Evidence,
        m: Membership,
        keys: &VerifiedKeys,
    ) -> Result<Membership> {
        keys.validate(&m)?;
        let before = self.membership_floor(db, identity).await?;
        if let Some((before, _, _)) = &before {
            ensure!(m.extends(before), "error membership-floor-fork");
        }
        let coverage = keys.protected_storage_bytes();
        self.write_owned(
            &format!("membership-coverage-{}", m.sequence()),
            COVERAGE_LIMIT,
            &coverage,
        )?;
        let reference = self.save_evidence(evidence)?;
        if before
            .as_ref()
            .is_none_or(|(before, _, _)| before.sequence() != m.sequence())
        {
            let floor = Floor {
                sequence: m.sequence(),
                head: m.head(),
                previous: before.as_ref().map_or(0, |(m, _, _)| m.sequence()),
                evidence: reference.clone(),
                coverage: Sha256::digest(&coverage).into(),
            };
            self.write_owned(
                &format!("membership-floor-{}", m.sequence()),
                FLOOR_LIMIT,
                &serde_json::to_vec(&floor)?,
            )?;
        }
        db.mirror_membership_checkpoint(identity, &m, reference.digest)
            .await?;
        Ok(m)
    }
    /// `target` must be what `evidence` verifies to. Replay only authenticated
    /// descendants, retaining each required recipient key.
    pub(super) async fn refresh_membership(
        &self,
        db: &Database,
        identity: Hash,
        device: Device<'_>,
        evidence: &Evidence,
        target: Membership,
        original: Verified<'_>,
    ) -> Result<(Membership, VerifiedKeys)> {
        let (mut before, mut keys) =
            if let Some((m, _, keys)) = self.membership_floor(db, identity).await? {
                ensure!(
                    m.extends(original.membership),
                    "error membership-original-mismatch"
                );
                (m, keys)
            } else {
                (original.membership.clone(), original.keys.clone())
            };
        device.validate(&before)?;
        ensure!(target.extends(&before), "error membership-floor-fork");
        for transition in evidence
            .transitions
            .iter()
            .skip(before.sequence() as usize - 1)
        {
            let next = before.append(
                &transition.declaration,
                &transition.request,
                &transition.record,
            )?;
            if device.validate(&next).is_err() {
                // Signed removal is a durable denial, not an inference from a network error.
                let mut removal = evidence.clone();
                removal.transitions.truncate(next.sequence() as usize - 1);
                self.adopt_membership(db, identity, &removal, next, &keys)
                    .await?;
                anyhow::bail!("error enrollment-revoked");
            }
            if next.generations().len() != before.generations().len() {
                keys = device.receive_rotation(&before, &transition.record, &keys)?;
            }
            keys.validate(&next)?;
            before = next;
        }
        let target = self
            .adopt_membership(db, identity, evidence, target, &keys)
            .await?;
        Ok((target, keys))
    }
}

/// A membership with the recipient keys verified against it.
#[derive(Clone, Copy)]
pub(super) struct Verified<'a> {
    pub(super) membership: &'a Membership,
    pub(super) keys: &'a VerifiedKeys,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn largest_floor_fits_protected_frame() {
        let floor = Floor {
            sequence: MAX_TRANSITIONS as u64 + 1,
            head: [255; 32],
            previous: MAX_TRANSITIONS as u64,
            evidence: EvidenceRef {
                digest: [255; 32],
                length: MAX_EVIDENCE_JSON_BYTES,
            },
            coverage: [255; 32],
        };
        let bytes = serde_json::to_vec(&floor).unwrap();
        assert!(bytes.len() + 44 <= FLOOR_LIMIT);
    }

    #[test]
    fn listed_item_that_vanishes_before_load_fails_closed() {
        let temp = tempfile::tempdir().unwrap();
        let database_path = temp.path().join("database.sqlite");
        std::fs::File::create(&database_path).unwrap();
        let store = test_support::account_store(
            test_support::raw_account(&database_path),
            &temp.path().join("keys"),
        );
        store.prepare().unwrap();
        {
            let _lock = store.lock().unwrap();
            store.write_owned("record", FLOOR_LIMIT, b"value").unwrap();
        }
        let _lock = store.lock().unwrap();
        assert!(
            store
                .read_owned("other", FLOOR_LIMIT, false)
                .unwrap()
                .is_none()
        );
        std::fs::remove_file(store.file_path("record")).unwrap();
        std::fs::remove_file(store.file_path("record-authority")).unwrap();
        let error = store.read_owned("record", FLOOR_LIMIT, false).unwrap_err();
        assert_eq!(error.to_string(), "error enrollment-protected-missing");
    }
}
