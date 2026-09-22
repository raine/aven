use crate::choices::TaskSource;
use crate::ids::{MetadataFieldId, ProjectId, TaskId, WorkspaceId};
use crate::recurrence::RecurrenceSeriesId;
use serde::{Deserialize, Serialize};

pub(crate) const EXPORT_FORMAT: &str = "aven-export";
pub(crate) const EXPORT_VERSION: i64 = 4;
pub(crate) const RELATED_LINKS_EXPORT_VERSION: i64 = 3;

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
    pub task_related_links: Vec<TaskRelatedLinkRow>,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shared_history_provenance: Vec<SharedHistoryProvenanceRow>,
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

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct TaskRelatedLinkRow {
    pub workspace_id: WorkspaceId,
    pub task_a_id: TaskId,
    pub task_b_id: TaskId,
    pub linked: i64,
    pub last_change_id: String,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct SharedHistoryProvenanceRow {
    pub change_id: String,
    pub source_server_seq: Option<i64>,
    pub source_pending_rank: Option<i64>,
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
