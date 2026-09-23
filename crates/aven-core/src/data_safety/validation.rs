use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use sqlx::SqliteConnection;

use crate::choices::TaskSource;
use crate::ids::{ProjectId, TaskId, WorkspaceId};
use crate::recurrence::RecurrenceSeriesId;

use super::AvenExport;
use super::export_types::{EXPORT_FORMAT, EXPORT_VERSION, RELATED_LINKS_EXPORT_VERSION};

pub(super) mod recurrence;

pub(super) async fn ensure_supported_export(
    _conn: &mut SqliteConnection,
    export: &AvenExport,
) -> Result<()> {
    if export.format != EXPORT_FORMAT {
        bail!("error export-format-unsupported format={}", export.format);
    }
    if !matches!(
        export.version,
        1 | 2 | RELATED_LINKS_EXPORT_VERSION | EXPORT_VERSION
    ) {
        bail!(
            "error export-version-unsupported version={}",
            export.version
        );
    }
    Ok(())
}

pub(super) fn accepted_history_server(export: &AvenExport) -> Result<Option<String>> {
    if !export
        .tables
        .changes
        .iter()
        .any(|change| change.server_seq.is_some())
    {
        return Ok(None);
    }
    let mut servers = export
        .tables
        .meta
        .iter()
        .filter(|row| row.key == "sync_server_url")
        .map(|row| row.value.as_str());
    let server = servers.next().context(
        "error invalid-export-snapshot accepted sync history is missing its server identity",
    )?;
    ensure!(
        servers.next().is_none(),
        "error invalid-export-snapshot sync server identity is duplicated"
    );
    ensure!(
        crate::sync::wire::sync_server_url_is_valid(server),
        "error invalid-export-snapshot sync server identity is invalid"
    );
    Ok(Some(server.trim_end_matches('/').to_string()))
}

pub(super) fn portable_history_server(export: &AvenExport) -> Result<Option<String>> {
    let has_accepted_history = export
        .tables
        .changes
        .iter()
        .any(|change| change.server_seq.is_some());
    if !has_accepted_history {
        return Ok(None);
    }
    if export
        .tables
        .meta
        .iter()
        .any(|row| row.key == "sync_server_url")
    {
        return accepted_history_server(export);
    }
    let accepted_ids = export
        .tables
        .changes
        .iter()
        .filter(|change| change.server_seq.is_some())
        .map(|change| change.change_id.as_str())
        .collect::<HashSet<_>>();
    let provenance_ids = export
        .tables
        .shared_history_provenance
        .iter()
        .map(|row| row.change_id.as_str())
        .collect::<HashSet<_>>();
    ensure!(
        export.version == EXPORT_VERSION && accepted_ids.is_subset(&provenance_ids),
        "error invalid-export-snapshot accepted sync history is missing its server identity or complete shared-history provenance"
    );
    Ok(None)
}

pub(super) fn validate_export_snapshot(export: &AvenExport) -> Result<()> {
    ensure!(
        !export
            .tables
            .meta
            .iter()
            .any(|m| m.key == "e2ee_data_only" || m.key == "e2ee_association"),
        "error e2ee-data-only-import-unavailable"
    );
    portable_history_server(export)?;
    validate_shared_snapshot(export)
}

fn validate_shared_history_provenance(export: &AvenExport) -> Result<()> {
    let rows = &export.tables.shared_history_provenance;
    ensure!(
        rows.is_empty() || export.version == EXPORT_VERSION,
        "error invalid-export-snapshot shared-history provenance requires version {EXPORT_VERSION}"
    );
    ensure!(
        export.version != EXPORT_VERSION || !rows.is_empty(),
        "error invalid-export-snapshot version {EXPORT_VERSION} requires shared-history provenance"
    );
    let changes = export
        .tables
        .changes
        .iter()
        .map(|change| (change.change_id.as_str(), change))
        .collect::<HashMap<_, _>>();
    let mut ids = HashSet::new();
    for row in rows {
        ensure!(
            ids.insert(row.change_id.as_str()),
            "error invalid-export-snapshot shared-history provenance identity is duplicated"
        );
        let change = changes
            .get(row.change_id.as_str())
            .context("error invalid-export-snapshot shared-history provenance change is missing")?;
        ensure!(
            change.server_seq.is_some(),
            "error invalid-export-snapshot shared-history provenance change is not in the effective prefix"
        );
        ensure!(
            matches!(
                (row.source_server_seq, row.source_pending_rank),
                (Some(1..), None) | (None, Some(1..))
            ),
            "error invalid-export-snapshot shared-history provenance order is invalid"
        );
    }
    Ok(())
}

