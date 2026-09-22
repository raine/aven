use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, ensure};
use sqlx::FromRow;

use crate::data_safety::export_types::{AvenExport, EXPORT_FORMAT, EXPORT_VERSION};
use crate::data_safety::{self, tables, validation};
use crate::db::{self, Database};

/// A consistent, installation-ready copy of shared domain state and retained history.
///
/// This value deliberately has no serialized wire representation. Encryption and
/// publication layers can package it later without making this local interchange
/// type a protocol contract.
#[derive(Debug)]
pub struct SharedStateCapture {
    snapshot: AvenExport,
    provenance: Vec<HistoryProvenance>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HistoryProvenance {
    change_id: String,
    source_server_seq: Option<i64>,
    source_pending_rank: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SharedStateInstallReport {
    pub prefix_count: u64,
    pub attachment_count: u64,
}

#[derive(FromRow)]
struct StoredProvenance {
    change_id: String,
    source_server_seq: Option<i64>,
    source_pending_rank: Option<i64>,
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
        let mut tables = data_safety::scan_export_tables(&mut tx).await?;
        let stored = sqlx::query_as::<_, StoredProvenance>(
            "SELECT change_id, source_server_seq, source_pending_rank
             FROM shared_history_provenance",
        )
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .map(|row| (row.change_id.clone(), row))
        .collect::<HashMap<_, _>>();
        tx.commit().await?;

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
                HistoryProvenance {
                    change_id: row.change_id.clone(),
                    source_server_seq: stored.source_server_seq,
                    source_pending_rank: stored.source_pending_rank,
                }
            } else if let Some(source_server_seq) = row.server_seq {
                HistoryProvenance {
                    change_id: row.change_id.clone(),
                    source_server_seq: Some(source_server_seq),
                    source_pending_rank: None,
                }
            } else {
                let pending_rank = pending_ranks
                    .get(&index)
                    .context("pending history order is incomplete")?;
                HistoryProvenance {
                    change_id: row.change_id.clone(),
                    source_server_seq: None,
                    source_pending_rank: Some(i64::try_from(*pending_rank)?),
                }
            };
            row.server_seq = Some(i64::try_from(position + 1)?);
            provenance.push(source);
        }

        let capture = SharedStateCapture {
            snapshot: AvenExport {
                format: EXPORT_FORMAT.to_string(),
                version: EXPORT_VERSION,
                exported_at: crate::ids::now(),
                schema_version,
                blobs_included: false,
                tables,
            },
            provenance,
        };
        capture.validate()?;
        Ok(capture)
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
        let mut conn = self.acquire_writer().await?;
        let mut tx = db::begin_immediate(&mut conn).await?;
        ensure_empty_target(&mut tx).await?;

        let identity = db::get_meta(&mut tx, "client_id")
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
            .execute(&mut *tx)
            .await?;
        let t = &capture.snapshot.tables;
        tables::import_workspaces(&mut tx, &t.workspaces).await?;
        tables::import_projects(&mut tx, &t.projects).await?;
        tables::import_project_id_aliases(&mut tx, &t.project_id_aliases).await?;
        tables::import_labels(&mut tx, &t.labels).await?;
        tables::import_metadata_fields(&mut tx, &t.metadata_fields).await?;
        tables::import_metadata_field_id_aliases(&mut tx, &t.metadata_field_id_aliases).await?;
        tables::import_tasks(&mut tx, &t.tasks).await?;
        tables::import_task_metadata(&mut tx, &t.task_metadata).await?;
        tables::import_task_labels(&mut tx, &t.task_labels).await?;
        tables::import_notes(&mut tx, &t.notes).await?;
        tables::import_task_dependencies(&mut tx, &t.task_dependencies).await?;
        tables::import_task_epic_links(&mut tx, &t.task_epic_links).await?;
        tables::import_blob_inventory(&mut tx, &t.blob_inventory).await?;
        tables::import_task_attachments(&mut tx, &t.task_attachments).await?;
        tables::import_recurrence_series(&mut tx, &t.recurrence_series).await?;
        tables::import_recurrence_series_labels(&mut tx, &t.recurrence_series_labels).await?;
        tables::import_recurrence_series_metadata(&mut tx, &t.recurrence_series_metadata).await?;
        tables::import_recurrence_occurrences(&mut tx, &t.recurrence_occurrences).await?;
        tables::import_recurrence_pause_intervals(&mut tx, &t.recurrence_pause_intervals).await?;
        tables::import_changes(&mut tx, &t.changes).await?;
        tables::import_task_related_links(&mut tx, &t.task_related_links).await?;
        tables::import_field_versions(&mut tx, &t.field_versions).await?;
        tables::import_conflicts(&mut tx, &t.conflicts).await?;
        for row in &capture.provenance {
            sqlx::query(
                "INSERT INTO shared_history_provenance(
                     change_id, source_server_seq, source_pending_rank
                 ) VALUES (?, ?, ?)",
            )
            .bind(&row.change_id)
            .bind(row.source_server_seq)
            .bind(row.source_pending_rank)
            .execute(&mut *tx)
            .await?;
        }
        for row in &t.meta {
            db::set_meta(&mut tx, &row.key, &row.value).await?;
        }
        let local_seq = t.changes.iter().map(|row| row.local_seq).max().unwrap_or(0);
        db::set_meta(&mut tx, "local_seq", &local_seq.to_string()).await?;
        data_safety::ensure_integrity_ok(
            &crate::data_safety::integrity_report_in_transaction(&mut tx).await?,
        )?;
        tx.commit().await?;

        Ok(SharedStateInstallReport {
            prefix_count: u64::try_from(t.changes.len())?,
            attachment_count: u64::try_from(t.task_attachments.len())?,
        })
    }
}

impl SharedStateCapture {
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
            .provenance
            .iter()
            .map(|row| row.change_id.as_str())
            .collect::<HashSet<_>>();
        ensure!(
            change_ids.len() == self.snapshot.tables.changes.len()
                && provenance_ids.len() == self.provenance.len()
                && change_ids == provenance_ids,
            "error invalid-shared-state history provenance does not match retained history"
        );
        for row in &self.provenance {
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

async fn ensure_empty_target(conn: &mut sqlx::SqliteConnection) -> Result<()> {
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
           + (SELECT count(*) FROM shared_history_provenance)",
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
