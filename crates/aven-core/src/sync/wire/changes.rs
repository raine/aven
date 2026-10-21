use std::collections::HashSet;

use super::*;
use crate::change_log::op_type;
use crate::choices::TaskSource;
use crate::recurrence::RecurrenceSeriesId;
use crate::task_fields::TaskField;
use anyhow::{Context, Result, bail};
use serde_json::Value;

pub(super) fn validate_publish_device_label(change: &ChangeWire) -> Result<()> {
    ensure_entity_type(change, "device")?;
    if change.entity_id.len() != 64
        || !change
            .entity_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        bail!("error invalid-sync-change entity_id invalid-device-id");
    }
    if change.field.is_some()
        || change.base_version.is_some()
        || change
            .payload
            .as_object()
            .is_none_or(|payload| payload.len() != 1)
    {
        bail!("error invalid-sync-change device-label shape");
    }
    let label = required_string_payload("label", &change.payload)?;
    crate::sync::device_labels::validate_device_label(&label)?;
    Ok(())
}

pub(super) fn validate_create_workspace(change: &ChangeWire) -> Result<()> {
    ensure_entity_type(change, "workspace")?;
    ensure_sync_id("entity_id", &change.entity_id)?;
    required_string_payload("key", &change.payload)?;
    required_string_payload("name", &change.payload)?;
    required_string_payload("created_at", &change.payload)?;

    Ok(())
}

pub(super) fn validate_set_workspace_field(change: &ChangeWire) -> Result<()> {
    ensure_entity_type(change, "workspace")?;
    ensure_sync_id("entity_id", &change.entity_id)?;
    let field = change
        .field
        .as_deref()
        .filter(|field| !field.trim().is_empty())
        .context("error invalid-sync-change field missing")?;
    if !matches!(field, "name" | "key") {
        bail!("error invalid-sync-change field={field}");
    }
    required_string_payload("value", &change.payload)?;

    Ok(())
}

pub(super) fn validate_delete_workspace(change: &ChangeWire) -> Result<()> {
    ensure_entity_type(change, "workspace")?;
    ensure_sync_id("entity_id", &change.entity_id)?;
    if change.entity_id == crate::workspaces::DEFAULT_WORKSPACE_ID {
        bail!("error invalid-sync-change default-workspace-protected");
    }
    if change.field.is_some()
        || change.base_version.is_some()
        || change
            .payload
            .as_object()
            .is_none_or(|payload| payload.len() != 1)
    {
        bail!("error invalid-sync-change delete-workspace shape");
    }
    required_timestamp_payload("deleted_at", &change.payload)?;

    Ok(())
}

pub(super) fn validate_create_project(change: &ChangeWire) -> Result<()> {
    ensure_entity_type(change, "project")?;
    ensure_project_id("entity_id", &change.entity_id)?;
    optional_workspace_payload(&change.payload)?;
    required_string_payload("key", &change.payload)?;
    required_string_payload("name", &change.payload)?;
    required_string_payload("prefix", &change.payload)?;
    required_string_payload("created_at", &change.payload)?;

    Ok(())
}

pub(super) fn validate_set_project_metadata(change: &ChangeWire) -> Result<()> {
    ensure_entity_type(change, "project")?;
    ensure_project_id("entity_id", &change.entity_id)?;
    optional_workspace_payload(&change.payload)?;
    required_string_payload("key", &change.payload)?;
    required_string_payload("name", &change.payload)?;
    required_string_payload("prefix", &change.payload)?;
    required_string_payload("updated_at", &change.payload)?;

    Ok(())
}

pub(super) fn validate_create_label(change: &ChangeWire) -> Result<()> {
    ensure_entity_type(change, "label")?;
    optional_workspace_payload(&change.payload)?;
    required_string_payload("name", &change.payload)?;
    required_string_payload("created_at", &change.payload)?;

    Ok(())
}

pub(super) fn validate_create_metadata_field(change: &ChangeWire) -> Result<()> {
    ensure_entity_type(change, "metadata_field")?;
    ensure_metadata_field_id("entity_id", &change.entity_id)?;
    required_workspace_payload(&change.payload)?;
    validate_metadata_key_payload("key", &change.payload)?;
    required_timestamp_payload("created_at", &change.payload)?;

    Ok(())
}