pub(crate) fn validate_shared_snapshot(export: &AvenExport) -> Result<()> {
    use crate::sync::protocol::{MAINTAINED_PROTOCOL_BASELINE, validate_operation};
    for row in &export.tables.tasks {
        validate_operation(
            MAINTAINED_PROTOCOL_BASELINE,
            "create_task",
            None,
            &serde_json::to_value(row)?,
        )?;
    }
    for row in &export.tables.recurrence_series {
        validate_operation(
            MAINTAINED_PROTOCOL_BASELINE,
            "create_recurrence_series",
            None,
            &serde_json::to_value(row)?,
        )?;
    }
    for row in &export.tables.task_attachments {
        validate_operation(
            MAINTAINED_PROTOCOL_BASELINE,
            "attachment_add",
            Some("attachments"),
            &serde_json::to_value(row)?,
        )?;
    }
    for row in &export.tables.changes {
        validate_operation(
            MAINTAINED_PROTOCOL_BASELINE,
            &row.op_type,
            row.field.as_deref(),
            &serde_json::from_str(&row.payload)?,
        )?;
    }
    ensure!(
        export.version >= RELATED_LINKS_EXPORT_VERSION
            || (export.tables.task_related_links.is_empty()
                && !export.tables.changes.iter().any(|change| matches!(
                    change.op_type.as_str(),
                    "related_add" | "related_remove"
                ))),
        "error invalid-export-snapshot related links require version {RELATED_LINKS_EXPORT_VERSION}; related changes are not supported in older versions"
    );
    validate_shared_history_provenance(export)?;
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

    for meta in &export.tables.meta {
        if let Some((workspace_id, child_id, parent_id)) =
            crate::epic_membership::parse_baseline_identity(&meta.key, &meta.value)?
        {
            ensure!(
                task_ids.get(&workspace_id).is_some_and(|tasks| {
                    tasks.contains(&child_id) && tasks.contains(&parent_id)
                }),
                "error invalid-export-snapshot epic membership baseline missing task"
            );
        }
    }

    let changes_by_id = export
        .tables
        .changes
        .iter()
        .map(|change| (change.change_id.as_str(), change))
        .collect::<HashMap<_, _>>();
    for link in &export.tables.task_related_links {
        ensure!(
            link.task_a_id < link.task_b_id,
            "error invalid-export-snapshot related pair is not canonical"
        );
        ensure!(
            matches!(link.linked, 0 | 1),
            "error invalid-export-snapshot related linked value is invalid"
        );
        let tasks = task_ids
            .get(&link.workspace_id)
            .context("error invalid-export-snapshot related workspace is missing")?;
        ensure!(
            tasks.contains(&link.task_a_id) && tasks.contains(&link.task_b_id),
            "error invalid-export-snapshot related endpoints are missing"
        );
        let change = changes_by_id
            .get(link.last_change_id.as_str())
            .context("error invalid-export-snapshot related change is missing")?;
        let payload: Value = serde_json::from_str(&change.payload)?;
        let related_id: TaskId = payload
            .get("related_task_id")
            .and_then(Value::as_str)
            .context("error invalid-export-snapshot related payload is invalid")?
            .parse()?;
        let initiating_id: TaskId = change.entity_id.parse()?;
        let (change_a, change_b) =
            crate::operations::canonical_related_pair(&initiating_id, &related_id)?;
        ensure!(
            change.entity_type == "task"
                && change.field.as_deref() == Some("related")
                && payload.get("workspace_id").and_then(Value::as_str)
                    == Some(link.workspace_id.as_str())
                && change_a == &link.task_a_id
                && change_b == &link.task_b_id
                && ((link.linked == 1 && change.op_type == "related_add")
                    || (link.linked == 0 && change.op_type == "related_remove")),
            "error invalid-export-snapshot related change does not match state"
        );
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
    }

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

    if recurrence::has_recurrence_data(export) {
        recurrence::validate_recurrence_snapshot(
            export,
            &workspace_ids,
            &project_ids,
            &label_keys,
            &task_ids,
        )?;
    }

    Ok(())
}
