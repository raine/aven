use std::collections::BTreeMap;

use serde::Serialize;

use crate::query::{TaskDependencyLink, TaskDependencySummary, TaskListItem};

use super::{AttachmentMetadataJson, TaskFullReport};

pub fn task_full_json(report: &TaskFullReport) -> TaskFullJson {
    let detail = &report.detail;
    let task = &detail.item.task;
    TaskFullJson {
        task: task_line_json_item(&detail.item),
        project_prefix: task.project_prefix.clone(),
        description: task.description.clone(),
        metadata: detail
            .item
            .metadata
            .iter()
            .map(|metadata| (metadata.key.clone(), metadata.value.clone()))
            .collect(),
        metadata_details: detail
            .item
            .metadata
            .iter()
            .map(|metadata| MetadataDetailJson {
                field_id: metadata.field_id.to_string(),
                key: metadata.key.clone(),
                value: metadata.value.clone(),
            })
            .collect(),
        dependencies: task_dependency_summary_json(&detail.dependencies),
        related: detail.related.iter().map(task_related_json).collect(),
        notes: detail
            .notes
            .iter()
            .map(|note| TaskNoteJson {
                id: note.id.clone(),
                body: note.body.clone(),
                created_at: note.created_at.clone(),
            })
            .collect(),
        conflicts: report.conflicts.clone(),
        attachments: report.attachments.clone(),
    }
}

// --- JSON DTOs ---

#[derive(Serialize)]
pub struct TaskEpicLinkJson {
    pub r#ref: String,
    pub id: String,
    pub title: String,
    pub status: String,
    pub priority: String,
    pub open: bool,
}

#[derive(Serialize)]
pub struct TaskRecurrenceJson {
    pub series_ref: String,
    pub series_id: String,
    pub slot_on: String,
    pub rule: String,
    pub timezone: String,
    pub lifecycle: String,
    pub outcome: Option<String>,
    pub projection_state: String,
}

#[derive(Serialize)]
pub struct TaskRecurrenceGroupJson {
    pub series_ref: String,
    pub series_id: String,
    pub completed: usize,
    pub skipped: usize,
    pub missed: usize,
    pub pause_intervals: usize,
    pub latest_slot_on: Option<String>,
    pub latest_outcome: Option<String>,
}

#[derive(Serialize)]
pub struct TaskLineJson {
    pub r#ref: String,
    pub id: String,
    pub title: String,
    pub project: String,
    pub status: String,
    pub priority: String,
    pub labels: Vec<String>,
    pub deleted: bool,
    pub is_epic: bool,
    pub epic_parent: Option<TaskEpicLinkJson>,
    pub epic_children: Vec<TaskEpicLinkJson>,
    pub has_conflict: bool,
    pub blocked_by: i64,
    pub blocks: i64,
    pub available_at: String,
    pub due_on: String,
    pub recurrence: Option<TaskRecurrenceJson>,
    pub recurrence_group: Option<TaskRecurrenceGroupJson>,
    pub created_at: String,
    pub updated_at: String,
}

pub fn task_line_json_item(item: &TaskListItem) -> TaskLineJson {
    TaskLineJson {
        r#ref: item
            .recurrence_group
            .as_ref()
            .map(|group| group.series_ref.clone())
            .unwrap_or_else(|| item.display_ref.clone()),
        id: item
            .recurrence_group
            .as_ref()
            .map(|group| group.series_id.to_string())
            .unwrap_or_else(|| item.task.id.to_string()),
        title: item.task.title.clone(),
        project: item.task.project_key.clone(),
        status: item.task.status.to_string(),
        priority: item.task.priority.to_string(),
        labels: item.labels.clone(),
        deleted: item.task.deleted,
        is_epic: item.task.is_epic,
        epic_parent: item.epic_parent.as_ref().map(task_epic_link_json),
        epic_children: item.epic_children.iter().map(task_epic_link_json).collect(),
        has_conflict: item.has_conflict,
        blocked_by: item.unresolved_blocker_count,
        blocks: item.dependent_count,
        available_at: item.task.available_at.clone().unwrap_or_default(),
        due_on: item.task.due_on.clone().unwrap_or_default(),
        recurrence: item.recurrence.as_ref().map(task_recurrence_json),
        recurrence_group: item
            .recurrence_group
            .as_ref()
            .map(task_recurrence_group_json),
        created_at: item.task.created_at.clone(),
        updated_at: item.task.updated_at.clone(),
    }
}

pub fn task_recurrence_json(value: &crate::query::TaskRecurrenceSummary) -> TaskRecurrenceJson {
    TaskRecurrenceJson {
        series_ref: value.series_ref.clone(),
        series_id: value.series_id.to_string(),
        slot_on: value.slot_on.clone(),
        rule: value.rule_label.clone(),
        timezone: value.timezone.clone(),
        lifecycle: value.lifecycle.as_str().to_string(),
        outcome: value.outcome.map(|outcome| outcome.as_str().to_string()),
        projection_state: value.projection_state.as_str().to_string(),
    }
}

