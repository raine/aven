use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use crate::db::{self, Database};

mod archive;
pub(crate) mod export_types;
mod import;
mod integrity;
mod scan;
pub(crate) mod tables;
pub(crate) mod validation;

pub use export_types::{
    AvenExport, BlobInventoryExportRow, ChangeRow, ConflictRow, ExportTables, FieldVersionRow,
    LabelRow, MetaRow, MetadataFieldIdAliasRow, MetadataFieldRow, NoteRow, ProjectIdAliasRow,
    ProjectPathRow, ProjectRow, RecurrenceOccurrenceRow, RecurrencePauseIntervalRow,
    RecurrenceSeriesLabelRow, RecurrenceSeriesMetadataRow, RecurrenceSeriesRow,
    SharedHistoryProvenanceRow, TaskAttachmentRow, TaskDependencyRow, TaskEpicLinkRow,
    TaskLabelRow, TaskMetadataRow, TaskRelatedLinkRow, TaskRow, WorkspaceRow,
};
use export_types::{EXPORT_FORMAT, EXPORT_VERSION};

#[derive(Debug, Clone)]
pub struct IntegrityReport {
    pub quick_check_ok: bool,
    pub quick_check_value: String,
    pub checks: Vec<IntegrityCheck>,
}

#[derive(Debug, Clone)]
pub struct IntegrityCheck {
    pub label: &'static str,
    pub ok: bool,
    pub value: String,
}

pub(crate) fn ensure_integrity_ok(report: &IntegrityReport) -> Result<()> {
    integrity::ensure_ok(report)
}

pub(crate) async fn integrity_report_in_transaction(
    conn: &mut sqlx::SqliteConnection,
) -> Result<IntegrityReport> {
    integrity::database_report(conn).await
}

pub(crate) async fn scan_export_tables(conn: &mut sqlx::SqliteConnection) -> Result<ExportTables> {
    Ok(ExportTables {
        workspaces: scan::scan_workspaces(conn).await?,
        projects: scan::scan_projects(conn).await?,
        project_paths: scan::scan_project_paths(conn).await?,
        project_id_aliases: scan::scan_project_id_aliases(conn).await?,
        labels: scan::scan_labels(conn).await?,
        metadata_fields: scan::scan_metadata_fields(conn).await?,
        metadata_field_id_aliases: scan::scan_metadata_field_id_aliases(conn).await?,
        tasks: scan::scan_tasks(conn).await?,
        task_metadata: scan::scan_task_metadata(conn).await?,
        task_labels: scan::scan_task_labels(conn).await?,
        notes: scan::scan_notes(conn).await?,
        task_dependencies: scan::scan_task_dependencies(conn).await?,
        task_epic_links: scan::scan_task_epic_links(conn).await?,
        task_related_links: scan::scan_task_related_links(conn).await?,
        task_attachments: scan::scan_task_attachments(conn).await?,
        blob_inventory: scan::scan_blob_inventory(conn).await?,
        recurrence_series: scan::scan_recurrence_series(conn).await?,
        recurrence_series_labels: scan::scan_recurrence_series_labels(conn).await?,
        recurrence_series_metadata: scan::scan_recurrence_series_metadata(conn).await?,
        recurrence_occurrences: scan::scan_recurrence_occurrences(conn).await?,
        recurrence_pause_intervals: scan::scan_recurrence_pause_intervals(conn).await?,
        changes: scan::scan_changes(conn).await?,
        shared_history_provenance: scan::scan_shared_history_provenance(conn).await?,
        field_versions: scan::scan_field_versions(conn).await?,
        conflicts: scan::scan_conflicts(conn).await?,
        meta: scan::scan_meta(conn).await?,
    })
}

