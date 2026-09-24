//! Original-installation adoption. Never installs the captured domain snapshot.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::SqliteConnection;

use super::{load_persisted_local_capture, package};
use crate::db::{self, Database};
use crate::sync::LocalSharedStatePackageKey;
use crate::sync::seed_claim::{Genesis, Publication, PublicationOutcome, SeedAuthority};

/// Host-persisted source identity, independent of replaceable SQLite state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeedSourceAuthority(Vec<u8>);

impl SeedSourceAuthority {
    pub fn generate(account: [u8; 32], genesis: &Genesis) -> Result<Self> {
        let mut bytes = b"AVENSRC1".to_vec();
        bytes.extend(account);
        let mut incarnation = [0; 32];
        getrandom::fill(&mut incarnation).context("error seed-source-entropy")?;
        bytes.extend(incarnation);
        bytes.extend(genesis.commitment());
        Ok(Self(bytes))
    }

    pub fn from_protected_storage(
        bytes: &[u8],
        account: [u8; 32],
        genesis: &Genesis,
    ) -> Result<Self> {
        ensure!(
            bytes.len() == 104
                && &bytes[..8] == b"AVENSRC1"
                && bytes[8..40] == account
                && bytes[72..] == genesis.commitment(),
            "error seed-source-authority-mismatch"
        );
        Ok(Self(bytes.to_vec()))
    }

    pub fn protected_storage_bytes(&self) -> &[u8] {
        &self.0
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct IntentData {
    version: u32,
    source: Vec<u8>,
    client: String,
    generation: i64,
    candidate: String,
    descriptor: Vec<u8>,
    publication: Vec<u8>,
    history: [u8; 32],
}

/// Exact local intent, not permission to dispatch or evidence of server acceptance.
#[derive(Clone, Debug)]
pub struct SeedPublicationIntent {
    bytes: Vec<u8>,
    data: IntentData,
}

impl SeedPublicationIntent {
    pub fn from_protected_storage(
        bytes: &[u8],
        source: &SeedSourceAuthority,
        genesis: &Genesis,
    ) -> Result<Self> {
        ensure!(bytes.len() <= 65536, "error seed-intent-too-large");
        let data: IntentData =
            serde_json::from_slice(bytes).context("error seed-intent-corrupt")?;
        ensure!(
            source.0[72..] == genesis.commitment()
                && data.version == 1
                && data.source == source.0
                && serde_json::to_vec(&data)? == bytes,
            "error seed-intent-source-mismatch"
        );
        let publication = Publication::from_record(genesis, &data.descriptor, &data.publication)?;
        ensure!(
            hex::encode(publication.binding().bootstrap_id) == data.candidate,
            "error seed-intent-candidate-mismatch"
        );
        Ok(Self {
            bytes: bytes.to_vec(),
            data,
        })
    }

    pub fn protected_storage_bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn descriptor(&self) -> &[u8] {
        &self.data.descriptor
    }

    pub fn publication(&self, genesis: &Genesis) -> Result<Publication> {
        Publication::from_record(genesis, &self.data.descriptor, &self.data.publication)
    }
}

pub(crate) async fn ensure_unbound(conn: &mut SqliteConnection) -> Result<()> {
    let bound: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM local_seed_source) OR EXISTS(SELECT 1 FROM local_seed_publication_intent)").fetch_one(&mut *conn).await?;
    let peer_bound: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM local_peer_enrollment)")
        .fetch_one(&mut *conn)
        .await?;
    ensure!(
        !bound && !peer_bound,
        "error e2ee-installation-fenced encrypted-tail-and-replacement-unavailable"
    );
    Ok(())
}

pub(super) async fn ensure_no_intent(conn: &mut SqliteConnection) -> Result<()> {
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM local_seed_publication_intent) OR EXISTS(SELECT 1 FROM local_shared_capture_journal WHERE publication_owned = 1)")
            .fetch_one(conn)
            .await?;
    ensure!(
        !exists,
        "error seed-publication-intent-owned local-cancellation-unavailable"
    );
    Ok(())
}

async fn source_matches(
    conn: &mut SqliteConnection,
    source: &SeedSourceAuthority,
) -> Result<String> {
    let (bytes, client): (Vec<u8>, String) =
        sqlx::query_as("SELECT authority, client_id FROM local_seed_source WHERE singleton = 1")
            .fetch_optional(&mut *conn)
            .await?
            .context("error seed-source-missing")?;
    ensure!(
        bytes == source.0 && db::get_meta(conn, "client_id").await?.as_deref() == Some(&client),
        "error seed-source-mismatch"
    );
    Ok(client)
}

