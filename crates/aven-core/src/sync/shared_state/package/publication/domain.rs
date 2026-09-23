// This schema is owned by the experimental bootstrap profile, not export JSON.
use super::codec::*;
use super::{Error, Result};
use crate::data_safety::export_types as local;
use crate::ids::{MetadataFieldId, ProjectId, TaskId, WorkspaceId};
use crate::recurrence::RecurrenceSeriesId;
use serde::{Deserialize, Serialize};

macro_rules! row {
    ($name:ident { $($field:ident : $ty:ty),* $(,)? }) => {
        #[derive(Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        struct $name { $($field: $ty),* }
        impl From<&local::$name> for $name {
            #[allow(clippy::clone_on_copy)]
            fn from(value: &local::$name) -> Self {
                Self { $($field: value.$field.clone()),* }
            }
        }
        impl From<$name> for local::$name {
            fn from(value: $name) -> Self { Self { $($field: value.$field),* } }
        }
    };
}

row! { WorkspaceRow {
    id: WorkspaceId,
    name: String,
    key: String,
    created_at: String,
    updated_at: String,
    archived: i64,
} }

row! { ProjectRow {
    id: ProjectId,
    workspace_id: WorkspaceId,
    key: String,
    name: String,
    prefix: String,
    created_at: String,
    updated_at: String,
    deleted: i64,
} }

row! { ProjectIdAliasRow {
    workspace_id: WorkspaceId,
    remote_project_id: ProjectId,
    local_project_id: ProjectId,
} }

row! { LabelRow {
    workspace_id: WorkspaceId,
    name: String,
    created_at: String,
} }

row! { MetadataFieldRow {
    id: MetadataFieldId,
    workspace_id: WorkspaceId,
    key: String,
    created_at: String,
    updated_at: String,
} }

row! { MetadataFieldIdAliasRow {
    workspace_id: WorkspaceId,
    remote_field_id: MetadataFieldId,
    local_field_id: MetadataFieldId,
} }

row! { TaskRow {
    workspace_id: WorkspaceId,
    id: TaskId,
    title: String,
    description: String,
    project_id: ProjectId,
    status: String,
    priority: String,
    source: String,
    created_at: String,
    updated_at: String,
    queue_activity_at: String,
    available_at: String,
    due_on: String,
    deleted: i64,
    is_epic: i64,
} }

row! { TaskMetadataRow {
    workspace_id: WorkspaceId,
    task_id: TaskId,
    field_id: MetadataFieldId,
    value: String,
    created_at: String,
    updated_at: String,
} }

row! { TaskLabelRow {
    workspace_id: WorkspaceId,
    task_id: TaskId,
    label: String,
} }

row! { NoteRow {
    workspace_id: WorkspaceId,
    id: String,
    task_id: TaskId,
    body: String,
    created_at: String,
    change_id: String,
} }

row! { TaskDependencyRow {
    workspace_id: WorkspaceId,
    task_id: TaskId,
    depends_on_task_id: TaskId,
    created_at: String,
} }

row! { TaskEpicLinkRow {
    workspace_id: WorkspaceId,
    child_task_id: TaskId,
    epic_task_id: TaskId,
    created_at: String,
} }

row! { TaskRelatedLinkRow {
    workspace_id: WorkspaceId,
    task_a_id: TaskId,
    task_b_id: TaskId,
    linked: i64,
    last_change_id: String,
} }

row! { TaskAttachmentRow {
    workspace_id: WorkspaceId,
    attachment_id: String,
    task_id: TaskId,
    sha256: String,
    byte_size: i64,
    media_type: String,
    filename: Option<String>,
    alt_text: Option<String>,
    width: Option<i64>,
    height: Option<i64>,
    created_at: String,
    created_by_change_id: Option<String>,
    deleted: i64,
    deleted_at: Option<String>,
    deleted_by_change_id: Option<String>,
} }

row! { RecurrenceSeriesRow {
    workspace_id: WorkspaceId,
    id: RecurrenceSeriesId,
    title: String,
    description: String,
    project_id: ProjectId,
    priority: String,
    initial_status: String,
    frequency: String,
    interval: i64,
    weekdays: String,
    timezone: String,
    start_on: String,
    available_local_time: String,
    due_policy: String,
    state: String,
    stopped_at: String,
    created_at: String,
    updated_at: String,
    deleted: i64,
} }