pub(super) fn validate_set_metadata_field(change: &ChangeWire) -> Result<()> {
    ensure_entity_type(change, "metadata_field")?;
    ensure_metadata_field_id("entity_id", &change.entity_id)?;
    required_workspace_payload(&change.payload)?;
    if change.field.as_deref() != Some("key") {
        bail!("error invalid-sync-change field=key");
    }
    validate_metadata_key_payload("key", &change.payload)?;
    optional_bool_payload("conflict_resolution", &change.payload)?;

    Ok(())
}

pub(super) fn validate_task_metadata(change: &ChangeWire) -> Result<()> {
    ensure_entity_type(change, "task")?;
    ensure_sync_id("entity_id", &change.entity_id)?;
    required_workspace_payload(&change.payload)?;
    let field_id = required_string_payload("field_id", &change.payload)?;
    ensure_metadata_field_id("field_id", &field_id)?;
    let expected_field = format!("metadata:{field_id}");
    if change.field.as_deref() != Some(expected_field.as_str()) {
        bail!("error invalid-sync-change metadata-field-mismatch");
    }
    validate_metadata_key_payload("key", &change.payload)?;
    optional_bool_payload("conflict_resolution", &change.payload)?;
    if change.op_type == op_type::SET_TASK_METADATA {
        validate_metadata_value_payload("value", &change.payload)?;
    }

    Ok(())
}

pub(super) fn validate_move_tasks(change: &ChangeWire) -> Result<MoveTasksPayload> {
    ensure_entity_type(change, "task_move")?;
    ensure_sync_id("entity_id", &change.entity_id)?;
    if change.field.is_some() || change.base_version.is_some() {
        bail!("error invalid-sync-change move-envelope");
    }
    let payload: MoveTasksPayload = serde_json::from_value(change.payload.clone())
        .context("error invalid-sync-change move payload")?;
    if payload.source_workspace_id == payload.target_workspace_id {
        bail!("error invalid-sync-change move-same-workspace");
    }
    if payload.tasks.is_empty() || payload.tasks.len() > MAX_MOVE_TASKS {
        bail!("error invalid-sync-change move-task-count");
    }
    let mut task_ids = HashSet::with_capacity(payload.tasks.len());
    let mut previous_task = None;
    for task in &payload.tasks {
        if previous_task
            .as_ref()
            .is_some_and(|previous| previous >= &task.task_id)
        {
            bail!("error invalid-sync-change move-task-order");
        }
        task_ids.insert(task.task_id.clone());
        previous_task = Some(task.task_id.clone());
        if task.labels.windows(2).any(|pair| pair[0] >= pair[1])
            || task.labels.iter().any(|label| {
                label.is_empty() || crate::labels::normalize_label(label) != label.as_str()
            })
        {
            bail!("error invalid-sync-change move-labels");
        }
        let mut previous_key: Option<&str> = None;
        let mut field_ids = HashSet::new();
        let mut total_bytes = 0_usize;
        for metadata in &task.metadata {
            let normalized = crate::metadata::normalize_metadata_key(&metadata.key)
                .context("error invalid-sync-change move-metadata-key")?;
            if normalized != metadata.key
                || previous_key.is_some_and(|previous| previous >= metadata.key.as_str())
                || !field_ids.insert(metadata.field_id.clone())
                || metadata
                    .value
                    .as_ref()
                    .is_some_and(|value| value.len() > crate::metadata::MAX_METADATA_VALUE_BYTES)
            {
                bail!("error invalid-sync-change move-metadata");
            }
            if let Some(version) = &metadata.version {
                ensure_sync_id("metadata version", version)?;
            }
            total_bytes += metadata.value.as_ref().map_or(0, String::len);
            previous_key = Some(&metadata.key);
        }
        if task
            .metadata
            .iter()
            .filter(|value| value.value.is_some())
            .count()
            > crate::metadata::MAX_METADATA_VALUES
            || total_bytes > crate::metadata::MAX_METADATA_TOTAL_BYTES
        {
            bail!("error invalid-sync-change move-metadata-limit");
        }
    }
    let mut dependencies = HashSet::new();
    for dependency in &payload.dependencies {
        if dependency.task_id == dependency.depends_on_task_id
            || !task_ids.contains(&dependency.task_id)
            || !task_ids.contains(&dependency.depends_on_task_id)
            || !dependencies.insert((
                dependency.task_id.clone(),
                dependency.depends_on_task_id.clone(),
            ))
        {
            bail!("error invalid-sync-change move-dependency");
        }
        validate_timestamp_value("move dependency created_at", &dependency.created_at)?;
    }
    let mut related_pairs = HashSet::new();
    for related in &payload.related {
        if related.task_a_id >= related.task_b_id
            || !task_ids.contains(&related.task_a_id)
            || !task_ids.contains(&related.task_b_id)
            || !related_pairs.insert((related.task_a_id.clone(), related.task_b_id.clone()))
        {
            bail!("error invalid-sync-change move-related");
        }
        ensure_sync_id("last_change_id", &related.last_change_id)?;
    }
    let mut epic_links = HashSet::new();
    for epic in &payload.epics {
        if epic.child_task_id == epic.epic_task_id
            || !task_ids.contains(&epic.child_task_id)
            || !task_ids.contains(&epic.epic_task_id)
            || !epic_links.insert((epic.child_task_id.clone(), epic.epic_task_id.clone()))
        {
            bail!("error invalid-sync-change move-epic");
        }
        validate_timestamp_value("move epic created_at", &epic.created_at)?;
    }
    Ok(payload)
}

