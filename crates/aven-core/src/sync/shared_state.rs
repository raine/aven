use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::data_safety::export_types::{
    AvenExport, EXPORT_FORMAT, EXPORT_VERSION, RELATED_LINKS_EXPORT_VERSION,
    SharedHistoryProvenanceRow,
};
use crate::data_safety::{self, tables, validation};
use crate::db::{self, Database};
use anyhow::{Context, Result, ensure};

pub mod adoption;
mod package;
mod peer_install;

pub use package::publication as bootstrap_format;

pub use package::{
    DecryptedLocalSharedStateImage, EncryptedLocalSharedStateImage,
    EncryptedLocalSharedStatePackage, LocalSharedStatePackageContext, LocalSharedStatePackageKey,
    decrypt_local_shared_state_package_images,
};

/// A consistent, installation-ready copy of shared domain state and retained history.
///
/// This value deliberately has no serialized wire representation. Encryption and
/// publication layers can package it later without making this local interchange
/// type a protocol contract.
const LOCAL_CAPTURE_FORMAT: &str = "aven-local-shared-capture";
const LOCAL_CAPTURE_VERSION: i64 = 1;
const LOCAL_CAPTURE_STATE: &str = "never_dispatched";

/// A transient consistent copy used by installation and future packaging layers.
#[derive(Debug)]
pub struct SharedStateCapture {
    snapshot: AvenExport,
}

/// A durable local capture that has never been made available to a dispatcher.
///
/// Cancellation of this type is local-only. Publication code must use a separate
/// state machine with a server-confirmed cancellation fence.
#[derive(Debug)]
pub struct NeverDispatchedLocalSharedCapture {
    candidate_id: String,
    stream_id: String,
    capture: SharedStateCapture,
    images: Vec<PersistedCaptureImage>,
}

