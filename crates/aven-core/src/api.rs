mod epics;
pub use epics::{IosEpicProgress, IosEpicSummary};

use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::Error as InternalError;
use chrono::{Days, LocalResult, NaiveDate, NaiveTime, TimeZone, Utc, Weekday};

pub use crate::attachments::AttachmentBytesState as IosAttachmentAvailability;
use crate::choices::{TaskPriority, TaskSource, TaskStatus};
use crate::db::Database;
use crate::ids::{MetadataFieldId, ProjectId, TaskId, WorkspaceId};
use crate::metadata::{MetadataFieldUsage, TaskMetadataInput, TaskMetadataValue};
use crate::operations::{
    CreateRecurrenceSeriesParams, IosTaskMutation as InternalIosTaskMutation,
    RecurrenceSeriesDraft, RecurrenceTemplateUpdate as InternalRecurrenceTemplateUpdate,
    TaskCreationOptions, TaskDraft, TaskUpdate as InternalTaskUpdate,
    UpdateRecurrenceTemplateParams,
};
pub use crate::pairing::{PairingInvitation, PairingInvitationError};
use crate::query::{
    MAX_RECURRENCE_HISTORY_LIMIT, RecurrenceCounts as InternalRecurrenceCounts,
    RecurrenceHistoryEntry as InternalRecurrenceHistoryEntry,
    RecurrenceSeriesDetail as InternalRecurrenceSeriesDetail,
    RecurrenceSeriesSummary as InternalRecurrenceSeriesSummary, SortDirection, TaskFilters,
    TaskListItem, TaskQueryMode, TaskSearchQuery, TaskSort,
};
pub use crate::query::{RecurrenceHistoryKind, SearchMatchedField as IosSearchMatchedField};
pub use crate::queue::{QueueBand, QueueDateKind, QueueReason};
pub use crate::recurrence::{
    RecurrenceDuePolicy, RecurrenceFrequency, RecurrenceOutcome, RecurrenceProjectionState,
    RecurrenceSeriesState,
};
use crate::recurrence::{
    RecurrenceRule as InternalRecurrenceRule, RecurrenceSchedule, RecurrenceSeriesId, TimeZoneId,
    WeekdaySet,
};
use crate::sync::SyncSession;
use crate::task_fields::TaskField;
use crate::types::{Project, RecurrenceOccurrence, RecurrenceSeries, Task};
use crate::undo::TaskUndoSnapshot;
use crate::workspaces::Workspace;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IosSyncFacts {
    pub pending_changes: i64,
    pub attachment_uploads: i64,
    pub attachment_downloads: u64,
    pub metadata_confirmed_at: Option<String>,
    pub metadata_caught_up: bool,
}

#[derive(Clone)]
pub struct IosConnectionInspection {
    pub server_url: Option<String>,
    pub server_origin: Option<String>,
    pub last_success_at: Option<String>,
    pub facts: IosSyncFacts,
}

/// Reads an isolated snapshot without initializing, migrating, or repairing the replica.
pub async fn inspect_ios_connection(
    path: impl AsRef<Path>,
) -> Result<IosConnectionInspection, Error> {
    let database = Database::inspect(path.as_ref())
        .await
        .database
        .ok_or_else(|| {
            Error::new(
                ErrorCode::Database,
                "local connection information is unavailable".to_string(),
            )
        })?;
    let server_url = database
        .meta("sync_server_url")
        .await
        .map_err(Error::from_internal)?;
    let server_origin = server_url.as_deref().and_then(|server| {
        let url = url::Url::parse(server).ok()?;
        crate::sync::wire::sync_server_url_is_valid_url(&url)
            .then(|| url.origin().ascii_serialization())
    });
    let last_success_at = database
        .meta("sync_last_success_at")
        .await
        .map_err(Error::from_internal)?;
    let facts = database
        .ios_sync_facts()
        .await
        .map_err(Error::from_internal)?;
    Ok(IosConnectionInspection {
        server_url,
        server_origin,
        last_success_at,
        facts,
    })
}

#[derive(Clone)]
pub struct Store {
    database: Database,
    #[cfg(test)]
    fail_queue_reads: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Store {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        let database = Database::open(path.as_ref())
            .await
            .map_err(Error::database_open)?;
        Ok(Self {
            database,
            #[cfg(test)]
            fail_queue_reads: Default::default(),
        })
    }

    pub fn initialize_storage(&self) -> Result<StorageLayout, Error> {
        let root = crate::attachments::default_blob_dir(self.database.path());
        let objects = root.join("objects").join("sha256");
        let trash = root.join("trash");
        let previews = root.join("cache").join("previews");
        for directory in [&objects, &trash, &previews] {
            std::fs::create_dir_all(directory)
                .map_err(|error| Error::from_internal(error.into()))?;
        }
        Ok(StorageLayout {
            root,
            staging: objects.clone(),
            objects,
            trash,
            previews,
        })
    }

    pub async fn start_sync_session(
        &self,
        server: String,
        auth_token: Option<String>,
        page_budget: Option<usize>,
    ) -> Result<SyncSession, Error> {
        if !crate::sync::wire::sync_server_url_is_valid(&server) {
            return Err(Error::new(
                ErrorCode::Validation,
                "invalid sync server URL".to_string(),
            ));
        }
        SyncSession::start(self.database.clone(), server, auth_token, page_budget)
            .await
            .map_err(Error::from_internal)
    }

    pub async fn list_workspaces(&self) -> Result<Vec<WorkspaceRecord>, Error> {
        self.database
            .list_workspaces()
            .await
            .map(|workspaces| workspaces.into_iter().map(WorkspaceRecord::from).collect())
            .map_err(Error::from_internal)
    }

    pub async fn ios_sync_facts(&self) -> Result<IosSyncFacts, Error> {
        self.database
            .ios_sync_facts()
            .await
            .map_err(Error::from_internal)
    }

    pub async fn ios_queue_state(&self) -> Result<IosQueueState, Error> {
        let selected = self
            .database
            .restore_ios_queue_workspace()
            .await
            .map_err(Error::from_internal)?;
        self.ios_queue_state_for(selected).await
    }

    pub async fn select_ios_queue_workspace(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<IosQueueState, Error> {
        let selected = self
            .database
            .select_ios_queue_workspace(workspace_id)
            .await
            .map_err(Error::from_internal)?;
        self.ios_queue_state_for(selected).await
    }

    async fn ios_queue_state_for(&self, selected: Workspace) -> Result<IosQueueState, Error> {
        #[cfg(test)]
        if self
            .fail_queue_reads
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(Error::from_internal(sqlx::Error::PoolClosed.into()));
        }
        let workspaces = self
            .database
            .list_workspace_open_summaries()
            .await
            .map_err(Error::from_internal)?
            .into_iter()
            .map(|summary| WorkspaceQueueSummary {
                workspace: summary.workspace.into(),
                open_task_count: summary.open_task_count,
            })
            .collect();
        let projects = self
            .database
            .list_projects(&selected.id, None)
            .await
            .map_err(Error::from_internal)?
            .into_iter()
            .map(ProjectRecord::from)
            .collect();
        let labels = self
            .database
            .list_labels(&selected.id, None)
            .await
            .map_err(Error::from_internal)?;
        let queue = self.queue_report(&selected.id).await?;
        Ok(IosQueueState {
            selected_workspace: selected.into(),
            workspaces,
            projects,
            labels,
            queue,
        })
    }

    pub async fn ios_project_tasks(
        &self,
        workspace_id: &WorkspaceId,
        project_id: &ProjectId,
    ) -> Result<Vec<IosTaskListRow>, Error> {
        self.workspace(workspace_id).await?;
        let project = self
            .database
            .find_project_by_id(workspace_id, project_id)
            .await
            .map_err(Error::from_internal)?
            .ok_or_else(|| Error::new(ErrorCode::NotFound, "project not found".to_string()))?;
        self.database
            .list_task_summary_items(
                workspace_id,
                TaskFilters {
                    project: Some(project.key),
                    ..TaskFilters::default()
                },
                TaskQueryMode::Flat,
                TaskSort::Updated,
                SortDirection::Desc,
                None,
            )
            .await
            .map(|items| items.into_iter().map(IosTaskListRow::from).collect())
            .map_err(Error::from_internal)
    }

    pub async fn ios_search_tasks(
        &self,
        workspace_id: &WorkspaceId,
        text: &str,
    ) -> Result<Vec<IosTaskSearchResult>, Error> {
        self.workspace(workspace_id).await?;
        let text = text.trim();
        if text.is_empty() {
            return Ok(Vec::new());
        }
        if text.chars().count() > 256 {
            return Err(Error::new(
                ErrorCode::Validation,
                "search query is too long".to_string(),
            ));
        }
        self.database
            .search_task_items(
                workspace_id,
                TaskSearchQuery {
                    text: text.to_string(),
                    project: None,
                    metadata: Vec::new(),
                    has_metadata: Vec::new(),
                    missing_metadata: Vec::new(),
                    include_deleted: false,
                    limit: 100,
                },
            )
            .await
            .map(|results| {
                results
                    .into_iter()
                    .map(|result| IosTaskSearchResult {
                        task: IosTaskListRow::from(result.item),
                        matched_field: result.matched_field,
                        snippet: result.snippet,
                    })
                    .collect()
            })
            .map_err(Error::from_internal)
    }

    pub async fn ios_task_detail(
        &self,
        workspace_id: &WorkspaceId,
        task_id: &TaskId,
    ) -> Result<IosTaskDetail, Error> {
        let workspace = self.workspace(workspace_id).await?;
        let mut connection = self
            .database
            .acquire_reader()
            .await
            .map_err(Error::from_internal)?;
        let task = crate::refs::get_task_in_workspace(&mut connection, &workspace, task_id)
            .await
            .map_err(Error::from_internal)?;
        drop(connection);
        let detail = self
            .database
            .task_detail(&task)
            .await
            .map_err(Error::from_internal)?;
        IosTaskDetail::from_detail(detail, workspace)
    }