pub fn task_recurrence_group_json(
    value: &crate::query::RecurrenceTaskGroup,
) -> TaskRecurrenceGroupJson {
    TaskRecurrenceGroupJson {
        series_ref: value.series_ref.clone(),
        series_id: value.series_id.to_string(),
        completed: value.counts.completed,
        skipped: value.counts.skipped,
        missed: value.counts.missed,
        pause_intervals: value.counts.pause_intervals,
        latest_slot_on: value.counts.latest_slot_on.clone(),
        latest_outcome: value
            .counts
            .latest_outcome
            .map(|outcome| outcome.as_str().to_string()),
    }
}

pub fn task_epic_link_json(link: &TaskDependencyLink) -> TaskEpicLinkJson {
    TaskEpicLinkJson {
        r#ref: link.display_ref.clone(),
        id: link.task_id.to_string(),
        title: link.title.clone(),
        status: link.status.clone(),
        priority: link.priority.clone(),
        open: link.unresolved,
    }
}

#[derive(Serialize)]
pub struct TaskRelatedJson {
    pub task_id: String,
    pub display_ref: String,
    pub title: String,
    pub status: String,
    pub priority: String,
    pub deleted: bool,
    pub linked_at: String,
}

pub fn task_related_json(link: &crate::query::TaskRelatedLink) -> TaskRelatedJson {
    TaskRelatedJson {
        task_id: link.task_id.to_string(),
        display_ref: link.display_ref.clone(),
        title: link.title.clone(),
        status: link.status.as_str().to_string(),
        priority: link.priority.as_str().to_string(),
        deleted: link.deleted,
        linked_at: link.linked_at.clone(),
    }
}

#[derive(Serialize)]
pub struct TaskFullJson {
    pub task: TaskLineJson,
    pub project_prefix: String,
    pub description: String,
    pub metadata: BTreeMap<String, String>,
    pub metadata_details: Vec<MetadataDetailJson>,
    pub dependencies: TaskDependencySummaryJson,
    pub related: Vec<TaskRelatedJson>,
    pub notes: Vec<TaskNoteJson>,
    pub conflicts: Vec<TaskConflictReport>,
    pub attachments: Vec<AttachmentMetadataJson>,
}

#[derive(Serialize)]
pub struct MetadataDetailJson {
    pub field_id: String,
    pub key: String,
    pub value: String,
}

#[derive(Serialize)]
pub struct TaskNoteJson {
    pub id: String,
    pub body: String,
    pub created_at: String,
}

#[derive(Clone, Serialize)]
pub struct TaskConflictReport {
    pub field: String,
    pub variant_a: String,
    pub local_value: String,
    pub variant_b: String,
    pub remote_value: String,
}

#[derive(Serialize)]
pub struct TaskDependencySummaryJson {
    pub depends_on_open: i64,
    pub depends_on_total: i64,
    pub blocks_open: i64,
    pub blocks_total: i64,
    pub depends_on: Vec<TaskDependencyItemJson>,
    pub blocks: Vec<TaskDependencyItemJson>,
}

#[derive(Serialize)]
pub struct TaskDependencyItemJson {
    pub r#ref: String,
    pub id: String,
    pub title: String,
    pub status: String,
    pub priority: String,
    pub deleted: bool,
    pub unresolved: bool,
    pub created_at: String,
}

pub fn task_dependency_summary_json(summary: &TaskDependencySummary) -> TaskDependencySummaryJson {
    TaskDependencySummaryJson {
        depends_on_open: summary.depends_on.iter().filter(|d| d.unresolved).count() as i64,
        depends_on_total: summary.depends_on.len() as i64,
        blocks_open: summary.blocks.iter().filter(|d| d.unresolved).count() as i64,
        blocks_total: summary.blocks.len() as i64,
        depends_on: summary
            .depends_on
            .iter()
            .map(|d| TaskDependencyItemJson {
                r#ref: d.display_ref.clone(),
                id: d.task.id.to_string(),
                title: d.task.title.clone(),
                status: d.task.status.to_string(),
                priority: d.task.priority.to_string(),
                deleted: d.task.deleted,
                unresolved: d.unresolved,
                created_at: d.task.created_at.clone(),
            })
            .collect(),
        blocks: summary
            .blocks
            .iter()
            .map(|d| TaskDependencyItemJson {
                r#ref: d.display_ref.clone(),
                id: d.task.id.to_string(),
                title: d.task.title.clone(),
                status: d.task.status.to_string(),
                priority: d.task.priority.to_string(),
                deleted: d.task.deleted,
                unresolved: d.unresolved,
                created_at: d.task.created_at.clone(),
            })
            .collect(),
    }
}
