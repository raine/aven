//! Independent installation identity, inbound enrollment and outbound journals.
use super::membership::{EvidenceRef, FLOOR_LIMIT};
use super::*;
use anyhow::{Context, Result, ensure};
use aven_core::db::installation::InstallationGuard;
use aven_core::sync::{
    SeedPublicationIntent,
    seed_claim::{
        Secret, SeedAuthority,
        membership::{
            self, Declaration, Device, Evidence, Invitation, Joiner, Membership,
            VerifiedEnrollment, VerifiedKeys,
        },
    },
};
use serde::{Deserialize, Serialize};
type Hash = [u8; 32];
const IDENTITY_LIMIT: usize = 8192;
const JOURNAL_LIMIT: usize = 4096;
const CANDIDATE_LIMIT: usize = 4 * membership::MAX_RECORD_BYTES + 1024;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Identity {
    role: String,
    client: String,
    incarnation: Hash,
    locator: String,
    authority: Vec<u8>,
}
impl Drop for Identity {
    fn drop(&mut self) {
        self.authority.zeroize();
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Outbound {
    pub handle: Hash,
    invitation: Vec<u8>,
    pub declaration: Vec<u8>,
}
impl Drop for Outbound {
    fn drop(&mut self) {
        self.invitation.zeroize();
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Candidate {
    predecessor: EvidenceRef,
    record: Vec<u8>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Verified {
    evidence: EvidenceRef,
    outcome: Hash,
    key: Vec<u8>,
}
impl Drop for Verified {
    fn drop(&mut self) {
        self.key.zeroize();
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum EnrollmentReadiness {
    NotSelected,
    Pending,
    UnresolvedDisclosure,
    Enrolled { head: Hash },
}
impl EnrollmentReadiness {
    pub fn require_resolved_disclosure(&self) -> Result<()> {
        ensure!(
            !matches!(self, Self::UnresolvedDisclosure),
            "error withdrawal-required-unsupported"
        );
        ensure!(
            !matches!(self, Self::Pending),
            "error enrollment-unresolved"
        );
        Ok(())
    }
}
enum Keys {
    Seed(Box<SeedAuthority>),
    Peer(Box<Joiner>),
}
impl Keys {
    fn authority(&self) -> Device<'_> {
        match self {
            Self::Seed(s) => Device::seed(s),
            Self::Peer(p) => p.authority(),
        }
    }
    fn device(&self) -> Hash {
        match self {
            Self::Seed(s) => s.genesis().device_id(),
            Self::Peer(p) => p.device(),
        }
    }
    fn bearer(&self) -> &Secret {
        match self {
            Self::Seed(s) => s.bearer(),
            Self::Peer(p) => p.bearer(),
        }
    }
}
pub(crate) struct ActiveInputs {
    id: Identity,
    keys: Keys,
    pub membership: Membership,
    pub evidence: Evidence,
    coverage: VerifiedKeys,
    _installation: InstallationGuard,
    _lock: File,
}
impl ActiveInputs {
    pub fn generation_keys(&self) -> &VerifiedKeys {
        &self.coverage
    }
    pub fn device(&self) -> Hash {
        self.keys.device()
    }
    pub fn bearer(&self) -> &Secret {
        self.keys.bearer()
    }
}
impl ProtectedLocalKeyStore {
    pub(super) fn peer_authority_exists(&self) -> StoreResult<bool> {
        Ok(read_restricted_file(
            &self
                .directory
                .join(format!("{}.peer-identity-authority", self.account)),
            32,
        )?
        .is_some()
            || self
                .adoption_backend("peer-identity")
                .load_bounded(IDENTITY_LIMIT)?
                .is_some())
    }
    async fn phase(
        &self,
        db: &Database,
        name: &str,
        size: usize,
    ) -> Result<Option<Zeroizing<Vec<u8>>>> {
        let pin = db.enrollment_artifact(name).await?;
        let bytes = self.read_owned(name, size, pin.is_some())?;
        if let (Some(pin), Some(bytes)) = (pin, &bytes) {
            ensure!(
                pin == Sha256::digest(bytes.as_slice()).as_slice(),
                "error enrollment-phase-mismatch"
            );
        }
        Ok(bytes)
    }
    async fn save_phase(
        &self,
        db: &Database,
        id: &Identity,
        name: &str,
        size: usize,
        bytes: &[u8],
    ) -> Result<()> {
        self.phase(db, name, size).await?;
        self.write_owned(name, size, bytes)?;
        db.pin_enrollment_artifact(id.incarnation, name, Sha256::digest(bytes).into())
            .await
    }
    async fn identity(&self, db: &Database, guard: &InstallationGuard) -> Result<Option<Identity>> {
        let pin = db.enrollment_pin().await?;
        let Some(bytes) = self.read_owned("peer-identity", IDENTITY_LIMIT, pin.is_some())? else {
            ensure!(
                db.membership_checkpoint_mirror().await?.is_none()
                    && self.journals(db).await?.is_empty(),
                "error enrollment-protected-missing"
            );
            for (kind, size) in [
                ("peer-sent", 128),
                ("peer-response", CANDIDATE_LIMIT + 4096),
                ("peer-verified", 2048),
                ("peer-ready", 128),
                ("peer-installed", 128),
            ] {
                ensure!(
                    self.phase(db, kind, size).await?.is_none(),
                    "error enrollment-protected-missing"
                );
            }
            for sequence in 1..=membership::MAX_TRANSITIONS + 1 {
                ensure!(
                    self.read_owned(&format!("membership-floor-{sequence}"), FLOOR_LIMIT, false)?
                        .is_none(),
                    "error enrollment-protected-missing"
                );
                ensure!(
                    self.read_owned(&format!("membership-coverage-{sequence}"), 4096, false)?
                        .is_none(),
                    "error enrollment-protected-missing"
                );
            }
            return Ok(None);
        };
        let id: Identity = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("error enrollment-protected-framing"))?;
        ensure!(
            id.locator.len() <= 2048 && matches!(id.role.as_str(), "inviter" | "peer"),
            "error enrollment-protected-framing"
        );
        if pin.is_none() {
            ensure!(
                self.read_owned("peer-ready", 128, false)?.is_none()
                    && self.read_owned("peer-sent", 128, false)?.is_none()
                    && self
                        .read_owned("outbound-0", JOURNAL_LIMIT, false)?
                        .is_none(),
                "error enrollment-database-lost"
            );
        }
        db.pin_enrollment(id.incarnation, &id.client, &id.role, guard)
            .await?;
        Ok(Some(id))
    }
    fn adopted_inputs(&self, seed: &SeedAuthority) -> Result<SeedPublicationIntent> {
        let source = self.decode_source(
            &self
                .load_adoption_record("source", 104, true)?
                .context("error enrollment-source-missing")?,
            seed,
        )?;
        SeedPublicationIntent::from_protected_storage(
            &self
                .load_adoption_record("intent", 65536, true)?
                .context("error enrollment-intent-missing")?,
            &source,
            seed.genesis(),
        )
    }
    async fn seed_inputs(
        &self,
        db: &Database,
    ) -> Result<(SeedAuthority, Evidence, LocalSharedStatePackageKey)> {
        let package = self.load_required_locked()?;
        let seed = self.required_seed(&package)?;
        let intent = self.adopted_inputs(&seed)?;
        let (saved, state) = db
            .seed_publication_intent_bytes()
            .await?
            .context("error seed-intent-missing")?;
        ensure!(
            state == "adopted" && saved == intent.protected_storage_bytes(),
            "error seed-intent-mismatch"
        );
        let source = self.decode_source(
            &self
                .load_adoption_record("source", 104, true)?
                .context("error seed-source-missing")?,
            &seed,
        )?;
        ensure!(
            db.seed_source_pin().await?.as_deref() == Some(source.protected_storage_bytes()),
            "error seed-source-mismatch"
        );
        let publication = intent.publication(seed.genesis())?;
        let evidence = Evidence {
            genesis: seed.genesis().record().to_vec(),
            publication: publication.record().to_vec(),
            descriptor: intent.descriptor().to_vec(),
            transitions: vec![],
        };
        Ok((
            seed,
            evidence,
            LocalSharedStatePackageKey::new(*package.package_key().protected_storage_bytes()),
        ))
    }
    pub(crate) async fn active_inputs(&self, db: &Database, locator: &str) -> Result<ActiveInputs> {
        let installation = InstallationGuard::acquire(db.path())?;
        self.validate_database(db)?;
        ensure!(locator.len() <= 2048, "error enrollment-locator-limit");
        prepare_directory(&self.directory)?;
        let lock = self.lock()?;
        let id = match self.identity(db, &installation).await? {
            Some(id) => id,
            None => {
                let client = db.adopted_enrollment_client().await?;
                self.seed_inputs(db).await?;
                let id = Identity {
                    role: "inviter".into(),
                    client,
                    incarnation: *Secret::generate()?.expose(),
                    locator: locator.into(),
                    authority: vec![],
                };
                self.write_owned(
                    "peer-identity",
                    IDENTITY_LIMIT,
                    &Zeroizing::new(serde_json::to_vec(&id)?),
                )?;
                db.pin_enrollment(id.incarnation, &id.client, &id.role, &installation)
                    .await?;
                id
            }
        };
        ensure!(id.locator == locator, "error enrollment-context");
        let (keys, original, original_keys) = if id.role == "peer" {
            ensure!(
                self.phase(db, "peer-ready", 128).await?.is_some(),
                "error enrollment-unresolved"
            );
            let (peer, verified, record) = self.verified_peer(db, &id).await?;
            let installed = self
                .phase(db, "peer-installed", 128)
                .await?
                .context("error snapshot-not-installed")?;
            ensure!(
                installed.as_slice() == verified.checkpoint()
                    && db
                        .peer_snapshot_receipt(&verified, id.incarnation, &id.client, &installation)
                        .await?
                        .is_some(),
                "error snapshot-receipt-mismatch"
            );
            (
                Keys::Peer(Box::new(peer)),
                self.load_evidence(&record.evidence)?,
                VerifiedKeys::from_protected_storage(
                    verified.membership(),
                    &verified.keys().protected_storage_bytes(),
                )?,
            )
        } else {
            ensure!(
                db.adopted_enrollment_client().await? == id.client,
                "error enrollment-client-mismatch"
            );
            let (seed, evidence, key) = self.seed_inputs(db).await?;
            let coverage = evidence.verify()?.verify_initial_key(&key)?;
            (Keys::Seed(Box::new(seed)), evidence, coverage)
        };
        let original_membership = original.verify()?;
        let (membership, evidence, coverage) =
            if let Some((m, r, coverage)) = self.membership_floor(db, id.incarnation).await? {
                (m, self.load_evidence(&r)?, coverage)
            } else {
                ensure!(
                    id.role == "inviter" && self.journals(db).await?.is_empty(),
                    "error membership-floor-missing"
                );
                let m = self
                    .adopt_membership(db, id.incarnation, &original, &original_keys)
                    .await?;
                (m, original, original_keys)
            };
        ensure!(
            membership.extends(&original_membership),
            "error membership-original-mismatch"
        );
        keys.authority().validate(&membership)?;
        coverage.validate(&membership)?;
        for journal in self.journals(db).await? {
            if let Some(ready) = self.phase(db, &journal.name("ready"), 128).await? {
                ensure!(
                    membership.contains_head(&ready.as_slice().try_into()?),
                    "error enrollment-checkpoint-mismatch"
                );
            }
        }
        Ok(ActiveInputs {
            id,
            keys,
            membership,
            evidence,
            coverage,
            _installation: installation,
            _lock: lock,
        })
    }
    pub(crate) async fn adopt_refresh(
        &self,
        db: &Database,
        inputs: &mut ActiveInputs,
        evidence: Evidence,
    ) -> Result<()> {
        let (membership, coverage) = self
            .refresh_membership(
                db,
                inputs.id.incarnation,
                inputs.keys.authority(),
                &evidence,
                &inputs.membership,
                &inputs.coverage,
            )
            .await?;
        inputs.membership = membership;
        inputs.coverage = coverage;
        inputs.evidence = evidence;
        Ok(())
    }
    async fn journals(&self, db: &Database) -> Result<Vec<Outbound>> {
        let mut journals = Vec::new();
        let mut gap = false;
        for index in 0..membership::MAX_INVITATIONS {
            let Some(bytes) = self
                .phase(db, &format!("outbound-{index}"), JOURNAL_LIMIT)
                .await?
            else {
                gap = true;
                continue;
            };
            ensure!(!gap, "error enrollment-journal-missing");
            let journal: Outbound = serde_json::from_slice(&bytes)
                .map_err(|_| anyhow::anyhow!("error enrollment-journal-corrupt"))?;
            let invitation = Invitation::from_protected_storage(&journal.invitation)?;
            ensure!(
                journal.handle == invitation.handle(),
                "error enrollment-journal-corrupt"
            );
            for (phase, size) in [("registered", 128), ("bound", 2048), ("ready", 128)] {
                self.phase(db, &journal.name(phase), size).await?;
            }
            let mut gap = false;
            let bound = self.phase(db, &journal.name("bound"), 2048).await?;
            let ready = self.phase(db, &journal.name("ready"), 128).await?;
            let mut resolved = ready.is_none();
            for candidate in 0..membership::MAX_CANDIDATES {
                let bytes = self
                    .phase(
                        db,
                        &journal.name(&format!("candidate-{candidate}")),
                        CANDIDATE_LIMIT,
                    )
                    .await?;
                let sent = self
                    .phase(db, &journal.name(&format!("sent-{candidate}")), 128)
                    .await?;
                if let Some(bytes) = bytes {
                    ensure!(!gap, "error enrollment-candidate-missing");
                    let saved: Candidate = serde_json::from_slice(&bytes)
                        .map_err(|_| anyhow::anyhow!("error enrollment-protected-framing"))?;
                    let before = self.load_evidence(&saved.predecessor)?.verify()?;
                    let after = before.append(
                        &journal.declaration,
                        bound.as_ref().context("error enrollment-binding-missing")?,
                        &saved.record,
                    )?;
                    if let Some(sent) = &sent {
                        ensure!(
                            sent.as_slice() == after.head(),
                            "error enrollment-sent-mismatch"
                        );
                    }
                    if ready.as_ref().is_some_and(|r| r.as_slice() == after.head()) {
                        ensure!(sent.is_some(), "error enrollment-sent-missing");
                        resolved = true;
                    }
                } else {
                    gap = true;
                    ensure!(sent.is_none(), "error enrollment-candidate-missing");
                }
            }
            ensure!(resolved, "error enrollment-candidate-missing");
            journals.push(journal);
        }
        Ok(journals)
    }
    async fn outbound_readiness(&self, db: &Database) -> Result<Option<EnrollmentReadiness>> {
        for journal in self.journals(db).await? {
            if self.phase(db, &journal.name("ready"), 128).await?.is_some() {
                continue;
            }
            let sent = self
                .phase(db, &journal.name("sent-0"), 128)
                .await?
                .is_some();
            return Ok(Some(if sent {
                EnrollmentReadiness::UnresolvedDisclosure
            } else {
                EnrollmentReadiness::Pending
            }));
        }
        Ok(None)
    }
    pub(crate) async fn prepare_invitation(
        &self,
        db: &Database,
        inputs: &ActiveInputs,
        expires: Option<u64>,
        handle: Option<Hash>,
    ) -> Result<Outbound> {
        let mut journals = self.journals(db).await?;
        if let Some(handle) = handle {
            return journals
                .into_iter()
                .find(|j| j.handle == handle)
                .context("error enrollment-invitation-missing");
        }
        if expires.is_none() {
            return journals
                .pop()
                .context("error enrollment-invitation-missing");
        }
        for journal in &journals {
            if self.phase(db, &journal.name("ready"), 128).await?.is_none() {
                // Exact registration retry retains the immutable invitation.
                ensure!(
                    self.phase(db, &journal.name("sent-0"), 128)
                        .await?
                        .is_none(),
                    "error withdrawal-required-unsupported"
                );
                return journals
                    .pop()
                    .context("error enrollment-invitation-missing");
            }
        }
        ensure!(
            journals.len() < membership::MAX_INVITATIONS,
            "error membership-limit"
        );
        let (invitation, d) = inputs.keys.authority().prepare_invitation(
            &inputs.membership,
            expires.context("error enrollment-expiry")?,
        )?;
        let journal = Outbound {
            handle: invitation.handle(),
            invitation: invitation.protected_storage_bytes().to_vec(),
            declaration: d.record().to_vec(),
        };
        self.save_phase(
            db,
            &inputs.id,
            &format!("outbound-{}", journals.len()),
            JOURNAL_LIMIT,
            &Zeroizing::new(serde_json::to_vec(&journal)?),
        )
        .await?;
        Ok(journal)
    }
    pub(crate) async fn registered_invitation(
        &self,
        db: &Database,
        inputs: &ActiveInputs,
        journal: &Outbound,
    ) -> Result<Invitation> {
        self.save_phase(
            db,
            &inputs.id,
            &journal.name("registered"),
            128,
            &Sha256::digest(&journal.declaration),
        )
        .await?;
        Invitation::from_protected_storage(&journal.invitation)
    }
    pub(crate) async fn prepare_admission(
        &self,
        db: &Database,
        inputs: &ActiveInputs,
        journal: &Outbound,
        request: &[u8],
    ) -> Result<Vec<u8>> {
        ensure!(
            self.phase(db, &journal.name("registered"), 128)
                .await?
                .is_some(),
            "error enrollment-not-registered"
        );
        // Binding precedes even tentative grant preparation, and never rebinds.
        self.save_phase(db, &inputs.id, &journal.name("bound"), 2048, request)
            .await?;
        let d = Declaration::from_record(&inputs.membership, &journal.declaration)?;
        let mut index = 0;
        for attempt in 0..membership::MAX_CANDIDATES {
            let Some(bytes) = self
                .phase(
                    db,
                    &journal.name(&format!("candidate-{attempt}")),
                    CANDIDATE_LIMIT,
                )
                .await?
            else {
                break;
            };
            let candidate: Candidate = serde_json::from_slice(&bytes)
                .map_err(|_| anyhow::anyhow!("error enrollment-protected-framing"))?;
            let predecessor = self.load_evidence(&candidate.predecessor)?.verify()?;
            let result = predecessor.append(&journal.declaration, request, &candidate.record)?;
            ensure!(
                inputs.membership.extends(&predecessor),
                "error enrollment-candidate-fork"
            );
            if inputs.membership.contains_head(&result.head())
                || inputs.membership.head() == predecessor.head()
            {
                self.save_phase(
                    db,
                    &inputs.id,
                    &journal.name(&format!("sent-{attempt}")),
                    128,
                    &result.head(),
                )
                .await?;
                return Ok(candidate.record);
            }
            // A verified different successor at the signed slot resolves CAS loss.
            ensure!(
                inputs
                    .membership
                    .head_at(result.sequence())
                    .is_some_and(|head| head != result.head()),
                "error enrollment-candidate-unresolved"
            );
            index = attempt + 1;
        }
        ensure!(
            index < membership::MAX_CANDIDATES,
            "error membership-candidate-limit"
        );
        let inv = Invitation::from_protected_storage(&journal.invitation)?;
        let record = inputs.keys.authority().prepare_admission(
            &inputs.membership,
            &d,
            &inv,
            request,
            inputs.generation_keys(),
        )?;
        let candidate = Candidate {
            predecessor: self.save_evidence(&inputs.evidence)?,
            record: record.clone(),
        };
        self.save_phase(
            db,
            &inputs.id,
            &journal.name(&format!("candidate-{index}")),
            CANDIDATE_LIMIT,
            &serde_json::to_vec(&candidate)?,
        )
        .await?;
        self.save_phase(
            db,
            &inputs.id,
            &journal.name(&format!("sent-{index}")),
            128,
            &Sha256::digest(&record),
        )
        .await?;
        Ok(record)
    }
    pub(crate) async fn finish_inviter(
        &self,
        db: &Database,
        inputs: &ActiveInputs,
        journal: &Outbound,
        record: &[u8],
    ) -> Result<()> {
        let head: Hash = Sha256::digest(record).into();
        ensure!(
            inputs.membership.contains_head(&head),
            "error enrollment-outcome-missing"
        );
        let mut matched = false;
        for attempt in 0..membership::MAX_CANDIDATES {
            let Some(bytes) = self
                .phase(
                    db,
                    &journal.name(&format!("candidate-{attempt}")),
                    CANDIDATE_LIMIT,
                )
                .await?
            else {
                break;
            };
            let candidate: Candidate = serde_json::from_slice(&bytes)
                .map_err(|_| anyhow::anyhow!("error enrollment-protected-framing"))?;
            if candidate.record == record {
                let sent = self
                    .phase(db, &journal.name(&format!("sent-{attempt}")), 128)
                    .await?
                    .context("error enrollment-sent-missing")?;
                ensure!(sent.as_slice() == head, "error enrollment-sent-mismatch");
                matched = true;
            }
        }
        ensure!(matched, "error enrollment-outcome-mismatch");
        self.save_phase(db, &inputs.id, &journal.name("ready"), 128, &head)
            .await
    }
    pub(crate) async fn prepare_peer(
        &self,
        db: &Database,
        locator: &str,
        invitation: Option<Invitation>,
    ) -> Result<Joiner> {
        let guard = InstallationGuard::acquire(db.path())?;
        self.validate_database(db)?;
        ensure!(locator.len() <= 2048, "error enrollment-locator-limit");
        prepare_directory(&self.directory)?;
        let _lock = self.lock()?;
        let id = match self.identity(db, &guard).await? {
            Some(id) => id,
            None => {
                let client = db.peer_target_preflight().await?;
                guard.ensure_unbound()?;
                ensure!(
                    self.backend.load()?.is_none()
                        && read_marker(&self.marker_path())?.is_none()
                        && !self.seed_authority_exists()?,
                    "error enrollment-existing-authority"
                );
                let peer = Joiner::generate(
                    invitation
                        .as_ref()
                        .map(|i| Invitation::from_protected_storage(&i.protected_storage_bytes()))
                        .transpose()?
                        .context("error enrollment-invitation-required")?,
                )?;
                guard.fence()?;
                let id = Identity {
                    role: "peer".into(),
                    client,
                    incarnation: *Secret::generate()?.expose(),
                    locator: locator.into(),
                    authority: peer.protected_storage_bytes().to_vec(),
                };
                self.write_owned(
                    "peer-identity",
                    IDENTITY_LIMIT,
                    &Zeroizing::new(serde_json::to_vec(&id)?),
                )?;
                db.pin_enrollment(id.incarnation, &id.client, &id.role, &guard)
                    .await?;
                id
            }
        };
        ensure!(
            id.role == "peer" && id.locator == locator,
            "error enrollment-context"
        );
        let peer = Joiner::from_protected_storage(&id.authority)?;
        ensure!(
            invitation
                .as_ref()
                .is_none_or(|i| peer.matches_invitation(i)),
            "error enrollment-invitation-conflict"
        );
        self.save_phase(db, &id, "peer-sent", 128, &Sha256::digest(peer.request()))
            .await?;
        Ok(peer)
    }
    pub(crate) async fn pin_peer_response(
        &self,
        db: &Database,
        mail: &membership::Mailbox,
    ) -> Result<membership::ProvisionalGrant> {
        let guard = InstallationGuard::acquire(db.path())?;
        self.validate_database(db)?;
        let _lock = self.lock()?;
        let id = self
            .identity(db, &guard)
            .await?
            .context("error enrollment-missing")?;
        ensure!(id.role == "peer", "error enrollment-role");
        let peer = Joiner::from_protected_storage(&id.authority)?;
        ensure!(
            mail.request.as_deref() == Some(peer.request()),
            "error enrollment-request-mismatch"
        );
        let grant = peer.open_provisional(
            &mail.declaration,
            mail.admission
                .as_deref()
                .context("error enrollment-outcome-missing")?,
        )?;
        self.save_phase(
            db,
            &id,
            "peer-response",
            CANDIDATE_LIMIT + 4096,
            &serde_json::to_vec(mail)?,
        )
        .await?;
        Ok(grant)
    }
    pub(crate) async fn finish_peer(&self, db: &Database, evidence: &Evidence) -> Result<()> {
        let guard = InstallationGuard::acquire(db.path())?;
        self.validate_database(db)?;
        let _lock = self.lock()?;
        let id = self
            .identity(db, &guard)
            .await?
            .context("error enrollment-missing")?;
        ensure!(id.role == "peer", "error enrollment-role");
        let response = self
            .phase(db, "peer-response", CANDIDATE_LIMIT + 4096)
            .await?
            .context("error enrollment-response-missing")?;
        let mail: membership::Mailbox = serde_json::from_slice(&response)
            .map_err(|_| anyhow::anyhow!("error enrollment-protected-framing"))?;
        let peer = Joiner::from_protected_storage(&id.authority)?;
        let grant = peer.open_provisional(
            &mail.declaration,
            mail.admission
                .as_deref()
                .context("error enrollment-outcome-missing")?,
        )?;
        let verified = evidence.enrollment(&peer, grant.outcome)?;
        let current = evidence.verify()?;
        // Retain only the original outcome's ancestry, never a mutable Ready value.
        let mut original = evidence.clone();
        original
            .transitions
            .truncate(verified.membership().sequence() as usize - 1);
        let record = Verified {
            evidence: self.save_evidence(&original)?,
            outcome: grant.outcome,
            key: verified.key().protected_storage_bytes().to_vec(),
        };
        self.save_phase(
            db,
            &id,
            "peer-verified",
            2048,
            &Zeroizing::new(serde_json::to_vec(&record)?),
        )
        .await?;
        if let Some((floor, _, _)) = self.membership_floor(db, id.incarnation).await?
            && floor.extends(&current)
        {
            peer.authority().validate(&floor)?;
            self.save_phase(db, &id, "peer-ready", 128, &record.outcome)
                .await?;
            return Ok(());
        }
        self.refresh_membership(
            db,
            id.incarnation,
            peer.authority(),
            evidence,
            verified.membership(),
            verified.keys(),
        )
        .await?;
        self.save_phase(db, &id, "peer-ready", 128, &record.outcome)
            .await
    }
    async fn verified_peer(
        &self,
        db: &Database,
        id: &Identity,
    ) -> Result<(Joiner, VerifiedEnrollment, Verified)> {
        let ready = self
            .phase(db, "peer-ready", 128)
            .await?
            .context("error enrollment-not-ready")?;
        let bytes = self
            .phase(db, "peer-verified", 2048)
            .await?
            .context("error enrollment-key-coverage-missing")?;
        let record: Verified = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("error enrollment-protected-framing"))?;
        let peer = Joiner::from_protected_storage(&id.authority)?;
        let verified = self
            .load_evidence(&record.evidence)?
            .enrollment(&peer, record.outcome)?;
        ensure!(
            ready.as_slice() == record.outcome
                && verified.key().protected_storage_bytes().as_slice() == record.key,
            "error enrollment-verified-corrupt"
        );
        Ok((peer, verified, record))
    }
    pub(crate) async fn install_peer_snapshot(
        &self,
        db: &Database,
        transport: &crate::peer_enrollment_http::Client,
        locator: &str,
    ) -> Result<aven_core::sync::SharedStateInstallReport> {
        let guard = InstallationGuard::acquire(db.path())?;
        self.validate_database(db)?;
        let _lock = self.lock()?;
        let id = self
            .identity(db, &guard)
            .await?
            .context("error enrollment-missing")?;
        ensure!(
            id.role == "peer" && id.locator == locator,
            "error enrollment-context"
        );
        let (peer, verified, record) = self.verified_peer(db, &id).await?;
        let completed = self.phase(db, "peer-installed", 128).await?;
        if let Some(report) = db
            .peer_snapshot_receipt(&verified, id.incarnation, &id.client, &guard)
            .await?
        {
            let (floor, _, _) = self
                .membership_floor(db, id.incarnation)
                .await?
                .context("error membership-floor-missing")?;
            peer.authority().validate(&floor)?;
            self.save_phase(db, &id, "peer-installed", 128, &verified.checkpoint())
                .await?;
            return Ok(report);
        }
        ensure!(
            completed.is_none(),
            "error snapshot-installed-database-lost"
        );
        let (floor, _, _) = self
            .membership_floor(db, id.incarnation)
            .await?
            .context("error membership-floor-missing")?;
        peer.authority().validate(&floor)?;
        let package = transport
            .download(
                self,
                db,
                id.incarnation,
                &peer,
                &verified,
                &self.load_evidence(&record.evidence)?.descriptor,
            )
            .await?;
        let report = db
            .install_peer_snapshot(&verified, id.incarnation, &id.client, &guard, &package)
            .await?;
        self.save_phase(db, &id, "peer-installed", 128, &verified.checkpoint())
            .await?;
        Ok(report)
    }
    pub(crate) async fn adopt_download_refresh(
        &self,
        db: &Database,
        identity: Hash,
        peer: &Joiner,
        verified: &VerifiedEnrollment,
        evidence: &Evidence,
    ) -> Result<Membership> {
        Ok(self
            .refresh_membership(
                db,
                identity,
                peer.authority(),
                evidence,
                verified.membership(),
                verified.keys(),
            )
            .await?
            .0)
    }
    pub(crate) async fn tail_inputs(&self, db: &Database, locator: &str) -> Result<TailInputs> {
        let inputs = self.active_inputs(db, locator).await?;
        if let Some(readiness) = self.outbound_readiness(db).await? {
            readiness.require_resolved_disclosure()?;
        }
        ensure!(
            !inputs.membership.rotation_pending() && inputs.membership.generations().len() == 1,
            "error membership-transition-unsupported"
        );
        let b = inputs.membership.publication().binding();
        let association = format!(
            "{}:{}:{}",
            hex::encode(b.vault_id),
            hex::encode(b.stream_id),
            hex::encode(b.bootstrap_id)
        );
        ensure!(
            db.meta("e2ee_association").await?.as_deref() == Some(association.as_str()),
            "error encrypted-tail-association"
        );
        let authority = aven_core::sync::encrypted_tail::Authority {
            context: aven_core::sync::encrypted_tail::Context {
                vault: b.vault_id,
                genesis: inputs.membership.genesis().commitment(),
                device: inputs.device(),
                credential_version: 1,
                head: inputs.membership.head(),
                stream: b.stream_id,
                descriptor: b.descriptor_commitment,
            },
            generation: inputs.membership.genesis().context().generation_id,
            key: LocalSharedStatePackageKey::new(
                *inputs
                    .coverage
                    .key(inputs.membership.genesis().context().generation_id)?
                    .protected_storage_bytes(),
            ),
            prefix: i64::try_from(b.prefix_count)?,
            association,
            sync_generation: db
                .meta("sync_generation")
                .await?
                .context("error encrypted-tail-generation")?
                .parse()?,
        };
        Ok(TailInputs {
            authority,
            bearer: Secret::new(*inputs.bearer().expose()),
            _inputs: inputs,
        })
    }
    pub async fn enrollment_readiness(&self, db: &Database) -> Result<EnrollmentReadiness> {
        let guard = InstallationGuard::acquire(db.path())?;
        self.validate_database(db)?;
        prepare_directory(&self.directory)?;
        let _lock = self.lock()?;
        let Some(id) = self.identity(db, &guard).await? else {
            return Ok(EnrollmentReadiness::NotSelected);
        };
        if let Some(readiness) = self.outbound_readiness(db).await? {
            return Ok(readiness);
        }
        if id.role == "peer" && self.phase(db, "peer-ready", 128).await?.is_none() {
            return Ok(EnrollmentReadiness::Pending);
        }
        let Some((m, _, _)) = self.membership_floor(db, id.incarnation).await? else {
            return Ok(EnrollmentReadiness::Pending);
        };
        if id.role == "peer" {
            self.verified_peer(db, &id)
                .await?
                .0
                .authority()
                .validate(&m)?;
        } else {
            Device::seed(&self.seed_inputs(db).await?.0).validate(&m)?;
        }
        Ok(EnrollmentReadiness::Enrolled { head: m.head() })
    }
}
impl Outbound {
    fn name(&self, phase: &str) -> String {
        format!("invite-{}-{phase}", hex::encode(self.handle))
    }
}
pub(crate) struct TailInputs {
    pub authority: aven_core::sync::encrypted_tail::Authority,
    pub bearer: Secret,
    _inputs: ActiveInputs,
}