pub(super) fn validate_create_task(change: &ChangeWire) -> Result<()> {
    ensure_entity_type(change, "task")?;
    ensure_sync_id("entity_id", &change.entity_id)?;
    optional_workspace_payload(&change.payload)?;
    required_string_payload("title", &change.payload)?;
    let project_id = required_string_payload("project_id", &change.payload)?;
    ensure_project_id("project_id", &project_id)?;
    optional_string_payload("description", &change.payload)?;
    let recurrence_task = change
        .payload
        .get("series_id")
        .and_then(Value::as_str)
        .is_some();
    if recurrence_task {
        let series_id = required_string_payload("series_id", &change.payload)?;
        series_id
            .parse::<RecurrenceSeriesId>()
            .map_err(|_| anyhow::anyhow!("error invalid-sync-change series_id invalid-id"))?;
        super::recurrence::validate_recurrence_task(change)?;
    } else {
        required_string_payload("project_key", &change.payload)?;
        required_string_payload("project_name", &change.payload)?;
        required_string_payload("project_prefix", &change.payload)?;
    }
    if let Some(status) = optional_string_payload("status", &change.payload)? {
        validate_sync_task_field_value(TaskField::Status, &status)?;
    }
    if let Some(priority) = optional_string_payload("priority", &change.payload)? {
        validate_sync_task_field_value(TaskField::Priority, &priority)?;
    }
    if let Some(source) = optional_string_payload("source", &change.payload)? {
        TaskSource::parse(&source).context("error invalid-sync-change invalid-task-source")?;
    }
    if let Some(available_at) = optional_string_payload("available_at", &change.payload)? {
        validate_sync_task_field_value(TaskField::AvailableAt, &available_at)?;
    }
    if let Some(due_on) = optional_string_payload("due_on", &change.payload)? {
        validate_sync_task_field_value(TaskField::DueOn, &due_on)?;
    }
    if let Some(is_epic) = optional_string_payload("is_epic", &change.payload)? {
        validate_sync_task_field_value(TaskField::IsEpic, &is_epic)?;
    }
    optional_string_array_payload("labels", &change.payload)?;
    validate_metadata_array_payload(&change.payload)?;
    optional_string_payload("created_at", &change.payload)?;

    Ok(())
}

