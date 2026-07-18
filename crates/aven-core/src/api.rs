use std::fmt;
use std::path::Path;

use anyhow::Error as InternalError;

use crate::choices::{TaskPriority, TaskStatus};
use crate::db::Database;
use crate::ids::{ProjectId, TaskId, WorkspaceId};
use crate::operations::{TaskDraft, TaskUpdate as InternalTaskUpdate};
use crate::query::{SortDirection, TaskFilters, TaskQueryMode, TaskSort};
use crate::task_fields::TaskField;
use crate::types::Task;
use crate::workspaces::Workspace;

#[derive(Clone)]
pub struct Store {
    database: Database,
}

impl Store {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        let database = Database::open(path.as_ref())
            .await
            .map_err(Error::database_open)?;
        Ok(Self { database })
    }

    pub async fn list_workspaces(&self) -> Result<Vec<WorkspaceRecord>, Error> {
        self.database
            .list_workspaces()
            .await
            .map(|workspaces| workspaces.into_iter().map(WorkspaceRecord::from).collect())
            .map_err(Error::from_internal)
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

    pub async fn create_task(
        &self,
        workspace_id: &WorkspaceId,
        input: CreateTask,
    ) -> Result<TaskRecord, Error> {
        validate_project(&input.project)?;
        validate_optional_date("available_at", input.available_at.as_deref())?;
        validate_optional_date("due_on", input.due_on.as_deref())?;
        let workspace = self.workspace(workspace_id).await?;
        self.database
            .create_task(
                &workspace,
                TaskDraft {
                    title: input.title,
                    description: input.description,
                    project: Some(input.project),
                    status: input.status.as_str().to_string(),
                    priority: input.priority.as_str().to_string(),
                    labels: Vec::new(),
                    available_at: input.available_at,
                    due_on: input.due_on,
                    is_epic: false,
                },
            )
            .await
            .map(|outcome| TaskRecord::from(outcome.task))
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
        self.database
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
                    ..InternalTaskUpdate::default()
                },
            )
            .await
            .map(|outcome| TaskUpdateResult {
                task: TaskRecord::from(outcome.task),
                changed: outcome.changed,
            })
            .map_err(Error::from_internal)
    }

    pub async fn list_tasks(&self, workspace_id: &WorkspaceId) -> Result<Vec<TaskRecord>, Error> {
        self.workspace(workspace_id).await?;
        let filters = TaskFilters {
            exclude_epics: true,
            ..TaskFilters::default()
        };
        self.database
            .list_task_items(
                workspace_id,
                filters,
                TaskQueryMode::Flat,
                TaskSort::Created,
                SortDirection::Asc,
            )
            .await
            .map(|items| {
                items
                    .into_iter()
                    .map(|item| TaskRecord::from(item.task))
                    .collect()
            })
            .map_err(Error::from_internal)
    }

    pub async fn fetch_task(
        &self,
        workspace_id: &WorkspaceId,
        task_id: &TaskId,
    ) -> Result<TaskRecord, Error> {
        let workspace = self.workspace(workspace_id).await?;
        let mut connection = self
            .database
            .acquire()
            .await
            .map_err(Error::from_internal)?;
        crate::refs::get_task_in_workspace(&mut connection, &workspace, task_id)
            .await
            .map(TaskRecord::from)
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
        let mut conflicts = Vec::with_capacity(details.len());
        for detail in details {
            let field = TaskField::parse_or_unknown(&detail.field).map_err(Error::from_internal)?;
            let local_value = self
                .database
                .conflict_display_value(workspace_id, field.as_str(), &detail.local_value)
                .await
                .map_err(Error::from_internal)?;
            let remote_value = self
                .database
                .conflict_display_value(workspace_id, field.as_str(), &detail.remote_value)
                .await
                .map_err(Error::from_internal)?;
            conflicts.push(Conflict {
                task_id: task_id.clone(),
                field: ConflictField::from_task_field(field),
                local_value,
                remote_value,
            });
        }
        Ok(conflicts)
    }

    pub async fn resolve_conflict(
        &self,
        workspace_id: &WorkspaceId,
        task_id: &TaskId,
        field: ConflictField,
        choice: ConflictChoice,
    ) -> Result<TaskRecord, Error> {
        let workspace = self.workspace(workspace_id).await?;
        let field_name = field.as_str();
        let choice = match choice {
            ConflictChoice::Local => crate::operations::ConflictValueChoice::Local,
            ConflictChoice::Remote => crate::operations::ConflictValueChoice::Remote,
        };
        let mut connection = self
            .database
            .acquire()
            .await
            .map_err(Error::from_internal)?;
        crate::operations::resolve_conflict_choice(
            &mut connection,
            &workspace,
            task_id,
            field_name,
            choice,
        )
        .await
        .map(|outcome| TaskRecord::from(outcome.task))
        .map_err(Error::from_internal)
    }

    async fn workspace(&self, workspace_id: &WorkspaceId) -> Result<Workspace, Error> {
        self.database
            .list_workspaces()
            .await
            .map_err(Error::from_internal)?
            .into_iter()
            .find(|workspace| workspace.id == *workspace_id)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::NotFound,
                    format!("workspace not found: {workspace_id}"),
                )
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
pub struct CreateTask {
    pub title: String,
    pub description: String,
    pub project: String,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    pub available_at: Option<String>,
    pub due_on: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UpdateTask {
    pub title: Option<String>,
    pub description: Option<String>,
    pub project: Option<String>,
    pub status: Option<TaskStatus>,
    pub priority: Option<TaskPriority>,
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
        }
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictChoice {
    Local,
    Remote,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    Validation,
    NotFound,
    OpenConflict,
    Database,
    Internal,
}

impl ErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Validation => "validation",
            Self::NotFound => "not_found",
            Self::OpenConflict => "open_conflict",
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
        let code = if error.chain().any(|cause| {
            cause
                .downcast_ref::<crate::mutation::OpenConflictError>()
                .is_some()
        }) {
            ErrorCode::OpenConflict
        } else if error.chain().any(|cause| {
            cause
                .downcast_ref::<crate::operations::ConflictNotFoundError>()
                .is_some()
        }) || error
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
        };
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

    #[test]
    fn error_codes_are_stable_and_internal_errors_are_distinct() {
        assert_eq!(ErrorCode::Validation.as_str(), "validation");
        assert_eq!(ErrorCode::NotFound.as_str(), "not_found");
        assert_eq!(ErrorCode::OpenConflict.as_str(), "open_conflict");
        assert_eq!(ErrorCode::Database.as_str(), "database");
        assert_eq!(ErrorCode::Internal.as_str(), "internal");
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
