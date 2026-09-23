//! Installation-bound, append-only first-peer enrollment ownership.
use super::*;
use anyhow::{Context, Result, ensure};
use aven_core::db::installation::InstallationGuard;
use aven_core::sync::{
    SeedPublicationIntent,
    seed_claim::{
        Publication, SeedAuthority,
        peer::{self, Declaration, Evidence, Invitation, PeerAuthority},
    },
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy)]
enum Artifact {
    Identity,
    Registered,
    Bound,
    Candidate,
    Sent,
    Response,
    Verified,
    Ready,
    Installed,
}
impl Artifact {
    fn name(self) -> &'static str {
        match self {
            Self::Identity => "peer-identity",
            Self::Registered => "peer-registered",
            Self::Bound => "peer-bound",
            Self::Candidate => "peer-candidate",
            Self::Sent => "peer-sent",
            Self::Response => "peer-response",
            Self::Verified => "peer-verified",
            Self::Ready => "peer-ready",
            Self::Installed => "peer-installed",
        }
    }
    fn size(self) -> usize {
        match self {
            Self::Identity => 8192,
            Self::Bound => 2048,
            Self::Candidate => 8192,
            Self::Response | Self::Verified => 32768,
            _ => 128,
        }
    }
}
const PHASES: [Artifact; 8] = [
    Artifact::Registered,
    Artifact::Bound,
    Artifact::Candidate,
    Artifact::Sent,
    Artifact::Response,
    Artifact::Verified,
    Artifact::Ready,
    Artifact::Installed,
];

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Identity {
    role: String,
    client: String,
    incarnation: [u8; 32],
    locator: String,
    authority: Vec<u8>,
    declaration: Vec<u8>,
}
impl Drop for Identity {
    fn drop(&mut self) {
        self.authority.zeroize();
    }
}