pub(super) fn validate_task_field(change: &ChangeWire) -> Result<()> {
    ensure_entity_type(change, "task")?;
    ensure_sync_id("entity_id", &change.entity_id)?;
    optional_workspace_payload(&change.payload)?;
    let field = change
        .field
        .as_deref()
        .filter(|field| !field.trim().is_empty())
        .context("error invalid-sync-change field missing")?;
    let task_field = TaskField::parse_for_sync(field)?;
    let value = required_string_payload("value", &change.payload)?;
    validate_sync_task_field_value(task_field, &value)?;
    if task_field == TaskField::Project {
        let project_id = required_string_payload("project_id", &change.payload)?;
        ensure_project_id("project_id", &project_id)?;
        if value != project_id {
            bail!("error invalid-sync-change project-value-mismatch");
        }
        required_string_payload("project_key", &change.payload)?;
        required_string_payload("project_name", &change.payload)?;
        required_string_payload("project_prefix", &change.payload)?;
    }

    Ok(())
}

pub(super) fn validate_label_change(change: &ChangeWire) -> Result<()> {
    ensure_entity_type(change, "task")?;
    ensure_sync_id("entity_id", &change.entity_id)?;
    optional_workspace_payload(&change.payload)?;
    required_string_payload("label", &change.payload)?;

    Ok(())
}

pub(super) fn validate_note_add(change: &ChangeWire) -> Result<()> {
    ensure_entity_type(change, "task")?;
    ensure_sync_id("entity_id", &change.entity_id)?;
    optional_workspace_payload(&change.payload)?;
    let note_id = required_string_payload("note_id", &change.payload)?;
    ensure_sync_id("note_id", &note_id)?;
    required_string_payload("body", &change.payload)?;
    required_string_payload("created_at", &change.payload)?;

    Ok(())
}

pub(super) fn validate_dependency_change(change: &ChangeWire) -> Result<()> {
    ensure_entity_type(change, "task")?;
    ensure_sync_id("entity_id", &change.entity_id)?;
    required_workspace_payload(&change.payload)?;
    let depends_on_task_id = required_string_payload("depends_on_task_id", &change.payload)?;
    ensure_sync_id("depends_on_task_id", &depends_on_task_id)?;
    if change.entity_id == depends_on_task_id {
        bail!("error invalid-sync-change dependency-self");
    }

    Ok(())
}

pub(super) fn validate_related_change(change: &ChangeWire) -> Result<()> {
    ensure_entity_type(change, "task")?;
    ensure_sync_id("entity_id", &change.entity_id)?;
    required_workspace_payload(&change.payload)?;
    if change.field.as_deref() != Some("related") {
        bail!("error invalid-sync-change field=related");
    }
    let related_task_id = required_string_payload("related_task_id", &change.payload)?;
    ensure_sync_id("related_task_id", &related_task_id)?;
    if change.entity_id == related_task_id {
        bail!("error invalid-sync-change related-self");
    }

    Ok(())
}

pub(super) fn validate_epic_link_change(change: &ChangeWire) -> Result<()> {
    ensure_entity_type(change, "task")?;
    ensure_sync_id("entity_id", &change.entity_id)?;
    required_workspace_payload(&change.payload)?;
    let epic_task_id = required_string_payload("epic_task_id", &change.payload)?;
    ensure_sync_id("epic_task_id", &epic_task_id)?;
    if change.entity_id == epic_task_id {
        bail!("error invalid-sync-change epic-self");
    }
    if change.op_type == op_type::EPIC_LINK_ADD {
        required_timestamp_payload("created_at", &change.payload)?;
    }

    Ok(())
}

pub(super) fn validate_project_delete(change: &ChangeWire) -> Result<()> {
    ensure_entity_type(change, "project")?;
    ensure_project_id("entity_id", &change.entity_id)?;
    required_workspace_payload(&change.payload)?;
    required_timestamp_payload("deleted_at", &change.payload)?;

    Ok(())
}

pub(super) fn validate_set_label_name(change: &ChangeWire) -> Result<()> {
    ensure_entity_type(change, "label")?;
    required_workspace_payload(&change.payload)?;
    let name = required_string_payload("name", &change.payload)?;
    if name != change.entity_id {
        bail!("error invalid-sync-change label-value-mismatch");
    }
    required_string_payload("new_name", &change.payload)?;
    required_timestamp_payload("renamed_at", &change.payload)?;

    Ok(())
}

