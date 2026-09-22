use anyhow::Result;
use sqlx::SqliteConnection;

use super::tables;
use super::{
    BlobInventoryExportRow, ChangeRow, ConflictRow, FieldVersionRow, LabelRow, MetaRow,
    MetadataFieldIdAliasRow, MetadataFieldRow, NoteRow, ProjectIdAliasRow, ProjectPathRow,
    ProjectRow, RecurrenceOccurrenceRow, RecurrencePauseIntervalRow, RecurrenceSeriesLabelRow,
    RecurrenceSeriesMetadataRow, RecurrenceSeriesRow, SharedHistoryProvenanceRow,
    TaskAttachmentRow, TaskDependencyRow, TaskEpicLinkRow, TaskLabelRow, TaskMetadataRow,
    TaskRelatedLinkRow, TaskRow, WorkspaceRow,
};

pub(super) async fn scan_workspaces(conn: &mut SqliteConnection) -> Result<Vec<WorkspaceRow>> {
    tables::scan_rows(
        conn,
        "SELECT id, name, key, created_at, updated_at, archived FROM workspaces",
    )
    .await
}

pub(super) async fn scan_projects(conn: &mut SqliteConnection) -> Result<Vec<ProjectRow>> {
    tables::scan_rows(
        conn,
        "SELECT id, workspace_id, key, name, prefix, created_at, updated_at, deleted FROM projects",
    )
    .await
}

pub(super) async fn scan_project_paths(conn: &mut SqliteConnection) -> Result<Vec<ProjectPathRow>> {
    tables::scan_rows(
        conn,
        "SELECT workspace_id, project_id, path FROM project_paths",
    )
    .await
}

pub(super) async fn scan_project_id_aliases(
    conn: &mut SqliteConnection,
) -> Result<Vec<ProjectIdAliasRow>> {
    tables::scan_rows(
        conn,
        "SELECT workspace_id, remote_project_id, local_project_id FROM project_id_aliases",
    )
    .await
}

pub(super) async fn scan_labels(conn: &mut SqliteConnection) -> Result<Vec<LabelRow>> {
    tables::scan_rows(conn, "SELECT workspace_id, name, created_at FROM labels").await
}

pub(super) async fn scan_metadata_fields(
    conn: &mut SqliteConnection,
) -> Result<Vec<MetadataFieldRow>> {
    tables::scan_rows(
        conn,
        "SELECT id, workspace_id, key, created_at, updated_at FROM metadata_fields",
    )
    .await
}

pub(super) async fn scan_metadata_field_id_aliases(
    conn: &mut SqliteConnection,
) -> Result<Vec<MetadataFieldIdAliasRow>> {
    tables::scan_rows(
        conn,
        "SELECT workspace_id, remote_field_id, local_field_id FROM metadata_field_id_aliases",
    )
    .await
}

pub(super) async fn scan_task_metadata(
    conn: &mut SqliteConnection,
) -> Result<Vec<TaskMetadataRow>> {
    tables::scan_rows(
        conn,
        "SELECT workspace_id, task_id, field_id, value, created_at, updated_at FROM task_metadata",
    )
    .await
}

pub(super) async fn scan_tasks(conn: &mut SqliteConnection) -> Result<Vec<TaskRow>> {
    tables::scan_rows(conn, "SELECT workspace_id, id, title, description, project_id, status, priority, source, created_at, updated_at, queue_activity_at, available_at, due_on, deleted, is_epic FROM tasks").await
}

pub(super) async fn scan_task_labels(conn: &mut SqliteConnection) -> Result<Vec<TaskLabelRow>> {
    tables::scan_rows(conn, "SELECT workspace_id, task_id, label FROM task_labels").await
}

pub(super) async fn scan_notes(conn: &mut SqliteConnection) -> Result<Vec<NoteRow>> {
    tables::scan_rows(
        conn,
        "SELECT workspace_id, id, task_id, body, created_at, change_id FROM notes",
    )
    .await
}

pub(super) async fn scan_task_dependencies(
    conn: &mut SqliteConnection,
) -> Result<Vec<TaskDependencyRow>> {
    tables::scan_rows(
        conn,
        "SELECT workspace_id, task_id, depends_on_task_id, created_at FROM task_dependencies",
    )
    .await
}

pub(super) async fn scan_task_epic_links(
    conn: &mut SqliteConnection,
) -> Result<Vec<TaskEpicLinkRow>> {
    tables::scan_rows(
        conn,
        "SELECT workspace_id, child_task_id, epic_task_id, created_at FROM task_epic_links",
    )
    .await
}

