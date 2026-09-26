use super::*;
use crate::sync::seed_claim::{SEED_STORAGE_BYTES, SeedAuthority};
use std::path::Path;

const SEED_MAGIC: &[u8; 8] = b"AVENSED1";
const SEED_MARKER: &[u8; 8] = b"AVENSPN1";
pub(super) const SEED_BYTES: usize = 8 + 32 + SEED_STORAGE_BYTES + 32;
const SEED_MARKER_BYTES: usize = 8 + 32;
pub(super) const SEED_ITEM: &str = "seed";
/// Nonsecret record that detects loss of the seed item.
const SEED_MARKER_RECORD: &str = "seed-authority";

impl ProtectedLocalKeyStore {
    /// Persists private seed keys, bearer and exact validated genesis before use.
    /// Reuses package vault/generation/secret and refuses a changed setup binding.
    /// The public DB pin detects lost protected authority, never authorizes replacement.
    pub async fn prepare_seed_claim(
        &self,
        database: &Database,
        setup_id: [u8; 32],
    ) -> anyhow::Result<SeedAuthority> {
        self.validate_database(database).await?;
        anyhow::ensure!(
            database
                .enrollment_pin()
                .await?
                .is_none_or(|(_, _, role)| role == "inviter"),
            "error enrollment-peer-cannot-become-seed"
        );
        let mut pin = database.local_seed_genesis_commitment().await?;
        if let Some(commitment) = pin
            && self.clear_unfenced_orphan_pin(database, commitment).await?
        {
            pin = None;
        }
        let package = if pin.is_some()
            || database
                .has_local_shared_state_package_never_dispatched()
                .await?
        {
            self.load_required()?
        } else {
            self.load_or_create()?
        };
        let seed = match (self.load_or_create_seed(&package, setup_id, pin), pin) {
            // A refused claim stays unconfirmed, so its authority is kept for
            // an exact retry. Choosing another server's invitation abandons it.
            (Err(error), Some(commitment))
                if error.kind() == ProtectedLocalKeyStoreErrorKind::SetupMismatch
                    && database.local_seed_claim_refused().await? =>
            {
                self.rollback_seed_commitment(database, commitment).await?;
                self.load_or_create_seed(&package, setup_id, None)?
            }
            (result, _) => result?,
        };
        database.pin_local_seed_genesis(seed.genesis()).await?;
        Ok(seed)
    }

    /// Freezes a local package only under already persisted seed genesis context.
    /// Incompatible frozen bytes fail unchanged; cancel/recapture explicitly.
    /// This does not prove server claim admission or enable dispatch.
    pub async fn package_seed_capture(
        &self,
        database: &Database,
        blob_dir: &Path,
        setup_id: [u8; 32],
    ) -> anyhow::Result<EncryptedLocalSharedStatePackage> {
        let seed = self.prepare_seed_claim(database, setup_id).await?;
        self.package_local_capture(database, blob_dir, seed.genesis().commitment())
            .await
    }

    /// Removes only the provisional seed authority for an exact, unfenced
    /// claim. Package keys remain available for ordinary local data.
    pub async fn rollback_seed_claim(
        &self,
        database: &Database,
        seed: &SeedAuthority,
    ) -> anyhow::Result<()> {
        self.validate_database(database).await?;
        self.rollback_seed_commitment(database, seed.genesis().commitment())
            .await
    }