row! { RecurrenceSeriesLabelRow {
    workspace_id: WorkspaceId,
    series_id: RecurrenceSeriesId,
    label: String,
} }

row! { RecurrenceSeriesMetadataRow {
    workspace_id: WorkspaceId,
    series_id: RecurrenceSeriesId,
    field_id: MetadataFieldId,
    value: String,
    created_at: String,
    updated_at: String,
} }

row! { RecurrenceOccurrenceRow {
    workspace_id: WorkspaceId,
    series_id: RecurrenceSeriesId,
    slot_on: String,
    task_id: String,
    outcome: String,
    resolved_at: String,
    outcome_change_id: String,
    projection_state: String,
    archived_at: String,
} }

row! { RecurrencePauseIntervalRow {
    workspace_id: WorkspaceId,
    id: String,
    series_id: RecurrenceSeriesId,
    paused_at: String,
    resumed_at: String,
    suspended_slot_on: String,
    suspended_task_id: String,
    created_by_change_id: String,
    resolved_by_change_id: String,
} }

row! { ChangeRow {
    change_id: String,
    client_id: String,
    local_seq: i64,
    entity_type: String,
    entity_id: String,
    field: Option<String>,
    op_type: String,
    payload: String,
    base_version: Option<String>,
    created_at: String,
    server_seq: Option<i64>,
} }

row! { SharedHistoryProvenanceRow {
    change_id: String,
    source_server_seq: Option<i64>,
    source_pending_rank: Option<i64>,
} }

row! { FieldVersionRow {
    workspace_id: WorkspaceId,
    entity_type: String,
    entity_id: String,
    field: String,
    version: String,
} }