impl NeverDispatchedLocalSharedCapture {
    pub fn candidate_id(&self) -> &str {
        &self.candidate_id
    }

    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }

    pub fn shared_state(&self) -> &SharedStateCapture {
        &self.capture
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct PersistedLocalCapture {
    format: String,
    version: i64,
    candidate_id: String,
    stream_id: String,
    images: Vec<PersistedCaptureImage>,
    snapshot: AvenExport,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
struct PersistedCaptureImage {
    sha256: String,
    classification: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SharedStateInstallReport {
    pub prefix_count: u64,
    pub attachment_count: u64,
}

impl Database {
    /// Captures materialized shared state and retained history in one SQLite read boundary.
    ///
    /// The source database is not acknowledged, renumbered, or associated with a
    /// different server. Device-private metadata and attachment availability are
    /// excluded from the captured value.
    pub async fn capture_shared_state(&self) -> Result<SharedStateCapture> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = db::begin_immediate(&mut conn).await?;
        let schema_version = db::current_schema_version(&mut tx).await?;
        let tables = data_safety::scan_export_tables(&mut tx).await?;
        let capture = SharedStateCapture::from_tables(schema_version, tables)?;
        tx.commit().await?;
        Ok(capture)
    }

    /// Creates or resumes the one durable, never-dispatched local capture.
    ///
    /// The snapshot, exact history map, stable candidate and stream identities,
    /// image classification, ownership pins, counter floor, and sync fence commit
    /// in one immediate transaction. An existing capture is resumed without
    /// reading current domain rows or attachment bytes again.
    pub async fn capture_local_shared_state_never_dispatched(
        &self,
        blob_dir: &Path,
    ) -> Result<NeverDispatchedLocalSharedCapture> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = db::begin_immediate(&mut conn).await?;
        adoption::ensure_no_intent(&mut tx).await?;
        if let Some(capture) = load_persisted_local_capture(&mut tx).await? {
            tx.commit().await?;
            return Ok(capture);
        }

        let schema_version = db::current_schema_version(&mut tx).await?;
        let tables = data_safety::scan_export_tables(&mut tx).await?;
        let source_history = adoption::history_bytes(&tables.changes)?;
        let mut source_provenance = tables.shared_history_provenance.clone();
        source_provenance.sort_by(|a, b| a.change_id.cmp(&b.change_id));
        let source_provenance = serde_json::to_string(&source_provenance)?;
        let image_classes = classify_and_validate_images(&tables, blob_dir).await?;
        let capture = SharedStateCapture::from_tables(schema_version, tables)?;
        let candidate_id = random_cryptographic_id()?;
        let stream_id = random_cryptographic_id()?;
        let created_at = crate::ids::now();
        let local_seq_floor = capture
            .snapshot
            .tables
            .changes
            .iter()
            .map(|row| row.local_seq)
            .max()
            .unwrap_or(0);
        let existing_floor = db::get_meta(&mut tx, "local_seq")
            .await?
            .unwrap_or_else(|| "0".to_string())
            .parse::<i64>()?;
        db::set_meta(
            &mut tx,
            "local_seq",
            &existing_floor.max(local_seq_floor).to_string(),
        )
        .await?;
        let sync_generation = db::get_meta(&mut tx, "sync_generation")
            .await?
            .unwrap_or_else(|| "0".to_string())
            .parse::<i64>()?
            .checked_add(1)
            .context("sync generation overflow")?;
        db::set_meta(&mut tx, "sync_generation", &sync_generation.to_string()).await?;

        let persisted = PersistedLocalCapture {
            format: LOCAL_CAPTURE_FORMAT.to_string(),
            version: LOCAL_CAPTURE_VERSION,
            candidate_id: candidate_id.clone(),
            stream_id: stream_id.clone(),
            images: image_classes
                .into_iter()
                .map(|(sha256, classification)| PersistedCaptureImage {
                    sha256,
                    classification: classification.to_string(),
                })
                .collect(),
            snapshot: capture.snapshot,
        };
        let snapshot_json = serde_json::to_string(&persisted)?;
        sqlx::query(
            "INSERT INTO local_shared_capture_journal(
                 singleton, candidate_id, stream_id, state, internal_format,
                 internal_version, snapshot_json, local_seq_floor,
                 sync_generation, created_at
             ) VALUES (1, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&candidate_id)
        .bind(&stream_id)
        .bind(LOCAL_CAPTURE_STATE)
        .bind(LOCAL_CAPTURE_FORMAT)
        .bind(LOCAL_CAPTURE_VERSION)
        .bind(&snapshot_json)
        .bind(local_seq_floor)
        .bind(sync_generation)
        .bind(&created_at)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE local_shared_capture_journal SET source_authority = (SELECT authority FROM local_seed_source WHERE singleton = 1), source_history = ?, source_provenance = ? WHERE singleton = 1")
            .bind(source_history).bind(source_provenance).execute(&mut *tx).await?;
        let provenance_by_id = persisted
            .snapshot
            .tables
            .shared_history_provenance
            .iter()
            .map(|row| (row.change_id.as_str(), row))
            .collect::<HashMap<_, _>>();
        for change in &persisted.snapshot.tables.changes {
            let provenance = provenance_by_id
                .get(change.change_id.as_str())
                .context("shared history provenance is missing")?;
            sqlx::query(
                "INSERT INTO local_shared_capture_changes(
                     candidate_id, change_id, prefix_rank, source_server_seq,
                     source_pending_rank
                 ) VALUES (?, ?, ?, ?, ?)",
            )
            .bind(&candidate_id)
            .bind(&change.change_id)
            .bind(
                change
                    .server_seq
                    .context("shared history rank is missing")?,
            )
            .bind(provenance.source_server_seq)
            .bind(provenance.source_pending_rank)
            .execute(&mut *tx)
            .await?;
        }
        for image in &persisted.images {
            sqlx::query(
                "INSERT INTO local_shared_capture_images(candidate_id, sha256, classification)
                 VALUES (?, ?, ?)",
            )
            .bind(&candidate_id)
            .bind(&image.sha256)
            .bind(&image.classification)
            .execute(&mut *tx)
            .await?;
            if image.classification != "unavailable" {
                sqlx::query(
                    "INSERT INTO local_shared_capture_pins(candidate_id, sha256) VALUES (?, ?)",
                )
                .bind(&candidate_id)
                .bind(&image.sha256)
                .execute(&mut *tx)
                .await?;
            }
        }
        validate_persisted_local_capture(
            &mut tx,
            &candidate_id,
            &persisted.images,
            &persisted.snapshot,
        )
        .await?;
        tx.commit().await?;
        Ok(NeverDispatchedLocalSharedCapture {
            candidate_id,
            stream_id,
            capture: SharedStateCapture {
                snapshot: persisted.snapshot,
            },
            images: persisted.images,
        })
    }

    /// Reopens the durable local capture without consulting newer domain rows.
    pub async fn resume_local_shared_state_never_dispatched(
        &self,
    ) -> Result<Option<NeverDispatchedLocalSharedCapture>> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = db::begin_immediate(&mut conn).await?;
        adoption::ensure_no_intent(&mut tx).await?;
        let capture = load_persisted_local_capture(&mut tx).await?;
        tx.commit().await?;
        Ok(capture)
    }

    /// Cancels only this slice's never-dispatched capture and releases its pins.
    ///
    /// Repeating cancellation after success is harmless. A different active
    /// candidate fails closed. Domain rows and later edits are never removed.
    pub async fn cancel_local_shared_state_never_dispatched(
        &self,
        candidate_id: &str,
    ) -> Result<bool> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = db::begin_immediate(&mut conn).await?;
        adoption::ensure_no_intent(&mut tx).await?;
        let active: Option<String> = sqlx::query_scalar(
            "SELECT candidate_id FROM local_shared_capture_journal WHERE singleton = 1",
        )
        .fetch_optional(&mut *tx)
        .await?;
        let Some(active) = active else {
            tx.commit().await?;
            return Ok(false);
        };
        ensure!(
            active == candidate_id,
            "error local-shared-capture-candidate-mismatch"
        );
        sqlx::query(
            "DELETE FROM local_shared_capture_journal
             WHERE singleton = 1 AND candidate_id = ? AND state = 'never_dispatched'",
        )
        .bind(candidate_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    /// Atomically installs captured shared state into a fresh database.
    ///
    /// The target keeps its own client identity and local settings. Retained
    /// history receives dense effective prefix ranks, while source ordering
    /// provenance remains separate. Attachment metadata is installed without
    /// claiming that image bytes are locally available.
    pub async fn install_shared_state(
        &self,
        capture: &SharedStateCapture,
    ) -> Result<SharedStateInstallReport> {
        capture.validate()?;
        let _installation = self.plaintext_installation_guard()?;
        let mut conn = self.acquire_writer().await?;
        let mut tx = db::begin_immediate(&mut conn).await?;
        ensure_empty_target(&mut tx).await?;

        let report = install_in_transaction(&mut tx, capture).await?;
        tx.commit().await?;

        Ok(report)
    }
}