    async fn rollback_seed_commitment(
        &self,
        database: &Database,
        commitment: [u8; 32],
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            database.seed_source_pin().await?.is_none()
                && database.seed_publication_intent_bytes().await?.is_none()
                && !database
                    .has_local_shared_state_package_never_dispatched()
                    .await?,
            "error seed-claim-rollback-fenced"
        );
        // Protected authority goes first so an interruption leaves an unfenced
        // pin without authority, which `prepare_seed_claim` clears.
        {
            let _guard = self.lock()?;
            let package = self.load_required_locked()?;
            if let Some(bytes) = self.load_secret(SEED_ITEM, SEED_BYTES)? {
                let stored = self.decode_seed(&bytes, &package)?;
                if stored.genesis().commitment() != commitment {
                    anyhow::bail!("error seed-claim-rollback-conflict");
                }
            }
            self.delete_secret(SEED_ITEM)?;
            self.remove_record(SEED_MARKER_RECORD)?;
        }
        database.rollback_local_seed_genesis(commitment).await
    }

    /// Clears a seed pin whose protected authority is gone when nothing was
    /// fenced, captured or dispatched under it. Only an interrupted rollback
    /// leaves that state; the refused claim authorized nothing on the server.
    async fn clear_unfenced_orphan_pin(
        &self,
        database: &Database,
        commitment: [u8; 32],
    ) -> anyhow::Result<bool> {
        if database.seed_source_pin().await?.is_some()
            || database.seed_publication_intent_bytes().await?.is_some()
            || database
                .has_local_shared_state_package_never_dispatched()
                .await?
        {
            return Ok(false);
        }
        {
            let _guard = self.lock()?;
            if self.load_secret(SEED_ITEM, SEED_BYTES)?.is_some() {
                return Ok(false);
            }
            self.remove_record(SEED_MARKER_RECORD)?;
        }
        database.rollback_local_seed_genesis(commitment).await?;
        Ok(true)
    }

    pub(super) fn seed_authority_exists(&self) -> StoreResult<bool> {
        Ok(self
            .read_record(SEED_MARKER_RECORD, SEED_MARKER_BYTES)?
            .is_some()
            || self.load_secret(SEED_ITEM, SEED_BYTES)?.is_some())
    }

    fn load_or_create_seed(
        &self,
        package: &ProtectedLocalPackageKey,
        setup: [u8; 32],
        pin: Option<[u8; 32]>,
    ) -> StoreResult<SeedAuthority> {
        self.prepare()?;
        let _guard = self.lock()?;
        let marker = self.read_record(SEED_MARKER_RECORD, SEED_MARKER_BYTES)?;
        let seed = match self.load_secret(SEED_ITEM, SEED_BYTES)? {
            Some(bytes) => self.decode_seed(&bytes, package)?,
            None if marker.is_some() || pin.is_some() => {
                return Err(error(ProtectedLocalKeyStoreErrorKind::MissingAuthority));
            }
            None => {
                let seed = SeedAuthority::generate(package.context(), package.package_key(), setup)
                    .map_err(|_| error(ProtectedLocalKeyStoreErrorKind::Unavailable))?;
                let encoded = self.encode_seed(&seed)?;
                self.create_secret(SEED_ITEM, &encoded)?;
                let saved = self
                    .load_secret(SEED_ITEM, SEED_BYTES)?
                    .ok_or_else(|| error(ProtectedLocalKeyStoreErrorKind::WriteFailed))?;
                if saved.as_slice() != encoded.as_slice() {
                    return Err(error(ProtectedLocalKeyStoreErrorKind::WriteFailed));
                }
                self.decode_seed(&saved, package)?
            }
        };
        let commitment = seed.genesis().commitment();
        if seed.genesis().setup_id() != setup {
            return Err(error(ProtectedLocalKeyStoreErrorKind::SetupMismatch));
        }
        if pin.is_some_and(|pin| pin != commitment) {
            return Err(error(ProtectedLocalKeyStoreErrorKind::Corrupt));
        }
        let mut expected = SEED_MARKER.to_vec();
        expected.extend(commitment);
        match marker {
            Some(marker) if marker != expected => {
                return Err(error(ProtectedLocalKeyStoreErrorKind::Corrupt));
            }
            Some(_) => {}
            None => self.write_record(SEED_MARKER_RECORD, &expected)?,
        }
        Ok(seed)
    }

    fn encode_seed(&self, seed: &SeedAuthority) -> StoreResult<Zeroizing<Vec<u8>>> {
        let mut out = Zeroizing::new(Vec::with_capacity(SEED_BYTES));
        out.extend_from_slice(SEED_MAGIC);
        out.extend_from_slice(
            &hex::decode(&self.account)
                .map_err(|_| error(ProtectedLocalKeyStoreErrorKind::WrongDatabase))?,
        );
        out.extend_from_slice(&seed.protected_storage_bytes());
        let checksum = Sha256::digest(out.as_slice());
        out.extend_from_slice(&checksum);
        Ok(out)
    }

    pub(super) fn decode_seed(
        &self,
        bytes: &[u8],
        package: &ProtectedLocalPackageKey,
    ) -> StoreResult<SeedAuthority> {
        let account = hex::decode(&self.account)
            .map_err(|_| error(ProtectedLocalKeyStoreErrorKind::WrongDatabase))?;
        if bytes.len() != SEED_BYTES
            || &bytes[..8] != SEED_MAGIC
            || bytes[8..40] != account
            || Sha256::digest(&bytes[..SEED_BYTES - 32]).as_slice() != &bytes[SEED_BYTES - 32..]
        {
            return Err(error(ProtectedLocalKeyStoreErrorKind::Corrupt));
        }
        SeedAuthority::from_protected_storage(
            &bytes[40..SEED_BYTES - 32],
            package.context(),
            package.package_key(),
        )
        .map_err(|_| error(ProtectedLocalKeyStoreErrorKind::Corrupt))
    }
}

#[cfg(test)]
mod tests;