row! { ConflictRow {
    id: i64,
    workspace_id: WorkspaceId,
    entity_type: String,
    entity_id: String,
    task_id: String,
    field: String,
    base_version: Option<String>,
    local_value: String,
    remote_value: String,
    local_change_id: Option<String>,
    remote_change_id: String,
    variant_a: String,
    variant_b: String,
    created_at: String,
    resolved: i64,
} }

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImageFact {
    sha256: String,
    byte_size: i64,
    media_type: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EpicBaseline {
    identity: String,
    value: String,
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Mapping {
    pub sha256: String,
    pub classification: String,
    pub object: Option<[u8; 32]>,
}

pub(super) const SECTIONS: usize = 26;
pub(super) type Stats = [(u64, u64); SECTIONS];

// JSON payloads have fixed struct field order, all fields present (including
// null), no whitespace, and serde_json string/integer encoding. Re-encoding
// rejects alternate encodings, unknown/duplicate fields and omitted nulls.
fn payload<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    struct Bounded {
        bytes: Vec<u8>,
        refused: bool,
    }
    impl std::io::Write for Bounded {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self
                .bytes
                .len()
                .checked_add(bytes.len())
                .is_none_or(|n| n as u64 > STATE_LIMIT)
                || self.bytes.try_reserve(bytes.len()).is_err()
            {
                self.refused = true;
                return Err(std::io::ErrorKind::OutOfMemory.into());
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = Bounded {
        bytes: Vec::new(),
        refused: false,
    };
    serde_json::to_writer(&mut writer, value).map_err(|_| {
        if writer.refused {
            Error::ResourceLimit
        } else {
            Error::Invalid
        }
    })?;
    Ok(writer.bytes)
}

fn section<T: Serialize>(out: &mut Vec<u8>, kind: usize, values: &[T]) -> Result<(u64, u64)> {
    bound(number(values.len())?, RECORD_LIMIT)?;
    let mut rows = Vec::new();
    let mut total = add(number(out.len())?, 10)?;
    for value in values {
        let row = payload(value)?;
        total = add(total, add(8, number(row.len())?)?)?;
        bound(total, STATE_LIMIT)?;
        rows.push(row);
    }
    rows.sort();
    valid(rows.windows(2).all(|pair| pair[0] < pair[1]))?;
    let start = out.len();
    out.extend_from_slice(&(kind as u16).to_be_bytes());
    u64_bytes(out, number(rows.len())?);
    for row in rows {
        bound(
            add(number(out.len())?, add(8, number(row.len())?)?)?,
            STATE_LIMIT,
        )?;
        bytes(out, &row)?;
    }
    Ok((number(values.len())?, number(out.len() - start)?))
}

fn read_section<T: Serialize + for<'de> Deserialize<'de>>(
    r: &mut Reader<'_>,
    kind: usize,
    remaining: &mut u64,
) -> Result<(Vec<T>, (u64, u64))> {
    let start = r.0.len();
    valid(u16::from_be_bytes(r.array()?) == kind as u16)?;
    let n = r.u64()?;
    bound(n, *remaining)?;
    *remaining -= n;
    valid(n <= number(r.0.len())? / 8)?;
    let mut rows = Vec::new();
    let mut previous: Option<&[u8]> = None;
    for _ in 0..n {
        let raw = r.bytes(STATE_LIMIT)?;
        valid(previous.is_none_or(|p| p < raw))?;
        let value: T = serde_json::from_slice(raw).map_err(|_| Error::Invalid)?;
        valid(payload(&value)? == raw)?;
        previous = Some(raw);
        rows.push(value);
    }
    Ok((rows, (n, number(start - r.0.len())?)))
}

pub(super) fn encode(t: &local::ExportTables, mappings: &[Mapping]) -> Result<(Vec<u8>, Stats)> {
    let mut out = b"AVBD\0\x01".to_vec();
    let mut stats = [(0, 0); SECTIONS];
    stats[0] = section(
        &mut out,
        1,
        &t.workspaces
            .iter()
            .map(WorkspaceRow::from)
            .collect::<Vec<_>>(),
    )?;
    stats[1] = section(
        &mut out,
        2,
        &t.projects.iter().map(ProjectRow::from).collect::<Vec<_>>(),
    )?;
    stats[2] = section(
        &mut out,
        3,
        &t.project_id_aliases
            .iter()
            .map(ProjectIdAliasRow::from)
            .collect::<Vec<_>>(),
    )?;
    stats[3] = section(
        &mut out,
        4,
        &t.labels.iter().map(LabelRow::from).collect::<Vec<_>>(),
    )?;
    stats[4] = section(
        &mut out,
        5,
        &t.metadata_fields
            .iter()
            .map(MetadataFieldRow::from)
            .collect::<Vec<_>>(),
    )?;
    stats[5] = section(
        &mut out,
        6,
        &t.metadata_field_id_aliases
            .iter()
            .map(MetadataFieldIdAliasRow::from)
            .collect::<Vec<_>>(),
    )?;
    stats[6] = section(
        &mut out,
        7,
        &t.tasks.iter().map(TaskRow::from).collect::<Vec<_>>(),
    )?;
    stats[7] = section(
        &mut out,
        8,
        &t.task_metadata
            .iter()
            .map(TaskMetadataRow::from)
            .collect::<Vec<_>>(),
    )?;
    stats[8] = section(
        &mut out,
        9,
        &t.task_labels
            .iter()
            .map(TaskLabelRow::from)
            .collect::<Vec<_>>(),
    )?;
    stats[9] = section(
        &mut out,
        10,
        &t.notes.iter().map(NoteRow::from).collect::<Vec<_>>(),
    )?;
    stats[10] = section(
        &mut out,
        11,
        &t.task_dependencies
            .iter()
            .map(TaskDependencyRow::from)
            .collect::<Vec<_>>(),
    )?;
    stats[11] = section(
        &mut out,
        12,
        &t.task_epic_links
            .iter()
            .map(TaskEpicLinkRow::from)
            .collect::<Vec<_>>(),
    )?;
    stats[12] = section(
        &mut out,
        13,
        &t.task_related_links
            .iter()
            .map(TaskRelatedLinkRow::from)
            .collect::<Vec<_>>(),
    )?;
    stats[13] = section(
        &mut out,
        14,
        &t.task_attachments
            .iter()
            .map(TaskAttachmentRow::from)
            .collect::<Vec<_>>(),
    )?;
    stats[14] = section(
        &mut out,
        15,
        &t.recurrence_series
            .iter()
            .map(RecurrenceSeriesRow::from)
            .collect::<Vec<_>>(),
    )?;
    stats[15] = section(
        &mut out,
        16,
        &t.recurrence_series_labels
            .iter()
            .map(RecurrenceSeriesLabelRow::from)
            .collect::<Vec<_>>(),
    )?;
    stats[16] = section(
        &mut out,
        17,
        &t.recurrence_series_metadata
            .iter()
            .map(RecurrenceSeriesMetadataRow::from)
            .collect::<Vec<_>>(),
    )?;
    stats[17] = section(
        &mut out,
        18,
        &t.recurrence_occurrences
            .iter()
            .map(RecurrenceOccurrenceRow::from)
            .collect::<Vec<_>>(),
    )?;
    stats[18] = section(
        &mut out,
        19,
        &t.recurrence_pause_intervals
            .iter()
            .map(RecurrencePauseIntervalRow::from)
            .collect::<Vec<_>>(),
    )?;
    stats[19] = section(
        &mut out,
        20,
        &t.changes.iter().map(ChangeRow::from).collect::<Vec<_>>(),
    )?;
    stats[20] = section(
        &mut out,
        21,
        &t.shared_history_provenance
            .iter()
            .map(SharedHistoryProvenanceRow::from)
            .collect::<Vec<_>>(),
    )?;
    stats[21] = section(
        &mut out,
        22,
        &t.field_versions
            .iter()
            .map(FieldVersionRow::from)
            .collect::<Vec<_>>(),
    )?;
    stats[22] = section(
        &mut out,
        23,
        &t.conflicts
            .iter()
            .map(ConflictRow::from)
            .collect::<Vec<_>>(),
    )?;
    stats[23] = section(
        &mut out,
        24,
        &t.blob_inventory
            .iter()
            .map(|r| ImageFact {
                sha256: r.sha256.clone(),
                byte_size: r.byte_size,
                media_type: r.media_type.clone(),
            })
            .collect::<Vec<_>>(),
    )?;
    let mut baselines = Vec::new();
    for row in &t.meta {
        let identity = row
            .key
            .strip_prefix("epic_membership_baseline:")
            .ok_or(Error::Invalid)?
            .to_owned();
        baselines.push(EpicBaseline {
            identity,
            value: row.value.clone(),
        });
    }
    stats[24] = section(&mut out, 25, &baselines)?;
    stats[25] = section(&mut out, 26, mappings)?;
    bound(
        stats.iter().try_fold(0, |n, (count, _)| add(n, *count))?,
        RECORD_LIMIT,
    )?;
    Ok((out, stats))
}

pub(super) fn decode(input: &[u8]) -> Result<(local::ExportTables, Vec<Mapping>, Stats)> {
    bound(number(input.len())?, STATE_LIMIT)?;
    let mut r = Reader(input);
    let mut remaining = RECORD_LIMIT;
    valid(r.take(6)? == b"AVBD\0\x01")?;
    let mut stats = [(0, 0); SECTIONS];
    let (workspaces, stat) = read_section::<WorkspaceRow>(&mut r, 1, &mut remaining)?;
    stats[0] = stat;
    let (projects, stat) = read_section::<ProjectRow>(&mut r, 2, &mut remaining)?;
    stats[1] = stat;
    let (project_id_aliases, stat) = read_section::<ProjectIdAliasRow>(&mut r, 3, &mut remaining)?;
    stats[2] = stat;
    let (labels, stat) = read_section::<LabelRow>(&mut r, 4, &mut remaining)?;
    stats[3] = stat;
    let (metadata_fields, stat) = read_section::<MetadataFieldRow>(&mut r, 5, &mut remaining)?;
    stats[4] = stat;
    let (metadata_field_id_aliases, stat) =
        read_section::<MetadataFieldIdAliasRow>(&mut r, 6, &mut remaining)?;
    stats[5] = stat;
    let (tasks, stat) = read_section::<TaskRow>(&mut r, 7, &mut remaining)?;
    stats[6] = stat;
    let (task_metadata, stat) = read_section::<TaskMetadataRow>(&mut r, 8, &mut remaining)?;
    stats[7] = stat;
    let (task_labels, stat) = read_section::<TaskLabelRow>(&mut r, 9, &mut remaining)?;
    stats[8] = stat;
    let (notes, stat) = read_section::<NoteRow>(&mut r, 10, &mut remaining)?;
    stats[9] = stat;
    let (task_dependencies, stat) = read_section::<TaskDependencyRow>(&mut r, 11, &mut remaining)?;
    stats[10] = stat;
    let (task_epic_links, stat) = read_section::<TaskEpicLinkRow>(&mut r, 12, &mut remaining)?;
    stats[11] = stat;
    let (task_related_links, stat) =
        read_section::<TaskRelatedLinkRow>(&mut r, 13, &mut remaining)?;
    stats[12] = stat;
    let (task_attachments, stat) = read_section::<TaskAttachmentRow>(&mut r, 14, &mut remaining)?;
    stats[13] = stat;
    let (recurrence_series, stat) =
        read_section::<RecurrenceSeriesRow>(&mut r, 15, &mut remaining)?;
    stats[14] = stat;
    let (recurrence_series_labels, stat) =
        read_section::<RecurrenceSeriesLabelRow>(&mut r, 16, &mut remaining)?;
    stats[15] = stat;
    let (recurrence_series_metadata, stat) =
        read_section::<RecurrenceSeriesMetadataRow>(&mut r, 17, &mut remaining)?;
    stats[16] = stat;
    let (recurrence_occurrences, stat) =
        read_section::<RecurrenceOccurrenceRow>(&mut r, 18, &mut remaining)?;
    stats[17] = stat;
    let (recurrence_pause_intervals, stat) =
        read_section::<RecurrencePauseIntervalRow>(&mut r, 19, &mut remaining)?;
    stats[18] = stat;
    let (changes, stat) = read_section::<ChangeRow>(&mut r, 20, &mut remaining)?;
    stats[19] = stat;
    let (shared_history_provenance, stat) =
        read_section::<SharedHistoryProvenanceRow>(&mut r, 21, &mut remaining)?;
    stats[20] = stat;
    let (field_versions, stat) = read_section::<FieldVersionRow>(&mut r, 22, &mut remaining)?;
    stats[21] = stat;
    let (conflicts, stat) = read_section::<ConflictRow>(&mut r, 23, &mut remaining)?;
    stats[22] = stat;
    let (images, stat) = read_section::<ImageFact>(&mut r, 24, &mut remaining)?;
    stats[23] = stat;
    let (baselines, stat) = read_section::<EpicBaseline>(&mut r, 25, &mut remaining)?;
    stats[24] = stat;
    let (mappings, stat) = read_section::<Mapping>(&mut r, 26, &mut remaining)?;
    stats[25] = stat;
    r.end()?;
    bound(
        stats.iter().try_fold(0, |n, (count, _)| add(n, *count))?,
        RECORD_LIMIT,
    )?;
    let tables = local::ExportTables {
        workspaces: workspaces.into_iter().map(Into::into).collect(),
        projects: projects.into_iter().map(Into::into).collect(),
        project_id_aliases: project_id_aliases.into_iter().map(Into::into).collect(),
        labels: labels.into_iter().map(Into::into).collect(),
        metadata_fields: metadata_fields.into_iter().map(Into::into).collect(),
        metadata_field_id_aliases: metadata_field_id_aliases
            .into_iter()
            .map(Into::into)
            .collect(),
        tasks: tasks.into_iter().map(Into::into).collect(),
        task_metadata: task_metadata.into_iter().map(Into::into).collect(),
        task_labels: task_labels.into_iter().map(Into::into).collect(),
        notes: notes.into_iter().map(Into::into).collect(),
        task_dependencies: task_dependencies.into_iter().map(Into::into).collect(),
        task_epic_links: task_epic_links.into_iter().map(Into::into).collect(),
        task_related_links: task_related_links.into_iter().map(Into::into).collect(),
        task_attachments: task_attachments.into_iter().map(Into::into).collect(),
        recurrence_series: recurrence_series.into_iter().map(Into::into).collect(),
        recurrence_series_labels: recurrence_series_labels
            .into_iter()
            .map(Into::into)
            .collect(),
        recurrence_series_metadata: recurrence_series_metadata
            .into_iter()
            .map(Into::into)
            .collect(),
        recurrence_occurrences: recurrence_occurrences.into_iter().map(Into::into).collect(),
        recurrence_pause_intervals: recurrence_pause_intervals
            .into_iter()
            .map(Into::into)
            .collect(),
        changes: changes.into_iter().map(Into::into).collect(),
        shared_history_provenance: shared_history_provenance
            .into_iter()
            .map(Into::into)
            .collect(),
        field_versions: field_versions.into_iter().map(Into::into).collect(),
        conflicts: conflicts.into_iter().map(Into::into).collect(),
        project_paths: Vec::new(),
        blob_inventory: images
            .into_iter()
            .map(|r| local::BlobInventoryExportRow {
                sha256: r.sha256,
                byte_size: r.byte_size,
                media_type: r.media_type,
                available: 0,
                first_seen_at: String::new(),
                last_verified_at: None,
            })
            .collect(),
        meta: baselines
            .into_iter()
            .map(|r| local::MetaRow {
                key: format!("epic_membership_baseline:{}", r.identity),
                value: r.value,
            })
            .collect(),
    };
    Ok((tables, mappings, stats))
}
