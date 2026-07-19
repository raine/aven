use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use aven_core::api as core_api;
use aven_core::choices::{TaskPriority as CoreTaskPriority, TaskStatus as CoreTaskStatus};
use aven_core::ids::{TaskId, WorkspaceId};
use aven_core::sync as core_sync;
use tokio::runtime::{Builder, Runtime};

static RUNTIME: OnceLock<Result<Runtime, String>> = OnceLock::new();

fn runtime() -> Result<&'static Runtime, AvenError> {
    match RUNTIME.get_or_init(|| {
        Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|error| error.to_string())
    }) {
        Ok(runtime) => Ok(runtime),
        Err(message) => Err(AvenError::internal(message.clone())),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ErrorCode {
    Validation,
    NotFound,
    OpenConflict,
    Database,
    Internal,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum AvenError {
    #[error("{message}")]
    Failure { code: ErrorCode, message: String },
}

impl AvenError {
    fn validation(message: impl Into<String>) -> Self {
        Self::Failure {
            code: ErrorCode::Validation,
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::Failure {
            code: ErrorCode::Internal,
            message: message.into(),
        }
    }
}

impl From<core_api::Error> for AvenError {
    fn from(error: core_api::Error) -> Self {
        Self::Failure {
            code: match error.code {
                core_api::ErrorCode::Validation => ErrorCode::Validation,
                core_api::ErrorCode::NotFound => ErrorCode::NotFound,
                core_api::ErrorCode::OpenConflict => ErrorCode::OpenConflict,
                core_api::ErrorCode::Database => ErrorCode::Database,
                core_api::ErrorCode::Internal => ErrorCode::Internal,
            },
            message: error.message,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum TaskStatus {
    Inbox,
    Backlog,
    Todo,
    Active,
    Done,
    Canceled,
}

impl From<TaskStatus> for CoreTaskStatus {
    fn from(value: TaskStatus) -> Self {
        match value {
            TaskStatus::Inbox => Self::Inbox,
            TaskStatus::Backlog => Self::Backlog,
            TaskStatus::Todo => Self::Todo,
            TaskStatus::Active => Self::Active,
            TaskStatus::Done => Self::Done,
            TaskStatus::Canceled => Self::Canceled,
        }
    }
}

impl From<CoreTaskStatus> for TaskStatus {
    fn from(value: CoreTaskStatus) -> Self {
        match value {
            CoreTaskStatus::Inbox => Self::Inbox,
            CoreTaskStatus::Backlog => Self::Backlog,
            CoreTaskStatus::Todo => Self::Todo,
            CoreTaskStatus::Active => Self::Active,
            CoreTaskStatus::Done => Self::Done,
            CoreTaskStatus::Canceled => Self::Canceled,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum TaskPriority {
    None,
    Low,
    Medium,
    High,
    Urgent,
}

impl From<TaskPriority> for CoreTaskPriority {
    fn from(value: TaskPriority) -> Self {
        match value {
            TaskPriority::None => Self::None,
            TaskPriority::Low => Self::Low,
            TaskPriority::Medium => Self::Medium,
            TaskPriority::High => Self::High,
            TaskPriority::Urgent => Self::Urgent,
        }
    }
}

impl From<CoreTaskPriority> for TaskPriority {
    fn from(value: CoreTaskPriority) -> Self {
        match value {
            CoreTaskPriority::None => Self::None,
            CoreTaskPriority::Low => Self::Low,
            CoreTaskPriority::Medium => Self::Medium,
            CoreTaskPriority::High => Self::High,
            CoreTaskPriority::Urgent => Self::Urgent,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum OptionalDateUpdate {
    Unchanged,
    Set { value: String },
    Clear,
}

impl From<OptionalDateUpdate> for core_api::OptionalDateUpdate {
    fn from(value: OptionalDateUpdate) -> Self {
        match value {
            OptionalDateUpdate::Unchanged => Self::Unchanged,
            OptionalDateUpdate::Set { value } => Self::Set(value),
            OptionalDateUpdate::Clear => Self::Clear,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct StorageLayout {
    pub root: String,
    pub objects: String,
    pub staging: String,
    pub trash: String,
    pub previews: String,
}

impl From<core_api::StorageLayout> for StorageLayout {
    fn from(value: core_api::StorageLayout) -> Self {
        Self {
            root: value.root.to_string_lossy().into_owned(),
            objects: value.objects.to_string_lossy().into_owned(),
            staging: value.staging.to_string_lossy().into_owned(),
            trash: value.trash.to_string_lossy().into_owned(),
            previews: value.previews.to_string_lossy().into_owned(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct WorkspaceRecord {
    pub id: String,
    pub key: String,
    pub name: String,
}

impl From<core_api::WorkspaceRecord> for WorkspaceRecord {
    fn from(value: core_api::WorkspaceRecord) -> Self {
        Self {
            id: value.id.to_string(),
            key: value.key,
            name: value.name,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct CreateTask {
    pub title: String,
    pub description: String,
    pub project: String,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    pub available_at: Option<String>,
    pub due_on: Option<String>,
}

impl From<CreateTask> for core_api::CreateTask {
    fn from(value: CreateTask) -> Self {
        Self {
            title: value.title,
            description: value.description,
            project: value.project,
            status: value.status.into(),
            priority: value.priority.into(),
            available_at: value.available_at,
            due_on: value.due_on,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct UpdateTask {
    pub title: Option<String>,
    pub description: Option<String>,
    pub project: Option<String>,
    pub status: Option<TaskStatus>,
    pub priority: Option<TaskPriority>,
    pub available_at: OptionalDateUpdate,
    pub due_on: OptionalDateUpdate,
}

impl From<UpdateTask> for core_api::UpdateTask {
    fn from(value: UpdateTask) -> Self {
        Self {
            title: value.title,
            description: value.description,
            project: value.project,
            status: value.status.map(Into::into),
            priority: value.priority.map(Into::into),
            available_at: value.available_at.into(),
            due_on: value.due_on.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TaskRecord {
    pub id: String,
    pub workspace_id: String,
    pub title: String,
    pub description: String,
    pub project_id: String,
    pub project_key: String,
    pub project_prefix: String,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    pub created_at: String,
    pub updated_at: String,
    pub available_at: Option<String>,
    pub due_on: Option<String>,
}

impl From<core_api::TaskRecord> for TaskRecord {
    fn from(value: core_api::TaskRecord) -> Self {
        Self {
            id: value.id.to_string(),
            workspace_id: value.workspace_id.to_string(),
            title: value.title,
            description: value.description,
            project_id: value.project_id.to_string(),
            project_key: value.project_key,
            project_prefix: value.project_prefix,
            status: value.status.into(),
            priority: value.priority.into(),
            created_at: value.created_at,
            updated_at: value.updated_at,
            available_at: value.available_at,
            due_on: value.due_on,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TaskUpdateResult {
    pub task: TaskRecord,
    pub changed: bool,
}

impl From<core_api::TaskUpdateResult> for TaskUpdateResult {
    fn from(value: core_api::TaskUpdateResult) -> Self {
        Self {
            task: value.task.into(),
            changed: value.changed,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
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

impl From<ConflictField> for core_api::ConflictField {
    fn from(value: ConflictField) -> Self {
        match value {
            ConflictField::Title => Self::Title,
            ConflictField::Description => Self::Description,
            ConflictField::Project => Self::Project,
            ConflictField::Status => Self::Status,
            ConflictField::Priority => Self::Priority,
            ConflictField::AvailableAt => Self::AvailableAt,
            ConflictField::DueOn => Self::DueOn,
            ConflictField::Deleted => Self::Deleted,
            ConflictField::IsEpic => Self::IsEpic,
        }
    }
}

impl From<core_api::ConflictField> for ConflictField {
    fn from(value: core_api::ConflictField) -> Self {
        match value {
            core_api::ConflictField::Title => Self::Title,
            core_api::ConflictField::Description => Self::Description,
            core_api::ConflictField::Project => Self::Project,
            core_api::ConflictField::Status => Self::Status,
            core_api::ConflictField::Priority => Self::Priority,
            core_api::ConflictField::AvailableAt => Self::AvailableAt,
            core_api::ConflictField::DueOn => Self::DueOn,
            core_api::ConflictField::Deleted => Self::Deleted,
            core_api::ConflictField::IsEpic => Self::IsEpic,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ConflictSummary {
    pub task_id: String,
    pub task_title: String,
    pub project_key: String,
    pub project_prefix: String,
    pub field: ConflictField,
}

impl From<core_api::ConflictSummary> for ConflictSummary {
    fn from(value: core_api::ConflictSummary) -> Self {
        Self {
            task_id: value.task_id.to_string(),
            task_title: value.task_title,
            project_key: value.project_key,
            project_prefix: value.project_prefix,
            field: value.field.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct Conflict {
    pub task_id: String,
    pub field: ConflictField,
    pub local_value: String,
    pub remote_value: String,
}

impl From<core_api::Conflict> for Conflict {
    fn from(value: core_api::Conflict) -> Self {
        Self {
            task_id: value.task_id.to_string(),
            field: value.field.into(),
            local_value: value.local_value,
            remote_value: value.remote_value,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ConflictChoice {
    Local,
    Remote,
}

impl From<ConflictChoice> for core_api::ConflictChoice {
    fn from(value: ConflictChoice) -> Self {
        match value {
            ConflictChoice::Local => Self::Local,
            ConflictChoice::Remote => Self::Remote,
        }
    }
}

#[derive(uniffi::Object)]
pub struct AvenClient {
    store: core_api::Store,
}

#[uniffi::export]
impl AvenClient {
    #[uniffi::constructor]
    pub fn open(path: String) -> Result<Arc<Self>, AvenError> {
        let store = runtime()?.block_on(core_api::Store::open(path))?;
        Ok(Arc::new(Self { store }))
    }

    pub fn list_workspaces(&self) -> Result<Vec<WorkspaceRecord>, AvenError> {
        runtime()?
            .block_on(self.store.list_workspaces())
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(Into::into)
    }

    pub fn initialize_storage(&self) -> Result<StorageLayout, AvenError> {
        self.store
            .initialize_storage()
            .map(Into::into)
            .map_err(Into::into)
    }

    pub fn resolve_workspace(&self, name_or_key: String) -> Result<WorkspaceRecord, AvenError> {
        runtime()?
            .block_on(self.store.resolve_workspace(&name_or_key))
            .map(Into::into)
            .map_err(Into::into)
    }

    pub fn create_task(
        &self,
        workspace_id: String,
        input: CreateTask,
    ) -> Result<TaskRecord, AvenError> {
        let workspace_id = parse_workspace_id(&workspace_id)?;
        runtime()?
            .block_on(self.store.create_task(&workspace_id, input.into()))
            .map(Into::into)
            .map_err(Into::into)
    }

    pub fn update_task(
        &self,
        workspace_id: String,
        task_id: String,
        input: UpdateTask,
    ) -> Result<TaskUpdateResult, AvenError> {
        let workspace_id = parse_workspace_id(&workspace_id)?;
        let task_id = parse_task_id(&task_id)?;
        runtime()?
            .block_on(
                self.store
                    .update_task(&workspace_id, &task_id, input.into()),
            )
            .map(Into::into)
            .map_err(Into::into)
    }

    pub fn list_tasks(&self, workspace_id: String) -> Result<Vec<TaskRecord>, AvenError> {
        let workspace_id = parse_workspace_id(&workspace_id)?;
        runtime()?
            .block_on(self.store.list_tasks(&workspace_id))
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(Into::into)
    }

    pub fn fetch_task(
        &self,
        workspace_id: String,
        task_id: String,
    ) -> Result<TaskRecord, AvenError> {
        let workspace_id = parse_workspace_id(&workspace_id)?;
        let task_id = parse_task_id(&task_id)?;
        runtime()?
            .block_on(self.store.fetch_task(&workspace_id, &task_id))
            .map(Into::into)
            .map_err(Into::into)
    }

    pub fn list_conflicts(&self, workspace_id: String) -> Result<Vec<ConflictSummary>, AvenError> {
        let workspace_id = parse_workspace_id(&workspace_id)?;
        runtime()?
            .block_on(self.store.list_conflicts(&workspace_id))
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(Into::into)
    }

    pub fn inspect_conflicts(
        &self,
        workspace_id: String,
        task_id: String,
    ) -> Result<Vec<Conflict>, AvenError> {
        let workspace_id = parse_workspace_id(&workspace_id)?;
        let task_id = parse_task_id(&task_id)?;
        runtime()?
            .block_on(self.store.inspect_conflicts(&workspace_id, &task_id))
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(Into::into)
    }

    pub fn resolve_conflict(
        &self,
        workspace_id: String,
        task_id: String,
        field: ConflictField,
        choice: ConflictChoice,
    ) -> Result<TaskRecord, AvenError> {
        let workspace_id = parse_workspace_id(&workspace_id)?;
        let task_id = parse_task_id(&task_id)?;
        runtime()?
            .block_on(self.store.resolve_conflict(
                &workspace_id,
                &task_id,
                field.into(),
                choice.into(),
            ))
            .map(Into::into)
            .map_err(Into::into)
    }

    pub fn start_sync_session(
        &self,
        server: String,
        auth_token: Option<String>,
        page_budget: Option<u64>,
    ) -> Result<Arc<AvenSyncSession>, AvenError> {
        let page_budget = page_budget
            .map(|value| {
                usize::try_from(value)
                    .map_err(|_| AvenError::validation("page budget exceeds the host limit"))
            })
            .transpose()?;
        let session = runtime()?.block_on(self.store.start_sync_session(
            server,
            auth_token,
            page_budget,
        ))?;
        Ok(Arc::new(AvenSyncSession {
            session: Mutex::new(session),
        }))
    }
}

fn parse_workspace_id(value: &str) -> Result<WorkspaceId, AvenError> {
    value
        .parse()
        .map_err(|error| AvenError::validation(format!("invalid workspace id: {error}")))
}

fn parse_task_id(value: &str) -> Result<TaskId, AvenError> {
    value
        .parse()
        .map_err(|error| AvenError::validation(format!("invalid task id: {error}")))
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SyncHttpHeader {
    pub name: String,
    pub value: String,
}

impl From<SyncHttpHeader> for core_sync::SyncHttpHeader {
    fn from(value: SyncHttpHeader) -> Self {
        Self {
            name: value.name,
            value: value.value,
        }
    }
}

impl From<core_sync::SyncHttpHeader> for SyncHttpHeader {
    fn from(value: core_sync::SyncHttpHeader) -> Self {
        Self {
            name: value.name,
            value: value.value,
        }
    }
}

#[derive(uniffi::Object)]
pub struct SyncRequestContext {
    context: core_sync::SyncRequestContext,
}

#[derive(Clone, uniffi::Record)]
pub struct PreparedSyncRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<SyncHttpHeader>,
    pub body: Vec<u8>,
    pub context: Arc<SyncRequestContext>,
}

impl From<core_sync::PreparedSyncRequest> for PreparedSyncRequest {
    fn from(value: core_sync::PreparedSyncRequest) -> Self {
        Self {
            method: value.method,
            url: value.url,
            headers: value.headers.into_iter().map(Into::into).collect(),
            body: value.body,
            context: Arc::new(SyncRequestContext {
                context: value.context,
            }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SyncHttpResponse {
    pub status: u16,
    pub headers: Vec<SyncHttpHeader>,
    pub body: Vec<u8>,
}

impl From<SyncHttpResponse> for core_sync::SyncHttpResponse {
    fn from(value: SyncHttpResponse) -> Self {
        Self {
            status: value.status,
            headers: value.headers.into_iter().map(Into::into).collect(),
            body: value.body,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SyncPageOutcome {
    pub page: u64,
    pub pushed: u64,
    pub pulled: u64,
    pub cursor: i64,
    pub complete: bool,
    pub has_more: bool,
    pub local_more: bool,
    pub request_bytes: u64,
    pub request_wire_bytes: u64,
    pub response_decoded_bytes: u64,
    pub response_compression: String,
    pub apply_ms: u64,
}

impl TryFrom<core_sync::SyncPageOutcome> for SyncPageOutcome {
    type Error = AvenError;

    fn try_from(value: core_sync::SyncPageOutcome) -> Result<Self, Self::Error> {
        Ok(Self {
            page: to_u64(value.page, "page")?,
            pushed: to_u64(value.pushed, "pushed")?,
            pulled: to_u64(value.pulled, "pulled")?,
            cursor: value.cursor,
            complete: value.complete,
            has_more: value.has_more,
            local_more: value.local_more,
            request_bytes: to_u64(value.request_bytes, "request bytes")?,
            request_wire_bytes: to_u64(value.request_wire_bytes, "request wire bytes")?,
            response_decoded_bytes: to_u64(value.response_decoded_bytes, "response decoded bytes")?,
            response_compression: value.response_compression,
            apply_ms: u64::try_from(value.apply_ms)
                .map_err(|_| AvenError::internal("apply duration exceeds the facade limit"))?,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SyncSessionSummary {
    pub pushed: i64,
    pub pulled: u64,
    pub cursor: i64,
    pub complete: bool,
    pub pages: u64,
    pub request_bytes: u64,
    pub request_wire_bytes: u64,
    pub response_decoded_bytes: u64,
    pub response_compression: String,
    pub apply_ms: u64,
}

impl TryFrom<core_sync::SyncSessionSummary> for SyncSessionSummary {
    type Error = AvenError;

    fn try_from(value: core_sync::SyncSessionSummary) -> Result<Self, Self::Error> {
        Ok(Self {
            pushed: value.pushed,
            pulled: to_u64(value.pulled, "pulled")?,
            cursor: value.cursor,
            complete: value.complete,
            pages: to_u64(value.pages, "pages")?,
            request_bytes: to_u64(value.request_bytes, "request bytes")?,
            request_wire_bytes: to_u64(value.request_wire_bytes, "request wire bytes")?,
            response_decoded_bytes: to_u64(value.response_decoded_bytes, "response decoded bytes")?,
            response_compression: value.response_compression,
            apply_ms: u64::try_from(value.apply_ms)
                .map_err(|_| AvenError::internal("apply duration exceeds the facade limit"))?,
        })
    }
}

fn to_u64(value: usize, name: &str) -> Result<u64, AvenError> {
    u64::try_from(value)
        .map_err(|_| AvenError::internal(format!("{name} exceeds the facade limit")))
}

#[derive(uniffi::Object)]
pub struct AvenSyncSession {
    session: Mutex<core_sync::SyncSession>,
}

#[uniffi::export]
impl AvenSyncSession {
    pub fn prepare_request(&self) -> Result<Option<PreparedSyncRequest>, AvenError> {
        let mut session = self.lock()?;
        runtime()?
            .block_on(session.prepare_request())
            .map(|request| request.map(Into::into))
            .map_err(|error| AvenError::internal(error.to_string()))
    }

    pub fn accept_response(
        &self,
        context: Arc<SyncRequestContext>,
        response: SyncHttpResponse,
    ) -> Result<SyncPageOutcome, AvenError> {
        let mut session = self.lock()?;
        let outcome = runtime()?
            .block_on(session.accept_response(&context.context, response.into()))
            .map_err(|error| AvenError::internal(error.to_string()))?;
        outcome.try_into()
    }

    pub fn fail_request(
        &self,
        context: Arc<SyncRequestContext>,
        message: String,
    ) -> Result<(), AvenError> {
        let mut session = self.lock()?;
        runtime()?
            .block_on(session.fail_request(&context.context, message))
            .map_err(|error| AvenError::internal(error.to_string()))
    }

    pub fn summary(&self) -> Result<SyncSessionSummary, AvenError> {
        self.lock()?.summary().try_into()
    }
}

impl AvenSyncSession {
    fn lock(&self) -> Result<MutexGuard<'_, core_sync::SyncSession>, AvenError> {
        self.session
            .lock()
            .map_err(|_| AvenError::internal("sync session lock is poisoned"))
    }
}

uniffi::setup_scaffolding!();