async fn install_in_transaction(
    conn: &mut sqlx::SqliteConnection,
    capture: &SharedStateCapture,
) -> Result<SharedStateInstallReport> {
    let identity = db::get_meta(conn, "client_id")
        .await?
        .context("missing target client identity")?;
    ensure!(
        !capture
            .snapshot
            .tables
            .changes
            .iter()
            .any(|row| row.client_id == identity),
        "error shared-state-install target identity is not distinct"
    );

    sqlx::query("DELETE FROM workspaces")
        .execute(&mut *conn)
        .await?;
    let t = &capture.snapshot.tables;
    tables::import_workspaces(conn, &t.workspaces).await?;
    tables::import_projects(conn, &t.projects).await?;
    tables::import_project_id_aliases(conn, &t.project_id_aliases).await?;
    tables::import_labels(conn, &t.labels).await?;
    tables::import_metadata_fields(conn, &t.metadata_fields).await?;
    tables::import_metadata_field_id_aliases(conn, &t.metadata_field_id_aliases).await?;
    tables::import_tasks(conn, &t.tasks).await?;
    tables::import_task_metadata(conn, &t.task_metadata).await?;
    tables::import_task_labels(conn, &t.task_labels).await?;
    tables::import_notes(conn, &t.notes).await?;
    tables::import_task_dependencies(conn, &t.task_dependencies).await?;
    tables::import_task_epic_links(conn, &t.task_epic_links).await?;
    tables::import_blob_inventory(conn, &t.blob_inventory).await?;
    tables::import_task_attachments(conn, &t.task_attachments).await?;
    tables::import_recurrence_series(conn, &t.recurrence_series).await?;
    tables::import_recurrence_series_labels(conn, &t.recurrence_series_labels).await?;
    tables::import_recurrence_series_metadata(conn, &t.recurrence_series_metadata).await?;
    tables::import_recurrence_occurrences(conn, &t.recurrence_occurrences).await?;
    tables::import_recurrence_pause_intervals(conn, &t.recurrence_pause_intervals).await?;
    tables::import_changes(conn, &t.changes).await?;
    tables::import_task_related_links(conn, &t.task_related_links).await?;
    tables::import_field_versions(conn, &t.field_versions).await?;
    tables::import_conflicts(conn, &t.conflicts).await?;
    tables::import_shared_history_provenance(conn, &t.shared_history_provenance).await?;
    for row in &t.meta {
        db::set_meta(conn, &row.key, &row.value).await?;
    }
    let local_seq = t.changes.iter().map(|row| row.local_seq).max().unwrap_or(0);
    db::set_meta(conn, "local_seq", &local_seq.to_string()).await?;
    data_safety::ensure_integrity_ok(
        &crate::data_safety::integrity_report_in_transaction(conn).await?,
    )?;
    Ok(SharedStateInstallReport {
        prefix_count: u64::try_from(t.changes.len())?,
        attachment_count: u64::try_from(t.task_attachments.len())?,
    })
}