pub(super) async fn scan_task_related_links(
    conn: &mut SqliteConnection,
) -> Result<Vec<TaskRelatedLinkRow>> {
    tables::scan_rows(
        conn,
        "SELECT workspace_id, task_a_id, task_b_id, linked, last_change_id FROM task_related_links",
    )
    .await
}

pub(super) async fn scan_task_attachments(
    conn: &mut SqliteConnection,
) -> Result<Vec<TaskAttachmentRow>> {
    tables::scan_rows(
        conn,
        "SELECT workspace_id, attachment_id, task_id, sha256, byte_size, media_type, filename, alt_text, width, height, created_at, created_by_change_id, deleted, deleted_at, deleted_by_change_id FROM task_attachments",
    )
    .await
}

pub(super) async fn scan_blob_inventory(
    conn: &mut SqliteConnection,
) -> Result<Vec<BlobInventoryExportRow>> {
    tables::scan_rows(
        conn,
        "SELECT sha256, byte_size, media_type, available, first_seen_at, last_verified_at FROM blob_inventory",
    )
    .await
}

pub(super) async fn scan_recurrence_series(
    conn: &mut SqliteConnection,
) -> Result<Vec<RecurrenceSeriesRow>> {
    tables::scan_rows(
        conn,
        "SELECT workspace_id, id, title, description, project_id, priority, initial_status, frequency, interval, weekdays, timezone, start_on, available_local_time, due_policy, state, stopped_at, created_at, updated_at, deleted FROM recurrence_series",
    )
    .await
}

pub(super) async fn scan_recurrence_series_labels(
    conn: &mut SqliteConnection,
) -> Result<Vec<RecurrenceSeriesLabelRow>> {
    tables::scan_rows(
        conn,
        "SELECT workspace_id, series_id, label FROM recurrence_series_labels",
    )
    .await
}

pub(super) async fn scan_recurrence_series_metadata(
    conn: &mut SqliteConnection,
) -> Result<Vec<RecurrenceSeriesMetadataRow>> {
    tables::scan_rows(
        conn,
        "SELECT workspace_id, series_id, field_id, value, created_at, updated_at
         FROM recurrence_series_metadata",
    )
    .await
}

pub(super) async fn scan_recurrence_occurrences(
    conn: &mut SqliteConnection,
) -> Result<Vec<RecurrenceOccurrenceRow>> {
    tables::scan_rows(
        conn,
        "SELECT workspace_id, series_id, slot_on, task_id, outcome, resolved_at, outcome_change_id, projection_state, archived_at FROM recurrence_occurrences",
    )
    .await
}

pub(super) async fn scan_recurrence_pause_intervals(
    conn: &mut SqliteConnection,
) -> Result<Vec<RecurrencePauseIntervalRow>> {
    tables::scan_rows(
        conn,
        "SELECT workspace_id, id, series_id, paused_at, resumed_at, suspended_slot_on, suspended_task_id, created_by_change_id, resolved_by_change_id FROM recurrence_pause_intervals",
    )
    .await
}

pub(super) async fn scan_changes(conn: &mut SqliteConnection) -> Result<Vec<ChangeRow>> {
    tables::scan_rows(conn, "SELECT change_id, client_id, local_seq, entity_type, entity_id, field, op_type, payload, base_version, created_at, server_seq FROM changes").await
}

pub(super) async fn scan_shared_history_provenance(
    conn: &mut SqliteConnection,
) -> Result<Vec<SharedHistoryProvenanceRow>> {
    tables::scan_rows(
        conn,
        "SELECT change_id, source_server_seq, source_pending_rank
         FROM shared_history_provenance",
    )
    .await
}

pub(super) async fn scan_field_versions(
    conn: &mut SqliteConnection,
) -> Result<Vec<FieldVersionRow>> {
    tables::scan_rows(
        conn,
        "SELECT workspace_id, entity_type, entity_id, field, version FROM field_versions",
    )
    .await
}

pub(super) async fn scan_conflicts(conn: &mut SqliteConnection) -> Result<Vec<ConflictRow>> {
    tables::scan_rows(conn, "SELECT id, workspace_id, entity_type, entity_id, task_id, field, base_version, local_value, remote_value, local_change_id, remote_change_id, variant_a, variant_b, created_at, resolved FROM conflicts").await
}

pub(super) async fn scan_meta(conn: &mut SqliteConnection) -> Result<Vec<MetaRow>> {
    tables::scan_rows(conn, "SELECT key, value FROM meta").await
}
