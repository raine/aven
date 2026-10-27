//! Protected host ownership for original-seed publication and adoption.
use super::*;
use crate::db::installation::InstallationGuard;
use crate::sync::seed_claim::{PublicationOutcome, SeedAuthority};
use crate::sync::shared_state::validated::ProofCache;
use crate::sync::{SeedPublicationIntent, SeedSourceAuthority};
use anyhow::Context;

impl ProtectedLocalKeyStore {
    /// Resumes sealed ownership before exposing any publication transport input.
    pub async fn seed_http_inputs(
        &self,
        database: &Database,
    ) -> anyhow::Result<(
        SeedAuthority,
        SeedPublicationIntent,
        Option<crate::sync::bootstrap_format::Package>,
    )> {
        let mut proofs = ProofCache::default();
        let (seed, intent) = self.seed_resume_intent(database, &mut proofs).await?;
        let upload = match self.seed_upload(database, &intent, &mut proofs).await? {
            Some(upload) => Some(upload.package(database).await?),
            None => None,
        };
        Ok((seed, intent, upload))
    }

    /// Recovers protected seed authority and the sealed publication intent.
    pub(crate) async fn seed_resume_intent(
        &self,
        database: &Database,
        proofs: &mut ProofCache,
    ) -> anyhow::Result<(SeedAuthority, SeedPublicationIntent)> {
        let intent = self
            .prepare_seed_adoption_intent_with(database, proofs)
            .await?;
        let package = self.load_required()?;
        let seed = {
            let _guard = self.lock()?;
            self.required_seed(&package)?
        };
        Ok((seed, intent))
    }

    /// Loads exact upload bytes for a sealed intent, or `None` once adopted.
    pub(crate) async fn seed_upload(
        &self,
        database: &Database,
        intent: &SeedPublicationIntent,
        proofs: &mut ProofCache,
    ) -> anyhow::Result<Option<crate::sync::shared_state::package::upload::FrozenUpload>> {
        let _installation = InstallationGuard::acquire_waiting(database.path()).await?;
        self.validate_database(database).await?;
        let package = self.load_required()?;
        let _guard = self.lock()?;
        let seed = self.required_seed(&package)?;
        let source = self.decode_source(
            &self
                .load_adoption_record("source", 104, true)?
                .context("missing source")?,
            &seed,
        )?;
        let protected = self
            .load_adoption_record("intent", 65536, true)?
            .context("missing intent")?;
        anyhow::ensure!(
            protected == intent.protected_storage_bytes(),
            "error seed-protected-intent-mismatch"
        );
        proofs.trust_protected(intent);
        let _timer = super::super::bootstrap::StageTimer::start("load_upload");
        database
            .seed_publication_upload(&source, intent, &seed, package.package_key(), proofs)
            .await
    }

    /// Nonsecret digest record that detects loss of secret item `kind`.
    fn adoption_marker(kind: &str) -> String {
        format!("{kind}-authority")
    }

    pub(super) fn load_adoption_record(
        &self,
        kind: &str,
        limit: usize,
        required: bool,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        let marker = self.read_record(&Self::adoption_marker(kind), 32)?;
        let bytes = self.load_secret(kind, limit)?;
        match bytes {
            Some(bytes) => {
                let digest = Sha256::digest(&bytes);
                anyhow::ensure!(
                    marker.as_deref().is_none_or(|m| m == digest.as_slice()),
                    "error seed-protected-record-corrupt"
                );
                if marker.is_none() {
                    self.write_record(&Self::adoption_marker(kind), &digest)?;
                }
                if kind == "intent" {
                    let length = u32::from_be_bytes(bytes[..4].try_into()?) as usize;
                    anyhow::ensure!(
                        length <= limit - 4 && bytes[4 + length..].iter().all(|b| *b == 0),
                        "error seed-protected-intent-framing"
                    );
                    return Ok(Some(bytes[4..4 + length].to_vec()));
                }
                Ok(Some(bytes.to_vec()))
            }
            None => {
                anyhow::ensure!(
                    !required && marker.is_none(),
                    "error seed-protected-authority-missing"
                );
                Ok(None)
            }
        }
    }

