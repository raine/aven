//! Append-only protected floors bind separately stored public chain evidence.
use super::*;
use anyhow::{Context, Result, ensure};
use aven_core::sync::seed_claim::membership::{
    Evidence, MAX_DEVICES, MAX_EVIDENCE_JSON_BYTES, Membership,
};
use serde::{Deserialize, Serialize};
type Hash = [u8; 32];
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
}

impl ProtectedLocalKeyStore {
    pub(super) fn read_owned(
        &self,
        kind: &str,
        limit: usize,
        required: bool,
    ) -> Result<Option<Zeroizing<Vec<u8>>>> {
        let marker_path = self
            .directory
            .join(format!("{}.{}-authority", self.account, kind));
        let marker = read_restricted_file(&marker_path, 32)?;
        let Some(frame) = self.adoption_backend(kind).load_bounded(limit)? else {
            ensure!(
                !required && marker.is_none(),
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
            write_restricted_new(&marker_path, &digest)?;
        }
        Ok(Some(Zeroizing::new(frame[44..44 + len].to_vec())))
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
        self.adoption_backend(kind).create(&frame)?;
        ensure!(
            self.read_owned(kind, limit, true)?
                .as_ref()
                .map(|v| v.as_slice())
                == Some(bytes),
            "error enrollment-protected-write"
        );
        #[cfg(test)]
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
    fn evidence_path(&self, digest: &Hash) -> PathBuf {
        self.directory.join(format!(
            "{}.membership-evidence-{}",
            self.account,
            hex::encode(digest)
        ))
    }
    pub(super) fn save_evidence(&self, evidence: &Evidence) -> Result<EvidenceRef> {
        evidence.verify()?;
        let bytes = serde_json::to_vec(evidence)?;
        ensure!(
            bytes.len() <= MAX_EVIDENCE_JSON_BYTES,
            "error membership-limit"
        );
        let reference = EvidenceRef {
            digest: Sha256::digest(&bytes).into(),
            length: bytes.len(),
        };
        let path = self.evidence_path(&reference.digest);
        if let Some(saved) = read_restricted_file(&path, bytes.len())? {
            ensure!(saved == bytes, "error membership-evidence-corrupt");
        } else {
            write_restricted_new(&path, &bytes)?;
        }
        self.load_evidence(&reference)?;
        Ok(reference)
    }
    pub(super) fn load_evidence(&self, reference: &EvidenceRef) -> Result<Evidence> {
        ensure!(
            reference.length <= MAX_EVIDENCE_JSON_BYTES,
            "error membership-limit"
        );
        let bytes = read_restricted_file(&self.evidence_path(&reference.digest), reference.length)?
            .context("error membership-evidence-missing")?;
        ensure!(
            Sha256::digest(&bytes).as_slice() == reference.digest,
            "error membership-evidence-corrupt"
        );
        Evidence::decode(&bytes)
    }
    /// Caller retains both installation and store exclusion across read and use.
    pub(super) async fn membership_floor(
        &self,
        db: &Database,
        identity: Hash,
    ) -> Result<Option<(Membership, EvidenceRef)>> {
        let mut latest: Option<(Membership, EvidenceRef)> = None;
        for sequence in 1..=MAX_DEVICES {
            let kind = format!("membership-floor-{sequence}");
            let Some(bytes) = self.read_owned(&kind, 512, false)? else {
                continue;
            };
            let floor: Floor = serde_json::from_slice(&bytes)
                .map_err(|_| anyhow::anyhow!("error membership-floor-corrupt"))?;
            let m = self.load_evidence(&floor.evidence)?.verify()?;
            ensure!(
                floor.sequence == sequence as u64
                    && m.sequence() == floor.sequence
                    && m.head() == floor.head,
                "error membership-floor-corrupt"
            );
            if let Some((before, _)) = &latest {
                ensure!(
                    floor.previous == before.sequence() && m.extends(before),
                    "error membership-floor-fork"
                );
            } else {
                ensure!(floor.previous == 0, "error membership-floor-missing");
            }
            latest = Some((m, floor.evidence));
        }
        if let Some((id, sequence, head, digest)) = db.membership_checkpoint_mirror().await? {
            let (m, _) = latest.as_ref().context("error membership-floor-missing")?;
            ensure!(
                id == identity && m.head_at(sequence) == Some(head),
                "error membership-mirror"
            );
            let bytes = self
                .read_owned(&format!("membership-floor-{sequence}"), 512, true)?
                .context("error membership-floor-missing")?;
            let floor: Floor = serde_json::from_slice(&bytes)?;
            ensure!(floor.evidence.digest == digest, "error membership-mirror");
        }
        if let Some((m, reference)) = &latest {
            db.mirror_membership_checkpoint(identity, m, reference.digest)
                .await?;
        }
        Ok(latest)
    }
    /// Key coverage and own identity must be verified by the caller first.
    pub(super) async fn adopt_membership(
        &self,
        db: &Database,
        identity: Hash,
        evidence: &Evidence,
        key: &LocalSharedStatePackageKey,
    ) -> Result<Membership> {
        let m = evidence.verify()?;
        m.validate_key(key)?;
        let before = self.membership_floor(db, identity).await?;
        if let Some((before, _)) = &before {
            ensure!(m.extends(before), "error membership-floor-fork");
        }
        let reference = self.save_evidence(evidence)?;
        if before
            .as_ref()
            .is_none_or(|(before, _)| before.sequence() != m.sequence())
        {
            let floor = Floor {
                sequence: m.sequence(),
                head: m.head(),
                previous: before.as_ref().map_or(0, |(m, _)| m.sequence()),
                evidence: reference.clone(),
            };
            self.write_owned(
                &format!("membership-floor-{}", m.sequence()),
                512,
                &serde_json::to_vec(&floor)?,
            )?;
        }
        db.mirror_membership_checkpoint(identity, &m, reference.digest)
            .await?;
        Ok(m)
    }
}
