use crate::choices::{TaskPriority, TaskSource, TaskStatus};
use crate::ids::{MetadataFieldId, ProjectId, TaskId, WorkspaceId};
use crate::recurrence::{
    RecurrenceDuePolicy, RecurrenceFrequency, RecurrenceOutcome, RecurrenceProjectionState,
    RecurrenceRule, RecurrenceSchedule, RecurrenceSeriesId, RecurrenceSeriesState, TimeZoneId,
    WeekdaySet, derive_occurrence_identity, is_slot, slot_values,
};
use crate::task_fields::TaskField;
use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, NaiveDate, NaiveTime};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{SqliteConnection, query_scalar};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

mod archive;
mod integrity;
mod tables;
use crate::db::{self, Database};

const EXPORT_FORMAT: &str = "aven-export";
const EXPORT_VERSION: i64 = 2;

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

#[derive(Debug, Serialize, Deserialize)]
pub struct AvenExport {
    pub format: String,
    pub version: i64,
    pub exported_at: String,
    pub schema_version: i64,
    #[serde(default)]
    pub blobs_included: bool,
    pub tables: ExportTables,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExportTables {
    pub workspaces: Vec<WorkspaceRow>,
    pub projects: Vec<ProjectRow>,
    pub project_paths: Vec<ProjectPathRow>,
    pub project_id_aliases: Vec<ProjectIdAliasRow>,
    pub labels: Vec<LabelRow>,
    #[serde(default)]
    pub metadata_fields: Vec<MetadataFieldRow>,
    #[serde(default)]
    pub metadata_field_id_aliases: Vec<MetadataFieldIdAliasRow>,
    pub tasks: Vec<TaskRow>,
    #[serde(default)]
    pub task_metadata: Vec<TaskMetadataRow>,
    pub task_labels: Vec<TaskLabelRow>,
    pub notes: Vec<NoteRow>,
    pub task_dependencies: Vec<TaskDependencyRow>,
    pub task_epic_links: Vec<TaskEpicLinkRow>,
    #[serde(default)]
    pub task_attachments: Vec<TaskAttachmentRow>,
    #[serde(default)]
    pub blob_inventory: Vec<BlobInventoryExportRow>,
    #[serde(default)]
    pub recurrence_series: Vec<RecurrenceSeriesRow>,
    #[serde(default)]
    pub recurrence_series_labels: Vec<RecurrenceSeriesLabelRow>,
    #[serde(default)]
    pub recurrence_series_metadata: Vec<RecurrenceSeriesMetadataRow>,
    #[serde(default)]
    pub recurrence_occurrences: Vec<RecurrenceOccurrenceRow>,
    #[serde(default)]
    pub recurrence_pause_intervals: Vec<RecurrencePauseIntervalRow>,
    pub changes: Vec<ChangeRow>,
    pub field_versions: Vec<FieldVersionRow>,
    pub conflicts: Vec<ConflictRow>,
    pub meta: Vec<MetaRow>,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct WorkspaceRow {
    pub id: WorkspaceId,
    pub name: String,
    pub key: String,
    pub created_at: String,
    pub updated_at: String,
    pub archived: i64,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct ProjectRow {
    pub id: ProjectId,
    pub workspace_id: WorkspaceId,
    pub key: String,
    pub name: String,
    pub prefix: String,
    pub created_at: String,
    pub updated_at: String,
    pub deleted: i64,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct ProjectPathRow {
    pub workspace_id: WorkspaceId,
    pub project_id: ProjectId,
    pub path: String,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct ProjectIdAliasRow {
    pub workspace_id: WorkspaceId,
    pub remote_project_id: ProjectId,
    pub local_project_id: ProjectId,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct LabelRow {
    pub workspace_id: WorkspaceId,
    pub name: String,
    pub created_at: String,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct MetadataFieldRow {
    pub id: MetadataFieldId,
    pub workspace_id: WorkspaceId,
    pub key: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct MetadataFieldIdAliasRow {
    pub workspace_id: WorkspaceId,
    pub remote_field_id: MetadataFieldId,
    pub local_field_id: MetadataFieldId,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct TaskMetadataRow {
    pub workspace_id: WorkspaceId,
    pub task_id: TaskId,
    pub field_id: MetadataFieldId,
    pub value: String,
    pub created_at: String,
    pub updated_at: String,
}

fn default_task_source() -> String {
    TaskSource::Unknown.as_str().to_string()
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct TaskRow {
    pub workspace_id: WorkspaceId,
    pub id: TaskId,
    pub title: String,
    pub description: String,
    pub project_id: ProjectId,
    pub status: String,
    pub priority: String,
    #[serde(default = "default_task_source")]
    pub source: String,
    pub created_at: String,
    pub updated_at: String,
    pub queue_activity_at: String,
    #[serde(default)]
    pub available_at: String,
    #[serde(default)]
    pub due_on: String,
    pub deleted: i64,
    pub is_epic: i64,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct TaskEpicLinkRow {
    pub workspace_id: WorkspaceId,
    pub child_task_id: TaskId,
    pub epic_task_id: TaskId,
    pub created_at: String,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct TaskLabelRow {
    pub workspace_id: WorkspaceId,
    pub task_id: TaskId,
    pub label: String,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct NoteRow {
    pub workspace_id: WorkspaceId,
    pub id: String,
    pub task_id: TaskId,
    pub body: String,
    pub created_at: String,
    pub change_id: String,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct TaskDependencyRow {
    pub workspace_id: WorkspaceId,
    pub task_id: TaskId,
    pub depends_on_task_id: TaskId,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct TaskAttachmentRow {
    pub workspace_id: WorkspaceId,
    pub attachment_id: String,
    pub task_id: TaskId,
    pub sha256: String,
    pub byte_size: i64,
    pub media_type: String,
    pub filename: Option<String>,
    pub alt_text: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub created_at: String,
    pub created_by_change_id: Option<String>,
    pub deleted: i64,
    pub deleted_at: Option<String>,
    pub deleted_by_change_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct BlobInventoryExportRow {
    pub sha256: String,
    pub byte_size: i64,
    pub media_type: String,
    pub available: i64,
    pub first_seen_at: String,
    pub last_verified_at: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct RecurrenceSeriesRow {
    pub workspace_id: WorkspaceId,
    pub id: RecurrenceSeriesId,
    pub title: String,
    pub description: String,
    pub project_id: ProjectId,
    pub priority: String,
    pub initial_status: String,
    pub frequency: String,
    pub interval: i64,
    pub weekdays: String,
    pub timezone: String,
    pub start_on: String,
    pub available_local_time: String,
    pub due_policy: String,
    pub state: String,
    pub stopped_at: String,
    pub created_at: String,
    pub updated_at: String,
    pub deleted: i64,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct RecurrenceSeriesLabelRow {
    pub workspace_id: WorkspaceId,
    pub series_id: RecurrenceSeriesId,
    pub label: String,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct RecurrenceSeriesMetadataRow {
    pub workspace_id: WorkspaceId,
    pub series_id: RecurrenceSeriesId,
    pub field_id: MetadataFieldId,
    pub value: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct RecurrenceOccurrenceRow {
    pub workspace_id: WorkspaceId,
    pub series_id: RecurrenceSeriesId,
    pub slot_on: String,
    pub task_id: String,
    pub outcome: String,
    pub resolved_at: String,
    pub outcome_change_id: String,
    pub projection_state: String,
    pub archived_at: String,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct RecurrencePauseIntervalRow {
    pub workspace_id: WorkspaceId,
    pub id: String,
    pub series_id: RecurrenceSeriesId,
    pub paused_at: String,
    pub resumed_at: String,
    pub suspended_slot_on: String,
    pub suspended_task_id: String,
    pub created_by_change_id: String,
    pub resolved_by_change_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ChangeRow {
    pub change_id: String,
    pub client_id: String,
    pub local_seq: i64,
    pub entity_type: String,
    pub entity_id: String,
    pub field: Option<String>,
    pub op_type: String,
    pub payload: String,
    pub base_version: Option<String>,
    pub created_at: String,
    pub server_seq: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct FieldVersionRow {
    pub workspace_id: WorkspaceId,
    pub entity_type: String,
    pub entity_id: String,
    pub field: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ConflictRow {
    pub id: i64,
    pub workspace_id: WorkspaceId,
    pub entity_type: String,
    pub entity_id: String,
    pub task_id: String,
    pub field: String,
    pub base_version: Option<String>,
    pub local_value: String,
    pub remote_value: String,
    pub local_change_id: Option<String>,
    pub remote_change_id: String,
    pub variant_a: String,
    pub variant_b: String,
    pub created_at: String,
    pub resolved: i64,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct MetaRow {
    pub key: String,
    pub value: String,
}

impl Database {
    pub async fn export_data(&self, exported_at: String) -> Result<AvenExport> {
        let mut conn = self.acquire_writer().await?;
        let schema_version = db::current_schema_version(&mut conn).await?;
        Ok(AvenExport {
            format: EXPORT_FORMAT.to_string(),
            version: EXPORT_VERSION,
            exported_at,
            schema_version,
            blobs_included: false,
            tables: ExportTables {
                workspaces: scan_workspaces(&mut conn).await?,
                projects: scan_projects(&mut conn).await?,
                project_paths: scan_project_paths(&mut conn).await?,
                project_id_aliases: scan_project_id_aliases(&mut conn).await?,
                labels: scan_labels(&mut conn).await?,
                metadata_fields: scan_metadata_fields(&mut conn).await?,
                metadata_field_id_aliases: scan_metadata_field_id_aliases(&mut conn).await?,
                tasks: scan_tasks(&mut conn).await?,
                task_metadata: scan_task_metadata(&mut conn).await?,
                task_labels: scan_task_labels(&mut conn).await?,
                notes: scan_notes(&mut conn).await?,
                task_dependencies: scan_task_dependencies(&mut conn).await?,
                task_epic_links: scan_task_epic_links(&mut conn).await?,
                task_attachments: scan_task_attachments(&mut conn).await?,
                blob_inventory: scan_blob_inventory(&mut conn).await?,
                recurrence_series: scan_recurrence_series(&mut conn).await?,
                recurrence_series_labels: scan_recurrence_series_labels(&mut conn).await?,
                recurrence_series_metadata: scan_recurrence_series_metadata(&mut conn).await?,
                recurrence_occurrences: scan_recurrence_occurrences(&mut conn).await?,
                recurrence_pause_intervals: scan_recurrence_pause_intervals(&mut conn).await?,
                changes: scan_changes(&mut conn).await?,
                field_versions: scan_field_versions(&mut conn).await?,
                conflicts: scan_conflicts(&mut conn).await?,
                meta: scan_meta(&mut conn).await?,
            },
        })
    }

    pub async fn validate_import_data(&self, export: &AvenExport) -> Result<()> {
        let mut conn = self.acquire_reader().await?;
        ensure_supported_export(&mut conn, export).await?;
        validate_export_snapshot(export)
    }

    pub async fn import_data(&self, export: &AvenExport) -> Result<IntegrityReport> {
        let mut conn = self.acquire_writer().await?;
        ensure_supported_export(&mut conn, export).await?;
        validate_export_snapshot(export)?;
        let target_client_id = db::get_meta(&mut conn, "client_id")
            .await?
            .context("missing target client_id")?;
        let mut tx = db::begin_immediate(&mut conn).await?;
        replace_from_export(&mut tx, export, &target_client_id).await?;
        let report = database_integrity_report_with_connection(&mut tx).await?;
        ensure_integrity_ok(&report)?;
        tx.commit().await?;
        Ok(report)
    }

    pub async fn database_integrity_report(&self) -> Result<IntegrityReport> {
        let mut conn = self.acquire_reader().await?;
        database_integrity_report_with_connection(&mut conn).await
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
        let mut conn = self.acquire_writer().await?;
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

async fn scan_workspaces(conn: &mut SqliteConnection) -> Result<Vec<WorkspaceRow>> {
    tables::scan_rows(
        conn,
        "SELECT id, name, key, created_at, updated_at, archived FROM workspaces",
    )
    .await
}

async fn scan_projects(conn: &mut SqliteConnection) -> Result<Vec<ProjectRow>> {
    tables::scan_rows(
        conn,
        "SELECT id, workspace_id, key, name, prefix, created_at, updated_at, deleted FROM projects",
    )
    .await
}

async fn scan_project_paths(conn: &mut SqliteConnection) -> Result<Vec<ProjectPathRow>> {
    tables::scan_rows(
        conn,
        "SELECT workspace_id, project_id, path FROM project_paths",
    )
    .await
}

async fn scan_project_id_aliases(conn: &mut SqliteConnection) -> Result<Vec<ProjectIdAliasRow>> {
    tables::scan_rows(
        conn,
        "SELECT workspace_id, remote_project_id, local_project_id FROM project_id_aliases",
    )
    .await
}

async fn scan_labels(conn: &mut SqliteConnection) -> Result<Vec<LabelRow>> {
    tables::scan_rows(conn, "SELECT workspace_id, name, created_at FROM labels").await
}

async fn scan_metadata_fields(conn: &mut SqliteConnection) -> Result<Vec<MetadataFieldRow>> {
    tables::scan_rows(
        conn,
        "SELECT id, workspace_id, key, created_at, updated_at FROM metadata_fields",
    )
    .await
}

async fn scan_metadata_field_id_aliases(
    conn: &mut SqliteConnection,
) -> Result<Vec<MetadataFieldIdAliasRow>> {
    tables::scan_rows(
        conn,
        "SELECT workspace_id, remote_field_id, local_field_id FROM metadata_field_id_aliases",
    )
    .await
}

async fn scan_task_metadata(conn: &mut SqliteConnection) -> Result<Vec<TaskMetadataRow>> {
    tables::scan_rows(
        conn,
        "SELECT workspace_id, task_id, field_id, value, created_at, updated_at FROM task_metadata",
    )
    .await
}

async fn scan_tasks(conn: &mut SqliteConnection) -> Result<Vec<TaskRow>> {
    tables::scan_rows(conn, "SELECT workspace_id, id, title, description, project_id, status, priority, source, created_at, updated_at, queue_activity_at, available_at, due_on, deleted, is_epic FROM tasks").await
}

async fn scan_task_labels(conn: &mut SqliteConnection) -> Result<Vec<TaskLabelRow>> {
    tables::scan_rows(conn, "SELECT workspace_id, task_id, label FROM task_labels").await
}

async fn scan_notes(conn: &mut SqliteConnection) -> Result<Vec<NoteRow>> {
    tables::scan_rows(
        conn,
        "SELECT workspace_id, id, task_id, body, created_at, change_id FROM notes",
    )
    .await
}

async fn scan_task_dependencies(conn: &mut SqliteConnection) -> Result<Vec<TaskDependencyRow>> {
    tables::scan_rows(
        conn,
        "SELECT workspace_id, task_id, depends_on_task_id, created_at FROM task_dependencies",
    )
    .await
}

async fn scan_task_epic_links(conn: &mut SqliteConnection) -> Result<Vec<TaskEpicLinkRow>> {
    tables::scan_rows(
        conn,
        "SELECT workspace_id, child_task_id, epic_task_id, created_at FROM task_epic_links",
    )
    .await
}

async fn scan_task_attachments(conn: &mut SqliteConnection) -> Result<Vec<TaskAttachmentRow>> {
    tables::scan_rows(
        conn,
        "SELECT workspace_id, attachment_id, task_id, sha256, byte_size, media_type, filename, alt_text, width, height, created_at, created_by_change_id, deleted, deleted_at, deleted_by_change_id FROM task_attachments",
    )
    .await
}

async fn scan_blob_inventory(conn: &mut SqliteConnection) -> Result<Vec<BlobInventoryExportRow>> {
    tables::scan_rows(
        conn,
        "SELECT sha256, byte_size, media_type, available, first_seen_at, last_verified_at FROM blob_inventory",
    )
    .await
}

async fn scan_recurrence_series(conn: &mut SqliteConnection) -> Result<Vec<RecurrenceSeriesRow>> {
    tables::scan_rows(
        conn,
        "SELECT workspace_id, id, title, description, project_id, priority, initial_status, frequency, interval, weekdays, timezone, start_on, available_local_time, due_policy, state, stopped_at, created_at, updated_at, deleted FROM recurrence_series",
    )
    .await
}

async fn scan_recurrence_series_labels(
    conn: &mut SqliteConnection,
) -> Result<Vec<RecurrenceSeriesLabelRow>> {
    tables::scan_rows(
        conn,
        "SELECT workspace_id, series_id, label FROM recurrence_series_labels",
    )
    .await
}

async fn scan_recurrence_series_metadata(
    conn: &mut SqliteConnection,
) -> Result<Vec<RecurrenceSeriesMetadataRow>> {
    tables::scan_rows(
        conn,
        "SELECT workspace_id, series_id, field_id, value, created_at, updated_at
         FROM recurrence_series_metadata",
    )
    .await
}

async fn scan_recurrence_occurrences(
    conn: &mut SqliteConnection,
) -> Result<Vec<RecurrenceOccurrenceRow>> {
    tables::scan_rows(
        conn,
        "SELECT workspace_id, series_id, slot_on, task_id, outcome, resolved_at, outcome_change_id, projection_state, archived_at FROM recurrence_occurrences",
    )
    .await
}

async fn scan_recurrence_pause_intervals(
    conn: &mut SqliteConnection,
) -> Result<Vec<RecurrencePauseIntervalRow>> {
    tables::scan_rows(
        conn,
        "SELECT workspace_id, id, series_id, paused_at, resumed_at, suspended_slot_on, suspended_task_id, created_by_change_id, resolved_by_change_id FROM recurrence_pause_intervals",
    )
    .await
}

async fn scan_changes(conn: &mut SqliteConnection) -> Result<Vec<ChangeRow>> {
    tables::scan_rows(conn, "SELECT change_id, client_id, local_seq, entity_type, entity_id, field, op_type, payload, base_version, created_at, server_seq FROM changes").await
}

async fn scan_field_versions(conn: &mut SqliteConnection) -> Result<Vec<FieldVersionRow>> {
    tables::scan_rows(
        conn,
        "SELECT workspace_id, entity_type, entity_id, field, version FROM field_versions",
    )
    .await
}

async fn scan_conflicts(conn: &mut SqliteConnection) -> Result<Vec<ConflictRow>> {
    tables::scan_rows(conn, "SELECT id, workspace_id, entity_type, entity_id, task_id, field, base_version, local_value, remote_value, local_change_id, remote_change_id, variant_a, variant_b, created_at, resolved FROM conflicts").await
}

async fn scan_meta(conn: &mut SqliteConnection) -> Result<Vec<MetaRow>> {
    tables::scan_rows(conn, "SELECT key, value FROM meta").await
}

async fn ensure_supported_export(_conn: &mut SqliteConnection, export: &AvenExport) -> Result<()> {
    if export.format != EXPORT_FORMAT {
        bail!("error export-format-unsupported format={}", export.format);
    }
    if !matches!(export.version, 1 | EXPORT_VERSION) {
        bail!(
            "error export-version-unsupported version={}",
            export.version
        );
    }
    Ok(())
}

fn validate_export_snapshot(export: &AvenExport) -> Result<()> {
    let mut workspace_ids = HashSet::new();
    for workspace in &export.tables.workspaces {
        if workspace_ids.contains(&workspace.id) {
            continue;
        }
        workspace_ids.insert(workspace.id.clone());
    }

    let mut project_ids: HashMap<WorkspaceId, HashSet<ProjectId>> = HashMap::new();
    for project in &export.tables.projects {
        if !workspace_ids.contains(&project.workspace_id) {
            bail!(
                "error invalid-export-snapshot project.workspace_id={} is missing",
                project.workspace_id
            );
        }
        project_ids
            .entry(project.workspace_id.clone())
            .or_default()
            .insert(project.id.clone());
    }

    for path in &export.tables.project_paths {
        let projects = project_ids.get(&path.workspace_id).ok_or_else(|| {
            anyhow::Error::msg(format!(
                "error invalid-export-snapshot project_path.workspace_id={} is missing",
                path.workspace_id
            ))
        })?;
        if !projects.contains(&path.project_id) {
            bail!(
                "error invalid-export-snapshot project_path.project_id={} is missing in workspace {}",
                path.project_id,
                path.workspace_id
            );
        }
    }

    let mut label_keys: HashSet<(WorkspaceId, String)> = HashSet::new();
    for label in &export.tables.labels {
        if !workspace_ids.contains(&label.workspace_id) {
            bail!(
                "error invalid-export-snapshot label.workspace_id={} is missing",
                label.workspace_id
            );
        }
        label_keys.insert((label.workspace_id.clone(), label.name.clone()));
    }

    let mut task_ids: HashMap<WorkspaceId, HashSet<TaskId>> = HashMap::new();
    for task in &export.tables.tasks {
        TaskSource::parse(&task.source)?;
        if let Err(error) = crate::time_validation::validate_due_on_value(&task.due_on) {
            bail!(
                "error invalid-export-snapshot task.due_on={} is invalid: {error}",
                task.due_on
            );
        }
        let workspace_projects = project_ids.get(&task.workspace_id).ok_or_else(|| {
            anyhow::Error::msg(format!(
                "error invalid-export-snapshot task.workspace_id={} is missing",
                task.workspace_id
            ))
        })?;
        if !workspace_projects.contains(&task.project_id) {
            bail!(
                "error invalid-export-snapshot task.project_id={} is missing in workspace {}",
                task.project_id,
                task.workspace_id
            );
        }
        task_ids
            .entry(task.workspace_id.clone())
            .or_default()
            .insert(task.id.clone());
    }

    for task_label in &export.tables.task_labels {
        let task_workspace = task_ids.get(&task_label.workspace_id).ok_or_else(|| {
            anyhow::Error::msg(format!(
                "error invalid-export-snapshot task_label.workspace_id={} is missing",
                task_label.workspace_id
            ))
        })?;
        if !task_workspace.contains(&task_label.task_id) {
            bail!(
                "error invalid-export-snapshot task_label.task_id={} is missing in workspace {}",
                task_label.task_id,
                task_label.workspace_id
            );
        }
        if !label_keys.contains(&(task_label.workspace_id.clone(), task_label.label.clone())) {
            bail!(
                "error invalid-export-snapshot task_label.label={} is missing in workspace {}",
                task_label.label,
                task_label.workspace_id
            );
        }
    }

    for note in &export.tables.notes {
        let task_workspace = task_ids.get(&note.workspace_id).ok_or_else(|| {
            anyhow::Error::msg(format!(
                "error invalid-export-snapshot note.workspace_id={} is missing",
                note.workspace_id
            ))
        })?;
        if !task_workspace.contains(&note.task_id) {
            bail!(
                "error invalid-export-snapshot note.task_id={} is missing in workspace {}",
                note.task_id,
                note.workspace_id
            );
        }
    }

    for dep in &export.tables.task_dependencies {
        let tasks = task_ids.get(&dep.workspace_id).ok_or_else(|| {
            anyhow::Error::msg(format!(
                "error invalid-export-snapshot dependency.workspace_id={} is missing",
                dep.workspace_id
            ))
        })?;
        if !tasks.contains(&dep.task_id) || !tasks.contains(&dep.depends_on_task_id) {
            bail!(
                "error invalid-export-snapshot task_dependencies are missing tasks in workspace {}",
                dep.workspace_id
            );
        }
    }

    for epic_link in &export.tables.task_epic_links {
        let tasks = task_ids.get(&epic_link.workspace_id).ok_or_else(|| {
            anyhow::Error::msg(format!(
                "error invalid-export-snapshot epic_link.workspace_id={} is missing",
                epic_link.workspace_id
            ))
        })?;
        if !tasks.contains(&epic_link.child_task_id) || !tasks.contains(&epic_link.epic_task_id) {
            bail!(
                "error invalid-export-snapshot task_epic_links are missing tasks in workspace {}",
                epic_link.workspace_id
            );
        }
    }

    let mut inventory = HashMap::new();
    for blob in &export.tables.blob_inventory {
        crate::attachments::validate_sha256(&blob.sha256)?;
        crate::attachments::validate_media_type(&blob.media_type)?;
        crate::attachments::validate_blob_size(usize::try_from(blob.byte_size).unwrap_or(0))?;
        if blob.available != 0 && blob.available != 1 {
            bail!("error invalid-export-snapshot blob_inventory.available invalid");
        }
        if inventory
            .insert(
                blob.sha256.clone(),
                (blob.byte_size, blob.media_type.as_str()),
            )
            .is_some()
        {
            bail!("error invalid-export-snapshot blob_inventory.sha256 duplicate");
        }
    }

    for attachment in &export.tables.task_attachments {
        crate::attachments::validate_attachment_id(&attachment.attachment_id)?;
        crate::attachments::validate_sha256(&attachment.sha256)?;
        crate::attachments::validate_media_type(&attachment.media_type)?;
        crate::attachments::validate_blob_size(usize::try_from(attachment.byte_size).unwrap_or(0))?;
        crate::attachments::validate_filename(attachment.filename.as_deref())?;
        crate::attachments::validate_alt_text(attachment.alt_text.as_deref())?;
        crate::attachments::validate_dimensions(attachment.width, attachment.height)?;
        let tasks = task_ids.get(&attachment.workspace_id).ok_or_else(|| {
            anyhow::Error::msg(format!(
                "error invalid-export-snapshot attachment.workspace_id={} is missing",
                attachment.workspace_id
            ))
        })?;
        if !tasks.contains(&attachment.task_id) {
            bail!(
                "error invalid-export-snapshot attachment.task_id={} is missing in workspace {}",
                attachment.task_id,
                attachment.workspace_id
            );
        }
        let Some((inventory_size, inventory_media_type)) = inventory.get(&attachment.sha256) else {
            bail!("error invalid-export-snapshot attachment inventory missing");
        };
        if *inventory_size != attachment.byte_size || *inventory_media_type != attachment.media_type
        {
            bail!("error invalid-export-snapshot attachment inventory metadata mismatch");
        }
        if attachment.deleted != 0 && attachment.deleted != 1 {
            bail!(
                "error invalid-export-snapshot attachment.deleted={} for attachment {}",
                attachment.deleted,
                attachment.attachment_id
            );
        }
    }

    for alias in &export.tables.project_id_aliases {
        let workspace_projects = project_ids.get(&alias.workspace_id).ok_or_else(|| {
            anyhow::Error::msg(format!(
                "error invalid-export-snapshot project_alias.workspace_id={} is missing",
                alias.workspace_id
            ))
        })?;
        if !workspace_projects.contains(&alias.local_project_id) {
            bail!(
                "error invalid-export-snapshot local_project_id={} is missing in workspace {}",
                alias.local_project_id,
                alias.workspace_id
            );
        }
    }

    let mut metadata_ids = HashSet::new();
    let mut metadata_keys = HashSet::new();
    for field in &export.tables.metadata_fields {
        ensure!(
            workspace_ids.contains(&field.workspace_id),
            "error invalid-export-snapshot metadata_field.workspace_id is missing"
        );
        let normalized = crate::metadata::normalize_metadata_key(&field.key)?;
        ensure!(
            normalized == field.key,
            "error invalid-export-snapshot metadata field key is noncanonical"
        );
        ensure!(
            metadata_ids.insert((field.workspace_id.clone(), field.id.clone())),
            "error invalid-export-snapshot metadata field identity is duplicated"
        );
        ensure!(
            metadata_keys.insert((field.workspace_id.clone(), field.key.clone())),
            "error invalid-export-snapshot metadata field key is duplicated"
        );
    }
    let mut metadata_aliases = HashSet::new();
    for alias in &export.tables.metadata_field_id_aliases {
        ensure!(
            metadata_ids.contains(&(alias.workspace_id.clone(), alias.local_field_id.clone())),
            "error invalid-export-snapshot metadata alias target is missing"
        );
        ensure!(
            metadata_aliases.insert((alias.workspace_id.clone(), alias.remote_field_id.clone())),
            "error invalid-export-snapshot metadata alias identity is duplicated"
        );
    }
    let mut task_metadata_keys = HashSet::new();
    let mut task_metadata_usage: HashMap<(WorkspaceId, TaskId), (usize, usize)> = HashMap::new();
    for value in &export.tables.task_metadata {
        ensure!(
            task_ids
                .get(&value.workspace_id)
                .is_some_and(|tasks| tasks.contains(&value.task_id)),
            "error invalid-export-snapshot task metadata task is missing"
        );
        ensure!(
            metadata_ids.contains(&(value.workspace_id.clone(), value.field_id.clone())),
            "error invalid-export-snapshot task metadata field is missing"
        );
        ensure!(
            value.value.len() <= crate::metadata::MAX_METADATA_VALUE_BYTES,
            "error invalid-export-snapshot task metadata value is too large"
        );
        ensure!(
            task_metadata_keys.insert((
                value.workspace_id.clone(),
                value.task_id.clone(),
                value.field_id.clone(),
            )),
            "error invalid-export-snapshot task metadata identity is duplicated"
        );
        let usage = task_metadata_usage
            .entry((value.workspace_id.clone(), value.task_id.clone()))
            .or_default();
        usage.0 += 1;
        usage.1 += value.value.len();
    }
    ensure!(
        task_metadata_usage.values().all(|(count, bytes)| {
            *count <= crate::metadata::MAX_METADATA_VALUES
                && *bytes <= crate::metadata::MAX_METADATA_TOTAL_BYTES
        }),
        "error invalid-export-snapshot task metadata limits exceeded"
    );

    let series_ids = export
        .tables
        .recurrence_series
        .iter()
        .map(|series| (series.workspace_id.clone(), series.id.clone()))
        .collect::<HashSet<_>>();
    let mut series_metadata_keys = HashSet::new();
    let mut series_metadata_usage: HashMap<(WorkspaceId, RecurrenceSeriesId), (usize, usize)> =
        HashMap::new();
    for value in &export.tables.recurrence_series_metadata {
        ensure!(
            series_ids.contains(&(value.workspace_id.clone(), value.series_id.clone())),
            "error invalid-export-snapshot recurrence metadata series is missing"
        );
        ensure!(
            metadata_ids.contains(&(value.workspace_id.clone(), value.field_id.clone())),
            "error invalid-export-snapshot recurrence metadata field is missing"
        );
        ensure!(
            value.value.len() <= crate::metadata::MAX_METADATA_VALUE_BYTES,
            "error invalid-export-snapshot recurrence metadata value is too large"
        );
        ensure!(
            series_metadata_keys.insert((
                value.workspace_id.clone(),
                value.series_id.clone(),
                value.field_id.clone(),
            )),
            "error invalid-export-snapshot recurrence metadata identity is duplicated"
        );
        let usage = series_metadata_usage
            .entry((value.workspace_id.clone(), value.series_id.clone()))
            .or_default();
        usage.0 += 1;
        usage.1 += value.value.len();
    }
    ensure!(
        series_metadata_usage.values().all(|(count, bytes)| {
            *count <= crate::metadata::MAX_METADATA_VALUES
                && *bytes <= crate::metadata::MAX_METADATA_TOTAL_BYTES
        }),
        "error invalid-export-snapshot recurrence metadata limits exceeded"
    );

    if has_recurrence_data(export) {
        validate_recurrence_snapshot(export, &workspace_ids, &project_ids, &label_keys, &task_ids)?;
    }

    Ok(())
}

fn has_recurrence_data(export: &AvenExport) -> bool {
    !export.tables.recurrence_series.is_empty()
        || !export.tables.recurrence_series_labels.is_empty()
        || !export.tables.recurrence_series_metadata.is_empty()
        || !export.tables.recurrence_occurrences.is_empty()
        || !export.tables.recurrence_pause_intervals.is_empty()
}

fn validate_recurrence_snapshot(
    export: &AvenExport,
    workspace_ids: &HashSet<WorkspaceId>,
    project_ids: &HashMap<WorkspaceId, HashSet<ProjectId>>,
    label_keys: &HashSet<(WorkspaceId, String)>,
    task_ids: &HashMap<WorkspaceId, HashSet<TaskId>>,
) -> Result<()> {
    struct ValidatedSeries<'a> {
        row: &'a RecurrenceSeriesRow,
        schedule: RecurrenceSchedule,
        state: RecurrenceSeriesState,
        created_at: DateTime<chrono::FixedOffset>,
        stopped_at: Option<DateTime<chrono::FixedOffset>>,
    }

    let task_rows = export
        .tables
        .tasks
        .iter()
        .map(|task| ((task.workspace_id.clone(), task.id.clone()), task))
        .collect::<HashMap<_, _>>();
    let mut change_rows = HashMap::new();
    for change in &export.tables.changes {
        ensure!(
            change_rows
                .insert(change.change_id.as_str(), change)
                .is_none(),
            "error invalid-export-snapshot change.change_id={} is duplicated",
            change.change_id
        );
    }
    let field_versions = export
        .tables
        .field_versions
        .iter()
        .map(|row| {
            (
                (
                    row.workspace_id.clone(),
                    row.entity_type.as_str(),
                    row.entity_id.as_str(),
                    row.field.as_str(),
                ),
                row.version.as_str(),
            )
        })
        .collect::<HashMap<_, _>>();

    let mut series_by_id = HashMap::new();
    for row in &export.tables.recurrence_series {
        ensure!(
            workspace_ids.contains(&row.workspace_id),
            "error invalid-export-snapshot recurrence_series.workspace_id={} is missing",
            row.workspace_id
        );
        ensure!(
            project_ids
                .get(&row.workspace_id)
                .is_some_and(|projects| projects.contains(&row.project_id)),
            "error invalid-export-snapshot recurrence_series.project_id={} is missing in workspace {}",
            row.project_id,
            row.workspace_id
        );
        TaskPriority::parse(&row.priority).context("invalid recurrence series priority")?;
        let initial_status =
            TaskStatus::parse(&row.initial_status).context("invalid recurrence initial status")?;
        ensure!(
            initial_status.is_open(),
            "error invalid-export-snapshot recurrence initial status must be open"
        );
        let frequency = RecurrenceFrequency::parse(&row.frequency)?;
        let interval = u32::try_from(row.interval).context("invalid recurrence interval")?;
        let weekdays = row
            .weekdays
            .parse::<WeekdaySet>()
            .map_err(anyhow::Error::msg)?;
        let rule = RecurrenceRule::new(frequency, interval, weekdays)?;
        let timezone = row.timezone.parse::<TimeZoneId>()?;
        let start_on = row
            .start_on
            .parse::<NaiveDate>()
            .context("invalid recurrence start date")?;
        let available_local_time = optional_import_text(&row.available_local_time)
            .map(|value| value.parse::<NaiveTime>())
            .transpose()
            .context("invalid recurrence availability time")?;
        let due_policy = RecurrenceDuePolicy::parse(&row.due_policy)?;
        let state = RecurrenceSeriesState::parse(&row.state)?;
        let stopped_at = optional_import_text(&row.stopped_at)
            .map(DateTime::parse_from_rfc3339)
            .transpose()
            .context("invalid recurrence stop time")?;
        ensure!(
            matches!(state, RecurrenceSeriesState::Stopped) == stopped_at.is_some(),
            "error invalid-export-snapshot recurrence stopped state and stop time disagree"
        );
        let created_at = DateTime::parse_from_rfc3339(&row.created_at)
            .context("invalid recurrence creation time")?;
        DateTime::parse_from_rfc3339(&row.updated_at).context("invalid recurrence update time")?;
        ensure!(
            matches!(row.deleted, 0 | 1),
            "error invalid-export-snapshot recurrence deleted value must be zero or one"
        );
        let key = (row.workspace_id.clone(), row.id.clone());
        ensure!(
            !series_by_id.contains_key(&key),
            "error invalid-export-snapshot recurrence series identity is duplicated"
        );
        series_by_id.insert(
            key,
            ValidatedSeries {
                row,
                schedule: RecurrenceSchedule::new(
                    rule,
                    timezone,
                    start_on,
                    available_local_time,
                    due_policy,
                ),
                state,
                created_at,
                stopped_at,
            },
        );
    }

    let mut series_label_keys = HashSet::new();
    for row in &export.tables.recurrence_series_labels {
        let series_key = (row.workspace_id.clone(), row.series_id.clone());
        ensure!(
            series_by_id.contains_key(&series_key),
            "error invalid-export-snapshot recurrence series label has no series"
        );
        ensure!(
            label_keys.contains(&(row.workspace_id.clone(), row.label.clone())),
            "error invalid-export-snapshot recurrence series label={} is missing",
            row.label
        );
        ensure!(
            series_label_keys.insert((row.workspace_id.clone(), row.series_id.clone(), &row.label)),
            "error invalid-export-snapshot recurrence series label is duplicated"
        );
    }

    let mut occurrence_keys = HashSet::new();
    let mut occurrence_tasks = HashSet::new();
    let mut projected_series = HashSet::new();
    for row in &export.tables.recurrence_occurrences {
        let series_key = (row.workspace_id.clone(), row.series_id.clone());
        let series = series_by_id
            .get(&series_key)
            .context("error invalid-export-snapshot recurrence occurrence has no series")?;
        let slot_on = row
            .slot_on
            .parse::<NaiveDate>()
            .context("invalid recurrence slot date")?;
        ensure!(
            occurrence_keys.insert((row.workspace_id.clone(), row.series_id.clone(), slot_on)),
            "error invalid-export-snapshot recurrence occurrence identity is duplicated"
        );
        ensure!(
            is_slot(&series.schedule.rule, series.schedule.start_on, slot_on),
            "error invalid-export-snapshot recurrence slot={} is outside the series lattice",
            slot_on
        );
        let slot = slot_values(&series.schedule, slot_on)?;
        let boundary = DateTime::parse_from_rfc3339(&slot.boundary_at)?;
        let creation_date = series
            .created_at
            .with_timezone(&series.schedule.timezone.timezone())
            .date_naive();
        ensure!(
            slot_on >= creation_date,
            "error invalid-export-snapshot recurrence slot={} precedes the series lifecycle",
            slot_on
        );
        if let Some(stopped_at) = series.stopped_at {
            ensure!(
                boundary <= stopped_at,
                "error invalid-export-snapshot recurrence slot={} exceeds the stop boundary",
                slot_on
            );
        }

        let projection_state = RecurrenceProjectionState::parse(&row.projection_state)?;
        let outcome = optional_import_text(&row.outcome)
            .map(RecurrenceOutcome::parse)
            .transpose()?;
        let task_id = optional_import_text(&row.task_id)
            .map(str::parse::<TaskId>)
            .transpose()?;
        let resolved_at = optional_import_text(&row.resolved_at);
        let outcome_change_id = optional_import_text(&row.outcome_change_id);
        let archived_at = optional_import_text(&row.archived_at);
        let valid_shape = match projection_state {
            RecurrenceProjectionState::Projected => {
                task_id.is_some()
                    && outcome.is_none()
                    && resolved_at.is_none()
                    && outcome_change_id.is_none()
                    && archived_at.is_none()
            }
            RecurrenceProjectionState::Resolved => {
                task_id.is_some()
                    && outcome.is_some()
                    && resolved_at.is_some()
                    && outcome_change_id.is_some()
                    && archived_at.is_none()
            }
            RecurrenceProjectionState::Archived => {
                task_id.is_some()
                    && outcome.is_none()
                    && resolved_at.is_none()
                    && outcome_change_id.is_none()
                    && archived_at.is_some()
            }
        };
        ensure!(
            valid_shape,
            "error invalid-export-snapshot recurrence occurrence state and fields disagree"
        );
        if matches!(projection_state, RecurrenceProjectionState::Projected) {
            ensure!(
                projected_series.insert(series_key.clone()),
                "error invalid-export-snapshot recurrence projection is not unique"
            );
        }
        for value in [resolved_at, archived_at].into_iter().flatten() {
            DateTime::parse_from_rfc3339(value)
                .context("invalid recurrence occurrence timestamp")?;
        }

        if let Some(task_id) = task_id {
            ensure!(
                task_ids
                    .get(&row.workspace_id)
                    .is_some_and(|tasks| tasks.contains(&task_id)),
                "error invalid-export-snapshot recurrence task={} is missing",
                task_id
            );
            ensure!(
                occurrence_tasks.insert((row.workspace_id.clone(), task_id.clone())),
                "error invalid-export-snapshot recurrence task link is duplicated"
            );
            let identity = derive_occurrence_identity(
                &row.workspace_id,
                &row.series_id,
                &series.schedule,
                slot_on,
            )?;
            ensure!(
                identity.task_id == task_id,
                "error invalid-export-snapshot recurrence deterministic task identity mismatch slot={slot_on}"
            );
            let task = task_rows
                .get(&(row.workspace_id.clone(), task_id.clone()))
                .context("error invalid-export-snapshot recurrence task row is missing")?;
            ensure!(
                task.created_at == identity.created_at,
                "error invalid-export-snapshot recurrence deterministic task timestamp mismatch slot={slot_on}"
            );
            match outcome {
                Some(RecurrenceOutcome::Completed) => ensure!(
                    task.status == "done",
                    "error invalid-export-snapshot recurrence completed outcome requires done task"
                ),
                Some(RecurrenceOutcome::Skipped) => ensure!(
                    task.status == "canceled",
                    "error invalid-export-snapshot recurrence skipped outcome requires canceled task"
                ),
                None if matches!(projection_state, RecurrenceProjectionState::Projected) => {
                    ensure!(
                        TaskStatus::parse(&task.status)?.is_open(),
                        "error invalid-export-snapshot recurrence projected task must be open"
                    );
                }
                None => {}
            }
            validate_deterministic_change(
                change_rows.get(identity.task_change_id.as_str()).copied(),
                &identity,
                slot_on,
                false,
            )?;
            validate_deterministic_change(
                change_rows
                    .get(identity.occurrence_change_id.as_str())
                    .copied(),
                &identity,
                slot_on,
                true,
            )?;
            for field in TaskField::VERSIONED {
                let version = field_versions
                    .get(&(
                        row.workspace_id.clone(),
                        "task",
                        task_id.as_str(),
                        field.as_str(),
                    ))
                    .context(
                        "error invalid-export-snapshot recurrence task field version is missing",
                    )?;
                if *version == identity.field_version_seeds.task {
                    continue;
                }
                ensure!(
                    change_rows.contains_key(version),
                    "error invalid-export-snapshot recurrence task field version has no change"
                );
            }
        }

        if let Some(change_id) = outcome_change_id {
            let change = change_rows
                .get(change_id)
                .context("error invalid-export-snapshot recurrence outcome change is missing")?;
            ensure!(
                change.entity_type == "recurrence_series"
                    && change.entity_id == row.series_id.as_str()
                    && change.field.as_deref() == Some("outcome")
                    && change.op_type == "resolve_recurrence_occurrence",
                "error invalid-export-snapshot recurrence outcome change identity mismatch"
            );
            let payload: Value = serde_json::from_str(&change.payload)
                .context("invalid recurrence outcome change payload")?;
            ensure!(
                payload.get("slot_on").and_then(Value::as_str) == Some(row.slot_on.as_str())
                    && payload.get("outcome").and_then(Value::as_str)
                        == outcome.map(RecurrenceOutcome::as_str)
                    && payload.get("resolved_at").and_then(Value::as_str) == resolved_at,
                "error invalid-export-snapshot recurrence outcome change payload mismatch"
            );
        }
    }

    let mut pause_ids = HashSet::new();
    let mut pauses_by_series: HashMap<_, Vec<_>> = HashMap::new();
    for row in &export.tables.recurrence_pause_intervals {
        let series_key = (row.workspace_id.clone(), row.series_id.clone());
        let series = series_by_id
            .get(&series_key)
            .context("error invalid-export-snapshot recurrence pause has no series")?;
        ensure!(
            pause_ids.insert((row.workspace_id.clone(), row.id.as_str())),
            "error invalid-export-snapshot recurrence pause identity is duplicated"
        );
        let paused_at = DateTime::parse_from_rfc3339(&row.paused_at)
            .context("invalid recurrence pause time")?;
        let resumed_at = optional_import_text(&row.resumed_at)
            .map(DateTime::parse_from_rfc3339)
            .transpose()
            .context("invalid recurrence resume time")?;
        ensure!(
            resumed_at.is_none_or(|resumed| resumed > paused_at),
            "error invalid-export-snapshot recurrence pause interval is inverted"
        );
        ensure!(
            paused_at >= series.created_at,
            "error invalid-export-snapshot recurrence pause precedes the series lifecycle"
        );
        if let Some(stopped_at) = series.stopped_at {
            ensure!(
                paused_at <= stopped_at && resumed_at.is_none_or(|resumed| resumed <= stopped_at),
                "error invalid-export-snapshot recurrence pause exceeds the stop boundary"
            );
        }
        ensure!(
            resumed_at.is_some() != row.resolved_by_change_id.is_empty(),
            "error invalid-export-snapshot recurrence pause resolution fields disagree"
        );
        ensure!(
            row.suspended_slot_on.is_empty() == row.suspended_task_id.is_empty(),
            "error invalid-export-snapshot recurrence suspended task fields disagree"
        );
        if !row.suspended_slot_on.is_empty() {
            let slot_on = row.suspended_slot_on.parse::<NaiveDate>()?;
            let task_id = row.suspended_task_id.parse::<TaskId>()?;
            ensure!(
                occurrence_keys.contains(&(
                    row.workspace_id.clone(),
                    row.series_id.clone(),
                    slot_on
                )) && occurrence_tasks.contains(&(row.workspace_id.clone(), task_id)),
                "error invalid-export-snapshot recurrence suspended task link is missing"
            );
        }
        for change_id in [
            Some(row.created_by_change_id.as_str()),
            optional_import_text(&row.resolved_by_change_id),
        ]
        .into_iter()
        .flatten()
        {
            let change = change_rows
                .get(change_id)
                .context("error invalid-export-snapshot recurrence pause change is missing")?;
            ensure!(
                change.entity_type == "recurrence_series"
                    && change.entity_id == row.series_id.as_str(),
                "error invalid-export-snapshot recurrence pause change identity mismatch"
            );
        }
        pauses_by_series
            .entry(series_key)
            .or_default()
            .push((paused_at, resumed_at));
    }
    for pauses in pauses_by_series.values_mut() {
        pauses.sort_by_key(|(paused_at, _)| *paused_at);
        for pair in pauses.windows(2) {
            ensure!(
                pair[0].1.is_some_and(|resumed_at| resumed_at <= pair[1].0),
                "error invalid-export-snapshot recurrence pause intervals overlap"
            );
        }
    }

    for series in series_by_id.values() {
        let has_lifecycle_conflict = export.tables.conflicts.iter().any(|conflict| {
            conflict.workspace_id == series.row.workspace_id
                && conflict.entity_type == "recurrence_series"
                && conflict.entity_id == series.row.id.as_str()
                && conflict.field == "state"
                && conflict.resolved == 0
        });
        let has_open_pause = pauses_by_series
            .get(&(series.row.workspace_id.clone(), series.row.id.clone()))
            .is_some_and(|pauses| pauses.iter().any(|(_, resumed_at)| resumed_at.is_none()));
        if !has_lifecycle_conflict {
            ensure!(
                matches!(series.state, RecurrenceSeriesState::Paused) == has_open_pause,
                "error invalid-export-snapshot recurrence state and open pause disagree"
            );
        }
    }

    Ok(())
}

fn validate_deterministic_change(
    change: Option<&ChangeRow>,
    identity: &crate::recurrence::RecurrenceOccurrenceIdentity,
    slot_on: NaiveDate,
    projection: bool,
) -> Result<()> {
    let change = change
        .context("error invalid-export-snapshot recurrence deterministic change is missing")?;
    let (entity_type, entity_id, field, op_type, created_at) = if projection {
        (
            "recurrence_series",
            identity.occurrence_link.series_id.as_str(),
            Some("projection"),
            "project_recurrence_occurrence",
            identity.occurrence_link.projected_at.as_str(),
        )
    } else {
        (
            "task",
            identity.task_id.as_str(),
            None,
            "create_task",
            identity.created_at.as_str(),
        )
    };
    ensure!(
        change.entity_type == entity_type
            && change.entity_id == entity_id
            && change.field.as_deref() == field
            && change.op_type == op_type
            && change.created_at == created_at,
        "error invalid-export-snapshot recurrence deterministic change identity mismatch"
    );
    let payload: Value = serde_json::from_str(&change.payload)
        .context("invalid recurrence deterministic change payload")?;
    for (key, expected) in [
        ("task_id", identity.task_id.as_str()),
        ("series_id", identity.occurrence_link.series_id.as_str()),
        ("task_change_id", identity.task_change_id.as_str()),
        (
            "occurrence_change_id",
            identity.occurrence_change_id.as_str(),
        ),
        (
            "task_field_version_seed",
            identity.field_version_seeds.task.as_str(),
        ),
        (
            "occurrence_field_version_seed",
            identity.field_version_seeds.occurrence.as_str(),
        ),
    ] {
        ensure!(
            payload.get(key).and_then(Value::as_str) == Some(expected),
            "error invalid-export-snapshot recurrence deterministic payload field={key} mismatch"
        );
    }
    ensure!(
        payload.get("slot_on").and_then(Value::as_str) == Some(&slot_on.to_string()),
        "error invalid-export-snapshot recurrence deterministic payload slot mismatch"
    );
    if projection {
        ensure!(
            payload.get("projected_at").and_then(Value::as_str)
                == Some(identity.occurrence_link.projected_at.as_str()),
            "error invalid-export-snapshot recurrence deterministic projection link mismatch"
        );
    }
    Ok(())
}

fn optional_import_text(value: &str) -> Option<&str> {
    (!value.is_empty()).then_some(value)
}

async fn replace_from_export(
    tx: &mut SqliteConnection,
    export: &AvenExport,
    target_client_id: &str,
) -> Result<()> {
    let recurrence_data = has_recurrence_data(export);
    let delete_order = [
        "DELETE FROM recurrence_pause_intervals",
        "DELETE FROM recurrence_occurrences",
        "DELETE FROM recurrence_series_metadata",
        "DELETE FROM recurrence_series_labels",
        "DELETE FROM recurrence_series",
        "DELETE FROM task_metadata",
        "DELETE FROM task_attachments",
        "DELETE FROM blob_inventory",
        "DELETE FROM task_epic_links",
        "DELETE FROM task_dependencies",
        "DELETE FROM task_labels",
        "DELETE FROM notes",
        "DELETE FROM conflicts",
        "DELETE FROM field_versions",
        "DELETE FROM changes",
        "DELETE FROM project_paths",
        "DELETE FROM project_id_aliases",
        "DELETE FROM metadata_field_id_aliases",
        "DELETE FROM tasks",
        "DELETE FROM metadata_fields",
        "DELETE FROM labels",
        "DELETE FROM projects",
        "DELETE FROM workspaces",
        "DELETE FROM meta",
    ];
    for sql in delete_order {
        sqlx::query(sql).execute(&mut *tx).await?;
    }

    db::set_meta(tx, "client_id", target_client_id).await?;
    db::set_meta(tx, "sync_cursor", "0").await?;
    let local_seq = export
        .tables
        .changes
        .iter()
        .map(|row| row.local_seq)
        .max()
        .unwrap_or(0);
    db::set_meta(tx, "local_seq", &local_seq.to_string()).await?;

    for meta in &export.tables.meta {
        if matches!(
            meta.key.as_str(),
            "client_id" | "sync_server_url" | "sync_cursor" | "local_seq"
        ) {
            continue;
        }
        db::set_meta(tx, &meta.key, &meta.value).await?;
    }

    let suppressed_attachment_changes = export
        .tables
        .changes
        .iter()
        .filter(|change| {
            change.server_seq.is_none()
                && change.field.as_deref() == Some("attachments")
                && matches!(
                    change.op_type.as_str(),
                    "attachment_add" | "attachment_delete"
                )
        })
        .map(|change| change.change_id.as_str())
        .collect::<HashSet<_>>();
    let mut attachments = export.tables.task_attachments.clone();
    for attachment in &mut attachments {
        if attachment
            .created_by_change_id
            .as_deref()
            .is_some_and(|id| suppressed_attachment_changes.contains(id))
        {
            attachment.created_by_change_id = None;
        }
        if attachment
            .deleted_by_change_id
            .as_deref()
            .is_some_and(|id| suppressed_attachment_changes.contains(id))
        {
            attachment.deleted_by_change_id = None;
        }
    }
    let changes = export
        .tables
        .changes
        .iter()
        .filter(|change| !suppressed_attachment_changes.contains(change.change_id.as_str()))
        .filter(|change| {
            recurrence_data
                || (change.entity_type != "recurrence_series"
                    && !change.op_type.contains("recurrence"))
        })
        .cloned()
        .collect::<Vec<_>>();
    let field_versions = export
        .tables
        .field_versions
        .iter()
        .filter(|row| recurrence_data || row.entity_type != "recurrence_series")
        .cloned()
        .collect::<Vec<_>>();
    let conflicts = export
        .tables
        .conflicts
        .iter()
        .filter(|row| recurrence_data || row.entity_type != "recurrence_series")
        .cloned()
        .collect::<Vec<_>>();

    tables::import_workspaces(tx, &export.tables.workspaces).await?;
    tables::import_projects(tx, &export.tables.projects).await?;
    tables::import_project_id_aliases(tx, &export.tables.project_id_aliases).await?;
    tables::import_project_paths(tx, &export.tables.project_paths).await?;
    tables::import_labels(tx, &export.tables.labels).await?;
    tables::import_metadata_fields(tx, &export.tables.metadata_fields).await?;
    tables::import_metadata_field_id_aliases(tx, &export.tables.metadata_field_id_aliases).await?;
    tables::import_tasks(tx, &export.tables.tasks).await?;
    tables::import_task_metadata(tx, &export.tables.task_metadata).await?;
    tables::import_task_labels(tx, &export.tables.task_labels).await?;
    tables::import_notes(tx, &export.tables.notes).await?;
    tables::import_task_dependencies(tx, &export.tables.task_dependencies).await?;
    tables::import_task_epic_links(tx, &export.tables.task_epic_links).await?;
    tables::import_blob_inventory(tx, &export.tables.blob_inventory).await?;
    tables::import_task_attachments(tx, &attachments).await?;
    if recurrence_data {
        tables::import_recurrence_series(tx, &export.tables.recurrence_series).await?;
        tables::import_recurrence_series_labels(tx, &export.tables.recurrence_series_labels)
            .await?;
        tables::import_recurrence_series_metadata(tx, &export.tables.recurrence_series_metadata)
            .await?;
        tables::import_recurrence_occurrences(tx, &export.tables.recurrence_occurrences).await?;
        tables::import_recurrence_pause_intervals(tx, &export.tables.recurrence_pause_intervals)
            .await?;
    }
    tables::import_changes(tx, &changes).await?;
    tables::import_field_versions(tx, &field_versions).await?;
    tables::import_conflicts(tx, &conflicts).await?;

    Ok(())
}

async fn database_integrity_report_with_connection(
    conn: &mut SqliteConnection,
) -> Result<IntegrityReport> {
    let quick_check_value: String = query_scalar("PRAGMA quick_check")
        .fetch_one(&mut *conn)
        .await?;
    let mut checks = Vec::new();
    checks.push(count_check(
        conn,
        "task projects",
        "SELECT count(*) FROM tasks t LEFT JOIN projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id WHERE p.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "project paths",
        "SELECT count(*) FROM project_paths pp LEFT JOIN projects p ON p.workspace_id = pp.workspace_id AND p.id = pp.project_id WHERE p.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "project aliases",
        "SELECT count(*) FROM project_id_aliases a LEFT JOIN projects p ON p.workspace_id = a.workspace_id AND p.id = a.local_project_id WHERE p.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "metadata field workspaces",
        "SELECT count(*) FROM metadata_fields f LEFT JOIN workspaces w ON w.id = f.workspace_id WHERE w.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "metadata aliases",
        "SELECT count(*) FROM metadata_field_id_aliases a LEFT JOIN metadata_fields f ON f.workspace_id = a.workspace_id AND f.id = a.local_field_id WHERE f.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "task metadata tasks",
        "SELECT count(*) FROM task_metadata m LEFT JOIN tasks t ON t.workspace_id = m.workspace_id AND t.id = m.task_id WHERE t.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "task metadata fields",
        "SELECT count(*) FROM task_metadata m LEFT JOIN metadata_fields f ON f.workspace_id = m.workspace_id AND f.id = m.field_id WHERE f.id IS NULL",
    )
    .await?);
    checks.push(
        count_check(
            conn,
            "task metadata value size",
            "SELECT count(*) FROM task_metadata WHERE length(CAST(value AS BLOB)) > 4096",
        )
        .await?,
    );
    checks.push(count_check(
        conn,
        "task metadata aggregate limits",
        "SELECT count(*) FROM (SELECT workspace_id, task_id FROM task_metadata GROUP BY workspace_id, task_id HAVING count(*) > 128 OR sum(length(CAST(value AS BLOB))) > 32768)",
    )
    .await?);
    checks.push(count_check(
        conn,
        "recurrence metadata series",
        "SELECT count(*) FROM recurrence_series_metadata m LEFT JOIN recurrence_series s ON s.workspace_id = m.workspace_id AND s.id = m.series_id WHERE s.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "recurrence metadata fields",
        "SELECT count(*) FROM recurrence_series_metadata m LEFT JOIN metadata_fields f ON f.workspace_id = m.workspace_id AND f.id = m.field_id WHERE f.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "recurrence metadata value size",
        "SELECT count(*) FROM recurrence_series_metadata WHERE length(CAST(value AS BLOB)) > 4096",
    )
    .await?);
    checks.push(count_check(
        conn,
        "recurrence metadata aggregate limits",
        "SELECT count(*) FROM (SELECT workspace_id, series_id FROM recurrence_series_metadata GROUP BY workspace_id, series_id HAVING count(*) > 128 OR sum(length(CAST(value AS BLOB))) > 32768)",
    )
    .await?);
    let invalid_metadata_keys = sqlx::query_scalar::<_, String>("SELECT key FROM metadata_fields")
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .filter(|key| match crate::metadata::normalize_metadata_key(key) {
            Ok(normalized) => normalized != key.as_str(),
            Err(_) => true,
        })
        .count();
    checks.push(IntegrityCheck {
        label: "metadata field keys",
        ok: invalid_metadata_keys == 0,
        value: invalid_metadata_keys.to_string(),
    });
    checks.push(count_check(
        conn,
        "task label tasks",
        "SELECT count(*) FROM task_labels tl LEFT JOIN tasks t ON t.workspace_id = tl.workspace_id AND t.id = tl.task_id WHERE t.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "task label labels",
        "SELECT count(*) FROM task_labels tl LEFT JOIN labels l ON l.workspace_id = tl.workspace_id AND l.name = tl.label WHERE l.name IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "notes",
        "SELECT count(*) FROM notes n LEFT JOIN tasks t ON t.workspace_id = n.workspace_id AND t.id = n.task_id WHERE t.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "note changes",
        "SELECT count(*) FROM notes n LEFT JOIN changes c ON c.change_id = n.change_id WHERE c.change_id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "dependency tasks",
        "SELECT count(*) FROM task_dependencies d LEFT JOIN tasks t ON t.workspace_id = d.workspace_id AND t.id = d.task_id WHERE t.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "dependency targets",
        "SELECT count(*) FROM task_dependencies d LEFT JOIN tasks t ON t.workspace_id = d.workspace_id AND t.id = d.depends_on_task_id WHERE t.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "epic link children",
        "SELECT count(*) FROM task_epic_links l LEFT JOIN tasks t ON t.workspace_id = l.workspace_id AND t.id = l.child_task_id WHERE t.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "epic link parents",
        "SELECT count(*) FROM task_epic_links l LEFT JOIN tasks t ON t.workspace_id = l.workspace_id AND t.id = l.epic_task_id WHERE t.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "epic link parent flags",
        "SELECT count(*) FROM task_epic_links l JOIN tasks t ON t.workspace_id = l.workspace_id AND t.id = l.epic_task_id WHERE t.is_epic = 0",
    )
    .await?);
    checks.push(count_check(
        conn,
        "conflict tasks",
        "SELECT count(*) FROM conflicts c LEFT JOIN tasks t ON t.workspace_id = c.workspace_id AND t.id = c.entity_id WHERE c.resolved = 0 AND c.entity_type = 'task' AND t.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "task due dates",
        "SELECT count(*) FROM tasks WHERE due_on != '' AND (length(due_on) != 10 OR substr(due_on, 5, 1) != '-' OR substr(due_on, 8, 1) != '-' OR date(due_on) IS NULL OR strftime('%Y-%m-%d', due_on) != due_on)",
    )
    .await?);
    checks.push(count_check(
        conn,
        "field version tasks",
        "SELECT count(*) FROM field_versions fv LEFT JOIN tasks t ON t.workspace_id = fv.workspace_id AND t.id = fv.entity_id WHERE fv.entity_type = 'task' AND t.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "field version changes",
        "SELECT count(*) FROM field_versions fv LEFT JOIN changes c ON c.change_id = fv.version WHERE c.change_id IS NULL AND NOT (fv.entity_type = 'task' AND EXISTS (SELECT 1 FROM recurrence_occurrences o WHERE o.workspace_id = fv.workspace_id AND o.task_id = fv.entity_id))",
    )
    .await?);
    checks.extend(integrity::recurrence_integrity_checks(conn).await?);
    push_meta_checks(conn, &mut checks).await?;

    Ok(IntegrityReport {
        quick_check_ok: quick_check_value == "ok",
        quick_check_value,
        checks,
    })
}

pub(crate) fn ensure_integrity_ok(report: &IntegrityReport) -> Result<()> {
    let mut bad = vec![];
    if !report.quick_check_ok {
        bad.push("quick check");
    }
    for check in &report.checks {
        if !check.ok && check.label != "recurrence projection gaps" {
            bad.push(check.label);
        }
    }
    if bad.is_empty() {
        return Ok(());
    }
    bail!("error data-integrity-failed checks={}", bad.join(", "))
}

async fn count_check(
    conn: &mut SqliteConnection,
    label: &'static str,
    query: &'static str,
) -> Result<IntegrityCheck> {
    let count: i64 = query_scalar(query).fetch_one(&mut *conn).await?;
    Ok(IntegrityCheck {
        label,
        ok: count == 0,
        value: format!("{count} orphaned"),
    })
}

async fn push_meta_checks(
    conn: &mut SqliteConnection,
    checks: &mut Vec<IntegrityCheck>,
) -> Result<()> {
    let local_seq = db::get_meta(conn, "local_seq").await?;
    let local_seq_check = match local_seq {
        Some(raw) => match raw.parse::<i64>() {
            Ok(value) => {
                let max_seq: i64 = query_scalar("SELECT COALESCE(MAX(local_seq), 0) FROM changes")
                    .fetch_one(&mut *conn)
                    .await?;
                let ok = value >= max_seq;
                IntegrityCheck {
                    label: "meta local_seq",
                    ok,
                    value: value.to_string(),
                }
            }
            Err(error) => IntegrityCheck {
                label: "meta local_seq",
                ok: false,
                value: error.to_string(),
            },
        },
        None => IntegrityCheck {
            label: "meta local_seq",
            ok: false,
            value: "missing".to_string(),
        },
    };
    checks.push(local_seq_check);

    let sync_cursor = db::get_meta(conn, "sync_cursor").await?;
    let sync_cursor_ok = match sync_cursor {
        Some(raw) => match raw.parse::<i64>() {
            Ok(_) => IntegrityCheck {
                label: "sync cursor",
                ok: true,
                value: raw,
            },
            Err(error) => IntegrityCheck {
                label: "sync cursor",
                ok: false,
                value: error.to_string(),
            },
        },
        None => IntegrityCheck {
            label: "sync cursor",
            ok: false,
            value: "missing".to_string(),
        },
    };
    checks.push(sync_cursor_ok);

    Ok(())
}