    fn create_adoption_record(&self, kind: &str, bytes: &[u8], limit: usize) -> anyhow::Result<()> {
        let encoded;
        let storage = if kind == "intent" {
            anyhow::ensure!(bytes.len() <= limit - 4, "error seed-intent-too-large");
            let mut frame = vec![0; limit];
            frame[..4].copy_from_slice(&(bytes.len() as u32).to_be_bytes());
            frame[4..4 + bytes.len()].copy_from_slice(bytes);
            encoded = frame;
            encoded.as_slice()
        } else {
            bytes
        };
        self.create_secret(kind, storage)?;
        let stored = self
            .load_adoption_record(kind, limit, true)?
            .context("error seed-protected-write-missing")?;
        anyhow::ensure!(stored == bytes, "error seed-protected-write-mismatch");
        Ok(())
    }

    pub fn required_seed(
        &self,
        package: &ProtectedLocalPackageKey,
    ) -> anyhow::Result<SeedAuthority> {
        let bytes = self
            .load_secret(seed::SEED_ITEM, seed::SEED_BYTES)?
            .context("error seed-protected-authority-missing")?;
        Ok(self.decode_seed(&bytes, package)?)
    }

    pub(super) fn decode_source(
        &self,
        bytes: &[u8],
        seed: &SeedAuthority,
    ) -> anyhow::Result<SeedSourceAuthority> {
        SeedSourceAuthority::from_protected_storage(
            bytes,
            hex::decode(&self.account)?
                .try_into()
                .map_err(|_| anyhow::anyhow!("invalid source account"))?,
            seed.genesis(),
        )
    }

    /// Explicit irreversible opt-in before capture. Early failure can leave the
    /// installation fenced; the denial marker never authorizes key regeneration.
    pub async fn prepare_seed_source(
        &self,
        database: &Database,
    ) -> anyhow::Result<SeedSourceAuthority> {
        let installation = InstallationGuard::acquire_waiting(database.path()).await?;
        self.validate_database(database).await?;
        #[cfg(test)]
        wait_source_boundary(database.path()).await;
        let was_unbound = installation.ensure_unbound().is_ok();
        let package = self.load_required()?;
        self.prepare()?;
        let _guard = self.lock()?;
        let seed = self.required_seed(&package)?;
        let pin = database.seed_source_pin().await?;
        let existing = self.load_adoption_record("source", 104, pin.is_some())?;
        if existing.is_none() {
            anyhow::ensure!(
                was_unbound,
                "error seed-source-incomplete installation-remains-fenced"
            );
            anyhow::ensure!(
                database
                    .resume_local_shared_state_never_dispatched()
                    .await?
                    .is_none(),
                "error seed-source-requires-recapture-never-dispatched"
            );
        }
        installation.fence()?;
        let source = match existing {
            Some(bytes) => self.decode_source(&bytes, &seed)?,
            None => {
                let source = SeedSourceAuthority::generate(
                    hex::decode(&self.account)?
                        .try_into()
                        .map_err(|_| anyhow::anyhow!("invalid source account"))?,
                    seed.genesis(),
                )?;
                self.create_adoption_record("source", source.protected_storage_bytes(), 104)?;
                source
            }
        };
        if pin.is_none() {
            anyhow::ensure!(
                self.load_adoption_record("intent", 65536, false)?.is_none(),
                "error seed-source-database-lost"
            );
        }
        database.bind_seed_source(&source, &installation).await?;
        Ok(source)
    }

    /// Fences SQLite cancellation before persisting exact protected intent.
    /// An intent cannot be locally abandoned.
    pub async fn prepare_seed_adoption_intent(
        &self,
        database: &Database,
    ) -> anyhow::Result<SeedPublicationIntent> {
        self.prepare_seed_adoption_intent_with(database, &mut ProofCache::default())
            .await
    }