/// Negative dispatch gate. Enrollment success is not encrypted-tail readiness.
#[derive(Debug, PartialEq, Eq)]
pub enum EnrollmentReadiness {
    NotSelected,
    Pending,
    UnresolvedDisclosure,
    Enrolled { head: [u8; 32] },
}
impl EnrollmentReadiness {
    /// Encrypted dispatch must consult this fence in addition to current
    /// membership, key coverage and its own installed/ordinary-sync readiness.
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

impl ProtectedLocalKeyStore {
    pub(super) fn peer_authority_exists(&self) -> StoreResult<bool> {
        Ok(
            read_restricted_file(&self.peer_marker(Artifact::Identity), 32)?.is_some()
                || self
                    .adoption_backend(Artifact::Identity.name())
                    .load_bounded(Artifact::Identity.size())?
                    .is_some(),
        )
    }
    fn peer_marker(&self, a: Artifact) -> PathBuf {
        self.directory
            .join(format!("{}.{}-authority", self.account, a.name()))
    }
    fn read_peer_record(&self, a: Artifact, required: bool) -> Result<Option<Zeroizing<Vec<u8>>>> {
        let marker = read_restricted_file(&self.peer_marker(a), 32)?;
        let stored = self.adoption_backend(a.name()).load_bounded(a.size())?;
        let Some(stored) = stored else {
            ensure!(
                !required && marker.is_none(),
                "error enrollment-protected-missing"
            );
            return Ok(None);
        };
        let digest = Sha256::digest(&stored);
        ensure!(
            marker
                .as_ref()
                .is_none_or(|m| m.as_slice() == digest.as_slice()),
            "error enrollment-protected-corrupt"
        );
        let account = hex::decode(&self.account)?;
        ensure!(
            &stored[..8] == b"AVENPER1" && stored[8..40] == account,
            "error enrollment-protected-context"
        );
        let len = u32::from_be_bytes(stored[40..44].try_into()?) as usize;
        ensure!(
            len <= a.size() - 44 && stored[44 + len..].iter().all(|b| *b == 0),
            "error enrollment-protected-framing"
        );
        if marker.is_none() {
            write_restricted_new(&self.peer_marker(a), &digest)?;
        }
        Ok(Some(Zeroizing::new(stored[44..44 + len].to_vec())))
    }
    fn write_peer_record(&self, a: Artifact, payload: &[u8]) -> Result<()> {
        ensure!(
            payload.len() <= a.size() - 44,
            "error enrollment-protected-limit"
        );
        if let Some(saved) = self.read_peer_record(a, false)? {
            ensure!(
                saved.as_slice() == payload,
                "error enrollment-protected-conflict"
            );
            return Ok(());
        }
        let mut frame = Zeroizing::new(vec![0; a.size()]);
        frame[..8].copy_from_slice(b"AVENPER1");
        frame[8..40].copy_from_slice(&hex::decode(&self.account)?);
        frame[40..44].copy_from_slice(&(payload.len() as u32).to_be_bytes());
        frame[44..44 + payload.len()].copy_from_slice(payload);
        self.adoption_backend(a.name()).create(&frame)?;
        let saved = self
            .read_peer_record(a, true)?
            .context("error enrollment-protected-missing")?;
        ensure!(
            saved.as_slice() == payload,
            "error enrollment-protected-write"
        );
        #[cfg(test)]
        if std::env::var("AVEN_PEER_CRASH_KIND").ok().as_deref() == Some(a.name()) {
            std::process::exit(79);
        }
        Ok(())
    }
    async fn phase(&self, db: &Database, a: Artifact) -> Result<Option<Zeroizing<Vec<u8>>>> {
        let pin = db.enrollment_artifact(a.name()).await?;
        let bytes = self.read_peer_record(a, pin.is_some())?;
        if let (Some(pin), Some(bytes)) = (pin, bytes.as_ref()) {
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
        a: Artifact,
        bytes: &[u8],
    ) -> Result<()> {
        // A surviving mirror is a loss detector, never permission to reconstruct.
        self.phase(db, a).await?;
        self.write_peer_record(a, bytes)?;
        db.pin_enrollment_artifact(id.incarnation, a.name(), Sha256::digest(bytes).into())
            .await
    }
    async fn identity(&self, db: &Database, guard: &InstallationGuard) -> Result<Option<Identity>> {
        let pin = db.enrollment_pin().await?;
        let Some(bytes) = self.read_peer_record(Artifact::Identity, pin.is_some())? else {
            return Ok(None);
        };
        let id: Identity = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("error enrollment-protected-framing"))?;
        ensure!(
            id.locator.len() <= 2048 && matches!(id.role.as_str(), "inviter" | "peer"),
            "error enrollment-protected-framing"
        );
        if pin.is_none() {
            for a in PHASES {
                ensure!(
                    self.phase(db, a).await?.is_none(),
                    "error enrollment-database-lost"
                );
            }
        }
        db.pin_enrollment(id.incarnation, &id.client, &id.role, guard)
            .await?;
        // Validate every surviving mirror/record before permitting any operation.
        for a in PHASES {
            self.phase(db, a).await?;
        }
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
    pub(crate) async fn prepare_invitation(
        &self,
        db: &Database,
        locator: &str,
        expires: Option<u64>,
    ) -> Result<(SeedAuthority, Publication, Declaration)> {
        let guard = InstallationGuard::acquire(db.path())?;
        self.validate_database(db)?;
        ensure!(locator.len() <= 2048, "error enrollment-locator-limit");
        let client = db.adopted_enrollment_client().await?;
        let package = self.load_required()?;
        prepare_directory(&self.directory)?;
        let _lock = self.lock()?;
        let seed = self.required_seed(&package)?;
        let intent = self.adopted_inputs(&seed)?;
        let publication = intent.publication(seed.genesis())?;
        let id = match self.identity(db, &guard).await? {
            Some(id) => id,
            None => {
                let (inv, d) = seed.prepare_peer_invitation(
                    &publication,
                    expires.context("error enrollment-invitation-required")?,
                )?;
                let id = Identity {
                    role: "inviter".into(),
                    client,
                    incarnation: *aven_core::sync::seed_claim::Secret::generate()?.expose(),
                    locator: locator.into(),
                    authority: inv.protected_storage_bytes().to_vec(),
                    declaration: d.record().to_vec(),
                };
                self.write_peer_record(
                    Artifact::Identity,
                    &Zeroizing::new(serde_json::to_vec(&id)?),
                )?;
                db.pin_enrollment(id.incarnation, &id.client, &id.role, &guard)
                    .await?;
                id
            }
        };
        ensure!(
            id.role == "inviter" && id.locator == locator,
            "error enrollment-context"
        );
        let declaration = Declaration::from_record(seed.genesis(), &publication, &id.declaration)?;
        Ok((seed, publication, declaration))
    }
    pub(crate) async fn registered_invitation(
        &self,
        db: &Database,
        declaration: &Declaration,
    ) -> Result<Invitation> {
        let guard = InstallationGuard::acquire(db.path())?;
        self.validate_database(db)?;
        let _lock = self.lock()?;
        let id = self
            .identity(db, &guard)
            .await?
            .context("error enrollment-missing")?;
        ensure!(
            id.role == "inviter" && id.declaration == declaration.record(),
            "error enrollment-context"
        );
        self.save_phase(db, &id, Artifact::Registered, &declaration.commitment())
            .await?;
        Invitation::from_protected_storage(&id.authority)
    }
    pub(crate) async fn prepare_peer(
        &self,
        db: &Database,
        locator: &str,
        invitation: Option<Invitation>,
    ) -> Result<PeerAuthority> {
        let guard = InstallationGuard::acquire(db.path())?;
        self.validate_database(db)?;
        ensure!(locator.len() <= 2048, "error enrollment-locator-limit");
        prepare_directory(&self.directory)?;
        let _lock = self.lock()?;
        let id = match self.identity(db, &guard).await? {
            Some(id) => {
                if let Some(invitation) = invitation.as_ref() {
                    ensure!(
                        id.role == "peer"
                            && PeerAuthority::from_protected_storage(&id.authority)?
                                .matches_invitation(invitation),
                        "error enrollment-invitation-conflict"
                    );
                }
                id
            }
            None => {
                // Reject the wrong target before irreversible opt-in, under the
                // same installation interlock used by supported replacement.
                let client = db.peer_target_preflight().await?;
                guard.ensure_unbound()?;
                ensure!(
                    self.backend.load()?.is_none()
                        && read_marker(&self.marker_path())?.is_none()
                        && !self.seed_authority_exists()?,
                    "error enrollment-existing-authority"
                );
                let invitation = invitation.context("error enrollment-invitation-required")?;
                guard.fence()?;
                let peer = PeerAuthority::generate(invitation)?;
                let id = Identity {
                    role: "peer".into(),
                    client,
                    incarnation: *aven_core::sync::seed_claim::Secret::generate()?.expose(),
                    locator: locator.into(),
                    authority: peer.protected_storage_bytes().to_vec(),
                    declaration: vec![],
                };
                self.write_peer_record(
                    Artifact::Identity,
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
        let peer = PeerAuthority::from_protected_storage(&id.authority)?;
        self.save_phase(db, &id, Artifact::Sent, &Sha256::digest(peer.request()))
            .await?;
        Ok(peer)
    }
    pub(crate) async fn prepare_admission(
        &self,
        db: &Database,
        locator: &str,
        request: &[u8],
    ) -> Result<(SeedAuthority, Publication, peer::Admission)> {
        let guard = InstallationGuard::acquire(db.path())?;
        self.validate_database(db)?;
        db.adopted_enrollment_client().await?;
        let package = self.load_required()?;
        let _lock = self.lock()?;
        let seed = self.required_seed(&package)?;
        let intent = self.adopted_inputs(&seed)?;
        let p = intent.publication(seed.genesis())?;
        let id = self
            .identity(db, &guard)
            .await?
            .context("error enrollment-missing")?;
        ensure!(
            id.role == "inviter" && id.locator == locator,
            "error enrollment-context"
        );
        ensure!(
            self.phase(db, Artifact::Registered).await?.is_some(),
            "error enrollment-not-registered"
        );
        let inv = Invitation::from_protected_storage(&id.authority)?;
        seed.validate_peer_request(&inv, request)?;
        self.save_phase(db, &id, Artifact::Bound, request).await?;
        let d = Declaration::from_record(seed.genesis(), &p, &id.declaration)?;
        let a = if let Some(bytes) = self.phase(db, Artifact::Candidate).await? {
            peer::Admission::from_record(seed.genesis(), &p, &d, request, &bytes)?
        } else {
            ensure!(
                self.phase(db, Artifact::Sent).await?.is_none(),
                "error enrollment-candidate-lost"
            );
            let inv = Invitation::from_protected_storage(&id.authority)?;
            let a = seed.prepare_peer_admission(&p, &d, &inv, request, package.package_key())?;
            self.save_phase(db, &id, Artifact::Candidate, a.record())
                .await?;
            a
        };
        self.save_phase(db, &id, Artifact::Sent, &a.commitment())
            .await?;
        Ok((seed, p, a))
    }
    pub(crate) async fn pin_peer_response(
        &self,
        db: &Database,
        evidence: &Evidence,
    ) -> Result<peer::ProvisionalGrant> {
        let guard = InstallationGuard::acquire(db.path())?;
        self.validate_database(db)?;
        let _lock = self.lock()?;
        let id = self
            .identity(db, &guard)
            .await?
            .context("error enrollment-missing")?;
        ensure!(id.role == "peer", "error enrollment-role");
        let peer = PeerAuthority::from_protected_storage(&id.authority)?;
        let grant = peer.open_provisional(evidence)?;
        self.save_phase(db, &id, Artifact::Response, &serde_json::to_vec(evidence)?)
            .await?;
        Ok(grant)
    }
    pub(crate) async fn finish_peer(
        &self,
        db: &Database,
        evidence: &Evidence,
        descriptor: &[u8],
    ) -> Result<()> {
        let guard = InstallationGuard::acquire(db.path())?;
        self.validate_database(db)?;
        let _lock = self.lock()?;
        let id = self
            .identity(db, &guard)
            .await?
            .context("error enrollment-missing")?;
        ensure!(id.role == "peer", "error enrollment-role");
        let response = self
            .phase(db, Artifact::Response)
            .await?
            .context("error enrollment-response-missing")?;
        ensure!(
            response.as_slice() == serde_json::to_vec(evidence)?,
            "error enrollment-response-changed"
        );
        let peer = PeerAuthority::from_protected_storage(&id.authority)?;
        let verified = peer.verify_enrollment(evidence, descriptor)?;
        let record = Verified {
            evidence: evidence.clone(),
            descriptor: descriptor.to_vec(),
            key: verified.key().protected_storage_bytes().to_vec(),
        };
        self.save_phase(
            db,
            &id,
            Artifact::Verified,
            &Zeroizing::new(serde_json::to_vec(&record)?),
        )
        .await?;
        self.save_phase(db, &id, Artifact::Ready, &verified.admission().commitment())
            .await
    }
    pub(crate) async fn finish_inviter(
        &self,
        db: &Database,
        evidence: &Evidence,
        descriptor: &[u8],
    ) -> Result<()> {
        let guard = InstallationGuard::acquire(db.path())?;
        self.validate_database(db)?;
        let package = self.load_required()?;
        let _lock = self.lock()?;
        let id = self
            .identity(db, &guard)
            .await?
            .context("error enrollment-missing")?;
        ensure!(id.role == "inviter", "error enrollment-role");
        let seed = self.required_seed(&package)?;
        let intent = self.adopted_inputs(&seed)?;
        let p = intent.publication(seed.genesis())?;
        let d = Declaration::from_record(seed.genesis(), &p, &id.declaration)?;
        let request = self
            .phase(db, Artifact::Bound)
            .await?
            .context("error enrollment-binding-missing")?;
        let candidate = self
            .phase(db, Artifact::Candidate)
            .await?
            .context("error enrollment-candidate-missing")?;
        let a = peer::Admission::from_record(seed.genesis(), &p, &d, &request, &candidate)?;
        evidence.validate_lengths()?;
        ensure!(
            evidence.genesis == seed.genesis().record()
                && evidence.publication == p.record()
                && evidence.declaration == d.record()
                && evidence.request == request.as_slice()
                && evidence.admission == a.record()
                && descriptor == intent.descriptor(),
            "error enrollment-outcome-mismatch"
        );
        self.save_phase(db, &id, Artifact::Ready, &a.commitment())
            .await
    }
    pub(crate) async fn tail_inputs(&self, db: &Database, locator: &str) -> Result<TailInputs> {
        self.enrollment_readiness(db)
            .await?
            .require_resolved_disclosure()?;
        let installation = InstallationGuard::acquire(db.path())?;
        self.validate_database(db)?;
        let lock = self.lock()?;
        let id = self
            .identity(db, &installation)
            .await?
            .context("error enrollment-missing")?;
        ensure!(id.locator == locator, "error enrollment-context");
        let ready = self
            .phase(db, Artifact::Ready)
            .await?
            .context("error enrollment-not-ready")?;
        let (genesis, publication, device, bearer, key) = if id.role == "peer" {
            let bytes = self
                .phase(db, Artifact::Verified)
                .await?
                .context("error enrollment-key-coverage-missing")?;
            let record: Verified = serde_json::from_slice(&bytes)
                .map_err(|_| anyhow::anyhow!("error enrollment-verified-corrupt"))?;
            let peer = PeerAuthority::from_protected_storage(&id.authority)?;
            let verified = peer.verify_enrollment(&record.evidence, &record.descriptor)?;
            ensure!(
                ready.as_slice() == verified.admission().commitment()
                    && verified.key().protected_storage_bytes().as_slice() == record.key,
                "error enrollment-verified-corrupt"
            );
            let installed = self
                .phase(db, Artifact::Installed)
                .await?
                .context("error snapshot-not-installed")?;
            ensure!(
                installed.as_slice() == ready.as_slice()
                    && db
                        .peer_snapshot_receipt(&verified, id.incarnation, &id.client, &installation)
                        .await?
                        .is_some(),
                "error snapshot-receipt-mismatch"
            );
            (
                verified.genesis().clone(),
                verified.publication().clone(),
                peer.device(),
                aven_core::sync::seed_claim::Secret::new(*peer.bearer().expose()),
                LocalSharedStatePackageKey::new(*verified.key().protected_storage_bytes()),
            )
        } else {
            ensure!(
                id.role == "inviter" && db.adopted_enrollment_client().await? == id.client,
                "error enrollment-role"
            );
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
            let source = self
                .load_adoption_record("source", 104, true)?
                .context("error seed-source-missing")?;
            let source = self.decode_source(&source, &seed)?;
            ensure!(
                db.seed_source_pin().await?.as_deref() == Some(source.protected_storage_bytes()),
                "error seed-source-mismatch"
            );
            let publication = intent.publication(seed.genesis())?;
            let declaration =
                Declaration::from_record(seed.genesis(), &publication, &id.declaration)?;
            let request = self
                .phase(db, Artifact::Bound)
                .await?
                .context("error enrollment-binding-missing")?;
            let candidate = self
                .phase(db, Artifact::Candidate)
                .await?
                .context("error enrollment-candidate-missing")?;
            let admission = peer::Admission::from_record(
                seed.genesis(),
                &publication,
                &declaration,
                &request,
                &candidate,
            )?;
            ensure!(
                ready.as_slice() == admission.commitment(),
                "error enrollment-checkpoint-mismatch"
            );
            (
                seed.genesis().clone(),
                publication,
                seed.genesis().device_id(),
                aven_core::sync::seed_claim::Secret::new(*seed.bearer().expose()),
                LocalSharedStatePackageKey::new(*package.package_key().protected_storage_bytes()),
            )
        };
        let b = publication.binding();
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
                genesis: genesis.commitment(),
                device,
                credential_version: 1,
                head: ready.as_slice().try_into()?,
                stream: b.stream_id,
                descriptor: b.descriptor_commitment,
            },
            generation: genesis.context().generation_id,
            key,
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
            bearer,
            _installation: installation,
            _lock: lock,
        })
    }

    pub(crate) async fn install_peer_snapshot(
        &self,
        db: &Database,
        transport: &crate::peer_enrollment_http::Client,
        locator: &str,
        blob_dir: &Path,
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
        let ready = self
            .phase(db, Artifact::Ready)
            .await?
            .context("error enrollment-not-ready")?;
        let bytes = self
            .phase(db, Artifact::Verified)
            .await?
            .context("error enrollment-key-coverage-missing")?;
        let record: Verified = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("error enrollment-verified-corrupt"))?;
        let peer = PeerAuthority::from_protected_storage(&id.authority)?;
        let verified = peer.verify_enrollment(&record.evidence, &record.descriptor)?;
        let head = verified.admission().commitment();
        ensure!(
            ready.as_slice() == head
                && verified.key().protected_storage_bytes().as_slice() == record.key,
            "error enrollment-verified-corrupt"
        );
        let completed = self.phase(db, Artifact::Installed).await?;
        if let Some(report) = db
            .peer_snapshot_receipt(&verified, id.incarnation, &id.client, &guard)
            .await?
        {
            self.save_phase(db, &id, Artifact::Installed, &head).await?;
            return Ok(report);
        }
        ensure!(
            completed.is_none(),
            "error snapshot-installed-database-lost"
        );
        let package = transport
            .download(&peer, &verified, &record.descriptor)
            .await?;
        let report = db
            .install_peer_snapshot(
                &verified,
                id.incarnation,
                &id.client,
                &guard,
                &package,
                blob_dir,
            )
            .await?;
        self.save_phase(db, &id, Artifact::Installed, &head).await?;
        Ok(report)
    }
    /// Tail dispatch must check this fence. Enrolled is not installed or synced.
    pub async fn enrollment_readiness(&self, db: &Database) -> Result<EnrollmentReadiness> {
        let guard = InstallationGuard::acquire(db.path())?;
        self.validate_database(db)?;
        prepare_directory(&self.directory)?;
        let _lock = self.lock()?;
        let Some(id) = self.identity(db, &guard).await? else {
            return Ok(EnrollmentReadiness::NotSelected);
        };
        if let Some(ready) = self.phase(db, Artifact::Ready).await? {
            let head: [u8; 32] = ready.as_slice().try_into()?;
            if id.role == "peer" {
                let bytes = self
                    .phase(db, Artifact::Verified)
                    .await?
                    .context("error enrollment-key-coverage-missing")?;
                let record: Verified = serde_json::from_slice(&bytes)
                    .map_err(|_| anyhow::anyhow!("error enrollment-verified-corrupt"))?;
                let peer = PeerAuthority::from_protected_storage(&id.authority)?;
                let verified = peer.verify_enrollment(&record.evidence, &record.descriptor)?;
                ensure!(
                    verified.admission().commitment() == head
                        && verified.key().protected_storage_bytes().as_slice() == record.key,
                    "error enrollment-verified-corrupt"
                );
            } else {
                let candidate = self
                    .phase(db, Artifact::Candidate)
                    .await?
                    .context("error enrollment-candidate-missing")?;
                ensure!(
                    Sha256::digest(candidate.as_slice()).as_slice() == head,
                    "error enrollment-checkpoint-mismatch"
                );
            }
            return Ok(EnrollmentReadiness::Enrolled { head });
        }
        Ok(
            if id.role == "inviter" && self.phase(db, Artifact::Sent).await?.is_some() {
                EnrollmentReadiness::UnresolvedDisclosure
            } else {
                EnrollmentReadiness::Pending
            },
        )
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Verified {
    evidence: Evidence,
    descriptor: Vec<u8>,
    key: Vec<u8>,
}
impl Drop for Verified {
    fn drop(&mut self) {
        self.key.zeroize();
    }
}

pub(crate) struct TailInputs {
    pub authority: aven_core::sync::encrypted_tail::Authority,
    pub bearer: aven_core::sync::seed_claim::Secret,
    _installation: InstallationGuard,
    _lock: File,
}
