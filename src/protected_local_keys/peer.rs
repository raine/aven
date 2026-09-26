//! Independent installation identity, inbound enrollment and outbound journals.
use super::membership::{COVERAGE_LIMIT, EvidenceRef, FLOOR_LIMIT};
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
#[cfg(test)]
use std::sync::atomic::Ordering;
type Hash = [u8; 32];
const IDENTITY_LIMIT: usize = 8192;
const JOURNAL_LIMIT: usize = 4096;
// Attempts and mailbox responses are stored as separate protected records, so
// these per-record bounds do not grow with MAX_JOIN_ATTEMPTS.
const CANDIDATE_LIMIT: usize = 4 * membership::MAX_RECORD_BYTES + 1024;
const RESPONSE_LIMIT: usize = CANDIDATE_LIMIT + 4096;
const ATTEMPT_LIMIT: usize = 512;
/// Retained join attempts per installation, including the original request.
pub(crate) const MAX_JOIN_ATTEMPTS: usize = 16;
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

/// This installation's own enrollment. Invitations it issued never change it.
#[derive(Debug, PartialEq, Eq)]
pub enum EnrollmentReadiness {
    NotSelected,
    Pending,
    Enrolled { head: Hash },
}
/// Secret-free state of an issued invitation that is neither admitted nor closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OpenInvitation {
    pub(crate) handle: Hash,
    pub(crate) expires_at: u64,
    pub(crate) keys_may_have_been_sent: bool,
}

/// Where one issued invitation stands, as recorded on this device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InvitationProgress {
    Open,
    Admitted,
    Closed,
}