    pub(crate) async fn prepare_seed_adoption_intent_with(
        &self,
        database: &Database,
        proofs: &mut ProofCache,
    ) -> anyhow::Result<SeedPublicationIntent> {
        let _installation = InstallationGuard::acquire_waiting(database.path()).await?;
        self.validate_database(database).await?;
        let package = self.load_required()?;
        let _guard = self.lock()?;
        let seed = self.required_seed(&package)?;
        let source = self.decode_source(
            &self
                .load_adoption_record("source", 104, true)?
                .context("missing source")?,
            &seed,
        )?;
        let existing = database.seed_publication_intent_bytes().await?;
        let protected = self.load_adoption_record(
            "intent",
            65536,
            existing
                .as_ref()
                .is_some_and(|(_, state)| state != "preparing"),
        )?;
        anyhow::ensure!(
            protected.is_none() || existing.is_some(),
            "error seed-intent-database-lost"
        );
        // Only the protected copy may vouch for a freeze; a `preparing`
        // intent in SQLite alone is authenticated again before promotion.
        if let Some(bytes) = &protected {
            proofs.trust_protected(&SeedPublicationIntent::from_protected_storage(
                bytes,
                &source,
                seed.genesis(),
            )?);
        }
        let timer = super::super::bootstrap::StageTimer::start("prepare_intent");
        let intent = database
            .prepare_seed_publication_intent_with(&source, &seed, package.package_key(), proofs)
            .await?;
        match protected {
            Some(bytes) => anyhow::ensure!(
                bytes == intent.protected_storage_bytes(),
                "error seed-protected-intent-mismatch"
            ),
            None => {
                self.create_adoption_record("intent", intent.protected_storage_bytes(), 65536)?
            }
        }
        let reloaded = SeedPublicationIntent::from_protected_storage(
            &self
                .load_adoption_record("intent", 65536, true)?
                .context("missing intent")?,
            &source,
            seed.genesis(),
        )?;
        drop(timer);
        let _timer = super::super::bootstrap::StageTimer::start("seal");
        if existing.is_none_or(|(_, state)| state != "adopted") {
            database
                .seal_seed_publication_intent(&source, &reloaded)
                .await?;
        }
        Ok(reloaded)
    }

    /// Authenticates host authority and commits adoption, then retries pin cleanup.
    /// Cleanup failure is reported without rolling back the committed adoption.
    pub async fn adopt_seed_publication(
        &self,
        database: &Database,
        outcome: &PublicationOutcome,
    ) -> anyhow::Result<bool> {
        self.adopt_seed_publication_with(database, outcome, &mut ProofCache::default())
            .await
    }

    pub(crate) async fn adopt_seed_publication_with(
        &self,
        database: &Database,
        outcome: &PublicationOutcome,
        proofs: &mut ProofCache,
    ) -> anyhow::Result<bool> {
        let _installation = InstallationGuard::acquire_waiting(database.path()).await?;
        self.validate_database(database).await?;
        let package = self.load_required()?;
        let _guard = self.lock()?;
        let seed = self.required_seed(&package)?;
        let source = self.decode_source(
            &self
                .load_adoption_record("source", 104, true)?
                .context("missing source")?,
            &seed,
        )?;
        let intent = SeedPublicationIntent::from_protected_storage(
            &self
                .load_adoption_record("intent", 65536, true)?
                .context("missing intent")?,
            &source,
            seed.genesis(),
        )?;
        proofs.trust_protected(&intent);
        let adopted = database
            .adopt_seed_publication(
                &source,
                &intent,
                &seed,
                package.package_key(),
                outcome,
                proofs,
            )
            .await?;
        database
            .cleanup_adopted_seed_capture(&source, &intent)
            .await?;
        Ok(adopted)
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
type SourceBarrier = (
    std::path::PathBuf,
    tokio::sync::oneshot::Sender<()>,
    tokio::sync::oneshot::Receiver<()>,
);
#[cfg(test)]
static SOURCE_BARRIER: std::sync::Mutex<Option<SourceBarrier>> = std::sync::Mutex::new(None);
#[cfg(test)]
async fn wait_source_boundary(path: &Path) {
    let barrier = {
        let mut slot = SOURCE_BARRIER.lock().unwrap();
        if slot.as_ref().is_some_and(|(p, _, _)| p == path) {
            slot.take()
        } else {
            None
        }
    };
    if let Some((_, entered, resume)) = barrier {
        let _ = entered.send(());
        let _ = resume.await;
    }
}