impl SharedStateCapture {
    fn from_tables(
        schema_version: i64,
        mut tables: crate::data_safety::export_types::ExportTables,
    ) -> Result<Self> {
        let stored = std::mem::take(&mut tables.shared_history_provenance)
            .into_iter()
            .map(|row| (row.change_id.clone(), row))
            .collect::<HashMap<_, _>>();

        tables.project_paths.clear();
        tables
            .meta
            .retain(|row| row.key.starts_with("epic_membership_baseline:"));
        for blob in &mut tables.blob_inventory {
            blob.available = 0;
            blob.last_verified_at = None;
        }

        let mut accepted = tables
            .changes
            .iter()
            .enumerate()
            .filter_map(|(index, row)| row.server_seq.map(|seq| (seq, index)))
            .collect::<Vec<_>>();
        accepted.sort_by_key(|(seq, _)| *seq);
        let mut pending = tables
            .changes
            .iter()
            .enumerate()
            .filter(|(_, row)| row.server_seq.is_none())
            .map(|(index, row)| {
                (
                    row.local_seq,
                    row.created_at.clone(),
                    row.change_id.clone(),
                    index,
                )
            })
            .collect::<Vec<_>>();
        pending.sort();

        let pending_ranks = pending
            .iter()
            .enumerate()
            .map(|(rank, (_, _, _, index))| (*index, rank + 1))
            .collect::<HashMap<_, _>>();
        let order = accepted
            .iter()
            .map(|(_, index)| *index)
            .chain(pending.iter().map(|(_, _, _, index)| *index));
        let mut provenance = Vec::with_capacity(tables.changes.len());
        for (position, index) in order.enumerate() {
            let row = &mut tables.changes[index];
            let source = if let Some(stored) = stored.get(&row.change_id) {
                SharedHistoryProvenanceRow {
                    change_id: row.change_id.clone(),
                    source_server_seq: stored.source_server_seq,
                    source_pending_rank: stored.source_pending_rank,
                }
            } else if let Some(source_server_seq) = row.server_seq {
                SharedHistoryProvenanceRow {
                    change_id: row.change_id.clone(),
                    source_server_seq: Some(source_server_seq),
                    source_pending_rank: None,
                }
            } else {
                let pending_rank = pending_ranks
                    .get(&index)
                    .context("pending history order is incomplete")?;
                SharedHistoryProvenanceRow {
                    change_id: row.change_id.clone(),
                    source_server_seq: None,
                    source_pending_rank: Some(i64::try_from(*pending_rank)?),
                }
            };
            row.server_seq = Some(i64::try_from(position + 1)?);
            provenance.push(source);
        }

        tables.shared_history_provenance = provenance;
        let version = if !tables.shared_history_provenance.is_empty() {
            EXPORT_VERSION
        } else if tables.task_related_links.is_empty() {
            2
        } else {
            RELATED_LINKS_EXPORT_VERSION
        };
        let capture = Self {
            snapshot: AvenExport {
                format: EXPORT_FORMAT.to_string(),
                version,
                exported_at: crate::ids::now(),
                schema_version,
                blobs_included: false,
                tables,
            },
        };
        capture.validate()?;
        Ok(capture)
    }