pub(super) fn validate_label_delete(change: &ChangeWire) -> Result<()> {
    ensure_entity_type(change, "label")?;
    required_workspace_payload(&change.payload)?;
    let name = required_string_payload("name", &change.payload)?;
    if name != change.entity_id {
        bail!("error invalid-sync-change label-value-mismatch");
    }
    required_timestamp_payload("deleted_at", &change.payload)?;

    Ok(())
}

pub(super) fn validate_note_edit(change: &ChangeWire) -> Result<()> {
    ensure_entity_type(change, "task")?;
    ensure_sync_id("entity_id", &change.entity_id)?;
    if change.field.as_deref() != Some("notes") {
        bail!("error invalid-sync-change field=notes");
    }
    required_workspace_payload(&change.payload)?;
    let note_id = required_string_payload("note_id", &change.payload)?;
    ensure_sync_id("note_id", &note_id)?;
    required_string_payload("body", &change.payload)?;
    required_timestamp_payload("edited_at", &change.payload)?;

    Ok(())
}

pub(super) fn validate_label_restore(change: &ChangeWire) -> Result<()> {
    ensure_entity_type(change, "label")?;
    required_workspace_payload(&change.payload)?;
    let name = required_string_payload("name", &change.payload)?;
    if name != change.entity_id {
        bail!("error invalid-sync-change label-value-mismatch");
    }
    required_timestamp_payload("created_at", &change.payload)?;
    required_timestamp_payload("restored_at", &change.payload)?;
    for key in ["task_ids", "series_ids"] {
        optional_string_array_payload(key, &change.payload)?;
        if change.payload.get(key).is_none() {
            bail!("error invalid-sync-change payload.{key} missing");
        }
    }
    for task_id in string_array_payload("task_ids", &change.payload)? {
        ensure_sync_id("task_ids", &task_id)?;
    }
    for series_id in string_array_payload("series_ids", &change.payload)? {
        ensure_sync_id("series_ids", &series_id)?;
    }

    Ok(())
}

pub(super) fn validate_note_delete(change: &ChangeWire) -> Result<()> {
    ensure_entity_type(change, "task")?;
    ensure_sync_id("entity_id", &change.entity_id)?;
    if change.field.as_deref() != Some("notes") {
        bail!("error invalid-sync-change field=notes");
    }
    required_workspace_payload(&change.payload)?;
    let note_id = required_string_payload("note_id", &change.payload)?;
    ensure_sync_id("note_id", &note_id)?;
    required_timestamp_payload("deleted_at", &change.payload)?;

    Ok(())
}