/// An issued invitation that is neither admitted nor closed.
#[cfg(test)]
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum OutboundInvitation {
    /// No grant was prepared for sending.
    Pending,
    /// A grant may have been sent.
    Disclosed,
}
/// New encrypted content waits for the withdrawal rotation of an expired
/// invitation whose grant may have been sent. Pulls and downloads continue.
#[derive(Debug)]
pub(crate) struct PublishingBlocked;
impl std::fmt::Display for PublishingBlocked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("error withdrawal-rotation-required")
    }
}
impl std::error::Error for PublishingBlocked {}
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
    _lock: StoreLock,
}
impl ActiveInputs {
    pub(super) fn authority(&self) -> Device<'_> {
        self.keys.authority()
    }

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
    pub(super) async fn phase(
        &self,
        db: &Database,
        name: &str,
        size: usize,
    ) -> Result<Option<Zeroizing<Vec<u8>>>> {
        let pin = self.enrollment_pin(db, name).await?;
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
        let digest: Hash = Sha256::digest(bytes).into();
        db.pin_enrollment_artifact(id.incarnation, name, digest)
            .await?;
        self.with_index(|index| {
            if let Some(pins) = &mut index.pins {
                pins.insert(name.to_string(), digest.to_vec());
            }
        });
        Ok(())
    }
    /// Pins are only ever added, by `save_phase`, so one bulk load serves the
    /// whole lock.
    async fn enrollment_pin(&self, db: &Database, name: &str) -> Result<Option<Vec<u8>>> {
        match self.with_index(|index| index.pins.as_ref().map(|pins| pins.get(name).cloned())) {
            None => db.enrollment_artifact(name).await,
            Some(Some(pin)) => Ok(pin),
            Some(None) => {
                let pins = db.enrollment_artifacts().await?;
                let pin = pins.get(name).cloned();
                self.with_index(|index| index.pins = Some(pins));
                Ok(pin)
            }
        }
    }
    pub(super) async fn save_management_phase(
        &self,
        db: &Database,
        inputs: &ActiveInputs,
        name: &str,
        size: usize,
        bytes: &[u8],
    ) -> Result<()> {
        self.save_phase(db, &inputs.id, name, size, bytes).await
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
                ("peer-response", RESPONSE_LIMIT),
                ("peer-verified", 2048),
                ("peer-ready", 128),
                ("peer-installed", 128),
            ] {
                ensure!(
                    self.phase(db, kind, size).await?.is_none(),
                    "error enrollment-protected-missing"
                );
            }
            for index in 1..MAX_JOIN_ATTEMPTS {
                ensure!(
                    self.phase(db, &format!("peer-attempt-{index}"), ATTEMPT_LIMIT)
                        .await?
                        .is_none(),
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
                    self.read_owned(
                        &format!("membership-coverage-{sequence}"),
                        COVERAGE_LIMIT,
                        false
                    )?
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
        self.validate_database(db).await?;
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
                verified.keys().clone(),
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
        let now = self.enrollment_now()?;
        for journal in self.journals(db).await? {
            if let Some(ready) = self.phase(db, &journal.name("ready"), 128).await? {
                ensure!(
                    membership.contains_head(&ready.as_slice().try_into()?),
                    "error enrollment-checkpoint-mismatch"
                );
            } else if !self.retired(db, &journal).await?
                && !self.may_have_sent(db, &journal).await?
                && now >= Declaration::from_record(&membership, &journal.declaration)?.expiry()
            {
                // This store lock also covers admission preparation and dispatch,
                // so no grant for this handle was or can be sent. Sent journals
                // stay unresolved until admission or a qualifying rotation.
                self.save_phase(db, &id, &journal.name("retired"), 128, &journal.handle)
                    .await?;
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
            self.phase(db, &journal.name("registered"), 128).await?;
            let bound = self.phase(db, &journal.name("bound"), 2048).await?;
            let ready = self.phase(db, &journal.name("ready"), 128).await?;
            let mut gap = false;
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
    /// Terminal pre-disclosure abandonment of an expired, never-sent invitation.
    async fn retired(&self, db: &Database, journal: &Outbound) -> Result<bool> {
        Ok(self
            .phase(db, &journal.name("retired"), 128)
            .await?
            .is_some())
    }
    /// Retired before disclosure, or withdrawn after a proven rotation.
    async fn closed(&self, db: &Database, journal: &Outbound) -> Result<bool> {
        Ok(self.retired(db, journal).await?
            || self
                .phase(db, &journal.name("withdrawn"), 128)
                .await?
                .is_some())
    }
    async fn may_have_sent(&self, db: &Database, journal: &Outbound) -> Result<bool> {
        for attempt in 0..membership::MAX_CANDIDATES {
            if self
                .phase(db, &journal.name(&format!("sent-{attempt}")), 128)
                .await?
                .is_some()
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
    /// Unadmitted, unclosed journals whose grant may have been sent, with their
    /// declared expiry.
    async fn unresolved_disclosures(
        &self,
        db: &Database,
        inputs: &ActiveInputs,
    ) -> Result<Vec<(Outbound, u64)>> {
        let mut unresolved = Vec::new();
        for journal in self.journals(db).await? {
            if self.phase(db, &journal.name("ready"), 128).await?.is_none()
                && !self.closed(db, &journal).await?
                && self.may_have_sent(db, &journal).await?
            {
                let expiry =
                    Declaration::from_record(&inputs.membership, &journal.declaration)?.expiry();
                unresolved.push((journal, expiry));
            }
        }
        Ok(unresolved)
    }
    pub(crate) async fn open_invitation(
        &self,
        db: &Database,
        inputs: &ActiveInputs,
    ) -> Result<Option<OpenInvitation>> {
        for journal in self.journals(db).await? {
            if self.phase(db, &journal.name("ready"), 128).await?.is_none()
                && !self.closed(db, &journal).await?
            {
                let declaration =
                    Declaration::from_record(&inputs.membership, &journal.declaration)?;
                return Ok(Some(OpenInvitation {
                    handle: journal.handle,
                    expires_at: declaration.expiry(),
                    keys_may_have_been_sent: self.may_have_sent(db, &journal).await?,
                }));
            }
        }
        Ok(None)
    }

    /// Reads only this invitation's terminal phases, so waiting for a join can
    /// check it often without loading every journal.
    pub(crate) async fn invitation_progress(
        &self,
        db: &Database,
        handle: &Hash,
    ) -> Result<InvitationProgress> {
        let recorded = async |name| {
            Ok::<_, anyhow::Error>(
                self.phase(db, &invitation_phase_name(handle, name), 128)
                    .await?
                    .is_some(),
            )
        };
        if recorded("ready").await? {
            Ok(InvitationProgress::Admitted)
        } else if recorded("retired").await? || recorded("withdrawn").await? {
            Ok(InvitationProgress::Closed)
        } else {
            Ok(InvitationProgress::Open)
        }
    }

    /// Retires the open invitation if no grant has been sent. The returned
    /// journal remains available for best-effort server cancellation.
    pub(crate) async fn retire_open_invitation(
        &self,
        db: &Database,
        inputs: &ActiveInputs,
    ) -> Result<(Option<Outbound>, Option<OpenInvitation>)> {
        let Some(state) = self.open_invitation(db, inputs).await? else {
            return Ok((None, None));
        };
        if state.keys_may_have_been_sent {
            return Ok((None, Some(state)));
        }
        let journal = self
            .journals(db)
            .await?
            .into_iter()
            .find(|journal| journal.handle == state.handle)
            .context("error enrollment-invitation-missing")?;
        self.save_phase(
            db,
            &inputs.id,
            &journal.name("retired"),
            128,
            &journal.handle,
        )
        .await?;
        Ok((Some(journal), Some(state)))
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
            if self.phase(db, &journal.name("ready"), 128).await?.is_none()
                && !self.closed(db, journal).await?
            {
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
        ensure!(
            !self.retired(db, journal).await?,
            "error enrollment-invitation-retired"
        );
        // Withdrawal never sends another candidate or resends an old one.
        ensure!(
            self.phase(db, &journal.name("withdrawing"), 128)
                .await?
                .is_none()
                && !self.closed(db, journal).await?,
            "error enrollment-invitation-withdrawing"
        );
        let d = Declaration::from_record(&inputs.membership, &journal.declaration)?;
        // Inputs may predate a long mailbox wait, so expiry is sampled here
        // rather than when they were loaded. A candidate that may already have
        // been sent can still be resent; anything else would be a first dispatch.
        let expired = self.enrollment_now()? >= d.expiry();
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
            // A stored candidate means this binding exists; it never rebinds.
            self.save_phase(db, &inputs.id, &journal.name("bound"), 2048, request)
                .await?;
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
                let sent = journal.name(&format!("sent-{attempt}"));
                ensure!(
                    !expired || self.phase(db, &sent, 128).await?.is_some(),
                    "error enrollment-expired"
                );
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
        ensure!(!expired, "error enrollment-expired");
        let inv = Invitation::from_protected_storage(&journal.invitation)?;
        let record = inputs.keys.authority().prepare_admission(
            &inputs.membership,
            &d,
            &inv,
            request,
            inputs.generation_keys(),
        )?;
        // Binding follows the request's full validation during preparation, so
        // an unauthenticated request never occupies it, and precedes the
        // candidate that depends on it.
        self.save_phase(db, &inputs.id, &journal.name("bound"), 2048, request)
            .await?;
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
    /// The unresolved journal whose grant may have been sent, once its
    /// declared expiry has passed. Expiry only starts withdrawal work.
    pub(crate) async fn expired_disclosure(
        &self,
        db: &Database,
        inputs: &ActiveInputs,
    ) -> Result<Option<Outbound>> {
        let now = self.enrollment_now()?;
        Ok(self
            .unresolved_disclosures(db, inputs)
            .await?
            .into_iter()
            .find(|(_, expiry)| now >= *expiry)
            .map(|(journal, _)| journal))
    }
    /// Fences local candidate creation and resends before remote cancellation.
    pub(crate) async fn mark_withdrawing(
        &self,
        db: &Database,
        inputs: &ActiveInputs,
        journal: &Outbound,
    ) -> Result<()> {
        self.save_phase(
            db,
            &inputs.id,
            &journal.name("withdrawing"),
            128,
            &journal.handle,
        )
        .await
    }
    /// Resolves a possibly disclosed journal from verified membership only.
    /// A candidate at its signed slot is an admission and finishes `ready`.
    /// `withdrawn` requires every stored candidate to have lost its slot, no
    /// candidate recipient ever becoming a member, and a later non-pending
    /// generation absent from every candidate predecessor.
    pub(crate) async fn reconcile_disclosure(
        &self,
        db: &Database,
        inputs: &ActiveInputs,
        journal: &Outbound,
    ) -> Result<Disclosure> {
        let bound = self
            .phase(db, &journal.name("bound"), 2048)
            .await?
            .context("error enrollment-binding-missing")?;
        let mut candidates = Vec::new();
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
            let saved: Candidate = serde_json::from_slice(&bytes)
                .map_err(|_| anyhow::anyhow!("error enrollment-protected-framing"))?;
            let before = self.load_evidence(&saved.predecessor)?.verify()?;
            ensure!(
                inputs.membership.extends(&before),
                "error enrollment-candidate-fork"
            );
            let after = before.append(&journal.declaration, &bound, &saved.record)?;
            match inputs.membership.head_at(after.sequence()) {
                Some(head) if head == after.head() => {
                    self.finish_inviter(db, inputs, journal, &saved.record)
                        .await?;
                    return Ok(Disclosure::Admitted);
                }
                Some(_) => {}
                None => return Ok(Disclosure::Unresolved),
            }
            let recipient = after
                .devices()
                .find(|device| !before.has_device(*device))
                .context("error enrollment-candidate-recipient")?;
            candidates.push((before, after.sequence(), recipient));
        }
        let disclosed = candidates
            .iter()
            .flat_map(|(before, _, _)| before.generations().iter().map(|g| g.id))
            .collect::<std::collections::HashSet<_>>();
        let last_slot = candidates.iter().map(|(_, slot, _)| *slot).max();
        let mut state = candidates
            .iter()
            .map(|(before, _, _)| before)
            .min_by_key(|before| before.sequence())
            .context("error enrollment-candidate-missing")?
            .clone();
        let mut qualifying = None;
        for t in inputs
            .evidence
            .transitions
            .iter()
            .skip(state.sequence() as usize - 1)
        {
            state = state.append(&t.declaration, &t.request, &t.record)?;
            ensure!(
                candidates
                    .iter()
                    .all(|(_, _, recipient)| !state.has_device(*recipient)),
                "error withdrawal-recipient-member"
            );
            if qualifying.is_none()
                && Some(state.sequence()) > last_slot
                && !state.rotation_pending()
                && !disclosed.contains(&state.current_generation().id)
            {
                qualifying = Some(state.head());
            }
        }
        ensure!(
            state.head() == inputs.membership.head(),
            "error enrollment-checkpoint-mismatch"
        );
        let Some(head) = qualifying else {
            return Ok(Disclosure::Unresolved);
        };
        self.save_phase(db, &inputs.id, &journal.name("withdrawn"), 128, &head)
            .await?;
        Ok(Disclosure::Withdrawn)
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
    /// The request to post: a known invitation's exact attempt, otherwise the
    /// latest attempt, or the one a pinned response answers.
    pub(crate) async fn prepare_peer(
        &self,
        db: &Database,
        locator: &str,
        invitation: Option<Invitation>,
    ) -> Result<Joiner> {
        self.select_peer(db, locator, invitation, false).await
    }
    /// Like `prepare_peer`, but an unknown invitation becomes a new attempt
    /// while joining is unfinished. The installation keys, enrollment pin and
    /// fence stay unchanged, and every earlier attempt is retained.
    pub(crate) async fn replace_peer(
        &self,
        db: &Database,
        locator: &str,
        invitation: Invitation,
    ) -> Result<Joiner> {
        self.select_peer(db, locator, Some(invitation), true).await
    }
    async fn select_peer(
        &self,
        db: &Database,
        locator: &str,
        invitation: Option<Invitation>,
        replace: bool,
    ) -> Result<Joiner> {
        let guard = InstallationGuard::acquire(db.path())?;
        self.validate_database(db).await?;
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
        let mut attempts = self.attempts(db, &id).await?;
        let known = invitation
            .as_ref()
            .map(|i| attempts.iter().position(|peer| peer.matches_invitation(i)));
        Ok(match (known, self.response(db).await?) {
            (Some(None), _) => {
                ensure!(replace, "error enrollment-invitation-conflict");
                self.ensure_retry_allowed(db, &id, &guard, attempts.len())
                    .await?;
                let peer = attempts[0].retry(invitation.expect("unknown invitation"))?;
                self.save_phase(
                    db,
                    &id,
                    &format!("peer-attempt-{}", attempts.len()),
                    ATTEMPT_LIMIT,
                    &peer.attempt_bytes(),
                )
                .await?;
                peer
            }
            (_, Some(mail)) => responding(attempts, &mail)?,
            (Some(Some(index)), None) => attempts.swap_remove(index),
            (None, None) => attempts.pop().expect("original attempt"),
        })
    }
    /// Every retained attempt, oldest first, or only the one a pinned
    /// response answers. Attempts are never removed.
    pub(crate) async fn peer_attempts(&self, db: &Database, locator: &str) -> Result<Vec<Joiner>> {
        let guard = InstallationGuard::acquire(db.path())?;
        self.validate_database(db).await?;
        let _lock = self.lock()?;
        let id = self
            .identity(db, &guard)
            .await?
            .context("error enrollment-missing")?;
        ensure!(
            id.role == "peer" && id.locator == locator,
            "error enrollment-context"
        );
        let attempts = self.attempts(db, &id).await?;
        Ok(match self.response(db).await? {
            Some(mail) => vec![responding(attempts, &mail)?],
            None => attempts,
        })
    }
    /// The immutable original followed by retained replacement attempts.
    /// The original's `peer-sent` record, written and committed before its
    /// first post, must match; a record written before its SQLite commitment
    /// is committed unchanged, as is any replacement attempt.
    async fn attempts(&self, db: &Database, id: &Identity) -> Result<Vec<Joiner>> {
        let original = Joiner::from_protected_storage(&id.authority)?;
        let sent = Sha256::digest(original.request());
        if let Some(saved) = self.phase(db, "peer-sent", 128).await? {
            ensure!(
                saved.as_slice() == sent.as_slice(),
                "error enrollment-sent-mismatch"
            );
        }
        self.save_phase(db, id, "peer-sent", 128, &sent).await?;
        let mut attempts = Vec::new();
        let mut gap = false;
        for index in 1..MAX_JOIN_ATTEMPTS {
            let name = format!("peer-attempt-{index}");
            let Some(bytes) = self.phase(db, &name, ATTEMPT_LIMIT).await? else {
                gap = true;
                continue;
            };
            ensure!(!gap, "error enrollment-attempt-missing");
            if db.enrollment_artifact(&name).await?.is_none() {
                self.save_phase(db, id, &name, ATTEMPT_LIMIT, &bytes)
                    .await?;
            }
            attempts.push(original.attempt(&bytes)?);
        }
        attempts.insert(0, original);
        Ok(attempts)
    }
    /// A replacement attempt only while no admission has been accepted here:
    /// no pinned response or later phase, membership floor, outbound journal,
    /// installation or association, and an empty domain.
    async fn ensure_retry_allowed(
        &self,
        db: &Database,
        id: &Identity,
        guard: &InstallationGuard,
        attempts: usize,
    ) -> Result<()> {
        for (kind, size) in [
            ("peer-response", RESPONSE_LIMIT),
            ("peer-verified", 2048),
            ("peer-ready", 128),
            ("peer-installed", 128),
        ] {
            ensure!(
                self.phase(db, kind, size).await?.is_none(),
                "error enrollment-retry-unavailable"
            );
        }
        ensure!(
            self.membership_floor(db, id.incarnation).await?.is_none()
                && db.membership_checkpoint_mirror().await?.is_none()
                && self.journals(db).await?.is_empty(),
            "error enrollment-retry-unavailable"
        );
        db.peer_retry_preflight(id.incarnation, &id.client, guard)
            .await?;
        ensure!(attempts < MAX_JOIN_ATTEMPTS, "error enrollment-retry-limit");
        Ok(())
    }
    async fn response(&self, db: &Database) -> Result<Option<membership::Mailbox>> {
        self.phase(db, "peer-response", RESPONSE_LIMIT)
            .await?
            .map(|bytes| {
                serde_json::from_slice(&bytes)
                    .map_err(|_| anyhow::anyhow!("error enrollment-protected-framing"))
            })
            .transpose()
    }
    /// Opens a mailbox admission without retaining it. The signature and
    /// membership binding are unchecked here, so only `finish_peer` pins it.
    pub(crate) async fn open_peer_response(
        &self,
        db: &Database,
        mail: &membership::Mailbox,
    ) -> Result<membership::ProvisionalGrant> {
        let guard = InstallationGuard::acquire(db.path())?;
        self.validate_database(db).await?;
        let _lock = self.lock()?;
        let id = self
            .identity(db, &guard)
            .await?
            .context("error enrollment-missing")?;
        ensure!(id.role == "peer", "error enrollment-role");
        responding(self.attempts(db, &id).await?, mail)?.open_provisional(
            &mail.declaration,
            mail.admission
                .as_deref()
                .context("error enrollment-outcome-missing")?,
        )
    }
    /// Pins `mail` only after `evidence` proves its exact outcome, so an
    /// unverified response never blocks a correct one.
    pub(crate) async fn finish_peer(
        &self,
        db: &Database,
        mail: &membership::Mailbox,
        evidence: &Evidence,
    ) -> Result<()> {
        let guard = InstallationGuard::acquire(db.path())?;
        self.validate_database(db).await?;
        let _lock = self.lock()?;
        let id = self
            .identity(db, &guard)
            .await?
            .context("error enrollment-missing")?;
        ensure!(id.role == "peer", "error enrollment-role");
        let peer = responding(self.attempts(db, &id).await?, mail)?;
        let grant = peer.open_provisional(
            &mail.declaration,
            mail.admission
                .as_deref()
                .context("error enrollment-outcome-missing")?,
        )?;
        let verified = evidence.enrollment(&peer, grant.outcome)?;
        let current = evidence.verify()?;
        self.save_phase(
            db,
            &id,
            "peer-response",
            RESPONSE_LIMIT,
            &serde_json::to_vec(mail)?,
        )
        .await?;
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
        let mail = self
            .response(db)
            .await?
            .context("error enrollment-response-missing")?;
        let peer = responding(self.attempts(db, id).await?, &mail)?;
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
        self.validate_database(db).await?;
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
    /// Validated tail material for one drain. It stays current until the
    /// enrollment marker changes or the server reports a stale context.
    pub(crate) async fn tail_inputs(&self, db: &Database, locator: &str) -> Result<TailSnapshot> {
        let inputs = self.active_inputs(db, locator).await?;
        let withdrawal_deadline = self
            .unresolved_disclosures(db, &inputs)
            .await?
            .into_iter()
            .map(|(_, expiry)| expiry)
            .min();
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
            membership: inputs.membership.clone(),
            keys: inputs.coverage.clone(),
            prefix: i64::try_from(b.prefix_count)?,
            association,
            sync_generation: db
                .meta("sync_generation")
                .await?
                .context("error encrypted-tail-generation")?
                .parse()?,
        };
        let snapshot = TailSnapshot {
            authority,
            bearer: Secret::new(*inputs.bearer().expose()),
            withdrawal_deadline,
            enrollment_marker: db.enrollment_artifact_marker().await?,
            #[cfg(test)]
            enrollment_clock: self.enrollment_clock.clone(),
        };
        drop(inputs);
        Ok(snapshot)
    }
    /// Whether this installation joined as a peer, and the server locator its
    /// enrollment identity is bound to.
    pub(crate) async fn association(&self, db: &Database) -> Result<Option<(bool, String)>> {
        let guard = InstallationGuard::acquire(db.path())?;
        self.validate_database(db).await?;
        prepare_directory(&self.directory)?;
        let _lock = self.lock()?;
        Ok(self
            .identity(db, &guard)
            .await?
            .map(|id| (id.role == "peer", id.locator.clone())))
    }
    #[cfg(test)]
    pub(crate) async fn outbound_invitation(
        &self,
        db: &Database,
    ) -> Result<Option<OutboundInvitation>> {
        let _guard = InstallationGuard::acquire(db.path())?;
        self.validate_database(db).await?;
        prepare_directory(&self.directory)?;
        let _lock = self.lock()?;
        for journal in self.journals(db).await? {
            if self.phase(db, &journal.name("ready"), 128).await?.is_none()
                && !self.closed(db, &journal).await?
            {
                return Ok(Some(if self.may_have_sent(db, &journal).await? {
                    OutboundInvitation::Disclosed
                } else {
                    OutboundInvitation::Pending
                }));
            }
        }
        Ok(None)
    }
    pub async fn enrollment_readiness(&self, db: &Database) -> Result<EnrollmentReadiness> {
        let guard = InstallationGuard::acquire(db.path())?;
        self.validate_database(db).await?;
        prepare_directory(&self.directory)?;
        let _lock = self.lock()?;
        let Some(id) = self.identity(db, &guard).await? else {
            return Ok(EnrollmentReadiness::NotSelected);
        };
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
/// The attempt whose exact request a mailbox response answers. Its grant
/// binds that attempt's invitation, handle and request hash.
fn responding(attempts: Vec<Joiner>, mail: &membership::Mailbox) -> Result<Joiner> {
    attempts
        .into_iter()
        .find(|peer| mail.request.as_deref() == Some(peer.request()))
        .context("error enrollment-request-mismatch")
}
impl Outbound {
    fn name(&self, phase: &str) -> String {
        invitation_phase_name(&self.handle, phase)
    }
}
fn invitation_phase_name(handle: &Hash, phase: &str) -> String {
    format!("invite-{}-{phase}", hex::encode(handle))
}
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Disclosure {
    Admitted,
    Withdrawn,
    Unresolved,
}
pub(crate) struct TailSnapshot {
    pub authority: aven_core::sync::encrypted_tail::Authority,
    pub bearer: Secret,
    /// Earliest expiry of an invitation whose grant may have been sent. From
    /// then on, publishing waits for its withdrawal rotation.
    withdrawal_deadline: Option<u64>,
    enrollment_marker: i64,
    #[cfg(test)]
    enrollment_clock: Option<std::sync::Arc<std::sync::atomic::AtomicU64>>,
}
impl TailSnapshot {
    /// Checked before every step that publishes new encrypted content.
    pub(crate) fn require_publishing_ready(&self) -> Result<()> {
        if self.publishing_blocked()? {
            return Err(PublishingBlocked.into());
        }
        Ok(())
    }

    pub(crate) fn publishing_blocked(&self) -> Result<bool> {
        #[cfg(test)]
        let now = match &self.enrollment_clock {
            Some(clock) => clock.load(Ordering::SeqCst),
            None => unix_now()?,
        };
        #[cfg(not(test))]
        let now = unix_now()?;
        Ok(self
            .withdrawal_deadline
            .is_some_and(|deadline| now >= deadline))
    }

    pub(crate) async fn is_current(&self, db: &Database) -> Result<bool> {
        Ok(self.enrollment_marker == db.enrollment_artifact_marker().await?)
    }
}

impl ProtectedLocalKeyStore {
    fn enrollment_now(&self) -> Result<u64> {
        #[cfg(test)]
        if let Some(clock) = &self.enrollment_clock {
            return Ok(clock.load(Ordering::SeqCst));
        }
        unix_now()
    }
}

fn unix_now() -> Result<u64> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs())
}