    fn validate(&self) -> Result<()> {
        validation::validate_shared_snapshot(&self.snapshot)?;
        ensure!(
            self.snapshot.tables.project_paths.is_empty(),
            "error invalid-shared-state project paths are device-private"
        );
        ensure!(
            self.snapshot
                .tables
                .meta
                .iter()
                .all(|row| row.key.starts_with("epic_membership_baseline:")),
            "error invalid-shared-state private metadata is present"
        );
        let change_ids = self
            .snapshot
            .tables
            .changes
            .iter()
            .map(|row| row.change_id.as_str())
            .collect::<HashSet<_>>();
        let provenance_ids = self
            .snapshot
            .tables
            .shared_history_provenance
            .iter()
            .map(|row| row.change_id.as_str())
            .collect::<HashSet<_>>();
        ensure!(
            change_ids.len() == self.snapshot.tables.changes.len()
                && provenance_ids.len() == self.snapshot.tables.shared_history_provenance.len()
                && change_ids == provenance_ids,
            "error invalid-shared-state history provenance does not match retained history"
        );
        for row in &self.snapshot.tables.shared_history_provenance {
            ensure!(
                matches!(
                    (row.source_server_seq, row.source_pending_rank),
                    (Some(1..), None) | (None, Some(1..))
                ),
                "error invalid-shared-state history provenance is invalid"
            );
        }
        let mut ranks = self
            .snapshot
            .tables
            .changes
            .iter()
            .map(|row| row.server_seq.context("shared history rank is missing"))
            .collect::<Result<Vec<_>>>()?;
        ranks.sort_unstable();
        let expected_ranks = (1..=i64::try_from(ranks.len())?).collect::<Vec<_>>();
        ensure!(
            ranks == expected_ranks,
            "error invalid-shared-state history ranks are not dense"
        );
        Ok(())
    }
}

pub(crate) async fn ensure_no_active_local_shared_capture(
    conn: &mut sqlx::SqliteConnection,
) -> Result<()> {
    adoption::ensure_unbound(conn).await?;
    let active: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM local_shared_capture_journal WHERE singleton = 1)",
    )
    .fetch_one(&mut *conn)
    .await?;
    ensure!(
        !active,
        "error local-shared-capture-active hint=cancel-never-dispatched-capture-first"
    );
    Ok(())
}