impl Database {
    pub async fn export_data(&self, exported_at: String) -> Result<AvenExport> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = db::begin_immediate(&mut conn).await?;
        let schema_version = db::current_schema_version(&mut tx).await?;
        let mut tables = scan_export_tables(&mut tx).await?;
        let bound: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM local_seed_source)")
            .fetch_one(&mut *tx)
            .await?;
        if bound {
            tables.meta.retain(|m| {
                m.key != "sync_server_url"
                    && m.key != "e2ee_association"
                    && m.key != "e2ee_data_only"
            });
            tables.meta.push(MetaRow {
                key: "e2ee_data_only".into(),
                value: "1".into(),
            });
        }
        let version = if !tables.shared_history_provenance.is_empty() {
            EXPORT_VERSION
        } else if tables.task_related_links.is_empty() {
            2
        } else {
            export_types::RELATED_LINKS_EXPORT_VERSION
        };
        tx.commit().await?;
        Ok(AvenExport {
            format: EXPORT_FORMAT.to_string(),
            version,
            exported_at,
            schema_version,
            blobs_included: false,
            tables,
        })
    }

    pub async fn validate_import_data(&self, export: &AvenExport) -> Result<()> {
        let mut conn = self.acquire_reader().await?;
        validation::ensure_supported_export(&mut conn, export).await?;
        validation::validate_export_snapshot(export)
    }

    pub async fn import_data(&self, export: &AvenExport) -> Result<IntegrityReport> {
        let _installation = self.plaintext_installation_guard()?;
        let mut conn = self.acquire_writer().await?;
        validation::ensure_supported_export(&mut conn, export).await?;
        validation::validate_export_snapshot(export)?;
        let target_client_id = db::get_meta(&mut conn, "client_id")
            .await?
            .context("missing target client_id")?;
        let mut tx = db::begin_immediate(&mut conn).await?;
        crate::sync::shared_state::ensure_no_active_local_shared_capture(&mut tx).await?;
        import::replace_from_export(&mut tx, export, &target_client_id).await?;
        let report = integrity::database_report(&mut tx).await?;
        ensure_integrity_ok(&report)?;
        tx.commit().await?;
        Ok(report)
    }

    pub async fn database_integrity_report(&self) -> Result<IntegrityReport> {
        let mut conn = self.acquire_reader().await?;
        integrity::database_report(&mut conn).await
    }

    pub async fn attachment_integrity_checks(
        &self,
        blob_dir: &Path,
        deep: bool,
    ) -> Result<Vec<IntegrityCheck>> {
        let mut conn = self.acquire_reader().await?;
        integrity::attachment_integrity_checks(&mut conn, blob_dir, deep).await
    }

    pub async fn create_backup_archive(&self, blob_dir: &Path, output: &Path) -> Result<()> {
        let _installation = self.plaintext_installation_guard()?;
        let mut conn = self.acquire_writer().await?;
        crate::sync::shared_state::ensure_no_active_local_shared_capture(&mut conn).await?;
        let hashes: Vec<String> = sqlx::query_scalar(
            "SELECT sha256 FROM blob_inventory WHERE available = 1 ORDER BY sha256",
        )
        .fetch_all(&mut *conn)
        .await?;
        let mut leases = Vec::with_capacity(hashes.len());
        for hash in hashes {
            match crate::attachments::lifecycle::acquire_lease(
                &mut conn,
                &hash,
                "backup",
                &crate::attachments::lifecycle::SystemClock,
            )
            .await
            {
                Ok(lease) => leases.push(lease),
                Err(error) => {
                    for lease in leases {
                        let _ =
                            crate::attachments::lifecycle::release_lease(&mut conn, &lease).await;
                    }
                    return Err(error);
                }
            }
        }
        let backup_result = archive::create_backup_archive(&mut conn, blob_dir, output).await;
        for lease in leases {
            crate::attachments::lifecycle::release_lease(&mut conn, &lease).await?;
        }
        backup_result
    }
}

pub fn is_backup_archive(path: &Path) -> Result<bool> {
    archive::is_archive_path(path)
}

pub async fn restore_backup_archive(
    db_path: &Path,
    blob_dir: &Path,
    source: &Path,
) -> Result<PathBuf> {
    archive::restore_backup_archive(db_path, blob_dir, source).await
}
