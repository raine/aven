//! Host ownership for original-seed adoption. No network dispatch capability.
use super::*;
use anyhow::Context;
use aven_core::db::installation::InstallationGuard;
use aven_core::sync::seed_claim::{PublicationOutcome, SeedAuthority};
use aven_core::sync::{SeedPublicationIntent, SeedSourceAuthority};

impl ProtectedLocalKeyStore {
    fn adoption_backend(&self, kind: &str) -> Backend {
        match &self.backend {
            #[cfg(target_os = "macos")]
            Backend::Keychain(backend) => Backend::Keychain(KeychainBackend {
                service: format!("{}.{}", backend.service, kind),
                account: backend.account.clone(),
            }),
            #[cfg(any(target_os = "linux", test))]
            Backend::File(_) => Backend::File(FileBackend {
                path: self.directory.join(format!("{}.{}", self.account, kind)),
            }),
            #[cfg(test)]
            Backend::FailWrite => Backend::FailWrite,
            #[cfg(test)]
            Backend::Unavailable => Backend::Unavailable,
        }
    }

    fn adoption_marker(&self, kind: &str) -> PathBuf {
        self.directory
            .join(format!("{}.{}-authority", self.account, kind))
    }

    fn load_adoption_record(
        &self,
        kind: &str,
        limit: usize,
        required: bool,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        let marker = read_restricted_file(&self.adoption_marker(kind), 32)?;
        let bytes = self.adoption_backend(kind).load_bounded(limit)?;
        match bytes {
            Some(bytes) => {
                let digest = Sha256::digest(&bytes);
                anyhow::ensure!(
                    marker.as_deref().is_none_or(|m| m == digest.as_slice()),
                    "error seed-protected-record-corrupt"
                );
                if marker.is_none() {
                    write_restricted_new(&self.adoption_marker(kind), &digest)?;
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
        self.adoption_backend(kind).create(storage)?;
        let stored = self
            .load_adoption_record(kind, limit, true)?
            .context("error seed-protected-write-missing")?;
        anyhow::ensure!(stored == bytes, "error seed-protected-write-mismatch");
        Ok(())
    }

    fn required_seed(&self, package: &ProtectedLocalPackageKey) -> anyhow::Result<SeedAuthority> {
        let bytes = self
            .seed_backend()
            .load_bounded(seed::SEED_BYTES)?
            .context("error seed-protected-authority-missing")?;
        Ok(self.decode_seed(&bytes, package)?)
    }

    fn decode_source(
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
        let installation = InstallationGuard::acquire(database.path())?;
        self.validate_database(database)?;
        #[cfg(test)]
        wait_source_boundary(database.path()).await;
        let was_unbound = installation.ensure_unbound().is_ok();
        let package = self.load_required()?;
        prepare_directory(&self.directory)?;
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
    /// The result has no dispatch API. An intent cannot be locally abandoned.
    pub async fn prepare_seed_adoption_intent(
        &self,
        database: &Database,
    ) -> anyhow::Result<SeedPublicationIntent> {
        let _installation = InstallationGuard::acquire(database.path())?;
        self.validate_database(database)?;
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
        let intent = database
            .prepare_seed_publication_intent(&source, &seed, package.package_key())
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
        let _installation = InstallationGuard::acquire(database.path())?;
        self.validate_database(database)?;
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
        let adopted = database
            .adopt_seed_publication(&source, &intent, &seed, package.package_key(), outcome)
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
    PathBuf,
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