pub(crate) async fn ensure_changes_not_local_capture_protected(
    conn: &mut sqlx::SqliteConnection,
    change_ids: &[&str],
) -> Result<()> {
    if change_ids.is_empty() {
        return Ok(());
    }
    let protected: bool = sqlx::query_scalar(
        "SELECT EXISTS(
             SELECT 1 FROM local_shared_capture_changes
             WHERE change_id IN (SELECT value FROM json_each(?))
         )",
    )
    .bind(serde_json::to_string(change_ids)?)
    .fetch_one(&mut *conn)
    .await?;
    ensure!(
        !protected,
        "error local-shared-capture-protected-history hint=cancel-never-dispatched-capture-first"
    );
    Ok(())
}

async fn load_persisted_local_capture(
    conn: &mut sqlx::SqliteConnection,
) -> Result<Option<NeverDispatchedLocalSharedCapture>> {
    let row: Option<(String, String, String, String, i64)> = sqlx::query_as(
        "SELECT candidate_id, stream_id, state, internal_format, internal_version
         FROM local_shared_capture_journal WHERE singleton = 1",
    )
    .fetch_optional(&mut *conn)
    .await?;
    let Some((candidate_id, stream_id, state, internal_format, internal_version)) = row else {
        return Ok(None);
    };
    ensure!(
        state == LOCAL_CAPTURE_STATE
            && internal_format == LOCAL_CAPTURE_FORMAT
            && internal_version == LOCAL_CAPTURE_VERSION,
        "error local-shared-capture-unsupported"
    );
    let snapshot_json: String = sqlx::query_scalar(
        "SELECT snapshot_json FROM local_shared_capture_journal WHERE singleton = 1",
    )
    .fetch_one(&mut *conn)
    .await?;
    let persisted: PersistedLocalCapture =
        serde_json::from_str(&snapshot_json).context("error local-shared-capture-malformed")?;
    ensure!(
        persisted.format == internal_format
            && persisted.version == internal_version
            && persisted.candidate_id == candidate_id
            && persisted.stream_id == stream_id,
        "error local-shared-capture-encoding-mismatch"
    );
    let capture = SharedStateCapture {
        snapshot: persisted.snapshot,
    };
    capture.validate()?;
    validate_persisted_local_capture(conn, &candidate_id, &persisted.images, &capture.snapshot)
        .await?;
    Ok(Some(NeverDispatchedLocalSharedCapture {
        candidate_id,
        stream_id,
        capture,
        images: persisted.images,
    }))
}

