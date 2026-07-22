use chrono::{NaiveDate, NaiveTime};

use crate::choices::{TaskPriority, TaskSource, TaskStatus};
use crate::ids::{ProjectId, TaskId, WorkspaceId};
use crate::recurrence::{
    RecurrenceDuePolicy, RecurrenceOutcome, RecurrenceProjectionState, RecurrenceRule,
    RecurrenceSchedule, RecurrenceSeriesId, RecurrenceSeriesState, TimeZoneId,
};

#[derive(Debug, Clone)]
pub struct Task {
    pub id: TaskId,
    pub workspace_id: WorkspaceId,
    pub title: String,
    pub description: String,
    pub project_id: ProjectId,
    pub project_key: String,
    pub project_prefix: String,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    pub source: TaskSource,
    pub created_at: String,
    pub updated_at: String,
    pub queue_activity_at: String,
    pub available_at: Option<String>,
    pub due_on: Option<String>,
    pub deleted: bool,
    pub is_epic: bool,
}

#[derive(Debug, Clone)]
pub struct RecurrenceSeries {
    pub workspace_id: WorkspaceId,
    pub id: RecurrenceSeriesId,
    pub title: String,
    pub description: String,
    pub project_id: ProjectId,
    pub priority: TaskPriority,
    pub initial_status: TaskStatus,
    pub rule: RecurrenceRule,
    pub timezone: TimeZoneId,
    pub start_on: NaiveDate,
    pub available_local_time: Option<NaiveTime>,
    pub due_policy: RecurrenceDuePolicy,
    pub state: RecurrenceSeriesState,
    pub stopped_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub deleted: bool,
}

impl RecurrenceSeries {
    pub fn schedule(&self) -> RecurrenceSchedule {
        RecurrenceSchedule::new(
            self.rule,
            self.timezone.clone(),
            self.start_on,
            self.available_local_time,
            self.due_policy,
        )
    }
}

#[derive(Debug, Clone)]
pub struct RecurrenceSeriesLabel {
    pub workspace_id: WorkspaceId,
    pub series_id: RecurrenceSeriesId,
    pub label: String,
}

#[derive(Debug, Clone)]
pub struct RecurrenceOccurrence {
    pub workspace_id: WorkspaceId,
    pub series_id: RecurrenceSeriesId,
    pub slot_on: NaiveDate,
    pub task_id: Option<TaskId>,
    pub outcome: Option<RecurrenceOutcome>,
    pub resolved_at: Option<String>,
    pub outcome_change_id: Option<String>,
    pub projection_state: RecurrenceProjectionState,
    pub archived_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RecurrencePauseInterval {
    pub workspace_id: WorkspaceId,
    pub id: String,
    pub series_id: RecurrenceSeriesId,
    pub paused_at: String,
    pub resumed_at: Option<String>,
    pub suspended_slot_on: Option<NaiveDate>,
    pub suspended_task_id: Option<TaskId>,
    pub created_by_change_id: String,
    pub resolved_by_change_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutableEntityType {
    Task,
    RecurrenceSeries,
}

impl MutableEntityType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::RecurrenceSeries => "recurrence_series",
        }
    }

    pub fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "task" => Ok(Self::Task),
            "recurrence_series" => Ok(Self::RecurrenceSeries),
            _ => anyhow::bail!("invalid mutable entity type: {value}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Project {
    pub id: ProjectId,
    pub workspace_id: WorkspaceId,
    pub key: String,
    pub name: String,
    pub prefix: String,
}

#[derive(Debug, Clone)]
pub struct TaskAttachment {
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
    pub deleted: bool,
    pub deleted_at: Option<String>,
    pub deleted_by_change_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BlobInventoryRow {
    pub sha256: String,
    pub byte_size: i64,
    pub media_type: String,
    pub available: bool,
    pub first_seen_at: String,
    pub last_verified_at: Option<String>,
}