pub(super) fn validate_recurrence_metadata(change: &ChangeWire) -> Result<()> {
    ensure_entity_type(change, "recurrence_series")?;
    change
        .entity_id
        .parse::<RecurrenceSeriesId>()
        .map_err(|_| anyhow::anyhow!("error invalid-sync-change entity_id invalid-id"))?;
    required_workspace_payload(&change.payload)?;
    let field_id = required_string_payload("field_id", &change.payload)?;
    ensure_metadata_field_id("field_id", &field_id)?;
    let expected_field = format!("metadata:{field_id}");
    if change.field.as_deref() != Some(expected_field.as_str()) {
        bail!("error invalid-sync-change metadata-field-mismatch");
    }
    validate_metadata_key_payload("key", &change.payload)?;
    optional_bool_payload("conflict_resolution", &change.payload)?;
    if change.op_type == op_type::SET_RECURRENCE_METADATA {
        validate_metadata_value_payload("value", &change.payload)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{make_change_wire, test_workspace};
    use super::super::validate_pushed_change;
    use crate::change_log::{ChangePayload, op_type};

    fn task_draft(title: &str, is_epic: bool) -> crate::operations::TaskDraft {
        crate::operations::TaskDraft {
            title: title.into(),
            description: "task body".into(),
            project: Some("app".into()),
            status: "todo".into(),
            priority: "high".into(),
            source: crate::choices::TaskSource::Cli,
            labels: Vec::new(),
            metadata: Vec::new(),
            available_at: None,
            due_on: None,
            is_epic,
        }
    }

    fn metadata(value: &str) -> crate::metadata::TaskMetadataInput {
        crate::metadata::TaskMetadataInput {
            expected_field_id: None,
            key: "owner".into(),
            value: value.into(),
        }
    }

    async fn exercise_task_operations(
        db: &crate::db::Database,
        ws: &crate::workspaces::Workspace,
        blob_dir: &std::path::Path,
    ) {
        use crate::operations::{AttachmentAddInput, TaskUpdate};

        db.create_project(ws, "app", None).await.unwrap();
        db.create_label(ws, "bug").await.unwrap();
        let a = db
            .create_task(ws, task_draft("first", false))
            .await
            .unwrap()
            .task
            .id;
        let b = db
            .create_task(ws, task_draft("second", false))
            .await
            .unwrap()
            .task
            .id;
        let epic = db
            .create_task(ws, task_draft("epic", true))
            .await
            .unwrap()
            .task
            .id;
        db.update_task(
            ws,
            &a,
            TaskUpdate {
                title: Some("renamed task".into()),
                add_labels: vec!["bug".into()],
                set_metadata: vec![metadata("Alice")],
                ..TaskUpdate::default()
            },
        )
        .await
        .unwrap();
        db.update_task(
            ws,
            &a,
            TaskUpdate {
                remove_labels: vec!["bug".into()],
                remove_metadata: vec!["owner".into()],
                ..TaskUpdate::default()
            },
        )
        .await
        .unwrap();
        db.update_task(
            ws,
            &b,
            TaskUpdate {
                add_labels: vec!["bug".into()],
                ..TaskUpdate::default()
            },
        )
        .await
        .unwrap();
        db.add_task_dependency(ws, &a, &b).await.unwrap();
        db.remove_task_dependency(ws, &a, &b).await.unwrap();
        db.add_task_related_link(ws, &a, &b).await.unwrap();
        db.remove_task_related_link(ws, &a, &b).await.unwrap();
        db.add_task_to_epic(ws, &a, &epic).await.unwrap();
        db.remove_task_from_epic(ws, &a, &epic).await.unwrap();
        let note = db.add_note(ws, &a, "note body".into()).await.unwrap();
        db.edit_note(ws, &a, &note.note_id, "edited body".into())
            .await
            .unwrap();
        db.delete_note(ws, &a, &note.note_id).await.unwrap();

        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::new(2, 1))
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let attachment = db
            .add_task_attachment(
                ws,
                blob_dir,
                crate::attachments::lifecycle::LifecyclePolicy::default(),
                &a,
                AttachmentAddInput {
                    filename: Some("example.png".into()),
                    alt_text: Some("two pixels".into()),
                    declared_media_type: Some("image/png".into()),
                    bytes: png.into_inner(),
                    optimization_policy:
                        crate::attachments::optimization::ImageOptimizationPolicy::Preserve,
                    dedupe_existing: false,
                },
            )
            .await
            .unwrap();
        db.delete_task_attachment(ws, &attachment.outcome.attachment.attachment_id)
            .await
            .unwrap();

        // A persisted conflict is a precondition for the real resolution operation.
        let mut conn = db.acquire_writer().await.unwrap();
        sqlx::query(
            "INSERT INTO conflicts(workspace_id, entity_type, entity_id, task_id, field,
             base_version, local_value, remote_value, local_change_id, remote_change_id,
             variant_a, variant_b, created_at, resolved)
             SELECT ?, 'task', ?, ?, 'title', NULL, 'renamed task', 'remote',
             version, ?, 'local', 'remote', ?, 0 FROM field_versions
             WHERE workspace_id = ? AND entity_type = 'task' AND entity_id = ? AND field = 'title'",
        )
        .bind(&ws.id)
        .bind(&a)
        .bind(&a)
        .bind(crate::ids::new_id())
        .bind(crate::ids::now())
        .bind(&ws.id)
        .bind(&a)
        .execute(&mut *conn)
        .await
        .unwrap();
        drop(conn);
        db.resolve_conflict(ws, &a, "title", "resolved")
            .await
            .unwrap();

        let destination = db.create_workspace("destination").await.unwrap();
        db.create_project(&destination, "target", None)
            .await
            .unwrap();
        db.move_tasks(
            ws,
            crate::operations::MoveTasksInput {
                task_ids: vec![a],
                target_workspace: destination,
                target_project: "target".into(),
            },
        )
        .await
        .unwrap();
    }

    async fn exercise_administration_operations(
        db: &crate::db::Database,
        ws: &crate::workspaces::Workspace,
    ) {
        db.rename_project(ws, "app", "renamed app", Some("APP"))
            .await
            .unwrap();
        db.create_project(ws, "disposable", None).await.unwrap();
        db.delete_project(ws, "disposable").await.unwrap();
        db.rename_label_with_tui_undo(ws, "bug", "defect")
            .await
            .unwrap();
        db.delete_label_with_tui_undo(ws, "defect").await.unwrap();
        assert!(db.apply_latest_tui_undo(&ws.id).await.unwrap().is_some());
        db.rename_metadata_field(ws, "owner", "assignee")
            .await
            .unwrap();
        db.rename_workspace(&ws.key, "renamed workspace")
            .await
            .unwrap();
        db.publish_device_label([7; 32], "Office Mac")
            .await
            .unwrap();
        crate::test_support::set_established_protocol(db, 19)
            .await
            .unwrap();
        let disposable = db.create_workspace("Disposable").await.unwrap();
        db.delete_workspace(&disposable.id, &[], false)
            .await
            .unwrap();
    }

    async fn exercise_recurrence_operations(
        db: &crate::db::Database,
        ws: &crate::workspaces::Workspace,
    ) {
        use crate::operations::{
            CreateRecurrenceSeriesParams, RecurrenceSeriesDraft, RecurrenceTemplateUpdate,
            UpdateRecurrenceTemplateParams,
        };
        use crate::recurrence::{
            RecurrenceDuePolicy, RecurrenceOutcome, RecurrenceRule, RecurrenceSchedule,
        };

        let at = chrono::Utc::now();
        let created = db
            .create_recurrence_series(
                ws,
                CreateRecurrenceSeriesParams::new(RecurrenceSeriesDraft {
                    title: "daily review".into(),
                    description: "review tasks".into(),
                    project: "app".into(),
                    priority: "high".into(),
                    initial_status: "todo".into(),
                    labels: vec!["bug".into()],
                    metadata: Vec::new(),
                    schedule: RecurrenceSchedule::new(
                        RecurrenceRule::daily(),
                        "UTC".parse().unwrap(),
                        at.date_naive(),
                        None,
                        RecurrenceDuePolicy::SameDay,
                    ),
                })
                .at(at),
            )
            .await
            .unwrap();
        db.update_recurrence_template(
            ws,
            &created.series.id,
            UpdateRecurrenceTemplateParams::new(RecurrenceTemplateUpdate {
                title: Some("updated review".into()),
                set_metadata: vec![metadata("Bob")],
                ..RecurrenceTemplateUpdate::default()
            }),
        )
        .await
        .unwrap();
        db.update_recurrence_template(
            ws,
            &created.series.id,
            UpdateRecurrenceTemplateParams::new(RecurrenceTemplateUpdate {
                remove_metadata: vec!["owner".into()],
                ..RecurrenceTemplateUpdate::default()
            }),
        )
        .await
        .unwrap();
        db.resolve_recurrence_occurrence(ws, &created.task.id, RecurrenceOutcome::Completed)
            .await
            .unwrap();
        db.pause_recurrence_series(ws, &created.series.id)
            .await
            .unwrap();
        db.resume_recurrence_series(ws, &created.series.id, chrono::Utc::now())
            .await
            .unwrap();
        db.stop_recurrence_series(ws, &created.series.id, true)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn real_operations_emit_wire_valid_changes_for_every_registered_operation() {
        let directory = tempfile::tempdir().unwrap();
        let db =
            crate::test_support::open_blank_database(&directory.path().join("operations.sqlite"))
                .await
                .unwrap();
        let ws = db.resolve_workspace("default").await.unwrap();
        Box::pin(exercise_task_operations(
            &db,
            &ws,
            &directory.path().join("blobs"),
        ))
        .await;
        Box::pin(exercise_recurrence_operations(&db, &ws)).await;
        Box::pin(exercise_administration_operations(&db, &ws)).await;

        let exported = db.export_data(crate::ids::now()).await.unwrap();
        let mut covered = std::collections::BTreeSet::new();
        for row in exported
            .tables
            .changes
            .into_iter()
            .filter(|row| row.server_seq.is_none())
        {
            let mut value = serde_json::to_value(&row).unwrap();
            value["payload"] = serde_json::from_str(&row.payload).unwrap();
            let change: super::super::ChangeWire = serde_json::from_value(value).unwrap();
            validate_pushed_change(&change).unwrap_or_else(|error| {
                panic!(
                    "{} emitted invalid change: {error}; {change:?}",
                    change.op_type
                )
            });
            if change.op_type == op_type::LABEL_RESTORE {
                // Exercise restoration of both task and series memberships.
                for key in ["task_ids", "series_ids"] {
                    assert!(!change.payload[key].as_array().unwrap().is_empty(), "{key}");
                }
            }
            covered.insert(change.op_type);
        }
        assert_eq!(
            covered,
            op_type::ALL.iter().map(|op| (*op).to_owned()).collect()
        );
    }

    #[test]
    fn project_changes_reject_invalid_project_ids() {
        let ws = test_workspace();
        let project_payload = ChangePayload::workspace(&ws)
            .set("key", "app")
            .set("name", "App")
            .set("prefix", "APP")
            .set("created_at", "2026-06-01T00:00:00Z")
            .into_value();
        let create_project = make_change_wire(
            op_type::CREATE_PROJECT,
            "project",
            "invalid",
            project_payload,
        );
        assert_eq!(
            validate_pushed_change(&create_project)
                .unwrap_err()
                .to_string(),
            "error invalid-sync-change entity_id invalid-id"
        );

        let task_payload = ChangePayload::workspace(&ws)
            .set("title", "test task")
            .set("project_id", "invalid")
            .set("project_key", "app")
            .set("project_name", "App")
            .set("project_prefix", "APP")
            .set("created_at", "2026-06-01T00:00:00Z")
            .into_value();
        let create_task = make_change_wire(
            op_type::CREATE_TASK,
            "task",
            "BBBBBBBBBBBBBBBB",
            task_payload,
        );
        assert_eq!(
            validate_pushed_change(&create_task)
                .unwrap_err()
                .to_string(),
            "error invalid-sync-change project_id invalid-id"
        );

        let field_payload = ChangePayload::workspace(&ws)
            .set("value", "invalid")
            .set("project_id", "invalid")
            .set("project_key", "app")
            .set("project_name", "App")
            .set("project_prefix", "APP")
            .into_value();
        let mut set_field = make_change_wire(
            op_type::SET_FIELD,
            "task",
            "BBBBBBBBBBBBBBBB",
            field_payload,
        );
        set_field.field = Some("project".to_string());
        assert_eq!(
            validate_pushed_change(&set_field).unwrap_err().to_string(),
            "error invalid-sync-change project_id invalid-id"
        );
    }

    #[test]
    fn related_payload_validation_requires_distinct_task_endpoints() {
        let ws = test_workspace();
        let payload = ChangePayload::workspace(&ws)
            .set("related_task_id", "CCCCCCCCCCCCCCCC")
            .into_value();
        let mut change =
            make_change_wire(op_type::RELATED_ADD, "task", "BBBBBBBBBBBBBBBB", payload);
        change.field = Some("related".to_string());
        validate_pushed_change(&change)
            .expect("related_add payload built with ChangePayload should be wire-valid");

        change.payload["related_task_id"] = serde_json::json!("BBBBBBBBBBBBBBBB");
        assert_eq!(
            validate_pushed_change(&change).unwrap_err().to_string(),
            "error invalid-sync-change related-self"
        );
    }
}
