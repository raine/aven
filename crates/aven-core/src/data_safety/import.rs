use std::collections::HashSet;

use anyhow::Result;
use sqlx::SqliteConnection;

use crate::db;

use super::AvenExport;
use super::tables;
use super::validation::recurrence::has_recurrence_data;

pub(super) async fn replace_from_export(
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
        "DELETE FROM task_related_links",
        "DELETE FROM task_epic_links",
        "DELETE FROM task_dependencies",
        "DELETE FROM task_labels",
        "DELETE FROM notes",
        "DELETE FROM conflicts",
        "DELETE FROM field_versions",
        "DELETE FROM shared_history_provenance",
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
    if let Some(server) = super::validation::portable_history_server(export)? {
        db::set_meta(tx, "sync_server_url", &server).await?;
    }

    for meta in &export.tables.meta {
        if matches!(
            meta.key.as_str(),
            "client_id"
                | "sync_server_url"
                | "sync_cursor"
                | "local_seq"
                | "sync_generation"
                | "sync_established_protocol"
                | "sync_blocked_protocol"
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
                && change.op_type == "attachment_add"
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
    tables::import_shared_history_provenance(tx, &export.tables.shared_history_provenance).await?;
    tables::import_task_related_links(tx, &export.tables.task_related_links).await?;
    tables::import_field_versions(tx, &field_versions).await?;
    tables::import_conflicts(tx, &conflicts).await?;
    crate::epic_membership::recover(tx, true).await?;

    Ok(())
}