async fn generation(conn: &mut SqliteConnection) -> Result<i64> {
    db::get_meta(conn, "sync_generation")
        .await?
        .context("error seed-generation-missing")?
        .parse()
        .context("error seed-generation-invalid")
}

// Typed values serialize with sorted object keys; payloads are parsed before
// comparison so historical meaning does not depend on JSON whitespace.
pub(super) fn history_bytes(
    rows: &[crate::data_safety::export_types::ChangeRow],
) -> Result<String> {
    let mut values = rows
        .iter()
        .map(|row| {
            let mut value = serde_json::to_value(row)?;
            value["payload"] = serde_json::from_str(&row.payload)?;
            Ok(value)
        })
        .collect::<Result<Vec<serde_json::Value>>>()?;
    values.sort_by(|a, b| a["change_id"].as_str().cmp(&b["change_id"].as_str()));
    Ok(serde_json::to_string(&values)?)
}

async fn load_history_tables(
    conn: &mut SqliteConnection,
) -> Result<(
    Vec<crate::data_safety::ChangeRow>,
    Vec<crate::data_safety::SharedHistoryProvenanceRow>,
)> {
    let changes = sqlx::query_as(
        "SELECT change_id, client_id, local_seq, entity_type, entity_id, field,
                op_type, payload, base_version, created_at, server_seq
         FROM changes",
    )
    .fetch_all(&mut *conn)
    .await?;
    let provenance = sqlx::query_as(
        "SELECT change_id, source_server_seq, source_pending_rank
         FROM shared_history_provenance",
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok((changes, provenance))
}

async fn validate_history(
    conn: &mut SqliteConnection,
    candidate: &str,
    source: &SeedSourceAuthority,
) -> Result<[u8; 32]> {
    let (capture_source, history, captured_generation): (Option<Vec<u8>>, Option<String>, i64) = sqlx::query_as("SELECT source_authority, source_history, sync_generation FROM local_shared_capture_journal WHERE candidate_id = ?").bind(candidate).fetch_optional(&mut *conn).await?.context("error seed-capture-missing")?;
    ensure!(
        capture_source.as_deref() == Some(source.0.as_slice())
            && captured_generation == generation(conn).await?,
        "error seed-capture-source-changed"
    );
    let history = history.context("error seed-capture-incompatible recapture-never-dispatched")?;
    let expected: Vec<serde_json::Value> = serde_json::from_str(&history)?;
    let capture = load_persisted_local_capture(conn)
        .await?
        .context("error seed-capture-missing")?;
    let frozen: Vec<serde_json::Value> =
        serde_json::from_str(&history_bytes(&capture.capture.snapshot.tables.changes)?)?;
    let without_rank = |rows: &[serde_json::Value]| {
        rows.iter()
            .map(|row| {
                let mut row = row.clone();
                if let Some(object) = row.as_object_mut() {
                    object.remove("server_seq");
                }
                row
            })
            .collect::<Vec<_>>()
    };
    ensure!(
        without_rank(&expected) == without_rank(&frozen),
        "error seed-captured-history-map-mismatch"
    );
    let (changes, provenance) = load_history_tables(conn).await?;
    let stored_provenance: String = sqlx::query_scalar(
        "SELECT source_provenance FROM local_shared_capture_journal WHERE candidate_id = ?",
    )
    .bind(candidate)
    .fetch_one(&mut *conn)
    .await?;
    let expected_provenance: Vec<crate::data_safety::SharedHistoryProvenanceRow> =
        serde_json::from_str(&stored_provenance)?;
    let captured_ids = expected
        .iter()
        .filter_map(|v| v["change_id"].as_str())
        .collect::<std::collections::HashSet<_>>();
    let mut current_provenance = provenance
        .iter()
        .filter(|p| captured_ids.contains(p.change_id.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    current_provenance.sort_by(|a, b| a.change_id.cmp(&b.change_id));
    ensure!(
        current_provenance == expected_provenance,
        "error seed-source-provenance-changed"
    );
    let current: Vec<serde_json::Value> = serde_json::from_str(&history_bytes(&changes)?)?;
    let current = current
        .into_iter()
        .map(|v| (v["change_id"].as_str().unwrap_or_default().to_string(), v))
        .collect::<std::collections::HashMap<_, _>>();
    for row in &expected {
        let id = row["change_id"]
            .as_str()
            .context("error seed-history-invalid")?;
        ensure!(
            current.get(id) == Some(row),
            "error seed-captured-history-changed"
        );
    }
    let ids = expected
        .iter()
        .filter_map(|v| v["change_id"].as_str())
        .collect::<std::collections::HashSet<_>>();
    ensure!(
        current
            .iter()
            .all(|(id, row)| row["server_seq"].is_null() || ids.contains(id.as_str())),
        "error seed-uncaptured-accepted-history"
    );
    let mut digest = Sha256::new();
    digest.update(b"aven-local-source-history-v1");
    digest.update((history.len() as u64).to_be_bytes());
    digest.update(history.as_bytes());
    digest.update(stored_provenance.as_bytes());
    Ok(digest.finalize().into())
}

impl Database {
    /// Loads exact upload bytes only for a sealed, protected publication intent.
    /// Adopted installations need only their retained intent and remote outcome.
    pub async fn seed_publication_upload(
        &self,
        source: &SeedSourceAuthority,
        intent: &SeedPublicationIntent,
        seed: &SeedAuthority,
        key: &LocalSharedStatePackageKey,
    ) -> Result<Option<crate::sync::bootstrap_format::Package>> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = db::begin_immediate(&mut conn).await?;
        source_matches(&mut tx, source).await?;
        let (stored, state): (Vec<u8>, String) = sqlx::query_as(
            "SELECT intent, state FROM local_seed_publication_intent WHERE singleton = 1",
        )
        .fetch_optional(&mut *tx)
        .await?
        .context("error seed-intent-missing")?;
        ensure!(stored == intent.bytes, "error seed-intent-mismatch");
        if state == "adopted" {
            return Ok(None);
        }
        ensure!(
            state == "sealed" && intent.data.generation == generation(&mut tx).await?,
            "error seed-intent-not-sealed"
        );
        ensure!(
            validate_history(&mut tx, &intent.data.candidate, source).await? == intent.data.history,
            "error seed-history-commitment-mismatch"
        );
        let capture = load_persisted_local_capture(&mut tx)
            .await?
            .context("error seed-capture-missing")?;
        let package = package::load_package(&mut tx, &intent.data.candidate)
            .await?
            .context("error seed-package-missing")?;
        let upload = package.upload_package();
        ensure!(
            upload.descriptor == intent.data.descriptor,
            "error seed-package-mismatch"
        );
        package::publication::validate_against_capture(
            &upload,
            &capture,
            key,
            seed.genesis().commitment(),
        )?;
        tx.commit().await?;
        Ok(Some(upload))
    }

    pub async fn seed_source_pin(&self) -> Result<Option<Vec<u8>>> {
        let mut conn = self.acquire_reader().await?;
        Ok(
            sqlx::query_scalar("SELECT authority FROM local_seed_source WHERE singleton = 1")
                .fetch_optional(&mut *conn)
                .await?,
        )
    }

    /// Called under the installation interlock after protected persistence.
    pub async fn bind_seed_source(
        &self,
        source: &SeedSourceAuthority,
        installation: &db::installation::InstallationGuard,
    ) -> Result<()> {
        ensure!(
            self.file_identity() == Some(installation.identity()),
            "error seed-source-installation-mismatch"
        );
        installation.fence()?;
        let mut conn = self.acquire_writer().await?;
        let mut tx = db::begin_immediate(&mut conn).await?;
        let existing: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT authority FROM local_seed_source WHERE singleton = 1")
                .fetch_optional(&mut *tx)
                .await?;
        if let Some(existing) = existing {
            ensure!(existing == source.0, "error seed-source-mismatch");
            source_matches(&mut tx, source).await?;
        } else {
            super::ensure_no_active_local_shared_capture(&mut tx).await?;
            let client = db::get_meta(&mut tx, "client_id")
                .await?
                .context("missing client identity")?;
            sqlx::query("INSERT INTO local_seed_source VALUES (1, ?, ?)")
                .bind(&source.0)
                .bind(client)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn seed_publication_intent_bytes(&self) -> Result<Option<(Vec<u8>, String)>> {
        let mut conn = self.acquire_reader().await?;
        Ok(sqlx::query_as(
            "SELECT intent, state FROM local_seed_publication_intent WHERE singleton = 1",
        )
        .fetch_optional(&mut *conn)
        .await?)
    }

    /// Commits cancellation exclusion before any protected intent is exposed.
    /// Retrying returns exact persisted bytes, never a newly signed context.
    pub async fn prepare_seed_publication_intent(
        &self,
        source: &SeedSourceAuthority,
        seed: &SeedAuthority,
        key: &LocalSharedStatePackageKey,
    ) -> Result<SeedPublicationIntent> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = db::begin_immediate(&mut conn).await?;
        let client = source_matches(&mut tx, source).await?;
        let pin: Vec<u8> =
            sqlx::query_scalar("SELECT commitment FROM local_seed_genesis_pin WHERE singleton = 1")
                .fetch_optional(&mut *tx)
                .await?
                .context("error seed-genesis-pin-missing")?;
        ensure!(
            pin == seed.genesis().commitment(),
            "error seed-genesis-pin-mismatch"
        );
        let existing: Option<(Vec<u8>, String)> = sqlx::query_as(
            "SELECT intent, state FROM local_seed_publication_intent WHERE singleton = 1",
        )
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((bytes, state)) = existing {
            let intent =
                SeedPublicationIntent::from_protected_storage(&bytes, source, seed.genesis())?;
            if state != "adopted" {
                ensure!(
                    intent.data.generation == generation(&mut tx).await?
                        && intent.data.history
                            == validate_history(&mut tx, &intent.data.candidate, source).await?,
                    "error seed-intent-source-changed"
                );
                let package = package::load_package(&mut tx, &intent.data.candidate)
                    .await?
                    .context("error seed-package-missing")?;
                ensure!(
                    package.descriptor() == intent.data.descriptor,
                    "error seed-package-mismatch"
                );
                let capture = load_persisted_local_capture(&mut tx)
                    .await?
                    .context("error seed-capture-missing")?;
                package::publication::validate_against_capture(
                    &package.upload_package(),
                    &capture,
                    key,
                    seed.genesis().commitment(),
                )?;
            }
            return Ok(intent);
        }
        ensure_no_intent(&mut tx).await?;
        let capture = load_persisted_local_capture(&mut tx)
            .await?
            .context("error seed-capture-missing")?;
        let history = validate_history(&mut tx, capture.candidate_id(), source).await?;
        let package = package::load_package(&mut tx, capture.candidate_id())
            .await?
            .context("error seed-package-missing")?;
        let upload = package.upload_package();
        package::publication::validate_against_capture(
            &upload,
            &capture,
            key,
            seed.genesis().commitment(),
        )?;
        let publication = seed.prepare_bootstrap_publication(&upload, key)?;
        let data = IntentData {
            version: 1,
            source: source.0.clone(),
            client,
            generation: generation(&mut tx).await?,
            candidate: capture.candidate_id().to_string(),
            descriptor: upload.descriptor,
            publication: publication.record().to_vec(),
            history,
        };
        let bytes = serde_json::to_vec(&data)?;
        sqlx::query("INSERT INTO local_seed_publication_intent(singleton, candidate_id, intent, state) VALUES (1, ?, ?, 'preparing')").bind(&data.candidate).bind(&bytes).execute(&mut *tx).await?;
        sqlx::query(
            "UPDATE local_shared_capture_journal SET publication_owned = 1 WHERE candidate_id = ?",
        )
        .bind(&data.candidate)
        .execute(&mut *tx)
        .await?;
        #[cfg(test)]
        wait_intent_boundary(&data.candidate).await;
        tx.commit().await?;
        SeedPublicationIntent::from_protected_storage(&bytes, source, seed.genesis())
    }

    /// Hosts supply an intent reloaded from protected storage, never a boolean.
    pub async fn seal_seed_publication_intent(
        &self,
        source: &SeedSourceAuthority,
        intent: &SeedPublicationIntent,
    ) -> Result<()> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = db::begin_immediate(&mut conn).await?;
        source_matches(&mut tx, source).await?;
        ensure!(
            intent.data.source == source.0
                && intent.data.generation == generation(&mut tx).await?
                && intent.data.history
                    == validate_history(&mut tx, &intent.data.candidate, source).await?,
            "error seed-intent-source-changed"
        );
        let commitment: Vec<u8> = sqlx::query_scalar("SELECT frozen_descriptor_commitment FROM local_shared_capture_journal WHERE candidate_id = ?").bind(&intent.data.candidate).fetch_one(&mut *tx).await?;
        ensure!(
            commitment == Sha256::digest(&intent.data.descriptor).as_slice(),
            "error seed-frozen-descriptor-changed"
        );
        let changed = sqlx::query("UPDATE local_seed_publication_intent SET state = 'sealed' WHERE singleton = 1 AND intent = ? AND state IN ('preparing', 'sealed')").bind(&intent.bytes).execute(&mut *tx).await?.rows_affected();
        ensure!(changed == 1, "error seed-intent-cas-failed");
        tx.commit().await?;
        Ok(())
    }

    /// Atomically adopts the original seed's history, never its old domain image.
    /// Returns false for an already committed matching adoption without rewinding.
    pub async fn adopt_seed_publication(
        &self,
        source: &SeedSourceAuthority,
        intent: &SeedPublicationIntent,
        seed: &SeedAuthority,
        key: &LocalSharedStatePackageKey,
        outcome: &PublicationOutcome,
    ) -> Result<bool> {
        let verified =
            SeedPublicationIntent::from_protected_storage(&intent.bytes, source, seed.genesis())?;
        outcome.validate_expected(seed.genesis(), &verified.data.descriptor)?;
        ensure!(
            outcome.publication().record().as_slice() == verified.data.publication,
            "error seed-publication-outcome-mismatch"
        );
        let binding = outcome.publication().binding();
        let association = format!(
            "{}:{}:{}",
            hex::encode(binding.vault_id),
            hex::encode(binding.stream_id),
            hex::encode(binding.bootstrap_id)
        );
        let mut conn = self.acquire_writer().await?;
        let mut tx = db::begin_immediate(&mut conn).await?;
        let pin: Vec<u8> =
            sqlx::query_scalar("SELECT commitment FROM local_seed_genesis_pin WHERE singleton = 1")
                .fetch_optional(&mut *tx)
                .await?
                .context("error seed-genesis-pin-missing")?;
        ensure!(
            pin == seed.genesis().commitment(),
            "error seed-genesis-pin-mismatch"
        );
        ensure!(
            source_matches(&mut tx, source).await? == intent.data.client,
            "error seed-source-client-changed"
        );
        let (stored, state, adopted_generation): (Vec<u8>, String, Option<i64>) = sqlx::query_as("SELECT intent, state, association_generation FROM local_seed_publication_intent WHERE singleton = 1").fetch_optional(&mut *tx).await?.context("error seed-intent-missing")?;
        ensure!(stored == intent.bytes, "error seed-intent-mismatch");
        if state == "adopted" {
            ensure!(
                db::get_meta(&mut tx, "e2ee_association").await?.as_deref() == Some(&association)
                    && adopted_generation == Some(generation(&mut tx).await?)
                    && db::get_meta(&mut tx, "sync_cursor")
                        .await?
                        .context("missing cursor")?
                        .parse::<u64>()?
                        >= binding.prefix_count,
                "error seed-adopted-association-changed"
            );
            crate::sync::encrypted_tail::dependencies::validate(
                &mut tx,
                &association,
                i64::try_from(binding.prefix_count)?,
            )
            .await?;
            crate::sync::encrypted_tail::attachments::client::validate(
                &mut tx,
                &association,
                i64::try_from(binding.prefix_count)?,
                &binding.descriptor_commitment,
            )
            .await?;
            tx.commit().await?;
            return Ok(false);
        }
        ensure!(
            state == "sealed" && intent.data.generation == generation(&mut tx).await?,
            "error seed-intent-not-sealed"
        );
        ensure!(
            validate_history(&mut tx, &intent.data.candidate, source).await? == intent.data.history,
            "error seed-history-commitment-mismatch"
        );
        let capture = load_persisted_local_capture(&mut tx)
            .await?
            .context("error seed-capture-missing")?;
        let package = package::load_package(&mut tx, &intent.data.candidate)
            .await?
            .context("error seed-package-missing")?;
        ensure!(
            package.descriptor() == intent.data.descriptor,
            "error seed-package-mismatch"
        );
        package::publication::validate_against_capture(
            &package.upload_package(),
            &capture,
            key,
            seed.genesis().commitment(),
        )?;
        ensure!(
            capture.capture.snapshot.tables.changes.len() as u64 == binding.prefix_count,
            "error seed-prefix-count-mismatch"
        );
        for provenance in &capture.capture.snapshot.tables.shared_history_provenance {
            let existing: Option<(Option<i64>, Option<i64>)> = sqlx::query_as("SELECT source_server_seq, source_pending_rank FROM shared_history_provenance WHERE change_id = ?").bind(&provenance.change_id).fetch_optional(&mut *tx).await?;
            ensure!(
                existing.is_none_or(
                    |v| v == (provenance.source_server_seq, provenance.source_pending_rank)
                ),
                "error seed-provenance-changed"
            );
            sqlx::query("INSERT OR IGNORE INTO shared_history_provenance(change_id, source_server_seq, source_pending_rank) VALUES (?, ?, ?)").bind(&provenance.change_id).bind(provenance.source_server_seq).bind(provenance.source_pending_rank).execute(&mut *tx).await?;
        }
        sqlx::query("UPDATE changes SET server_seq = NULL WHERE change_id IN (SELECT change_id FROM local_shared_capture_changes WHERE candidate_id = ?)").bind(&intent.data.candidate).execute(&mut *tx).await?;
        for row in &capture.capture.snapshot.tables.changes {
            let count = sqlx::query("UPDATE changes SET server_seq = ? WHERE change_id = ?")
                .bind(row.server_seq)
                .bind(&row.change_id)
                .execute(&mut *tx)
                .await?
                .rows_affected();
            ensure!(count == 1, "error seed-history-coverage-changed");
        }
        crate::epic_membership::recover(&mut tx, true).await?;
        let next = intent
            .data
            .generation
            .checked_add(1)
            .context("sync generation overflow")?;
        crate::sync::encrypted_tail::dependencies::initialize(
            &mut tx,
            &association,
            next,
            i64::try_from(binding.prefix_count)?,
            &capture.capture.snapshot.tables.task_dependencies,
        )
        .await?;
        crate::sync::encrypted_tail::attachments::client::initialize(
            &mut tx,
            &association,
            next,
            i64::try_from(binding.prefix_count)?,
            &package.upload_package(),
            key,
        )
        .await?;
        db::set_meta(&mut tx, "sync_generation", &next.to_string()).await?;
        db::set_meta(&mut tx, "sync_cursor", &binding.prefix_count.to_string()).await?;
        db::set_meta(&mut tx, "e2ee_association", &association).await?;
        sqlx::query("DELETE FROM meta WHERE key = 'sync_server_url'")
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE local_seed_publication_intent SET state = 'adopted', association_generation = ?, association = ? WHERE singleton = 1").bind(next).bind(&association).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(true)
    }

    /// Separate post-commit cleanup. Durable authority/receipt never cascades away.
    pub async fn cleanup_adopted_seed_capture(
        &self,
        source: &SeedSourceAuthority,
        intent: &SeedPublicationIntent,
    ) -> Result<()> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = db::begin_immediate(&mut conn).await?;
        source_matches(&mut tx, source).await?;
        let ready: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM local_seed_publication_intent WHERE intent = ? AND state = 'adopted' AND association = (SELECT value FROM meta WHERE key = 'e2ee_association') AND association_generation = CAST((SELECT value FROM meta WHERE key = 'sync_generation') AS INTEGER))").bind(&intent.bytes).fetch_one(&mut *tx).await?;
        ensure!(ready, "error seed-adoption-cleanup-not-authorized");
        sqlx::query("DELETE FROM local_shared_capture_journal WHERE candidate_id = ?")
            .bind(&intent.data.candidate)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
type IntentBarrier = (
    String,
    tokio::sync::oneshot::Sender<()>,
    tokio::sync::oneshot::Receiver<()>,
);
#[cfg(test)]
static INTENT_BARRIER: std::sync::Mutex<Option<IntentBarrier>> = std::sync::Mutex::new(None);
#[cfg(test)]
async fn wait_intent_boundary(candidate: &str) {
    let barrier = {
        let mut slot = INTENT_BARRIER.lock().unwrap();
        if slot.as_ref().is_some_and(|(id, _, _)| id == candidate) {
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
#[cfg(test)]
mod tests;