async fn validate_persisted_local_capture(
    conn: &mut sqlx::SqliteConnection,
    candidate_id: &str,
    persisted_images: &[PersistedCaptureImage],
    snapshot: &AvenExport,
) -> Result<()> {
    let stored: Vec<(String, i64, Option<i64>, Option<i64>)> = sqlx::query_as(
        "SELECT change_id, prefix_rank, source_server_seq, source_pending_rank
         FROM local_shared_capture_changes
         WHERE candidate_id = ? ORDER BY prefix_rank",
    )
    .bind(candidate_id)
    .fetch_all(&mut *conn)
    .await?;
    let provenance = snapshot
        .tables
        .shared_history_provenance
        .iter()
        .map(|row| (row.change_id.as_str(), row))
        .collect::<HashMap<_, _>>();
    let mut expected = snapshot
        .tables
        .changes
        .iter()
        .map(|change| {
            let source = provenance
                .get(change.change_id.as_str())
                .context("shared history provenance is missing")?;
            Ok((
                change.change_id.clone(),
                change
                    .server_seq
                    .context("shared history rank is missing")?,
                source.source_server_seq,
                source.source_pending_rank,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    expected.sort_by_key(|row| row.1);
    ensure!(
        stored == expected,
        "error local-shared-capture-history-mismatch"
    );

    let invalid_ownership: bool = sqlx::query_scalar(
        "SELECT EXISTS(
             SELECT 1 FROM local_shared_capture_images image
             LEFT JOIN local_shared_capture_pins pin
               ON pin.candidate_id = image.candidate_id AND pin.sha256 = image.sha256
             WHERE image.candidate_id = ? AND (
                 (image.classification = 'unavailable' AND pin.sha256 IS NOT NULL)
                 OR (image.classification != 'unavailable' AND pin.sha256 IS NULL)
             )
         )",
    )
    .bind(candidate_id)
    .fetch_one(&mut *conn)
    .await?;
    ensure!(
        !invalid_ownership,
        "error local-shared-capture-image-ownership-mismatch"
    );
    let stored_images: Vec<(String, String)> = sqlx::query_as(
        "SELECT sha256, classification FROM local_shared_capture_images
         WHERE candidate_id = ? ORDER BY sha256",
    )
    .bind(candidate_id)
    .fetch_all(&mut *conn)
    .await?;
    let mut encoded_images = persisted_images
        .iter()
        .map(|image| (image.sha256.clone(), image.classification.clone()))
        .collect::<Vec<_>>();
    encoded_images.sort();
    let inventory_hashes = snapshot
        .tables
        .blob_inventory
        .iter()
        .map(|row| row.sha256.as_str())
        .collect::<HashSet<_>>();
    ensure!(
        stored_images == encoded_images
            && encoded_images.len() == inventory_hashes.len()
            && encoded_images
                .iter()
                .all(|(sha256, _)| inventory_hashes.contains(sha256.as_str())),
        "error local-shared-capture-image-set-mismatch"
    );
    Ok(())
}

async fn classify_and_validate_images(
    tables: &crate::data_safety::export_types::ExportTables,
    blob_dir: &Path,
) -> Result<Vec<(String, &'static str)>> {
    let deleted_tasks = tables
        .tasks
        .iter()
        .map(|task| (task.id.as_str(), task.deleted != 0))
        .collect::<HashMap<_, _>>();
    let current_hashes = tables
        .task_attachments
        .iter()
        .filter(|attachment| {
            attachment.deleted == 0
                && !deleted_tasks
                    .get(attachment.task_id.as_str())
                    .copied()
                    .unwrap_or(true)
        })
        .map(|attachment| attachment.sha256.as_str())
        .collect::<HashSet<_>>();
    let mut result = Vec::with_capacity(tables.blob_inventory.len());
    for inventory in &tables.blob_inventory {
        if inventory.available == 0 {
            ensure!(
                !current_hashes.contains(inventory.sha256.as_str()),
                "error local-shared-capture-required-image-unavailable sha256={} hint=\"complete image download or remove the live attachment before capture\"",
                inventory.sha256
            );
            ensure!(
                unavailable_image_has_validated_history(tables, &inventory.sha256)?,
                "error local-shared-capture-unavailable-image-provenance-missing sha256={} hint=\"restore the image or explicitly delete its attachment before capture\"",
                inventory.sha256
            );
            result.push((inventory.sha256.clone(), "unavailable"));
            continue;
        }
        ensure!(
            inventory.available == 1,
            "error local-shared-capture-image-availability-invalid"
        );
        let path = crate::attachments::storage::object_path(blob_dir, &inventory.sha256)?;
        let bytes =
            crate::attachments::blocking::run(move || Ok::<_, anyhow::Error>(std::fs::read(path)?))
                .await
                .context("error local-shared-capture-selected-image-missing")?;
        ensure!(
            crate::attachments::storage::sha256_hex(&bytes) == inventory.sha256,
            "error local-shared-capture-selected-image-hash-mismatch"
        );
        ensure!(
            i64::try_from(bytes.len())? == inventory.byte_size,
            "error local-shared-capture-selected-image-size-mismatch"
        );
        let media_type = inventory.media_type.clone();
        let facts = crate::attachments::blocking::run(move || {
            crate::attachments::decode::validate_image_blocking(bytes, Some(&media_type))
        })
        .await
        .context("error local-shared-capture-selected-image-invalid")?
        .facts;
        for attachment in tables
            .task_attachments
            .iter()
            .filter(|attachment| attachment.sha256 == inventory.sha256)
        {
            ensure!(
                attachment.media_type == inventory.media_type
                    && attachment.byte_size == inventory.byte_size
                    && attachment.width == Some(facts.width)
                    && attachment.height == Some(facts.height),
                "error local-shared-capture-selected-image-metadata-mismatch"
            );
        }
        result.push((
            inventory.sha256.clone(),
            if current_hashes.contains(inventory.sha256.as_str()) {
                "current_selected"
            } else {
                "extra_selected"
            },
        ));
    }
    Ok(result)
}

fn random_cryptographic_id() -> Result<String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).context("error local-shared-capture-rng")?;
    Ok(hex::encode(bytes))
}

fn unavailable_image_has_validated_history(
    tables: &crate::data_safety::export_types::ExportTables,
    sha256: &str,
) -> Result<bool> {
    let attachments = tables
        .task_attachments
        .iter()
        .filter(|attachment| attachment.sha256 == sha256)
        .collect::<Vec<_>>();
    if attachments.is_empty() {
        return Ok(true);
    }
    let changes = tables
        .changes
        .iter()
        .map(|change| (change.change_id.as_str(), change))
        .collect::<HashMap<_, _>>();
    for attachment in attachments {
        if attachment.deleted != 1 {
            return Ok(false);
        }
        let Some(change_id) = attachment.deleted_by_change_id.as_deref() else {
            return Ok(false);
        };
        let Some(change) = changes.get(change_id) else {
            return Ok(false);
        };
        let payload: serde_json::Value = serde_json::from_str(&change.payload)?;
        if change.entity_type != "task"
            || change.entity_id != attachment.task_id.as_str()
            || change.field.as_deref() != Some("attachments")
            || change.op_type != crate::change_log::op_type::ATTACHMENT_DELETE
            || payload
                .get("workspace_id")
                .and_then(serde_json::Value::as_str)
                != Some(attachment.workspace_id.as_str())
            || payload
                .get("attachment_id")
                .and_then(serde_json::Value::as_str)
                != Some(attachment.attachment_id.as_str())
        {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(crate) async fn ensure_empty_target(conn: &mut sqlx::SqliteConnection) -> Result<()> {
    adoption::ensure_unbound(conn).await?;
    ensure_empty_domain(conn).await
}

async fn ensure_empty_domain(conn: &mut sqlx::SqliteConnection) -> Result<()> {
    let occupied: i64 = sqlx::query_scalar(
        "SELECT
             (SELECT count(*) FROM workspaces WHERE id != '0000000000000000')
           + (SELECT count(*) FROM projects)
           + (SELECT count(*) FROM project_paths)
           + (SELECT count(*) FROM project_id_aliases)
           + (SELECT count(*) FROM labels)
           + (SELECT count(*) FROM metadata_fields)
           + (SELECT count(*) FROM metadata_field_id_aliases)
           + (SELECT count(*) FROM tasks)
           + (SELECT count(*) FROM task_metadata)
           + (SELECT count(*) FROM task_labels)
           + (SELECT count(*) FROM notes)
           + (SELECT count(*) FROM task_dependencies)
           + (SELECT count(*) FROM task_epic_links)
           + (SELECT count(*) FROM task_related_links)
           + (SELECT count(*) FROM task_attachments)
           + (SELECT count(*) FROM blob_inventory)
           + (SELECT count(*) FROM recurrence_series)
           + (SELECT count(*) FROM recurrence_series_labels)
           + (SELECT count(*) FROM recurrence_series_metadata)
           + (SELECT count(*) FROM recurrence_occurrences)
           + (SELECT count(*) FROM recurrence_pause_intervals)
           + (SELECT count(*) FROM changes)
           + (SELECT count(*) FROM field_versions)
           + (SELECT count(*) FROM conflicts)
           + (SELECT count(*) FROM shared_history_provenance)
           + (SELECT count(*) FROM local_shared_capture_journal)",
    )
    .fetch_one(conn)
    .await?;
    ensure!(
        occupied == 0,
        "error shared-state-install target database is not empty"
    );
    Ok(())
}

#[cfg(test)]
#[path = "shared_state/tests.rs"]
mod tests;