    /// Returns an owned, bounded snapshot without exposing object storage paths.
    pub async fn ios_attachment_bytes(
        &self,
        workspace_id: &WorkspaceId,
        task_id: &TaskId,
        attachment_id: &str,
    ) -> Result<IosAttachmentRead, Error> {
        use crate::attachments::{MAX_BLOB_BYTES, default_blob_dir, object_path, sha256_hex};
        use std::io::Read;

        crate::attachments::validate_attachment_id(attachment_id).map_err(Error::from_internal)?;
        let workspace = self.workspace(workspace_id).await?;
        let lease = match self
            .database
            .acquire_live_attachment_read_lease(&workspace, attachment_id)
            .await
        {
            Ok(lease) => lease,
            Err(error) => {
                return match error.downcast_ref::<crate::operations::AttachmentReadFailure>() {
                    Some(crate::operations::AttachmentReadFailure::Invalidated) => {
                        Ok(IosAttachmentRead::Invalidated)
                    }
                    Some(crate::operations::AttachmentReadFailure::Unavailable) => {
                        Ok(IosAttachmentRead::Unavailable)
                    }
                    None => Err(Error::from_internal(error)),
                };
            }
        };
        let result = if lease.task_id != task_id.as_str() {
            Ok(IosAttachmentRead::Invalidated)
        } else {
            let path = object_path(&default_blob_dir(self.database.path()), &lease.sha256);
            match path {
                Err(error) => Err(error),
                Ok(path) => {
                    let expected_size = lease.byte_size;
                    let expected_hash = lease.sha256.clone();
                    crate::attachments::run_preview(move || {
                        let file = match std::fs::File::open(path) {
                            Ok(file) => file,
                            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                                return Ok(IosAttachmentRead::Missing);
                            }
                            Err(error) => return Err(error.into()),
                        };
                        let mut bytes = Vec::new();
                        file.take(MAX_BLOB_BYTES as u64 + 1)
                            .read_to_end(&mut bytes)?;
                        if bytes.len() > MAX_BLOB_BYTES
                            || bytes.len() as i64 != expected_size
                            || sha256_hex(&bytes) != expected_hash
                        {
                            return Ok(IosAttachmentRead::Corrupt);
                        }
                        Ok(IosAttachmentRead::Bytes { bytes })
                    })
                    .await
                }
            }
        };
        self.database
            .release_attachment_lease(&lease.lease_id)
            .await
            .map_err(Error::from_internal)?;
        result.map_err(Error::from_internal)
    }

    pub async fn capture_ios_queue_task(
        &self,
        workspace_id: &WorkspaceId,
        input: IosTaskCapture,
    ) -> Result<IosTaskCaptureResult, Error> {
        let title = input.title.trim();
        if title.is_empty() {
            return Err(Error::new(
                ErrorCode::Validation,
                "task title is required".to_string(),
            ));
        }
        validate_optional_date("due_on", input.due_on.as_deref())?;
        let workspace = self.workspace(workspace_id).await?;
        let project = input
            .project
            .ok_or_else(|| Error::new(ErrorCode::Validation, "project is required".to_string()))?;
        let outcome = self
            .database
            .create_task_with_options(
                &workspace,
                TaskDraft {
                    title: title.to_string(),
                    description: input.description,
                    project: Some(project),
                    status: TaskStatus::Inbox.as_str().to_string(),
                    priority: input.priority.as_str().to_string(),
                    source: TaskSource::Api,
                    labels: input.labels,
                    metadata: Vec::new(),
                    available_at: None,
                    due_on: input.due_on,
                    is_epic: false,
                },
                TaskCreationOptions::for_ios_capture(),
            )
            .await
            .map_err(Error::from_internal)?;
        let expected = outcome.undo_snapshot.ok_or_else(|| {
            Error::new(
                ErrorCode::Internal,
                "capture undo state is unavailable".to_string(),
            )
        })?;
        let undo_token = serde_json::to_string(&IosCaptureUndoToken {
            workspace_id: workspace_id.clone(),
            task_id: outcome.task.id.clone(),
            expected,
        })
        .map_err(|error| Error::from_internal(error.into()))?;
        let state = self.ios_queue_state_for(workspace).await.ok();
        let display_ref = state
            .as_ref()
            .and_then(|state| {
                state
                    .queue
                    .tasks
                    .iter()
                    .find(|task| task.id == outcome.task.id)
            })
            .map(|task| task.display_ref.clone())
            .unwrap_or_else(|| outcome.task.id.to_string());
        Ok(IosTaskCaptureResult {
            task_id: outcome.task.id,
            display_ref,
            undo_token,
            state,
        })
    }

    pub async fn undo_ios_queue_capture(
        &self,
        undo_token: &str,
    ) -> Result<Option<IosQueueState>, Error> {
        let token: IosCaptureUndoToken = serde_json::from_str(undo_token).map_err(|_| {
            Error::new(
                ErrorCode::Validation,
                "invalid capture undo token".to_string(),
            )
        })?;
        let workspace = self.workspace(&token.workspace_id).await?;
        self.database
            .undo_ios_capture(&workspace, &token.task_id, &token.expected)
            .await
            .map_err(Error::from_internal)?;
        Ok(self.ios_queue_state_for(workspace).await.ok())
    }

    /// Returns the committed status and its conditional Undo receipt without a read-model refresh.
    pub async fn update_ios_detail_status(
        &self,
        workspace_id: &WorkspaceId,
        task_id: &TaskId,
        status: TaskStatus,
    ) -> Result<IosDetailStatusReceipt, Error> {
        let workspace = self.workspace(workspace_id).await?;
        let mutation = InternalIosTaskMutation::DetailStatus {
            status: status.as_str().to_string(),
            expected_version: None,
        };
        let outcome = self
            .database
            .mutate_ios_task(&workspace, task_id, &mutation)
            .await
            .map_err(Error::from_internal)?;
        let status = TaskStatus::parse(&outcome.after.status)
            .expect("persisted task status is validated by core mutations");
        let mutation = InternalIosTaskMutation::DetailStatus {
            status: status.as_str().to_string(),
            expected_version: outcome.status_version,
        };
        let undo_token = outcome.changed.then(|| {
            serde_json::to_string(&IosMutationUndoToken {
                workspace_id: workspace_id.clone(),
                task_id: task_id.clone(),
                mutation,
                before: outcome.before,
                expected: outcome.after,
            })
            .expect("Undo snapshots contain only JSON-serializable values")
        });
        Ok(IosDetailStatusReceipt { status, undo_token })
    }

    pub async fn undo_ios_detail_status(&self, undo_token: &str) -> Result<(), Error> {
        let token: IosMutationUndoToken = serde_json::from_str(undo_token).map_err(|_| {
            Error::new(
                ErrorCode::Validation,
                "invalid detail status undo token".to_string(),
            )
        })?;
        if !matches!(token.mutation, InternalIosTaskMutation::DetailStatus { .. }) {
            return Err(Error::new(
                ErrorCode::Validation,
                "invalid detail status undo token".to_string(),
            ));
        }
        let workspace = self.workspace(&token.workspace_id).await?;
        self.database
            .undo_ios_task_mutation(
                &workspace,
                &token.task_id,
                &token.mutation,
                &token.before,
                &token.expected,
            )
            .await
            .map_err(Error::from_internal)?;
        Ok(())
    }

    pub async fn mutate_ios_queue_task(
        &self,
        workspace_id: &WorkspaceId,
        task_id: &TaskId,
        input: IosQueueMutation,
    ) -> Result<IosQueueMutationResult, Error> {
        let workspace = self.workspace(workspace_id).await?;
        let mutation = input.into_internal()?;
        let outcome = self
            .database
            .mutate_ios_task(&workspace, task_id, &mutation)
            .await
            .map_err(Error::from_internal)?;
        if !outcome.changed {
            return Err(Error::new(
                ErrorCode::GenerationConflict,
                "quick action did not change the task".to_string(),
            ));
        }
        let undo_token = serde_json::to_string(&IosMutationUndoToken {
            workspace_id: workspace_id.clone(),
            task_id: task_id.clone(),
            mutation,
            before: outcome.before,
            expected: outcome.after,
        })
        .map_err(|error| Error::from_internal(error.into()))?;
        Ok(IosQueueMutationResult {
            task_id: task_id.clone(),
            undo_token,
            state: self.ios_queue_state_for(workspace).await.ok(),
        })
    }

    pub async fn undo_ios_queue_mutation(
        &self,
        undo_token: &str,
    ) -> Result<Option<IosQueueState>, Error> {
        let token: IosMutationUndoToken = serde_json::from_str(undo_token).map_err(|_| {
            Error::new(
                ErrorCode::Validation,
                "invalid quick action undo token".to_string(),
            )
        })?;
        let workspace = self.workspace(&token.workspace_id).await?;
        self.database
            .undo_ios_task_mutation(
                &workspace,
                &token.task_id,
                &token.mutation,
                &token.before,
                &token.expected,
            )
            .await
            .map_err(Error::from_internal)?;
        Ok(self.ios_queue_state_for(workspace).await.ok())
    }

    pub async fn resolve_workspace(&self, name_or_key: &str) -> Result<WorkspaceRecord, Error> {
        self.database
            .find_workspace(name_or_key)
            .await
            .map_err(Error::from_internal)?
            .map(WorkspaceRecord::from)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::NotFound,
                    format!("workspace not found: {name_or_key}"),
                )
            })
    }

    pub async fn list_metadata_fields(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<Vec<MetadataFieldRecord>, Error> {
        self.workspace(workspace_id).await?;
        self.database
            .list_metadata_fields(workspace_id)
            .await
            .map(|fields| fields.into_iter().map(Into::into).collect())
            .map_err(Error::from_internal)
    }

    pub async fn rename_metadata_field(
        &self,
        workspace_id: &WorkspaceId,
        key: &str,
        new_key: &str,
    ) -> Result<MetadataFieldRecord, Error> {
        let workspace = self.workspace(workspace_id).await?;
        let field = self
            .database
            .rename_metadata_field(&workspace, key, new_key)
            .await
            .map_err(Error::from_internal)?;
        let fields = self.list_metadata_fields(workspace_id).await?;
        fields
            .into_iter()
            .find(|candidate| candidate.id == field.id)
            .ok_or_else(|| Error::new(ErrorCode::NotFound, "metadata field not found".to_string()))
    }

    pub async fn create_task(
        &self,
        workspace_id: &WorkspaceId,
        input: CreateTask,
    ) -> Result<TaskRecord, Error> {
        validate_project(&input.project)?;
        validate_optional_date("available_at", input.available_at.as_deref())?;
        validate_optional_date("due_on", input.due_on.as_deref())?;
        let workspace = self.workspace(workspace_id).await?;
        let outcome = self
            .database
            .create_task(
                &workspace,
                TaskDraft {
                    title: input.title,
                    description: input.description,
                    project: Some(input.project),
                    status: input.status.as_str().to_string(),
                    priority: input.priority.as_str().to_string(),
                    source: TaskSource::Api,
                    labels: Vec::new(),
                    metadata: input
                        .metadata
                        .into_iter()
                        .map(TaskMetadataInput::from)
                        .collect(),
                    available_at: input.available_at,
                    due_on: input.due_on,
                    is_epic: false,
                },
            )
            .await
            .map_err(Error::from_internal)?;
        let metadata = self
            .database
            .task_metadata(workspace_id, &outcome.task.id)
            .await
            .map_err(Error::from_internal)?;
        Ok(TaskRecord::with_metadata(outcome.task, metadata))
    }

    pub async fn ios_task_edit_context(
        &self,
        workspace_id: &WorkspaceId,
        task_id: &TaskId,
    ) -> Result<IosTaskEditContext, Error> {
        let detail = self.ios_task_detail(workspace_id, task_id).await?;
        let projects = self
            .database
            .list_projects(workspace_id, None)
            .await
            .map_err(Error::from_internal)?
            .into_iter()
            .map(ProjectRecord::from)
            .collect();
        let labels = self
            .database
            .list_labels(workspace_id, None)
            .await
            .map_err(Error::from_internal)?;
        Ok(IosTaskEditContext {
            detail,
            projects,
            labels,
        })
    }

    pub async fn edit_ios_task(
        &self,
        workspace_id: &WorkspaceId,
        task_id: &TaskId,
        input: IosTaskEdit,
    ) -> Result<bool, Error> {
        let title = input.title.map(|title| title.trim().to_string());
        if let Some(title) = title.as_deref() {
            if title.is_empty() {
                return Err(Error::new(
                    ErrorCode::Validation,
                    "title must not be empty".into(),
                ));
            }
            if title.chars().any(|character| {
                matches!(
                    character,
                    '\n' | '\r' | '\u{000B}' | '\u{000C}' | '\u{0085}' | '\u{2028}' | '\u{2029}'
                )
            }) {
                return Err(Error::new(
                    ErrorCode::Validation,
                    "title must be a single line".into(),
                ));
            }
        }
        validate_date_update("available_at", &input.available_at)?;
        validate_date_update("due_on", &input.due_on)?;
        let workspace = self.workspace(workspace_id).await?;
        self.database
            .edit_ios_task(
                &workspace,
                task_id,
                input.project_id.as_ref(),
                InternalTaskUpdate {
                    title,
                    description: input.description,
                    status: input.status.map(|value| value.as_str().to_string()),
                    priority: input.priority.map(|value| value.as_str().to_string()),
                    available_at: input.available_at.into_internal(),
                    due_on: input.due_on.into_internal(),
                    add_labels: input.add_labels,
                    remove_labels: input.remove_labels,
                    ..InternalTaskUpdate::default()
                },
            )
            .await
            .map_err(Error::from_internal)
    }

    pub async fn update_task(
        &self,
        workspace_id: &WorkspaceId,
        task_id: &TaskId,
        input: UpdateTask,
    ) -> Result<TaskUpdateResult, Error> {
        if let Some(project) = input.project.as_deref() {
            validate_project(project)?;
        }
        validate_date_update("available_at", &input.available_at)?;
        validate_date_update("due_on", &input.due_on)?;
        let workspace = self.workspace(workspace_id).await?;
        self.fetch_task(workspace_id, task_id).await?;
        let outcome = self
            .database
            .update_task(
                &workspace,
                task_id,
                InternalTaskUpdate {
                    title: input.title,
                    description: input.description,
                    project: input.project,
                    status: input.status.map(|status| status.as_str().to_string()),
                    priority: input.priority.map(|priority| priority.as_str().to_string()),
                    available_at: input.available_at.into_internal(),
                    due_on: input.due_on.into_internal(),
                    set_metadata: input
                        .set_metadata
                        .into_iter()
                        .map(TaskMetadataInput::from)
                        .collect(),
                    remove_metadata: input.remove_metadata,
                    ..InternalTaskUpdate::default()
                },
            )
            .await
            .map_err(Error::from_internal)?;
        let metadata = self
            .database
            .task_metadata(workspace_id, task_id)
            .await
            .map_err(Error::from_internal)?;
        let related = self
            .database
            .task_related_links(workspace_id, task_id)
            .await
            .map_err(Error::from_internal)?;
        Ok(TaskUpdateResult {
            task: TaskRecord::with_metadata_and_related(outcome.task, metadata, related),
            changed: outcome.changed,
        })
    }

    pub async fn list_tasks(&self, workspace_id: &WorkspaceId) -> Result<Vec<TaskRecord>, Error> {
        self.workspace(workspace_id).await?;
        let filters = TaskFilters {
            exclude_epics: true,
            ..TaskFilters::default()
        };
        self.database
            .list_base_tasks(workspace_id, filters, TaskSort::Created, SortDirection::Asc)
            .await
            .map(|tasks| tasks.into_iter().map(TaskRecord::from).collect())
            .map_err(Error::from_internal)
    }

    pub async fn queue_report(&self, workspace_id: &WorkspaceId) -> Result<QueueReport, Error> {
        self.workspace(workspace_id).await?;
        let tasks = self
            .database
            .list_task_summary_items(
                workspace_id,
                TaskFilters {
                    hide_done: true,
                    ..TaskFilters::default()
                },
                TaskQueryMode::RankedQueue,
                TaskSort::Created,
                SortDirection::Asc,
                None,
            )
            .await
            .map_err(Error::from_internal)?
            .into_iter()
            .map(QueueTaskSummary::from)
            .collect();
        let unresolved_conflict_count = self
            .database
            .unresolved_conflict_count_in_workspace(workspace_id)
            .await
            .map_err(Error::from_internal)?
            .clamp(0, i64::from(u32::MAX)) as u32;
        let last_success_at = self
            .database
            .sync_persistence_status()
            .await
            .map_err(Error::from_internal)?
            .last_success;
        Ok(QueueReport {
            tasks,
            unresolved_conflict_count,
            last_success_at,
        })
    }

    pub async fn fetch_task(
        &self,
        workspace_id: &WorkspaceId,
        task_id: &TaskId,
    ) -> Result<TaskRecord, Error> {
        let workspace = self.workspace(workspace_id).await?;
        let mut connection = self
            .database
            .acquire_reader()
            .await
            .map_err(Error::from_internal)?;
        let task = crate::refs::get_task_in_workspace(&mut connection, &workspace, task_id)
            .await
            .map_err(Error::from_internal)?;
        drop(connection);
        let metadata = self
            .database
            .task_metadata(workspace_id, task_id)
            .await
            .map_err(Error::from_internal)?;
        let related = self
            .database
            .task_related_links(workspace_id, task_id)
            .await
            .map_err(Error::from_internal)?;
        Ok(TaskRecord::with_metadata_and_related(
            task, metadata, related,
        ))
    }

    pub async fn add_related_task(
        &self,
        workspace_id: &WorkspaceId,
        task_id: &TaskId,
        related_task_id: &TaskId,
    ) -> Result<RelatedMutationResult, Error> {
        let workspace = self.workspace(workspace_id).await?;
        self.database
            .add_task_related_link(&workspace, task_id, related_task_id)
            .await
            .map(|outcome| RelatedMutationResult {
                changed: outcome.changed,
            })
            .map_err(Error::from_internal)
    }

    pub async fn remove_related_task(
        &self,
        workspace_id: &WorkspaceId,
        task_id: &TaskId,
        related_task_id: &TaskId,
    ) -> Result<RelatedMutationResult, Error> {
        let workspace = self.workspace(workspace_id).await?;
        self.database
            .remove_task_related_link(&workspace, task_id, related_task_id)
            .await
            .map(|outcome| RelatedMutationResult {
                changed: outcome.changed,
            })
            .map_err(Error::from_internal)
    }

    pub async fn create_recurrence_series(
        &self,
        workspace_id: &WorkspaceId,
        input: CreateRecurrenceSeries,
    ) -> Result<RecurrenceCreateResult, Error> {
        validate_project(&input.project)?;
        let workspace = self.workspace(workspace_id).await?;
        let schedule = input.schedule.into_internal()?;
        let outcome = self
            .database
            .create_recurrence_series(
                &workspace,
                CreateRecurrenceSeriesParams::new(RecurrenceSeriesDraft {
                    title: input.title,
                    description: input.description,
                    project: input.project,
                    priority: input.priority.as_str().to_string(),
                    initial_status: input.initial_status.as_str().to_string(),
                    labels: input.labels,
                    metadata: input.metadata.into_iter().map(Into::into).collect(),
                    schedule,
                }),
            )
            .await
            .map_err(Error::from_internal)?;
        let metadata = self
            .database
            .task_metadata(workspace_id, &outcome.task.id)
            .await
            .map_err(Error::from_internal)?;
        Ok(RecurrenceCreateResult {
            series: RecurrenceSeriesRecord::from(outcome.series),
            series_ref: outcome.series_ref,
            occurrence: RecurrenceOccurrenceRecord::from(outcome.occurrence),
            task: TaskRecord::with_metadata(outcome.task, metadata),
        })
    }

    pub async fn update_recurrence_template(
        &self,
        workspace_id: &WorkspaceId,
        series_id: &RecurrenceSeriesId,
        input: UpdateRecurrenceTemplate,
    ) -> Result<RecurrenceTemplateUpdateResult, Error> {
        if let Some(project) = input.project.as_deref() {
            validate_project(project)?;
        }
        let workspace = self.workspace(workspace_id).await?;
        self.database
            .update_recurrence_template(
                &workspace,
                series_id,
                UpdateRecurrenceTemplateParams::new(InternalRecurrenceTemplateUpdate {
                    title: input.title,
                    description: input.description,
                    project: input.project,
                    priority: input.priority.map(|value| value.as_str().to_string()),
                    initial_status: input.initial_status.map(|value| value.as_str().to_string()),
                    labels: input.labels,
                    set_metadata: input.set_metadata.into_iter().map(Into::into).collect(),
                    remove_metadata: input.remove_metadata,
                    available_local_time: input.available_local_time.into_internal()?,
                    due_policy: input.due_policy,
                }),
            )
            .await
            .map(|outcome| RecurrenceTemplateUpdateResult {
                series: RecurrenceSeriesRecord::from(outcome.series),
                changed: outcome.changed,
            })
            .map_err(Error::from_internal)
    }

    pub async fn resolve_recurrence_ref(
        &self,
        workspace_id: &WorkspaceId,
        input: &str,
    ) -> Result<RecurrenceRefResolution, Error> {
        let workspace = self.workspace(workspace_id).await?;
        let series = self
            .database
            .resolve_recurrence_ref(&workspace, input)
            .await
            .map_err(Error::from_internal)?;
        let series_ref = self
            .database
            .recurrence_series_ref(workspace_id, &series.id)
            .await
            .map_err(Error::from_internal)?;
        Ok(RecurrenceRefResolution {
            series_id: series.id,
            series_ref,
        })
    }

    pub async fn list_recurrence_series(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<Vec<RecurrenceSeriesSummary>, Error> {
        self.workspace(workspace_id).await?;
        self.database
            .list_recurrence_series(workspace_id)
            .await
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(Error::from_internal)
    }

    pub async fn show_recurrence_series(
        &self,
        workspace_id: &WorkspaceId,
        input: &str,
    ) -> Result<RecurrenceSeriesDetail, Error> {
        let resolution = self.resolve_recurrence_ref(workspace_id, input).await?;
        self.database
            .recurrence_series_detail(workspace_id, &resolution.series_id)
            .await
            .map(Into::into)
            .map_err(Error::from_internal)
    }

    pub async fn recurrence_history(
        &self,
        workspace_id: &WorkspaceId,
        input: &str,
        offset: usize,
        limit: usize,
    ) -> Result<RecurrenceHistoryPage, Error> {
        if limit == 0 || limit > MAX_RECURRENCE_HISTORY_LIMIT {
            return Err(Error::new(
                ErrorCode::Validation,
                format!(
                    "recurrence history limit must be between 1 and {MAX_RECURRENCE_HISTORY_LIMIT}"
                ),
            ));
        }
        let resolution = self.resolve_recurrence_ref(workspace_id, input).await?;
        self.database
            .recurrence_history(workspace_id, &resolution.series_id, offset, limit)
            .await
            .map(|page| RecurrenceHistoryPage {
                series_ref: page.series_ref,
                items: page.items.into_iter().map(Into::into).collect(),
                offset: page.offset,
                limit: page.limit,
                total: page.total,
                has_more: page.has_more,
            })
            .map_err(Error::from_internal)
    }

    pub async fn resolve_recurrence_occurrence(
        &self,
        workspace_id: &WorkspaceId,
        task_id: &TaskId,
        outcome: RecurrenceOutcome,
    ) -> Result<RecurrenceResolveResult, Error> {
        let workspace = self.workspace(workspace_id).await?;
        self.database
            .resolve_recurrence_occurrence(&workspace, task_id, outcome)
            .await
            .map(|value| RecurrenceResolveResult {
                series: value.series.into(),
                occurrence: value.resolved.into(),
                task: value.task.into(),
                successor: value.successor.map(Into::into),
            })
            .map_err(Error::from_internal)
    }

    pub async fn complete_recurrence_occurrence(
        &self,
        workspace_id: &WorkspaceId,
        task_id: &TaskId,
    ) -> Result<RecurrenceResolveResult, Error> {
        self.resolve_recurrence_occurrence(workspace_id, task_id, RecurrenceOutcome::Completed)
            .await
    }

    pub async fn skip_recurrence_occurrence(
        &self,
        workspace_id: &WorkspaceId,
        task_id: &TaskId,
    ) -> Result<RecurrenceResolveResult, Error> {
        self.resolve_recurrence_occurrence(workspace_id, task_id, RecurrenceOutcome::Skipped)
            .await
    }

    pub async fn pause_recurrence_series(
        &self,
        workspace_id: &WorkspaceId,
        series_id: &RecurrenceSeriesId,
    ) -> Result<RecurrenceStateResult, Error> {
        let workspace = self.workspace(workspace_id).await?;
        self.database
            .pause_recurrence_series(&workspace, series_id)
            .await
            .map(Into::into)
            .map_err(Error::from_internal)
    }

    pub async fn resume_recurrence_series(
        &self,
        workspace_id: &WorkspaceId,
        series_id: &RecurrenceSeriesId,
    ) -> Result<RecurrenceStateResult, Error> {
        let workspace = self.workspace(workspace_id).await?;
        let at = crate::ids::now_utc();
        self.database
            .resume_recurrence_series(&workspace, series_id, at)
            .await
            .map(Into::into)
            .map_err(Error::from_internal)
    }

    pub async fn stop_recurrence_series(
        &self,
        workspace_id: &WorkspaceId,
        series_id: &RecurrenceSeriesId,
        skip_current: bool,
    ) -> Result<RecurrenceStateResult, Error> {
        let workspace = self.workspace(workspace_id).await?;
        self.database
            .stop_recurrence_series(&workspace, series_id, skip_current)
            .await
            .map(Into::into)
            .map_err(Error::from_internal)
    }

    pub async fn recurrence_task_report(
        &self,
        workspace_id: &WorkspaceId,
        expand_recurring: bool,
    ) -> Result<Vec<TaskSummary>, Error> {
        self.workspace(workspace_id).await?;
        self.database
            .list_task_items(
                workspace_id,
                TaskFilters {
                    exclude_epics: true,
                    expand_recurring,
                    ..TaskFilters::default()
                },
                TaskQueryMode::Flat,
                TaskSort::Created,
                SortDirection::Asc,
            )
            .await
            .map(|items| items.into_iter().map(TaskSummary::from).collect())
            .map_err(Error::from_internal)
    }

    pub async fn list_conflicts(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<Vec<ConflictSummary>, Error> {
        let workspace = self.workspace(workspace_id).await?;
        self.database
            .list_conflicts(&workspace, None, None)
            .await
            .map_err(Error::from_internal)?
            .into_iter()
            .filter(|conflict| !conflict.field.starts_with("metadata:"))
            .map(|conflict| {
                Ok(ConflictSummary {
                    task_id: conflict.task_id,
                    task_title: conflict.title,
                    project_key: conflict.project_key,
                    project_prefix: conflict.project_prefix,
                    field: ConflictField::from_task_field(TaskField::parse_or_unknown(
                        &conflict.field,
                    )?),
                })
            })
            .collect::<Result<Vec<_>, InternalError>>()
            .map_err(Error::from_internal)
    }

    pub async fn inspect_conflicts(
        &self,
        workspace_id: &WorkspaceId,
        task_id: &TaskId,
    ) -> Result<Vec<Conflict>, Error> {
        let workspace = self.workspace(workspace_id).await?;
        let details = self
            .database
            .task_conflicts(&workspace, task_id, None)
            .await
            .map_err(Error::from_internal)?;
        let mut connection = self
            .database
            .acquire_reader()
            .await
            .map_err(Error::from_internal)?;
        let mut conflicts = Vec::with_capacity(details.len());
        for detail in details {
            if detail.field.starts_with("metadata:") {
                continue;
            }
            let field = TaskField::parse_or_unknown(&detail.field).map_err(Error::from_internal)?;
            let local_value = crate::query::conflict_display_value(
                &mut connection,
                workspace_id,
                field.as_str(),
                &detail.local_value,
            )
            .await
            .map_err(Error::from_internal)?;
            let remote_value = crate::query::conflict_display_value(
                &mut connection,
                workspace_id,
                field.as_str(),
                &detail.remote_value,
            )
            .await
            .map_err(Error::from_internal)?;
            conflicts.push(Conflict {
                task_id: task_id.clone(),
                field: ConflictField::from_task_field(field),
                local_value: bounded_conflict_display_value(local_value),
                remote_value: bounded_conflict_display_value(remote_value),
                variant_a: detail.variant_a,
                variant_b: detail.variant_b,
            });
        }
        Ok(conflicts)
    }

    pub async fn resolve_conflict(
        &self,
        workspace_id: &WorkspaceId,
        task_id: &TaskId,
        field: ConflictField,
        variant_a: String,
        variant_b: String,
        resolution: ConflictResolution,
    ) -> Result<TaskRecord, Error> {
        let workspace = self.workspace(workspace_id).await?;
        let field_name = field.as_str();
        let resolution = match &resolution {
            ConflictResolution::Local => crate::operations::ConflictResolutionValue::Local,
            ConflictResolution::Remote => crate::operations::ConflictResolutionValue::Remote,
            ConflictResolution::Explicit(value) => {
                crate::operations::ConflictResolutionValue::Explicit(value)
            }
        };
        let mut connection = self
            .database
            .acquire_writer()
            .await
            .map_err(Error::from_internal)?;
        crate::operations::resolve_conflict_transaction(
            &mut connection,
            &workspace,
            task_id,
            field_name,
            crate::operations::ExpectedConflictIdentity {
                variant_a: &variant_a,
                variant_b: &variant_b,
            },
            resolution,
        )
        .await
        .map(|outcome| TaskRecord::from(outcome.task))
        .map_err(Error::from_internal)
    }

    async fn workspace(&self, workspace_id: &WorkspaceId) -> Result<Workspace, Error> {
        self.database
            .workspace_for_id(workspace_id)
            .await
            .map_err(|error| {
                let error = Error::from_internal(error);
                if error.code == ErrorCode::NotFound {
                    Error::new(
                        ErrorCode::NotFound,
                        format!("workspace not found: {workspace_id}"),
                    )
                } else {
                    error
                }
            })
    }
}

fn validate_project(project: &str) -> Result<(), Error> {
    if crate::projects::normalize_key(project).is_empty() {
        return Err(Error::new(
            ErrorCode::Validation,
            "project must contain at least one letter or number".to_string(),
        ));
    }
    Ok(())
}

fn validate_optional_date(field: &str, value: Option<&str>) -> Result<(), Error> {
    let Some(value) = value else {
        return Ok(());
    };
    if value.is_empty() {
        return Err(Error::new(
            ErrorCode::Validation,
            format!("{field} must be absent or contain a date"),
        ));
    }
    let result = match field {
        "available_at" => crate::time_validation::validate_available_at_value(value),
        "due_on" => crate::time_validation::validate_due_on_value(value),
        _ => unreachable!("consumer API validates only task dates"),
    };
    result.map_err(|error| Error::new(ErrorCode::Validation, error.to_string()))
}

fn validate_date_update(field: &str, update: &OptionalDateUpdate) -> Result<(), Error> {
    match update {
        OptionalDateUpdate::Unchanged | OptionalDateUpdate::Clear => Ok(()),
        OptionalDateUpdate::Set(value) => validate_optional_date(field, Some(value)),
    }
}

fn parse_date(field: &str, value: &str) -> Result<NaiveDate, Error> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| {
        Error::new(
            ErrorCode::Validation,
            format!("{field} must use YYYY-MM-DD"),
        )
    })
}

fn parse_local_time(field: &str, value: &str) -> Result<NaiveTime, Error> {
    NaiveTime::parse_from_str(value, "%H:%M")
        .or_else(|_| NaiveTime::parse_from_str(value, "%H:%M:%S"))
        .map_err(|_| {
            Error::new(
                ErrorCode::Validation,
                format!("{field} must use HH:MM or HH:MM:SS"),
            )
        })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageLayout {
    pub root: PathBuf,
    pub objects: PathBuf,
    pub staging: PathBuf,
    pub trash: PathBuf,
    pub previews: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRecord {
    pub id: WorkspaceId,
    pub key: String,
    pub name: String,
}

impl From<Workspace> for WorkspaceRecord {
    fn from(workspace: Workspace) -> Self {
        Self {
            id: workspace.id,
            key: workspace.key,
            name: workspace.name,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceQueueSummary {
    pub workspace: WorkspaceRecord,
    pub open_task_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRecord {
    pub id: ProjectId,
    pub key: String,
    pub name: String,
}

impl From<Project> for ProjectRecord {
    fn from(project: Project) -> Self {
        Self {
            id: project.id,
            key: project.key,
            name: project.name,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IosQueueState {
    pub selected_workspace: WorkspaceRecord,
    pub workspaces: Vec<WorkspaceQueueSummary>,
    pub projects: Vec<ProjectRecord>,
    pub labels: Vec<String>,
    pub queue: QueueReport,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IosTaskListRow {
    pub id: TaskId,
    pub title: String,
    pub display_ref: String,
    pub project_key: String,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    pub due_on: Option<String>,
    pub is_epic: bool,
}

impl From<TaskListItem> for IosTaskListRow {
    fn from(item: TaskListItem) -> Self {
        Self {
            id: item.task.id,
            title: item.task.title,
            display_ref: item.display_ref,
            project_key: item.task.project_key,
            status: item.task.status,
            priority: item.task.priority,
            due_on: item.task.due_on,
            is_epic: item.task.is_epic,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IosTaskSearchResult {
    pub task: IosTaskListRow,
    pub matched_field: IosSearchMatchedField,
    pub snippet: Option<String>,
}

pub use crate::query::TaskNote as IosTaskNote;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IosTaskLink {
    pub task_id: TaskId,
    pub display_ref: String,
    pub title: String,
    pub project_key: Option<String>,
    pub status: TaskStatus,
    pub deleted: bool,
    pub unresolved: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IosAttachmentRead {
    Bytes { bytes: Vec<u8> },
    Unavailable,
    Missing,
    Corrupt,
    Invalidated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IosTaskAttachment {
    pub attachment_id: String,
    pub media_type: String,
    pub byte_size: i64,
    pub filename: Option<String>,
    pub alt_text: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub availability: IosAttachmentAvailability,
    pub has_blob: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IosTaskActivityKind {
    Created,
    Title,
    Description,
    Status,
    Priority,
    Project,
    Deletion,
    Availability,
    DueDate,
    Epic,
    Attachment,
    Metadata,
    Label,
    Note,
    Blocker,
    Related,
    Conflict,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IosTaskActivity {
    pub change_id: String,
    pub created_at: String,
    pub kind: IosTaskActivityKind,
    pub summary: String,
    pub anchors_queue_idle: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IosTaskDetail {
    pub id: TaskId,
    pub workspace_id: WorkspaceId,
    pub workspace_key: String,
    pub workspace_name: String,
    pub title: String,
    pub description: String,
    pub display_ref: String,
    pub project_key: String,
    pub project_name: String,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    pub created_at: String,
    pub updated_at: String,
    pub available_at: Option<String>,
    pub due_on: Option<String>,
    pub deleted: bool,
    pub is_epic: bool,
    pub labels: Vec<String>,
    pub notes: Vec<IosTaskNote>,
    pub activity: Vec<IosTaskActivity>,
    pub blocked_by: Vec<IosTaskLink>,
    pub blocks: Vec<IosTaskLink>,
    pub related: Vec<IosTaskLink>,
    pub epic_parent: Option<IosTaskLink>,
    pub epic_children: Vec<IosTaskLink>,
    pub epic_done_count: u32,
    pub epic_total_count: u32,
    pub epic_progress: Option<IosEpicProgress>,
    pub attachments: Vec<IosTaskAttachment>,
    pub unresolved_conflict_count: u32,
    pub unresolved_conflict_fields: Vec<ConflictField>,
}

impl IosTaskDetail {
    fn from_detail(detail: crate::query::TaskDetail, workspace: Workspace) -> Result<Self, Error> {
        let item = detail.item;
        let anchor = item.queue_idle_activity_index();
        let activity = item
            .activity
            .iter()
            .enumerate()
            .map(|(index, action)| IosTaskActivity {
                change_id: action.change_id.clone(),
                created_at: action.created_at.clone(),
                kind: match action.verb.as_str() {
                    "create" => IosTaskActivityKind::Created,
                    "title" => IosTaskActivityKind::Title,
                    "details" => IosTaskActivityKind::Description,
                    "status" => IosTaskActivityKind::Status,
                    "priority" => IosTaskActivityKind::Priority,
                    "project" => IosTaskActivityKind::Project,
                    "delete" => IosTaskActivityKind::Deletion,
                    "availability" => IosTaskActivityKind::Availability,
                    "due date" => IosTaskActivityKind::DueDate,
                    "epic" => IosTaskActivityKind::Epic,
                    "attachment" => IosTaskActivityKind::Attachment,
                    "metadata" => IosTaskActivityKind::Metadata,
                    "label" => IosTaskActivityKind::Label,
                    "note" => IosTaskActivityKind::Note,
                    "blocker" => IosTaskActivityKind::Blocker,
                    "related link" => IosTaskActivityKind::Related,
                    "conflict" => IosTaskActivityKind::Conflict,
                    _ => IosTaskActivityKind::Other,
                },
                summary: action.task_activity_summary(&item.task.title),
                anchors_queue_idle: anchor == Some(index),
            })
            .collect();
        let task = item.task;
        let blocked_by = detail
            .dependencies
            .depends_on
            .into_iter()
            .map(ios_task_link_from_dependency)
            .collect();
        let blocks = detail
            .dependencies
            .blocks
            .into_iter()
            .map(ios_task_link_from_dependency)
            .collect();
        let related = detail
            .related
            .into_iter()
            .map(|link| IosTaskLink {
                task_id: link.task_id,
                display_ref: link.display_ref,
                title: link.title,
                project_key: None,
                status: link.status,
                deleted: link.deleted,
                unresolved: false,
            })
            .collect();
        let epic_parent = item
            .epic_parent
            .map(ios_task_link_from_enriched)
            .transpose()?;
        let epic_children = item
            .epic_children
            .into_iter()
            .map(ios_task_link_from_enriched)
            .collect::<Result<Vec<_>, _>>()?;
        let epic_progress = item.epic_rollup.clone().map(IosEpicProgress::from);
        let (epic_done_count, epic_total_count) = item
            .epic_rollup
            .map(|rollup| {
                (
                    rollup.done.min(u32::MAX as usize) as u32,
                    rollup.total.min(u32::MAX as usize) as u32,
                )
            })
            .unwrap_or_default();
        let attachments = item
            .attachments
            .into_iter()
            .map(|attachment| IosTaskAttachment {
                attachment_id: attachment.attachment_id,
                media_type: attachment.media_type,
                byte_size: attachment.byte_size,
                filename: attachment.filename,
                alt_text: attachment.alt_text,
                width: attachment.width,
                height: attachment.height,
                availability: attachment.bytes_state,
                has_blob: attachment.has_blob,
            })
            .collect();
        Ok(Self {
            id: task.id,
            workspace_id: task.workspace_id,
            workspace_key: workspace.key,
            workspace_name: workspace.name,
            title: task.title,
            description: task.description,
            display_ref: item.display_ref,
            project_key: task.project_key,
            project_name: detail.project_name,
            status: task.status,
            priority: task.priority,
            created_at: task.created_at,
            updated_at: task.updated_at,
            available_at: task.available_at,
            due_on: task.due_on,
            deleted: task.deleted,
            is_epic: task.is_epic,
            labels: item.labels,
            notes: detail.notes,
            activity,
            blocked_by,
            blocks,
            related,
            epic_parent,
            epic_children,
            epic_done_count,
            epic_total_count,
            epic_progress,
            attachments,
            unresolved_conflict_count: detail.conflicts.len().min(u32::MAX as usize) as u32,
            unresolved_conflict_fields: detail
                .conflicts
                .into_iter()
                .filter(|conflict| !conflict.field.starts_with("metadata:"))
                .map(|conflict| {
                    TaskField::parse_or_unknown(&conflict.field)
                        .map(ConflictField::from_task_field)
                        .map_err(Error::from_internal)
                })
                .collect::<Result<Vec<_>, _>>()?,
        })
    }
}

fn ios_task_link_from_dependency(link: crate::query::TaskDependencyItem) -> IosTaskLink {
    IosTaskLink {
        task_id: link.task.id,
        display_ref: link.display_ref,
        title: link.task.title,
        project_key: Some(link.task.project_key),
        status: link.task.status,
        deleted: link.task.deleted,
        unresolved: link.unresolved,
    }
}

fn ios_task_link_from_enriched(
    link: crate::query::TaskDependencyLink,
) -> Result<IosTaskLink, Error> {
    let status =
        TaskStatus::parse(&link.status).map_err(|error| Error::from_internal(error.into()))?;
    Ok(IosTaskLink {
        task_id: link.task_id,
        display_ref: link.display_ref,
        title: link.title,
        project_key: None,
        status,
        deleted: false,
        unresolved: link.unresolved,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IosTaskCapture {
    pub title: String,
    pub description: String,
    pub project: Option<String>,
    pub priority: TaskPriority,
    pub due_on: Option<String>,
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IosTaskCaptureResult {
    pub task_id: TaskId,
    pub display_ref: String,
    pub undo_token: String,
    /// Optional refreshed projection. Absence does not change the committed receipt.
    pub state: Option<IosQueueState>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct IosCaptureUndoToken {
    workspace_id: WorkspaceId,
    task_id: TaskId,
    expected: TaskUndoSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IosQueueMutationKind {
    Start,
    Done,
    Snooze,
    SetPriority,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IosQueueMutation {
    pub kind: IosQueueMutationKind,
    pub priority: Option<TaskPriority>,
    pub local_date: Option<String>,
    pub time_zone: Option<String>,
}

impl IosQueueMutation {
    fn into_internal(self) -> Result<InternalIosTaskMutation, Error> {
        match self.kind {
            IosQueueMutationKind::Start => Ok(InternalIosTaskMutation::Start),
            IosQueueMutationKind::Done => Ok(InternalIosTaskMutation::Done),
            IosQueueMutationKind::SetPriority => self
                .priority
                .map(|priority| InternalIosTaskMutation::SetPriority {
                    priority: priority.as_str().to_string(),
                })
                .ok_or_else(|| {
                    Error::new(
                        ErrorCode::Validation,
                        "priority is required for this quick action".to_string(),
                    )
                }),
            IosQueueMutationKind::Snooze => {
                let local_date = self.local_date.ok_or_else(|| {
                    Error::new(
                        ErrorCode::Validation,
                        "local date is required for snooze".to_string(),
                    )
                })?;
                let time_zone = self.time_zone.ok_or_else(|| {
                    Error::new(
                        ErrorCode::Validation,
                        "time zone is required for snooze".to_string(),
                    )
                })?;
                Ok(InternalIosTaskMutation::Snooze {
                    available_at: tomorrow_start(&local_date, &time_zone)?,
                })
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IosQueueMutationResult {
    pub task_id: TaskId,
    pub undo_token: String,
    /// Optional refreshed projection. Absence does not change the committed receipt.
    pub state: Option<IosQueueState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IosDetailStatusReceipt {
    pub status: TaskStatus,
    pub undo_token: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct IosMutationUndoToken {
    workspace_id: WorkspaceId,
    task_id: TaskId,
    mutation: InternalIosTaskMutation,
    before: TaskUndoSnapshot,
    expected: TaskUndoSnapshot,
}

fn tomorrow_start(local_date: &str, time_zone: &str) -> Result<String, Error> {
    let date = NaiveDate::parse_from_str(local_date, "%Y-%m-%d")
        .map_err(|_| Error::new(ErrorCode::Validation, "invalid local date".to_string()))?
        .checked_add_days(Days::new(1))
        .ok_or_else(|| Error::new(ErrorCode::Validation, "invalid local date".to_string()))?;
    let zone = time_zone
        .parse::<chrono_tz::Tz>()
        .map_err(|_| Error::new(ErrorCode::Validation, "invalid IANA time zone".to_string()))?;
    let mut local = date.and_time(NaiveTime::MIN);
    let resolved = loop {
        match zone.from_local_datetime(&local) {
            LocalResult::Single(value) => break value,
            LocalResult::Ambiguous(first, second) => break first.min(second),
            LocalResult::None => {
                local = local
                    .checked_add_signed(chrono::Duration::minutes(1))
                    .ok_or_else(|| {
                        Error::new(ErrorCode::Validation, "invalid local date".to_string())
                    })?;
                if local.date() != date {
                    return Err(Error::new(
                        ErrorCode::Validation,
                        "time zone has no valid instant on the next local day".to_string(),
                    ));
                }
            }
        }
    };
    Ok(resolved
        .with_timezone(&Utc)
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataFieldRecord {
    pub id: MetadataFieldId,
    pub workspace_id: WorkspaceId,
    pub key: String,
    pub task_count: usize,
    pub series_count: usize,
}

impl From<MetadataFieldUsage> for MetadataFieldRecord {
    fn from(value: MetadataFieldUsage) -> Self {
        Self {
            id: value.field.id,
            workspace_id: value.field.workspace_id,
            key: value.field.key,
            task_count: value.task_count,
            series_count: value.series_count,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataValueRecord {
    pub field_id: MetadataFieldId,
    pub key: String,
    pub value: String,
}

impl From<TaskMetadataValue> for MetadataValueRecord {
    fn from(value: TaskMetadataValue) -> Self {
        Self {
            field_id: value.field_id,
            key: value.key,
            value: value.value,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataInput {
    pub key: String,
    pub value: String,
}

impl From<MetadataInput> for TaskMetadataInput {
    fn from(input: MetadataInput) -> Self {
        Self {
            expected_field_id: None,
            key: input.key,
            value: input.value,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateTask {
    pub title: String,
    pub description: String,
    pub project: String,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    pub metadata: Vec<MetadataInput>,
    pub available_at: Option<String>,
    pub due_on: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IosTaskEditContext {
    pub detail: IosTaskDetail,
    pub projects: Vec<ProjectRecord>,
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IosTaskEdit {
    pub title: Option<String>,
    pub description: Option<String>,
    pub project_id: Option<ProjectId>,
    pub status: Option<TaskStatus>,
    pub priority: Option<TaskPriority>,
    pub add_labels: Vec<String>,
    pub remove_labels: Vec<String>,
    pub available_at: OptionalDateUpdate,
    pub due_on: OptionalDateUpdate,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UpdateTask {
    pub title: Option<String>,
    pub description: Option<String>,
    pub project: Option<String>,
    pub status: Option<TaskStatus>,
    pub priority: Option<TaskPriority>,
    pub set_metadata: Vec<MetadataInput>,
    pub remove_metadata: Vec<String>,
    pub available_at: OptionalDateUpdate,
    pub due_on: OptionalDateUpdate,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum OptionalDateUpdate {
    #[default]
    Unchanged,
    Set(String),
    Clear,
}

impl OptionalDateUpdate {
    fn into_internal(self) -> Option<Option<String>> {
        match self {
            Self::Unchanged => None,
            Self::Set(value) => Some(Some(value)),
            Self::Clear => Some(None),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecurrenceWeekday {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

impl From<RecurrenceWeekday> for Weekday {
    fn from(value: RecurrenceWeekday) -> Self {
        match value {
            RecurrenceWeekday::Monday => Self::Mon,
            RecurrenceWeekday::Tuesday => Self::Tue,
            RecurrenceWeekday::Wednesday => Self::Wed,
            RecurrenceWeekday::Thursday => Self::Thu,
            RecurrenceWeekday::Friday => Self::Fri,
            RecurrenceWeekday::Saturday => Self::Sat,
            RecurrenceWeekday::Sunday => Self::Sun,
        }
    }
}

impl From<Weekday> for RecurrenceWeekday {
    fn from(value: Weekday) -> Self {
        match value {
            Weekday::Mon => Self::Monday,
            Weekday::Tue => Self::Tuesday,
            Weekday::Wed => Self::Wednesday,
            Weekday::Thu => Self::Thursday,
            Weekday::Fri => Self::Friday,
            Weekday::Sat => Self::Saturday,
            Weekday::Sun => Self::Sunday,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceRule {
    pub frequency: RecurrenceFrequency,
    pub interval: u32,
    pub weekdays: Vec<RecurrenceWeekday>,
}

impl RecurrenceRule {
    fn into_internal(self) -> Result<InternalRecurrenceRule, Error> {
        InternalRecurrenceRule::new(
            self.frequency,
            self.interval,
            WeekdaySet::from_weekdays(self.weekdays.into_iter().map(Into::into)),
        )
        .map_err(|error| Error::new(ErrorCode::Validation, error.to_string()))
    }
}

impl From<InternalRecurrenceRule> for RecurrenceRule {
    fn from(value: InternalRecurrenceRule) -> Self {
        Self {
            frequency: value.frequency(),
            interval: value.interval(),
            weekdays: value.weekdays_set().iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceScheduleInput {
    pub rule: RecurrenceRule,
    pub timezone: String,
    pub start_on: String,
    pub available_local_time: Option<String>,
    pub due_policy: RecurrenceDuePolicy,
}

impl RecurrenceScheduleInput {
    fn into_internal(self) -> Result<RecurrenceSchedule, Error> {
        let timezone = self
            .timezone
            .parse::<TimeZoneId>()
            .map_err(|error| Error::new(ErrorCode::Validation, error.to_string()))?;
        let start_on = parse_date("start_on", &self.start_on)?;
        let available_local_time = self
            .available_local_time
            .map(|value| parse_local_time("available_local_time", &value))
            .transpose()?;
        Ok(RecurrenceSchedule::new(
            self.rule.into_internal()?,
            timezone,
            start_on,
            available_local_time,
            self.due_policy,
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateRecurrenceSeries {
    pub title: String,
    pub description: String,
    pub project: String,
    pub priority: TaskPriority,
    pub initial_status: TaskStatus,
    pub labels: Vec<String>,
    pub metadata: Vec<MetadataInput>,
    pub schedule: RecurrenceScheduleInput,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum OptionalLocalTimeUpdate {
    #[default]
    Unchanged,
    Set(String),
    Clear,
}

impl OptionalLocalTimeUpdate {
    fn into_internal(self) -> Result<Option<Option<NaiveTime>>, Error> {
        match self {
            Self::Unchanged => Ok(None),
            Self::Set(value) => Ok(Some(Some(parse_local_time(
                "available_local_time",
                &value,
            )?))),
            Self::Clear => Ok(Some(None)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UpdateRecurrenceTemplate {
    pub title: Option<String>,
    pub description: Option<String>,
    pub project: Option<String>,
    pub priority: Option<TaskPriority>,
    pub initial_status: Option<TaskStatus>,
    pub labels: Option<Vec<String>>,
    pub set_metadata: Vec<MetadataInput>,
    pub remove_metadata: Vec<String>,
    pub available_local_time: OptionalLocalTimeUpdate,
    pub due_policy: Option<RecurrenceDuePolicy>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceSeriesRecord {
    pub workspace_id: WorkspaceId,
    pub id: RecurrenceSeriesId,
    pub title: String,
    pub description: String,
    pub project_id: ProjectId,
    pub priority: TaskPriority,
    pub initial_status: TaskStatus,
    pub rule: RecurrenceRule,
    pub timezone: String,
    pub start_on: String,
    pub available_local_time: Option<String>,
    pub due_policy: RecurrenceDuePolicy,
    pub state: RecurrenceSeriesState,
    pub stopped_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl From<RecurrenceSeries> for RecurrenceSeriesRecord {
    fn from(value: RecurrenceSeries) -> Self {
        Self {
            workspace_id: value.workspace_id,
            id: value.id,
            title: value.title,
            description: value.description,
            project_id: value.project_id,
            priority: value.priority,
            initial_status: value.initial_status,
            rule: value.rule.into(),
            timezone: value.timezone.to_string(),
            start_on: value.start_on.to_string(),
            available_local_time: value
                .available_local_time
                .map(|time| time.format("%H:%M:%S").to_string()),
            due_policy: value.due_policy,
            state: value.state,
            stopped_at: value.stopped_at,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceOccurrenceRecord {
    pub series_id: RecurrenceSeriesId,
    pub slot_on: String,
    pub task_id: Option<TaskId>,
    pub outcome: Option<RecurrenceOutcome>,
    pub resolved_at: Option<String>,
    pub projection_state: RecurrenceProjectionState,
    pub archived_at: Option<String>,
}

impl From<RecurrenceOccurrence> for RecurrenceOccurrenceRecord {
    fn from(value: RecurrenceOccurrence) -> Self {
        Self {
            series_id: value.series_id,
            slot_on: value.slot_on.to_string(),
            task_id: value.task_id,
            outcome: value.outcome,
            resolved_at: value.resolved_at,
            projection_state: value.projection_state,
            archived_at: value.archived_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceCreateResult {
    pub series: RecurrenceSeriesRecord,
    pub series_ref: String,
    pub occurrence: RecurrenceOccurrenceRecord,
    pub task: TaskRecord,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceTemplateUpdateResult {
    pub series: RecurrenceSeriesRecord,
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceResolveResult {
    pub series: RecurrenceSeriesRecord,
    pub occurrence: RecurrenceOccurrenceRecord,
    pub task: TaskRecord,
    pub successor: Option<TaskRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceStateResult {
    pub series: RecurrenceSeriesRecord,
    pub occurrence: Option<RecurrenceOccurrenceRecord>,
}

impl From<crate::operations::RecurrenceStateOutcome> for RecurrenceStateResult {
    fn from(value: crate::operations::RecurrenceStateOutcome) -> Self {
        Self {
            series: value.series.into(),
            occurrence: value.occurrence.map(Into::into),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceRefResolution {
    pub series_id: RecurrenceSeriesId,
    pub series_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RecurrenceCounts {
    pub completed: u64,
    pub skipped: u64,
    pub missed: u64,
    pub pause_intervals: u64,
    pub latest_slot_on: Option<String>,
    pub latest_outcome: Option<RecurrenceOutcome>,
}

impl From<InternalRecurrenceCounts> for RecurrenceCounts {
    fn from(value: InternalRecurrenceCounts) -> Self {
        Self {
            completed: value.completed as u64,
            skipped: value.skipped as u64,
            missed: value.missed as u64,
            pause_intervals: value.pause_intervals as u64,
            latest_slot_on: value.latest_slot_on,
            latest_outcome: value.latest_outcome,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceSeriesSummary {
    pub series: RecurrenceSeriesRecord,
    pub series_ref: String,
    pub rule_label: String,
    pub current_slot_on: Option<String>,
    pub current_task_ref: Option<String>,
    pub counts: RecurrenceCounts,
}

impl From<InternalRecurrenceSeriesSummary> for RecurrenceSeriesSummary {
    fn from(value: InternalRecurrenceSeriesSummary) -> Self {
        Self {
            series: value.series.into(),
            series_ref: value.series_ref,
            rule_label: value.rule_label,
            current_slot_on: value.current_slot_on,
            current_task_ref: value.current_task_ref,
            counts: value.counts.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceSeriesConflict {
    pub field: String,
    pub variant_a: String,
    pub local_value: String,
    pub variant_b: String,
    pub remote_value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceSeriesDetail {
    pub series: RecurrenceSeriesRecord,
    pub labels: Vec<String>,
    pub metadata: Vec<MetadataValueRecord>,
    pub summary: RecurrenceSeriesSummary,
    pub current_occurrence: Option<RecurrenceOccurrenceRecord>,
    pub lifecycle_conflicts: Vec<RecurrenceSeriesConflict>,
}

impl From<InternalRecurrenceSeriesDetail> for RecurrenceSeriesDetail {
    fn from(value: InternalRecurrenceSeriesDetail) -> Self {
        Self {
            series: value.series.into(),
            labels: value.labels,
            metadata: value.metadata.into_iter().map(Into::into).collect(),
            summary: value.summary.into(),
            current_occurrence: value.current_occurrence.map(Into::into),
            lifecycle_conflicts: value
                .lifecycle_conflicts
                .into_iter()
                .map(|conflict| RecurrenceSeriesConflict {
                    field: conflict.field,
                    variant_a: conflict.variant_a,
                    local_value: conflict.local_value,
                    variant_b: conflict.variant_b,
                    remote_value: conflict.remote_value,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceHistoryRow {
    pub kind: RecurrenceHistoryKind,
    pub slot_on: Option<String>,
    pub interval_started_at: Option<String>,
    pub interval_ended_at: Option<String>,
    pub task_id: Option<TaskId>,
    pub task_ref: Option<String>,
    pub openable: bool,
    pub archived_projection: bool,
    pub resolved_at: Option<String>,
}

impl From<InternalRecurrenceHistoryEntry> for RecurrenceHistoryRow {
    fn from(value: InternalRecurrenceHistoryEntry) -> Self {
        Self {
            kind: value.kind,
            slot_on: value.slot_on,
            interval_started_at: value.interval_started_at,
            interval_ended_at: value.interval_ended_at,
            task_id: value.task_id,
            task_ref: value.task_ref,
            openable: value.openable,
            archived_projection: value.archived_projection,
            resolved_at: value.resolved_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceHistoryPage {
    pub series_ref: String,
    pub items: Vec<RecurrenceHistoryRow>,
    pub offset: usize,
    pub limit: usize,
    pub total: usize,
    pub has_more: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRecurrenceSummary {
    pub series_id: RecurrenceSeriesId,
    pub series_ref: String,
    pub slot_on: String,
    pub rule_label: String,
    pub timezone: String,
    pub lifecycle: RecurrenceSeriesState,
    pub outcome: Option<RecurrenceOutcome>,
    pub projection_state: RecurrenceProjectionState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceTaskGroup {
    pub series_id: RecurrenceSeriesId,
    pub series_ref: String,
    pub counts: RecurrenceCounts,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueDate {
    Due { on: String, kind: QueueDateKind },
    Deferred { at: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueLabelSummary {
    pub first: String,
    pub remaining_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueTaskSummary {
    pub id: TaskId,
    pub title: String,
    pub display_ref: String,
    pub project_key: String,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    pub due_on: Option<String>,
    pub band: QueueBand,
    pub reason: Option<QueueReason>,
    pub date: Option<QueueDate>,
    pub label: Option<QueueLabelSummary>,
    pub is_epic: bool,
    pub live_attachment_count: u32,
    pub has_conflict: bool,
    pub unresolved_blocker_count: i64,
}

impl From<TaskListItem> for QueueTaskSummary {
    fn from(value: TaskListItem) -> Self {
        let date = if value.queue.has_deferred_date {
            value
                .task
                .available_at
                .clone()
                .map(|at| QueueDate::Deferred { at })
        } else {
            value.queue.date_kind.and_then(|kind| {
                value
                    .task
                    .due_on
                    .clone()
                    .map(|on| QueueDate::Due { on, kind })
            })
        };
        let label = value
            .labels
            .first()
            .cloned()
            .map(|first| QueueLabelSummary {
                first,
                remaining_count: value.labels.len().saturating_sub(1).min(u32::MAX as usize) as u32,
            });
        Self {
            id: value.task.id,
            title: value.task.title,
            display_ref: value.display_ref,
            project_key: value.task.project_key,
            status: value.task.status,
            priority: value.task.priority,
            due_on: value.task.due_on,
            band: value.queue.band,
            reason: value.queue.reason,
            date,
            label,
            is_epic: value.task.is_epic,
            live_attachment_count: value.live_attachment_count,
            has_conflict: value.has_conflict,
            unresolved_blocker_count: value.unresolved_blocker_count,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueReport {
    pub tasks: Vec<QueueTaskSummary>,
    pub unresolved_conflict_count: u32,
    pub last_success_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskSummary {
    pub task: TaskRecord,
    pub display_ref: String,
    pub recurrence: Option<TaskRecurrenceSummary>,
    pub recurrence_group: Option<RecurrenceTaskGroup>,
}

impl From<TaskListItem> for TaskSummary {
    fn from(value: TaskListItem) -> Self {
        Self {
            task: value.task.into(),
            display_ref: value.display_ref,
            recurrence: value.recurrence.map(|summary| TaskRecurrenceSummary {
                series_id: summary.series_id,
                series_ref: summary.series_ref,
                slot_on: summary.slot_on,
                rule_label: summary.rule_label,
                timezone: summary.timezone,
                lifecycle: summary.lifecycle,
                outcome: summary.outcome,
                projection_state: summary.projection_state,
            }),
            recurrence_group: value.recurrence_group.map(|group| RecurrenceTaskGroup {
                series_id: group.series_id,
                series_ref: group.series_ref,
                counts: group.counts.into(),
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelatedTaskRecord {
    pub task_id: TaskId,
    pub display_ref: String,
    pub title: String,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    pub deleted: bool,
    pub linked_at: String,
}

impl From<crate::query::TaskRelatedLink> for RelatedTaskRecord {
    fn from(value: crate::query::TaskRelatedLink) -> Self {
        Self {
            task_id: value.task_id,
            display_ref: value.display_ref,
            title: value.title,
            status: value.status,
            priority: value.priority,
            deleted: value.deleted,
            linked_at: value.linked_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelatedMutationResult {
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRecord {
    pub id: TaskId,
    pub workspace_id: WorkspaceId,
    pub title: String,
    pub description: String,
    pub project_id: ProjectId,
    pub project_key: String,
    pub project_prefix: String,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    pub created_at: String,
    pub updated_at: String,
    pub available_at: Option<String>,
    pub due_on: Option<String>,
    pub metadata: Vec<MetadataValueRecord>,
    pub related: Vec<RelatedTaskRecord>,
}

impl From<Task> for TaskRecord {
    fn from(task: Task) -> Self {
        Self {
            id: task.id,
            workspace_id: task.workspace_id,
            title: task.title,
            description: task.description,
            project_id: task.project_id,
            project_key: task.project_key,
            project_prefix: task.project_prefix,
            status: task.status,
            priority: task.priority,
            created_at: task.created_at,
            updated_at: task.updated_at,
            available_at: task.available_at,
            due_on: task.due_on,
            metadata: Vec::new(),
            related: Vec::new(),
        }
    }
}

impl TaskRecord {
    fn with_metadata(task: Task, metadata: Vec<TaskMetadataValue>) -> Self {
        Self::with_metadata_and_related(task, metadata, Vec::new())
    }

    fn with_metadata_and_related(
        task: Task,
        metadata: Vec<TaskMetadataValue>,
        related: Vec<crate::query::TaskRelatedLink>,
    ) -> Self {
        let mut record = Self::from(task);
        record.metadata = metadata.into_iter().map(Into::into).collect();
        record.related = related.into_iter().map(Into::into).collect();
        record
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskUpdateResult {
    pub task: TaskRecord,
    pub changed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictField {
    Title,
    Description,
    Project,
    Status,
    Priority,
    AvailableAt,
    DueOn,
    Deleted,
    IsEpic,
}

impl ConflictField {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Description => "description",
            Self::Project => "project",
            Self::Status => "status",
            Self::Priority => "priority",
            Self::AvailableAt => "available_at",
            Self::DueOn => "due_on",
            Self::Deleted => "deleted",
            Self::IsEpic => "is_epic",
        }
    }

    fn from_task_field(field: TaskField) -> Self {
        match field {
            TaskField::Title => Self::Title,
            TaskField::Description => Self::Description,
            TaskField::Project => Self::Project,
            TaskField::Status => Self::Status,
            TaskField::Priority => Self::Priority,
            TaskField::AvailableAt => Self::AvailableAt,
            TaskField::DueOn => Self::DueOn,
            TaskField::Deleted => Self::Deleted,
            TaskField::IsEpic => Self::IsEpic,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictSummary {
    pub task_id: TaskId,
    pub task_title: String,
    pub project_key: String,
    pub project_prefix: String,
    pub field: ConflictField,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub task_id: TaskId,
    pub field: ConflictField,
    pub local_value: String,
    pub remote_value: String,
    pub variant_a: String,
    pub variant_b: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictResolution {
    Local,
    Remote,
    Explicit(String),
}

fn bounded_conflict_display_value(value: String) -> String {
    const MAX_CHARACTERS: usize = 512;
    let mut characters = value.chars();
    let preview: String = characters.by_ref().take(MAX_CHARACTERS).collect();
    if characters.next().is_some() {
        format!("{preview}…")
    } else {
        preview
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    Validation,
    NotFound,
    OpenConflict,
    GenerationConflict,
    Database,
    Internal,
}

impl ErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Validation => "validation",
            Self::NotFound => "not_found",
            Self::OpenConflict => "open_conflict",
            Self::GenerationConflict => "generation_conflict",
            Self::Database => "database",
            Self::Internal => "internal",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub code: ErrorCode,
    pub message: String,
}

impl Error {
    fn new(code: ErrorCode, message: String) -> Self {
        Self { code, message }
    }

    fn database_open(error: InternalError) -> Self {
        Self::new(ErrorCode::Database, error.to_string())
    }

    fn from_internal(error: InternalError) -> Self {
        let code = error
            .chain()
            .find_map(|cause| cause.downcast_ref::<crate::error::CoreError>())
            .map(|error| match error.kind() {
                crate::error::ErrorKind::Validation => ErrorCode::Validation,
                crate::error::ErrorKind::NotFound => ErrorCode::NotFound,
                crate::error::ErrorKind::OpenConflict => ErrorCode::OpenConflict,
                crate::error::ErrorKind::GenerationConflict => ErrorCode::GenerationConflict,
            })
            .unwrap_or_else(|| {
                if error
                    .chain()
                    .filter_map(|cause| cause.downcast_ref::<sqlx::Error>())
                    .any(|error| matches!(error, sqlx::Error::RowNotFound))
                {
                    ErrorCode::NotFound
                } else if error
                    .chain()
                    .any(|cause| cause.downcast_ref::<sqlx::Error>().is_some())
                {
                    ErrorCode::Database
                } else {
                    ErrorCode::Internal
                }
            });
        Self::new(code, error.to_string())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ios_capture_receipt_survives_post_commit_read_failure() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path().join("capture.sqlite"))
            .await
            .unwrap();
        let workspace = store.resolve_workspace("default").await.unwrap();
        store
            .create_task(
                &workspace.id,
                CreateTask {
                    title: "Seed".into(),
                    description: String::new(),
                    project: "ios".into(),
                    status: TaskStatus::Done,
                    priority: TaskPriority::None,
                    metadata: Vec::new(),
                    available_at: None,
                    due_on: None,
                },
            )
            .await
            .unwrap();
        store
            .fail_queue_reads
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let result = store
            .capture_ios_queue_task(
                &workspace.id,
                IosTaskCapture {
                    title: "Captured".into(),
                    description: String::new(),
                    project: Some("ios".into()),
                    priority: TaskPriority::None,
                    due_on: None,
                    labels: Vec::new(),
                },
            )
            .await;
        let tasks = store.list_tasks(&workspace.id).await.unwrap();
        assert_eq!(
            tasks.iter().filter(|task| task.title == "Captured").count(),
            1
        );
        assert!(
            result.is_ok(),
            "committed capture lost its receipt: {result:?}"
        );
        let captured = result.unwrap();
        assert!(captured.state.is_none());
        assert_eq!(captured.display_ref, captured.task_id.to_string());
        assert!(!captured.undo_token.is_empty());
        let mutation_target = store
            .capture_ios_queue_task(
                &workspace.id,
                IosTaskCapture {
                    title: "Mutation target".into(),
                    description: String::new(),
                    project: Some("ios".into()),
                    priority: TaskPriority::None,
                    due_on: None,
                    labels: Vec::new(),
                },
            )
            .await
            .unwrap();
        // Mutations can invalidate capture undo even after their values are restored.
        let changed = store
            .mutate_ios_queue_task(
                &workspace.id,
                &mutation_target.task_id,
                IosQueueMutation {
                    kind: IosQueueMutationKind::SetPriority,
                    priority: Some(TaskPriority::High),
                    local_date: None,
                    time_zone: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(changed.task_id, mutation_target.task_id);
        assert!(changed.state.is_none());
        assert_eq!(
            store
                .ios_task_detail(&workspace.id, &mutation_target.task_id)
                .await
                .unwrap()
                .priority,
            TaskPriority::High
        );
        assert!(
            store
                .undo_ios_queue_mutation(&changed.undo_token)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            store
                .ios_task_detail(&workspace.id, &mutation_target.task_id)
                .await
                .unwrap()
                .priority,
            TaskPriority::None
        );
        assert!(
            store
                .undo_ios_queue_capture(&captured.undo_token)
                .await
                .unwrap()
                .is_none()
        );
        store
            .fail_queue_reads
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let state = store.ios_queue_state().await.unwrap();
        assert!(
            state
                .queue
                .tasks
                .iter()
                .all(|task| task.id != captured.task_id)
        );
    }

    #[test]
    fn error_codes_are_stable_and_internal_errors_are_distinct() {
        assert_eq!(ErrorCode::Validation.as_str(), "validation");
        assert_eq!(ErrorCode::NotFound.as_str(), "not_found");
        assert_eq!(ErrorCode::OpenConflict.as_str(), "open_conflict");
        assert_eq!(
            ErrorCode::GenerationConflict.as_str(),
            "generation_conflict"
        );
        assert_eq!(ErrorCode::Database.as_str(), "database");
        assert_eq!(ErrorCode::Internal.as_str(), "internal");
        assert_eq!(
            Error::from_internal(
                crate::error::CoreError::generation_conflict(
                    "error recurrence-generation-conflict slot=2026-08-01 field=task"
                )
                .into()
            )
            .code,
            ErrorCode::GenerationConflict
        );
        assert_eq!(
            Error::from_internal(
                crate::error::CoreError::not_found("error recurrence-series-not-found").into()
            )
            .code,
            ErrorCode::NotFound
        );
        assert_eq!(
            Error::from_internal(anyhow::anyhow!("unexpected invariant")).code,
            ErrorCode::Internal
        );
        assert_eq!(
            Error::from_internal(sqlx::Error::PoolClosed.into()).code,
            ErrorCode::Database
        );
    }
}
