use crate::launch;
use crate::launch::{Catalog, LaunchEnv, LaunchEvent, LaunchJob, LaunchMode, Place};
use crate::session::Session;
use crate::sync_ops;
use crate::sync_ops::{SyncOp, SyncRequest, SyncWork};
use crate::toast::{Subject, Toast, ToastAction};
use anyhow::{Context as _, Result, anyhow};
use aven_core::attachments::{ImageOptimizationPolicy, object_path};
use aven_core::choices::{TaskSource, TaskStatus};
use aven_core::db::Database;
use aven_core::ids::{TaskId, WorkspaceId, new_id};
use aven_core::local_state::OnboardingStatus;
use aven_core::metadata::TaskMetadataInput;
use aven_core::operations::{
    AttachmentAddInput, RecurrenceTemplateUpdate, TaskAttachmentAddInput, TaskDraft,
};
use aven_core::projects::{normalize_key, prefix_base};
use aven_core::query::{
    RecurrenceHistoryKind, RecurrenceSeriesLifecycleFilter, SortDirection, TaskAvailabilityFilter,
    TaskFilters, TaskListItem, TaskQueryMode, TaskSearchQuery, TaskSort,
};
use aven_core::queue::{
    STALE_ACTIVE_DAYS, STALE_TODO_DAYS, TriageReason, now_seconds, triage_reason,
    triage_review_current,
};
use aven_core::recurrence::{RecurrenceDuePolicy, RecurrenceProjectionState, RecurrenceSeriesId};
use aven_core::sync::client::engine::{LocalPhase, local_phase};
use aven_core::sync::client::keys::ProtectedStorage;
use aven_host::attachments::preview::{cached_preview_path, load_preview_png};
use aven_host::config::{AppConfig, RemindersConfig, config_file_path, resolve_blob_dir};
use aven_host::config_edit;
use aven_host::schedule_input::{ParsedScheduleInput, parse_schedule_input};
use aven_host::store::task_selection::TaskSelection;
use aven_host::store::{
    ClosedTaskVisibility, EpicContext, PendingTaskAttachment, PriorityMutation, SelectionRestore,
    TaskDateField, TaskLayout, TaskOrder, TaskProjectionOrigin, TaskQuery, TaskScope,
    TaskScopeTarget, TaskTextField, TaskViewState, TuiStore, mutation_committed, recurrence_draft,
    task_creation_committed,
};
use aven_host::sync::encrypted::{DaemonRound, DaemonSync};
use aven_host::sync::error_explanations::ErrorAction;
use aven_host::sync::scheduler::{ROUND_BUDGET, SyncScheduler};
use aven_host::time_input::{parse_available_at_input, parse_due_on_input};
use chrono::Local;
use sqlx::ConnectOptions as _;
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::ffi::OsStr;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::{Notify, Semaphore, mpsc};
use tokio::task::JoinHandle;
use tokio::time::{
    Instant, MissedTickBehavior, interval, interval_at, sleep, sleep_until, timeout,
};

use super::model::*;
use super::prefs::*;
use super::rows::*;

const SYNC_BUSY: &str = "Sync is busy. Try again when it finishes";
const JOIN_PAUSE: &str = "Joining sync: local changes wait until synced tasks are downloaded";
/// How often outside writes are looked for.
const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// Dates and availability change with the clock, with no write to notice.
const CLOCK_INTERVAL: Duration = Duration::from_secs(60);
/// Coalesces a burst of local changes into one sync round.
const SYNC_DEBOUNCE: Duration = Duration::from_millis(700);
/// How often a quit looks at user-started sync work.
const QUIT_POLL: Duration = Duration::from_millis(50);
/// How long a quit waits for a sync round to end before leaving it unfinished,
/// as a killed daemon would.
const QUIT_SYNC_BUDGET: Duration = Duration::from_secs(3);

pub(crate) fn failed(title: &str, error: &impl std::fmt::Display) -> Toast {
    Toast::error(title).detail(capitalised(&error.to_string()))
}

/// A message from the shared store, which writes it for the terminal.
fn said(message: &str) -> Toast {
    Toast::done(capitalised(message))
}

fn join_paused() -> Toast {
    Toast::info("Joining sync").detail("Local changes wait until synced tasks are downloaded")
}

pub(crate) fn attachment_input(image: &PendingImage, alt: String) -> AttachmentAddInput {
    AttachmentAddInput {
        filename: Some(image.name.clone()),
        alt_text: Some(alt).filter(|alt| !alt.is_empty()),
        declared_media_type: None,
        bytes: image.bytes.to_vec(),
        optimization_policy: ImageOptimizationPolicy::Optimize,
        dedupe_existing: false,
    }
}

/// The composer fields an agent's task fills.
pub(crate) fn intake_task(result: aven_host::task_intake::TaskIntakeResult) -> NewTask {
    let task = result.task;
    let schedule = crate::draft::schedule_text(
        task.available_at.as_deref().unwrap_or_default(),
        task.due_on.as_deref().unwrap_or_default(),
        result.recurrence.as_ref(),
    );
    NewTask {
        title: task.title,
        description: task.description,
        project: task.project,
        status: task.status,
        priority: task.priority,
        labels: task.labels,
        metadata: task
            .metadata
            .into_iter()
            .map(|field| (field.key, field.value))
            .collect(),
        is_epic: task.is_epic,
        schedule,
        images: Vec::new(),
        replaces: None,
        first_project: None,
    }
}

fn metadata_inputs(fields: &[(String, String)]) -> Vec<TaskMetadataInput> {
    fields
        .iter()
        .map(|(key, value)| TaskMetadataInput {
            expected_field_id: None,
            key: key.clone(),
            value: value.clone(),
        })
        .collect()
}

/// The name the first project takes: the workspace's, unless it still has the
/// name every database starts with.
pub fn first_project_name(workspace: &str) -> String {
    match normalize_key(workspace).as_str() {
        "" | "default" => "Personal".to_string(),
        _ => workspace.to_string(),
    }
}

/// The prefix core gives a project with this name in a workspace with no
/// other projects.
pub fn first_project_prefix(name: &str) -> String {
    prefix_base(&normalize_key(name))
}

/// Whether the welcome should show. The marker is shared with the terminal
/// app, and a database that already has tasks gets it without a welcome.
async fn welcome_due(database: &Database) -> bool {
    let status = match database.onboarding_status().await {
        Ok(status) => status,
        Err(error) => {
            tracing::warn!(%error, "could not read the welcome state");
            return false;
        }
    };
    match status {
        OnboardingStatus::Due => true,
        OnboardingStatus::Complete => false,
        OnboardingStatus::Established => {
            if let Err(error) = database.complete_onboarding().await {
                tracing::warn!(%error, "could not record the welcome as done");
            }
            false
        }
    }
}

/// Opens the store a launch resolves to: the queue's selected workspace on the
/// default view.
pub async fn open_store(path: &Path, config: &AppConfig) -> Result<TuiStore> {
    open_store_in_view(path, config, None).await
}

fn initial_view(saved: Option<&SavedView>) -> TaskViewState {
    let mut state = TaskViewState::default();
    if let Some(saved) = saved {
        if let Some((query, ..)) = crate::actions::VIEWS
            .iter()
            .find(|view| view.1 == saved.view)
        {
            state.set_query(*query);
            state.reset_projection_origin();
            if saved.columns && supports_columns(*query) {
                state.layout = TaskLayout::Columns;
            }
        }
        state.scope = saved
            .project
            .clone()
            .map_or(TaskScope::Workspace, TaskScope::Project);
    }
    state
}

async fn open_store_in_view(
    path: &Path,
    config: &AppConfig,
    saved: Option<&SavedView>,
) -> Result<TuiStore> {
    crate::startup::mark("database-start");
    let database = Database::open(path).await?;
    crate::startup::mark("database-ready");
    let workspace = database.restore_queue_workspace().await?;
    let mut store = TuiStore::new_with_view_state_and_config(
        database,
        workspace,
        initial_view(saved),
        config.clone(),
    )
    .await?;
    crate::startup::mark("projection-ready");
    // Core has no desktop source, and this one makes no claim.
    store.task_source = TaskSource::Unknown;
    Ok(store)
}

async fn catalog(database: &Database, workspace: &WorkspaceId) -> Result<Vec<CatalogTask>> {
    let items = database
        .list_task_summary_items_from_current_projection(
            workspace,
            TaskFilters::default(),
            TaskQueryMode::Flat,
            TaskSort::Updated,
            SortDirection::Desc,
            None,
        )
        .await?;
    Ok(items
        .iter()
        .map(|item| CatalogTask {
            id: item.task.id.clone(),
            reference: item.display_ref.clone(),
            project: item
                .display_ref
                .split('-')
                .next()
                .unwrap_or_default()
                .to_string(),
            project_key: item.task.project_key.clone(),
            title: item.task.title.clone(),
            status: item.task.status,
            is_epic: item.task.is_epic,
        })
        .collect())
}

/// `PRAGMA data_version` on its own connection, which sees every commit made by
/// any other connection, in this process or another.
struct OutsideWrites {
    connection: SqliteConnection,
    seen: i64,
}

impl OutsideWrites {
    async fn open(path: &Path) -> Result<Self> {
        let connection = SqliteConnectOptions::new()
            .filename(path)
            .read_only(true)
            .connect()
            .await?;
        let mut this = Self {
            connection,
            seen: 0,
        };
        this.seen = this.read().await?;
        Ok(this)
    }

    async fn read(&mut self) -> Result<i64> {
        Ok(sqlx::query_scalar("PRAGMA data_version")
            .fetch_one(&mut self.connection)
            .await?)
    }

    async fn changed(&mut self) -> Result<bool> {
        let version = self.read().await?;
        Ok(std::mem::replace(&mut self.seen, version) != version)
    }
}

/// `gh gist create`, fed the report on standard input.
pub fn create_secret_gist(
    program: &OsStr,
    markdown: &str,
    filename: &str,
    description: &str,
) -> Result<String> {
    let mut child = ProcessCommand::new(program)
        .args([
            "gist",
            "create",
            "--filename",
            filename,
            "--desc",
            description,
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("could not run gh; install and authenticate GitHub CLI")?;
    child
        .stdin
        .take()
        .context("gh standard input is unavailable")?
        .write_all(markdown.as_bytes())?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if message.is_empty() {
            return Err(anyhow!("gh gist create exited with {}", output.status));
        }
        return Err(anyhow!(message));
    }
    let url = String::from_utf8(output.stdout)
        .context("gh gist create returned a non-UTF-8 URL")?
        .trim()
        .to_string();
    if url.is_empty() {
        return Err(anyhow!("gh gist create returned no URL"));
    }
    Ok(url)
}

/// How many saving launcher jobs run their agent at once. More wait.
const LAUNCH_TURNS: usize = 3;

struct Actor {
    store: TuiStore,
    database: PathBuf,
    config: AppConfig,
    replies: mpsc::UnboundedSender<Reply>,
    sync_wake: Arc<Notify>,
    sync_stop: Arc<Notify>,
    storage: Option<Arc<dyn ProtectedStorage>>,
    /// The launch database's config, which other databases start from.
    launch_config: AppConfig,
    launch_path: PathBuf,
    background_sync: Option<JoinHandle<()>>,
    /// Set while user-started work holds the sync slot.
    sync_busy: Arc<AtomicBool>,
    invitation: Option<JoinHandle<()>>,
    handled: u64,
    /// The task the view has selected, which refreshes keep in the list.
    selected: Option<TaskId>,
    arrange: Arrange,
    /// Views last shown as columns.
    column_views: Vec<TaskQuery>,
    /// What each write in the undo history said, newest last.
    undoable: Vec<Toast>,
    /// Review marks a keep replaced, for taking the keep back.
    kept: HashMap<TaskId, Option<i64>>,
    /// Turns for launcher jobs that save, so few agents run at once.
    launch_turns: Arc<Semaphore>,
    /// The launcher's review in flight, with its id.
    launch_review: Option<(u64, JoinHandle<()>)>,
    /// Whether the open database still waits for the welcome.
    welcome: bool,
}

/// What a write reports once the list has caught up.
#[derive(Default)]
struct Outcome {
    created: Option<TaskId>,
    message: Option<Toast>,
    go: Option<Go>,
    undo: bool,
    /// A replaced series to ask about stopping.
    stop: Option<RecurrenceSeriesId>,
}

impl Outcome {
    fn saved(message: Toast) -> Self {
        Self {
            message: Some(message),
            undo: true,
            ..Self::default()
        }
    }

    fn with_go(self, label: &'static str, command: Command) -> Self {
        Self {
            go: Some(Go { label, command }),
            ..self
        }
    }

    /// A write core keeps no undo entry for.
    fn final_write(self) -> Self {
        Self {
            undo: false,
            ..self
        }
    }

    /// A change the undo history takes, with nothing to announce.
    fn quiet() -> Self {
        Self {
            undo: true,
            ..Self::default()
        }
    }
}

impl Actor {
    fn new(
        store: TuiStore,
        database: PathBuf,
        config: AppConfig,
        replies: mpsc::UnboundedSender<Reply>,
        sync_wake: Arc<Notify>,
        storage: Option<Arc<dyn ProtectedStorage>>,
    ) -> Self {
        Self {
            store,
            launch_path: database.clone(),
            database,
            launch_config: config.clone(),
            config,
            replies,
            sync_wake,
            sync_stop: Arc::new(Notify::new()),
            storage,
            background_sync: None,
            sync_busy: Arc::new(AtomicBool::new(false)),
            invitation: None,
            handled: 0,
            selected: None,
            arrange: Arrange::default(),
            column_views: Vec::new(),
            undoable: Vec::new(),
            kept: HashMap::new(),
            launch_turns: Arc::new(Semaphore::new(LAUNCH_TURNS)),
            launch_review: None,
            welcome: false,
        }
    }

    fn send(&self, body: ReplyBody) {
        let _ = self.replies.send(Reply {
            request: self.handled,
            body,
        });
    }

    fn spawn_background_sync(&mut self) {
        if self.config.sync_is_allowed() {
            crate::startup::mark("background-sync-start");
            self.background_sync = Some(tokio::spawn(run_sync(
                self.store.database(),
                self.config.clone(),
                self.storage.clone(),
                self.sync_wake.clone(),
                self.sync_stop.clone(),
                self.replies.clone(),
            )));
        }
    }

    fn sync_work(&self) -> SyncWork {
        SyncWork {
            database: self.store.database(),
            path: self.database.clone(),
            config: self.config.clone(),
            storage: self.storage.clone(),
            replies: self.replies.clone(),
            request: self.handled,
        }
    }

    async fn sync_request(&mut self, request: SyncRequest) {
        let work = self.sync_work();
        match request {
            SyncRequest::Run(op) => self.run_sync_op(work, op),
            SyncRequest::Preview => {
                tokio::spawn(async move { work.preview().await });
            }
            SyncRequest::CheckJoin => {
                tokio::spawn(async move { work.check_join().await });
            }
            SyncRequest::LoadDevices => {
                tokio::spawn(async move { work.load_devices().await });
            }
            SyncRequest::Invite => {
                if !self
                    .invitation
                    .as_ref()
                    .is_some_and(|task| !task.is_finished())
                {
                    self.invitation = Some(tokio::spawn(async move { work.invite().await }));
                }
            }
            SyncRequest::CancelInvitation => {
                if work.cancel_invitation().await
                    && let Some(task) = self.invitation.take()
                {
                    task.abort();
                }
            }
        }
    }

    fn run_sync_op(&mut self, work: SyncWork, op: SyncOp) {
        if self.sync_busy.swap(true, Ordering::SeqCst) {
            self.send(ReplyBody::Notice(Toast::info(SYNC_BUSY)));
            return;
        }
        let busy = self.sync_busy.clone();
        tokio::spawn(async move {
            let finished = work.run(op).await;
            busy.store(false, Ordering::SeqCst);
            work.send(finished);
        });
    }

    async fn settings_data(&self) -> Result<SettingsData> {
        let database = self.store.database();
        Ok(SettingsData {
            workspaces: database
                .list_workspaces()
                .await?
                .into_iter()
                .map(|workspace| WorkspaceEntry {
                    key: workspace.key,
                    name: workspace.name,
                })
                .collect(),
            projects: self
                .store
                .projects
                .iter()
                .map(|project| ProjectEntry {
                    key: project.key.clone(),
                    name: project.name.clone(),
                    prefix: project.prefix.clone(),
                })
                .collect(),
            labels: self
                .store
                .label_usage()
                .await?
                .into_iter()
                .map(|usage| (usage.name, usage.task_count))
                .collect(),
            fields: database
                .list_metadata_fields(&self.store.active_workspace.id)
                .await?
                .into_iter()
                .map(|usage| (usage.field.key, usage.task_count))
                .collect(),
            commands: self
                .config
                .tui
                .commands
                .iter()
                .map(|command| (command.name.clone(), command.description.clone()))
                .collect(),
            reminders: (&self.config.reminders).into(),
            agent: launch::has_agent(&self.config.agent.task_intake),
        })
    }

    async fn send_settings(&self) {
        match self.settings_data().await {
            Ok(data) => self.send(ReplyBody::Settings(Box::new(data))),
            Err(error) => self.send(ReplyBody::Notice(failed("Could not load settings", &error))),
        }
    }

    async fn edit_settings(&mut self, edit: SettingsEdit) -> Result<String> {
        let database = self.store.database();
        Ok(match edit {
            SettingsEdit::NewWorkspace(name) => {
                let workspace = database.create_workspace(&name).await?;
                format!("Created workspace {}", workspace.name)
            }
            SettingsEdit::RenameWorkspace(key, name) => {
                let workspace = database.rename_workspace(&key, &name).await?;
                format!("Renamed workspace to {}", workspace.name)
            }
            SettingsEdit::NewProject { name, prefix } => {
                let prefix = (!prefix.is_empty()).then_some(prefix.as_str());
                let outcome = (self.store.database())
                    .create_project(&self.store.active_workspace, &name, prefix)
                    .await?;
                if !outcome.created {
                    return Err(anyhow!("Project {name} already exists"));
                }
                self.sync_wake.notify_one();
                self.store.refresh(None).await?;
                format!("Created project {name}")
            }
            SettingsEdit::EditProject { key, name, prefix } => {
                self.store.rename_project(&key, &name, &prefix).await?;
                format!("Saved project {name}")
            }
            SettingsEdit::DeleteProject(key) => {
                let result = self.store.delete_project(&key).await?;
                capitalised(&result.message)
            }
            SettingsEdit::NewLabel(name) => {
                self.store.create_label(name).await?;
                "Label created".to_string()
            }
            SettingsEdit::RenameLabel(old, new) => {
                self.store.rename_label(&old, new.clone()).await?;
                format!("Renamed label to {new}")
            }
            SettingsEdit::RenameField(old, new) => {
                database
                    .rename_metadata_field(&self.store.active_workspace, &old, &new)
                    .await?;
                format!("Renamed field to {new}")
            }
            SettingsEdit::Reminders(reminders) => {
                let config = RemindersConfig {
                    enabled: reminders.enabled,
                    digest_time: reminders.digest_time.clone(),
                    due_digest: reminders.due_digest,
                    available: reminders.available,
                };
                config.digest_time()?;
                let path = config_file_path()?;
                let current = &self.config.reminders;
                let bool_text = |value: bool| if value { "true" } else { "false" };
                for (key, old, new) in [
                    ("enabled", current.enabled, reminders.enabled),
                    ("due_digest", current.due_digest, reminders.due_digest),
                    ("available", current.available, reminders.available),
                ] {
                    if old != new {
                        config_edit::set_scalar(&path, "reminders", key, bool_text(new))?;
                    }
                }
                if current.digest_time != reminders.digest_time {
                    let quoted = format!("\"{}\"", reminders.digest_time);
                    config_edit::set_scalar(&path, "reminders", "digest_time", &quoted)?;
                }
                self.config.reminders = config.clone();
                self.launch_config.reminders = config;
                "Reminders saved".to_string()
            }
        })
    }

    /// Replaces the open database. The old one stays open if the new one fails.
    async fn switch_database(&mut self, path: PathBuf) -> Result<()> {
        if self.sync_busy.load(Ordering::SeqCst) {
            return Err(anyhow!(SYNC_BUSY));
        }
        let config = sync_ops::config_for(&path, &self.launch_path, &self.launch_config);
        let store = open_store(&path, &config).await?;
        if let Some(task) = self.background_sync.take() {
            task.abort();
        }
        if let Some(task) = self.invitation.take() {
            task.abort();
        }
        let previous = (self.database.clone(), self.store.tasks.len());
        self.store.database().close().await;
        self.store = store;
        self.database = path.clone();
        self.config = config;
        self.selected = None;
        self.arrange = Arrange::default();
        self.undoable.clear();
        self.kept.clear();
        self.welcome = welcome_due(&self.store.database()).await;
        self.spawn_background_sync();
        self.send(ReplyBody::DatabaseOpened {
            path,
            tasks: self.store.tasks.len(),
            previous: Some(previous),
        });
        Ok(())
    }

    async fn open_database(&mut self, path: PathBuf) -> bool {
        let opened = match sync_ops::check_database_path(&path) {
            Ok(()) => self.switch_database(path.clone()).await,
            Err(error) => Err(error),
        };
        match opened {
            Ok(()) => {
                self.outside_change().await;
                self.send_settings().await;
                true
            }
            Err(error) => {
                self.send(ReplyBody::DatabaseFailed(path, error.to_string()));
                false
            }
        }
    }

    async fn restore(&mut self, archive: PathBuf) {
        if self.sync_busy.load(Ordering::SeqCst) {
            self.send(ReplyBody::Notice(Toast::info(SYNC_BUSY)));
            return;
        }
        match sync_ops::restore_to_new(&self.database, &self.launch_config, &archive).await {
            Ok(path) => {
                if self.open_database(path).await {
                    self.send(ReplyBody::Notice(Toast::done("Backup restored")));
                }
            }
            Err(error) => self.send(ReplyBody::Notice(failed("Could not restore", &error))),
        }
    }

    async fn backup(&self) -> Result<PathBuf> {
        let path =
            sync_ops::write_backup(&self.store.database(), &self.database, &self.blob_dir()?)
                .await?;
        Ok(path)
    }

    async fn export(&self) -> Result<PathBuf> {
        let path = sync_ops::write_export(&self.store.database(), &self.database).await?;
        Ok(path)
    }

    /// Edits made before joined tasks are installed would make the engine
    /// refuse the installation, so joining pauses them.
    async fn paused_by_join(&self) -> bool {
        matches!(
            local_phase(&self.store.database()).await,
            Ok(LocalPhase::JoinIncomplete)
        )
    }

    async fn metadata_matches(&self, filter: &MetaFilter) -> Result<HashSet<TaskId>> {
        let keys = vec![filter.key.clone()];
        let filters = TaskFilters {
            has_metadata: if filter.present {
                keys.clone()
            } else {
                Vec::new()
            },
            missing_metadata: if filter.present { Vec::new() } else { keys },
            ..self.store.view_state.filters()
        };
        let items = self
            .store
            .database()
            .list_task_summary_items_from_current_projection(
                &self.store.active_workspace.id,
                filters,
                TaskQueryMode::Flat,
                TaskSort::Updated,
                SortDirection::Desc,
                None,
            )
            .await?;
        Ok(items.into_iter().map(|item| item.task.id).collect())
    }

    /// Reads off the actor, because the read covers every task and commands
    /// should not queue behind it.
    fn send_catalog(&self) {
        let database = self.store.database();
        let workspace = self.store.active_workspace.id.clone();
        let replies = self.replies.clone();
        let request = self.handled;
        tokio::spawn(async move {
            if let Ok(catalog) = catalog(&database, &workspace).await {
                let body = ReplyBody::Catalog(catalog);
                let _ = replies.send(Reply { request, body });
            }
        });
    }

    async fn send_list(&mut self) {
        self.arrange.allowed = match self.arrange.metadata.clone() {
            Some(filter) => self.metadata_matches(&filter).await.ok(),
            None => None,
        };
        let mut list = arranged_snapshot(&self.store, Local::now(), &self.arrange);
        // Tasks from anywhere, such as a join or the CLI, end the welcome.
        if self.welcome {
            self.welcome = welcome_due(&self.store.database()).await;
        }
        list.welcome = self.welcome;
        if let Ok(fields) = self.store.metadata_fields().await {
            list.fields = fields.into_iter().map(|field| field.key).collect();
        }
        self.send(ReplyBody::List(Box::new(list)));
        self.send_catalog();
    }

    /// A desktop launch has no directory to infer a project from, so a new task
    /// joins the scoped project or the first one.
    fn project_for_new_task(&self) -> Option<String> {
        if let TaskScope::Project(project) = &self.store.view_state.scope {
            return Some(project.clone());
        }
        self.store
            .projects
            .first()
            .map(|project| project.key.clone())
    }

    /// A workspace with no projects gets one for its first task. Inference from
    /// the working directory is skipped: a desktop launch has none worth using,
    /// and the welcome has already shown the name.
    async fn create_first_project(&mut self, name: Option<&str>) -> Result<String> {
        let name = name.map_or_else(
            || first_project_name(&self.store.active_workspace.name),
            str::to_string,
        );
        let outcome = (self.store.database())
            .create_project(&self.store.active_workspace, &name, None)
            .await?;
        self.store.refresh(None).await?;
        Ok(outcome.project.key)
    }

    /// Refreshes the projection. Hiding the selected task is left for the next
    /// refresh after selection moves on, so a finished row stays under the cursor.
    async fn refresh(&mut self) -> Result<()> {
        let selected = self.selected.clone();
        let at = selected
            .as_ref()
            .and_then(|id| self.store.tasks.iter().position(|item| item.task.id == *id));
        self.store.refresh(selected.as_ref()).await?;
        let (Some(id), Some(at)) = (selected, at) else {
            return Ok(());
        };
        if self.store.tasks.iter().all(|item| item.task.id != id)
            && let Some(item) = self.store.load_task_item(&id).await?
            && !item.task.deleted
        {
            self.store.insert_task(at.min(self.store.tasks.len()), item);
        }
        Ok(())
    }

    /// Resolves a task, bringing back one the view keeps but the store dropped.
    async fn selection(&mut self, id: &TaskId) -> Result<TaskSelection> {
        if self.store.tasks.iter().all(|item| &item.task.id != id)
            && let Some(item) = self.store.load_task_item(id).await?
        {
            self.store.insert_task(self.store.tasks.len(), item);
        }
        let index = self.store.tasks.iter().position(|item| &item.task.id == id);
        index
            .and_then(|index| {
                TaskSelection::resolve(&self.store.tasks, &BTreeSet::new(), Some(index))
            })
            .ok_or_else(|| anyhow!("That task is no longer in this view"))
    }

    fn subject(&self, id: &TaskId) -> Option<Subject> {
        let item = self.store.tasks.iter().find(|item| item.task.id == *id)?;
        Some(Subject {
            reference: item.display_ref.clone(),
            project_key: item.task.project_key.clone(),
        })
    }

    fn blob_dir(&self) -> Result<PathBuf> {
        resolve_blob_dir(&self.database, &self.config)
    }

    /// Images with a local preview built on demand, as the TUI does.
    async fn images(&self, item: &TaskListItem) -> Vec<Image> {
        let Ok(blob_dir) = self.blob_dir() else {
            return Vec::new();
        };
        let quota = self.config.local.attachment_lifecycle.preview_quota_bytes;
        let mut images = Vec::new();
        for attachment in item.attachments.iter().filter(|image| !image.deleted) {
            let built = load_preview_png(&blob_dir, &attachment.sha256, quota).await;
            images.push(Image {
                id: attachment.attachment_id.clone(),
                name: attachment.filename.clone().unwrap_or_default(),
                alt: attachment.alt_text.clone().unwrap_or_default(),
                preview: built
                    .ok()
                    .and_then(|_| cached_preview_path(&blob_dir, &attachment.sha256).ok()),
            });
        }
        images
    }

    /// An epic's children with what `kidRow` says about each. A child that
    /// cannot be read stays out of the section until the next load.
    async fn children(&self, item: &TaskListItem) -> Vec<Child> {
        let today = Local::now().date_naive();
        let mut children = Vec::new();
        for link in &item.epic_children {
            let Ok(Some(child)) = self.store.load_task_item(&link.task_id).await else {
                continue;
            };
            let blocker = child
                .depends_on
                .iter()
                .find(|blocker| blocker.unresolved)
                .map(|blocker| blocker.display_ref.clone());
            let hidden = child
                .task
                .available_at
                .as_deref()
                .and_then(parse_local_timestamp)
                .filter(|at| *at > Local::now());
            let note = match (&hidden, &blocker) {
                (Some(at), _) => {
                    format!("Hidden until {}", available_text(&at.to_rfc3339(), today))
                }
                (None, Some(_)) if child.task.status == TaskStatus::Active => "Blocked".to_string(),
                (None, Some(blocker)) => format!("Waits on {blocker}"),
                (None, None) => String::new(),
            };
            let waiting = hidden.is_some() || blocker.is_some();
            children.push(Child {
                link: dependency(link),
                group: child_group(child.task.status, waiting),
                note,
                due: child
                    .task
                    .due_on
                    .as_deref()
                    .map(|due| due_text(due, today))
                    .unwrap_or_default(),
            });
        }
        children
    }

    async fn send_series(&mut self, id: &RecurrenceSeriesId) {
        match self.series_detail(id).await {
            Ok(detail) => self.send(ReplyBody::Series(Box::new(detail))),
            Err(error) => self.send(ReplyBody::RefreshFailed(error.to_string())),
        }
    }

    async fn series_detail(&self, id: &RecurrenceSeriesId) -> Result<SeriesDetail> {
        const HISTORY: usize = 20;
        let detail = self.store.recurrence_detail_for_series(id).await?;
        let history = self
            .store
            .recurrence_history_for_series(id, chrono::Utc::now(), 0, HISTORY)
            .await?;
        let series = &detail.series;
        let current = detail.current_occurrence.as_ref();
        let slot = detail.summary.current_slot_on.clone();
        let reference = detail.summary.current_task_ref.clone();
        Ok(SeriesDetail {
            id: id.clone(),
            reference: detail.summary.series_ref.clone(),
            title: series.title.clone(),
            description: series.description.clone(),
            state: series.state,
            rule: detail.summary.rule_label.clone(),
            schedule: series_schedule(series),
            due: match series.due_policy {
                RecurrenceDuePolicy::SameDay => "Same day".to_string(),
                RecurrenceDuePolicy::None => "None".to_string(),
            },
            timezone: series.timezone.to_string(),
            after: slot.as_deref().and_then(|slot| slot_after(series, slot)),
            current: current
                .and_then(|occurrence| occurrence.task_id.clone())
                .zip(slot.zip(reference))
                .map(|(id, (slot, reference))| Occurrence {
                    id,
                    reference,
                    slot,
                }),
            // Matches the TUI's availability rule for skipping.
            skippable: current.is_some_and(|occurrence| {
                occurrence.outcome.is_none()
                    && occurrence.projection_state == RecurrenceProjectionState::Projected
            }),
            history: history
                .items
                .iter()
                .map(|entry| {
                    let kind = match entry.kind {
                        RecurrenceHistoryKind::Completed => "completed",
                        RecurrenceHistoryKind::Skipped => "skipped",
                        RecurrenceHistoryKind::Missed => "missed",
                        RecurrenceHistoryKind::Paused => "paused",
                    };
                    (entry.slot_on.clone().unwrap_or_default(), kind.to_string())
                })
                .collect(),
        })
    }

    /// Resolves exactly `ids`, refusing if any of them is gone, so a write can
    /// never land on tasks that took their place.
    async fn marked(&mut self, ids: &[TaskId]) -> Result<TaskSelection> {
        for id in ids {
            if self.store.tasks.iter().all(|item| &item.task.id != id)
                && let Some(item) = self.store.load_task_item(id).await?
            {
                self.store.insert_task(self.store.tasks.len(), item);
            }
        }
        let marks: BTreeSet<TaskId> = ids.iter().cloned().collect();
        TaskSelection::resolve(&self.store.tasks, &marks, None)
            .filter(|selection| selection.len() == marks.len())
            .ok_or_else(|| anyhow!("A marked task is no longer available"))
    }

    async fn bulk(&mut self, ids: &[TaskId], bulk: &Bulk) -> Result<Outcome> {
        let selection = self.marked(ids).await?;
        let message = match bulk {
            Bulk::Status(status) => {
                let done = self
                    .store
                    .mutate_status_selection(&selection, *status, false);
                done.await?.message
            }
            Bulk::Priority(priority) => {
                let change = PriorityMutation::Set(*priority);
                self.store
                    .mutate_priority_selection(&selection, change)
                    .await?
                    .message
            }
            Bulk::Schedule {
                available_at,
                due_on,
            } => {
                let mut messages = Vec::new();
                if let Some(value) = available_at {
                    let field = TaskDateField::Availability;
                    let done =
                        self.store
                            .mutate_date_selection(&selection, field, value.clone(), false);
                    messages.push(done.await?.message);
                }
                if let Some(value) = due_on {
                    let field = TaskDateField::Due;
                    let done =
                        self.store
                            .mutate_date_selection(&selection, field, value.clone(), false);
                    messages.push(done.await?.message);
                }
                messages.join(" · ")
            }
            Bulk::Project(project) => {
                self.store
                    .mutate_project_selection(&selection, project.clone())
                    .await?
                    .message
            }
            Bulk::Labels { selected, partial } => {
                self.store
                    .mutate_labels_selection(&selection, selected.clone(), partial.clone())
                    .await?
                    .message
            }
            Bulk::Deleted(deleted) => {
                let done = self
                    .store
                    .mutate_deleted_selection(&selection, *deleted, false);
                let message = done.await?.message;
                if *deleted {
                    return Ok(Outcome::saved(said(&message))
                        .with_go("Show deleted", Command::FilterDeleted(DeletedFilter::Show)));
                }
                message
            }
            Bulk::Epic(epic) => return self.add_to_epic(ids, epic).await,
        };
        Ok(Outcome::saved(said(&message)))
    }

    async fn epic_context(&self, epic: &TaskId) -> Result<EpicContext> {
        let item = self
            .store
            .load_task_item(epic)
            .await?
            .ok_or_else(|| anyhow!("That epic no longer exists"))?;
        Ok(EpicContext {
            epic_id: item.task.id,
            display_ref: item.display_ref,
            project_key: item.task.project_key,
        })
    }

    /// Core takes one child at a time, so each leaves its own undo entry.
    async fn add_to_epic(&mut self, ids: &[TaskId], epic: &TaskId) -> Result<Outcome> {
        let context = self.epic_context(epic).await?;
        let mut last = String::new();
        for (at, id) in ids.iter().enumerate() {
            let added = self
                .store
                .add_epic_child(context.clone(), id.clone())
                .await?;
            last = added.message.message;
            if added.changed && at + 1 < ids.len() {
                self.undoable.push(said(&last));
            }
        }
        Ok(Outcome::saved(if ids.len() == 1 {
            said(&last)
        } else {
            Toast::done(format!(
                "Added {} tasks to {}",
                ids.len(),
                context.display_ref
            ))
        }))
    }

    async fn review(&self, exclude: &[TaskId]) -> Result<(Vec<ReviewCard>, usize)> {
        let store = &self.store;
        let scope = self.store.view_state.scope.clone();
        let project = match &scope {
            TaskScope::Workspace => None,
            TaskScope::Project(project) => Some(project.clone()),
        };
        let items = store
            .database()
            .list_task_items(
                &store.active_workspace.id,
                TaskFilters {
                    project: project.clone(),
                    statuses: ["inbox", "todo", "active"].map(str::to_string).to_vec(),
                    exclude_epics: true,
                    availability: TaskAvailabilityFilter::Available,
                    ..TaskFilters::default()
                },
                TaskQueryMode::Flat,
                TaskSort::Created,
                SortDirection::Asc,
            )
            .await?;
        let reviews = store.database().triage_reviews().await?;
        let now = now_seconds();
        let candidates: Vec<(TaskListItem, TriageReason)> = items
            .into_iter()
            .filter(|item| !exclude.contains(&item.task.id))
            .filter_map(|item| {
                let reason = triage_reason(&item.task, &item.queue, now)?;
                let reviewed = reviews.get(item.task.id.as_str()).copied();
                (!triage_review_current(reason, reviewed, now)).then_some((item, reason))
            })
            .collect();
        let total = candidates.len();
        let cards: Vec<ReviewCard> = review_page(candidates)
            .into_iter()
            .map(|(item, reason)| ReviewCard {
                id: item.task.id,
                why: match reason {
                    TriageReason::Inbox => "In the inbox, needs a decision".to_string(),
                    _ => {
                        let days = item.queue.idle_days.unwrap_or(0);
                        format!("No activity for {days} days")
                    }
                },
                stale: reason != TriageReason::Inbox,
            })
            .collect();
        let more = total - cards.len();
        Ok((cards, more))
    }

    async fn keep(&mut self, id: &TaskId) -> Result<Outcome> {
        let item = self
            .store
            .load_task_item(id)
            .await?
            .ok_or_else(|| anyhow!("That task no longer exists"))?;
        let now = now_seconds();
        let previous = self
            .store
            .database()
            .set_triage_review(id.as_str(), Some(now), now)
            .await?;
        self.kept.insert(id.clone(), previous);
        let window = match item.task.status {
            TaskStatus::Active => Some(STALE_ACTIVE_DAYS),
            TaskStatus::Todo => Some(STALE_TODO_DAYS),
            _ => None,
        };
        let subject = Some(Subject {
            reference: item.display_ref,
            project_key: item.task.project_key,
        });
        let kept =
            Toast::done(format!("Stays {}", status_label(item.task.status))).subject(subject);
        Ok(Outcome::saved(match window {
            Some(days) => kept.detail(format!("Back in triage in {days} days")),
            None => kept,
        })
        .final_write())
    }

    async fn unkeep(&mut self, id: &TaskId) -> Result<Outcome> {
        let Some(previous) = self.kept.remove(id) else {
            return Ok(Outcome::default());
        };
        let now = now_seconds();
        self.store
            .database()
            .set_triage_review(id.as_str(), previous, now)
            .await?;
        Ok(Outcome::saved(Toast::done("Keep undone")).final_write())
    }

    /// The report goes through `gh` off the actor, as publishing can take a while.
    async fn publish_gist(&mut self, id: &TaskId) {
        let blob_dir = self.blob_dir();
        let report = match blob_dir {
            Ok(blob_dir) => self.store.task_full_report(&blob_dir, id).await,
            Err(error) => Err(error),
        };
        let report = match report {
            Ok(Some(report)) => report,
            Ok(None) => {
                let message = "could not prepare gist: that task no longer exists";
                self.send(ReplyBody::Gist(Err(message.to_string())));
                return;
            }
            Err(error) => {
                let message = format!("could not prepare gist: {error:#}");
                self.send(ReplyBody::Gist(Err(message)));
                return;
            }
        };
        let markdown = aven_host::task_render::task_markdown(&report);
        let filename = aven_host::task_render::gist_filename(&report);
        let description = aven_host::task_render::gist_description(&report);
        let replies = self.replies.clone();
        let request = self.handled;
        tokio::spawn(async move {
            let run = tokio::task::spawn_blocking(move || {
                create_secret_gist(OsStr::new("gh"), &markdown, &filename, &description)
            })
            .await;
            let result = run
                .map_err(anyhow::Error::from)
                .and_then(|result| result)
                .map_err(|error| format!("gist creation failed: {error:#}"));
            let body = ReplyBody::Gist(result);
            let _ = replies.send(Reply { request, body });
        });
    }

    async fn send_detail(&mut self, id: &TaskId) {
        match self.store.load_task_item(id).await {
            Ok(Some(item)) => {
                let project = self
                    .store
                    .projects
                    .iter()
                    .find(|project| project.key == item.task.project_key);
                let name = project.map_or_else(String::new, |project| project.name.clone());
                let images = self.images(&item).await;
                let mut detail = detail_with(&item, Local::now(), name, images);
                detail.children = self.children(&item).await;
                // Conflicts on a recurring task can sit on its series.
                detail.conflicts = self
                    .store
                    .conflict_targets_for(&item)
                    .await
                    .unwrap_or_default();
                self.send(ReplyBody::Detail(Box::new(detail)));
            }
            Ok(None) => {}
            Err(error) => self.send(ReplyBody::RefreshFailed(error.to_string())),
        }
    }

    /// Keeps each view's column choice.
    async fn show_view(&mut self, query: TaskQuery) -> Result<()> {
        if self.store.view_state.layout == TaskLayout::Columns {
            let current = self.store.view_state.query;
            self.column_views.retain(|view| *view != current);
            self.column_views.push(current);
        } else {
            let current = self.store.view_state.query;
            self.column_views.retain(|view| *view != current);
        }
        if self.store.view_state.layout != TaskLayout::List {
            let mut state = self.store.view_state.clone();
            state.layout = TaskLayout::List;
            self.store
                .restore_view_state_with_restore(state, &SelectionRestore::Default)
                .await?;
        }
        self.store
            .show_view_restoring(query, &SelectionRestore::Default)
            .await?;
        if query == TaskQuery::Queue {
            self.arrange.order = ViewOrder::default();
        }
        if self.column_views.contains(&query) {
            self.set_columns(true).await?;
        }
        Ok(())
    }

    async fn show_recurring(&mut self, lifecycle: RecurrenceSeriesLifecycleFilter) -> Result<()> {
        if self.store.view_state.query != TaskQuery::Recurring {
            self.show_view(TaskQuery::Recurring).await?;
        }
        self.set_filters(|state| state.recurring.lifecycle = lifecycle)
            .await
    }

    async fn set_columns(&mut self, on: bool) -> Result<()> {
        let mut state = self.store.view_state.clone();
        state.layout = if on {
            TaskLayout::Columns
        } else {
            TaskLayout::List
        };
        self.store
            .restore_view_state_with_restore(state, &SelectionRestore::Default)
            .await?;
        Ok(())
    }

    async fn set_filters(&mut self, change: impl FnOnce(&mut TaskViewState)) -> Result<()> {
        let mut state = self.store.view_state.clone();
        change(&mut state);
        self.store
            .restore_view_state_with_restore(state, &SelectionRestore::Default)
            .await?;
        Ok(())
    }

    async fn search(&mut self, input: &SearchInput) -> Result<()> {
        let text = input.text.trim().to_string();
        let results = self
            .store
            .database()
            .search_task_items(
                &self.store.active_workspace.id,
                TaskSearchQuery {
                    text: text.clone(),
                    project: input.project.clone(),
                    metadata: Vec::new(),
                    has_metadata: Vec::new(),
                    missing_metadata: Vec::new(),
                    include_deleted: input.deleted,
                    limit: 100,
                },
            )
            .await?;
        let mut state = self.store.view_state.clone();
        state.scope = input
            .project
            .clone()
            .map_or(TaskScope::Workspace, TaskScope::Project);
        state.set_query(TaskQuery::Search);
        state.layout = TaskLayout::List;
        state.projection_origin = TaskProjectionOrigin::Search {
            query: text,
            task_ids: results
                .iter()
                .map(|result| result.item.task.id.clone())
                .collect(),
        };
        state.filter_modifiers = Default::default();
        self.arrange.metadata = None;
        self.store
            .restore_view_state_with_restore(state, &SelectionRestore::Default)
            .await?;
        Ok(())
    }

    /// Recent actions lists changes, not the tasks still loaded behind them.
    fn listed(&self, id: &TaskId) -> bool {
        self.store.view_state.query != TaskQuery::RecentActions
            && self.store.tasks.iter().any(|item| item.task.id == *id)
    }

    /// Widens the view one step at a time until the task is in it, then falls
    /// back to showing it alone.
    async fn goto(&mut self, id: &TaskId) -> Result<()> {
        let item = self
            .store
            .load_task_item(id)
            .await?
            .ok_or_else(|| anyhow!("That task no longer exists"))?;
        if item.task.deleted {
            self.set_filters(|state| {
                state.filter_modifiers.include_deleted = true;
                state.filter_modifiers.deleted_only = false;
            })
            .await?;
        }
        if self.listed(id) {
            return Ok(());
        }
        self.set_filters(|state| state.scope = TaskScope::Workspace)
            .await?;
        if self.listed(id) {
            return Ok(());
        }
        let home = if is_deferred(&item, Local::now()) {
            TaskQuery::Upcoming
        } else if item.task.status.is_terminal() {
            TaskQuery::Done
        } else {
            TaskQuery::Queue
        };
        self.show_view(home).await?;
        if self.listed(id) {
            return Ok(());
        }
        let deleted = item.task.deleted;
        self.arrange.metadata = None;
        self.set_filters(|state| {
            state.filter_modifiers = Default::default();
            state.filter_modifiers.include_deleted = deleted;
        })
        .await?;
        if self.listed(id) {
            self.send(ReplyBody::Notice(Toast::info(
                "Filters cleared to show this task",
            )));
            return Ok(());
        }
        self.store.show_task_by_id(id.clone()).await?;
        Ok(())
    }

    /// Runs a command that changes what is shown, not what is stored.
    async fn navigate(&mut self, command: &Command) -> Result<()> {
        match command {
            Command::ShowView(query) => self.show_view(*query).await,
            Command::SetClosed(closed) => {
                let closed = if *closed {
                    ClosedTaskVisibility::Included
                } else {
                    ClosedTaskVisibility::Default
                };
                self.set_filters(|state| state.filter_modifiers.closed = closed)
                    .await
            }
            Command::ShowRecurring(lifecycle) => self.show_recurring(*lifecycle).await,
            Command::OpenSeries(id) => {
                let lifecycle = self.store.view_state.recurring.lifecycle;
                self.show_recurring(lifecycle).await?;
                if self
                    .store
                    .recurrence_series
                    .iter()
                    .all(|item| item.series.id != *id)
                {
                    self.show_recurring(RecurrenceSeriesLifecycleFilter::All)
                        .await?;
                }
                self.send(ReplyBody::SelectSeries(id.clone()));
                Ok(())
            }
            Command::ShowProject(project) => {
                let target = project
                    .clone()
                    .map_or(TaskScopeTarget::Workspace, TaskScopeTarget::Project);
                self.store
                    .show_scope_restoring(target, &SelectionRestore::Default)
                    .await?;
                Ok(())
            }
            Command::SwitchWorkspace(key) => {
                self.arrange = Arrange::default();
                self.store.switch_workspace(key.clone()).await?;
                Ok(())
            }
            Command::FilterLabel(label) => {
                let label = label.clone();
                self.set_filters(|state| state.filter_modifiers.label = label)
                    .await
            }
            Command::FilterPriority(priority) => {
                let priority = priority.map(|priority| priority.to_string());
                self.set_filters(|state| state.filter_modifiers.priority = priority)
                    .await
            }
            Command::FilterDeleted(filter) => {
                let filter = *filter;
                self.set_filters(|state| {
                    let modifiers = &mut state.filter_modifiers;
                    modifiers.include_deleted = filter == DeletedFilter::Show;
                    modifiers.deleted_only = filter == DeletedFilter::Only;
                })
                .await
            }
            Command::FilterMetadata(filter) => {
                self.arrange.metadata = filter.clone();
                Ok(())
            }
            Command::ClearFilters => {
                self.arrange.metadata = None;
                self.set_filters(|state| state.filter_modifiers = Default::default())
                    .await
            }
            Command::SetOrder(order) => {
                self.arrange.order = *order;
                let from_queue = self.store.view_state.query == TaskQuery::Queue;
                let core = match order {
                    ViewOrder::Priority | ViewOrder::Available => TaskOrder::Priority,
                    ViewOrder::Due => TaskOrder::DueOn,
                    ViewOrder::Created => TaskOrder::Created,
                    ViewOrder::Updated => TaskOrder::Updated,
                    ViewOrder::Project => TaskOrder::Project,
                    ViewOrder::Title => TaskOrder::Title,
                };
                self.store
                    .set_order_restoring(core, &SelectionRestore::Default)
                    .await?;
                if from_queue && self.column_views.contains(&TaskQuery::Open) {
                    self.set_columns(true).await?;
                }
                Ok(())
            }
            Command::ReverseOrder => {
                let from_queue = self.store.view_state.query == TaskQuery::Queue;
                self.store
                    .reverse_sort_restoring(&SelectionRestore::Default)
                    .await?;
                if from_queue && self.column_views.contains(&TaskQuery::Open) {
                    self.set_columns(true).await?;
                }
                Ok(())
            }
            Command::SetGroup(group) => {
                self.arrange.group = *group;
                Ok(())
            }
            Command::SetColumns(on) => self.set_columns(*on).await,
            Command::Search(input) => self.search(input).await,
            Command::Goto(id) => {
                self.goto(id).await?;
                self.selected = Some(id.clone());
                self.send(ReplyBody::Select(id.clone()));
                Ok(())
            }
            _ => Ok(()),
        }
    }

    async fn add_image(&mut self, id: &TaskId, image: &PendingImage, alt: String) -> Result<()> {
        let blob_dir = self.blob_dir()?;
        let policy = self.config.local.attachment_lifecycle.policy();
        self.store
            .attachment_worker_context()
            .add_ordered_attachment(
                &blob_dir,
                policy,
                id,
                Local::now().to_utc().to_rfc3339(),
                TaskAttachmentAddInput {
                    attachment_id: new_id(),
                    input: attachment_input(image, alt),
                },
            )
            .await?;
        Ok(())
    }

    /// Core keeps no way to change alt text, so the image is added again with the
    /// new text and the old copy is removed.
    async fn set_image_alt(&mut self, id: &TaskId, image: &str, alt: &str) -> Result<()> {
        let item = self
            .store
            .load_task_item(id)
            .await?
            .ok_or_else(|| anyhow!("That task no longer exists"))?;
        let attachment = item
            .attachments
            .iter()
            .find(|attachment| attachment.attachment_id == image)
            .ok_or_else(|| anyhow!("That image no longer exists"))?;
        let path = object_path(&self.blob_dir()?, &attachment.sha256)?;
        let bytes = std::fs::read(path).context("This image has not finished downloading")?;
        let pending = PendingImage {
            name: attachment.filename.clone().unwrap_or_default(),
            bytes: Arc::new(bytes),
        };
        self.add_image(id, &pending, alt.to_string()).await?;
        self.store.delete_attachment(image).await
    }

    async fn create(&mut self, task: &NewTask) -> Result<Outcome> {
        let parsed = parse_schedule_input(&task.schedule)?;
        if task.replaces.is_some() && !matches!(parsed, ParsedScheduleInput::Recurring { .. }) {
            anyhow::bail!("A replacement needs a repeating schedule");
        }
        let project = match task.project.clone().or_else(|| self.project_for_new_task()) {
            Some(project) => project,
            None => {
                self.create_first_project(task.first_project.as_deref())
                    .await?
            }
        };
        let project = Some(project);
        let (available_at, due_on) = match parsed {
            ParsedScheduleInput::None => (None, None),
            ParsedScheduleInput::Once {
                available_at,
                due_on,
            } => (
                Some(available_at)
                    .filter(|at| !at.is_empty())
                    .map(|at| parse_available_at_input(&at))
                    .transpose()?
                    .filter(|at| !at.is_empty()),
                Some(due_on)
                    .filter(|due| !due.is_empty())
                    .map(|due| parse_due_on_input(&due))
                    .transpose()?
                    .filter(|due| !due.is_empty()),
            ),
            ParsedScheduleInput::Recurring { .. } => {
                return self.create_recurring(task, project).await;
            }
        };
        let deferred = available_at.is_some();
        let draft = TaskDraft {
            title: task.title.clone(),
            description: task.description.clone(),
            project,
            status: task.status.clone(),
            priority: task.priority.clone(),
            source: TaskSource::Unknown,
            labels: task.labels.clone(),
            metadata: metadata_inputs(&task.metadata),
            available_at,
            due_on,
            is_epic: task.is_epic,
        };
        let selected = if task.images.is_empty() {
            self.store.create_task(draft, None).await?.1
        } else {
            let attachments = task
                .images
                .iter()
                .map(|image| {
                    PendingTaskAttachment::new(new_id(), attachment_input(image, String::new()))
                })
                .collect();
            let blob_dir = self.blob_dir()?;
            let policy = self.config.local.attachment_lifecycle.policy();
            self.store
                .create_task_with_attachments(draft, None, &blob_dir, policy, attachments)
                .await?
                .1
        };
        let item = selected.and_then(|index| self.store.tasks.get(index));
        let created = item.map(|item| item.task.id.clone());
        let subject = item.map(|item| Subject {
            reference: item.display_ref.clone(),
            project_key: item.task.project_key.clone(),
        });
        let outcome = Outcome {
            created: created.clone(),
            ..Outcome::saved(Toast::done("Created").subject(subject))
        };
        Ok(match created {
            Some(id) if !deferred => outcome.with_go("View task", Command::Goto(id)),
            _ => outcome.with_go("View in Upcoming", Command::ShowView(TaskQuery::Upcoming)),
        })
    }

    async fn create_recurring(
        &mut self,
        task: &NewTask,
        project: Option<String>,
    ) -> Result<Outcome> {
        let schedule = crate::draft::recurrence_schedule(&task.schedule)?;
        let status = task.status.clone().unwrap_or_else(|| "todo".to_string());
        let mut draft = recurrence_draft(
            task.title.clone(),
            task.description.clone(),
            project,
            task.priority.clone(),
            status,
            task.labels.clone(),
            schedule,
        );
        draft.metadata = metadata_inputs(&task.metadata);
        let (message, _) = self.store.create_recurrence_series(draft, None).await?;
        Ok(Outcome {
            stop: task.replaces.clone(),
            ..Outcome::saved(said(&message))
        }
        .with_go("View in Upcoming", Command::ShowView(TaskQuery::Upcoming)))
    }

    /// Runs a write and reports what it did. An error that `mutation_committed`
    /// or `task_creation_committed` accepts means the write stands.
    async fn mutate(&mut self, command: &Command) -> Result<Outcome> {
        if self.paused_by_join().await {
            anyhow::bail!(JOIN_PAUSE);
        }
        let outcome = match command {
            Command::Bulk(ids, bulk) => self.bulk(ids, bulk).await?,
            Command::CreateChild(epic, title) => {
                let context = self.epic_context(epic).await?;
                let draft = TaskDraft {
                    title: title.clone(),
                    description: String::new(),
                    project: None,
                    status: None,
                    priority: "none".to_string(),
                    source: TaskSource::Unknown,
                    labels: Vec::new(),
                    metadata: Vec::new(),
                    available_at: None,
                    due_on: None,
                    is_epic: false,
                };
                let (message, ..) = self
                    .store
                    .create_task_for_epic(draft, None, &context)
                    .await?;
                Outcome::saved(said(&message))
            }
            Command::AddChild(epic, child) => {
                let context = self.epic_context(epic).await?;
                let added = self.store.add_epic_child(context, child.clone()).await?;
                Outcome::saved(said(&added.message.message))
            }
            Command::RemoveChild(child) => {
                let item = self
                    .store
                    .load_task_item(child)
                    .await?
                    .ok_or_else(|| anyhow!("That task no longer exists"))?;
                let target = self
                    .store
                    .resolve_epic_child_target_for_item(&item)
                    .ok_or_else(|| anyhow!("That task is not in an epic"))?;
                let removed = self.store.remove_epic_child(target).await?;
                Outcome::saved(said(&removed.message.message))
            }
            Command::PauseSeries(id) => {
                let selected = self.selected.clone();
                let paused = self.store.pause_recurrence(id, selected.as_ref()).await?;
                Outcome::saved(said(&paused.message)).final_write()
            }
            Command::ResumeSeries(id) => {
                let selected = self.selected.clone();
                let resumed = self.store.resume_recurrence(id, selected.as_ref()).await?;
                Outcome::saved(said(&resumed.message)).final_write()
            }
            Command::SkipSeries(id) => {
                let detail = self.store.recurrence_detail_for_series(id).await?;
                let current = detail.current_occurrence.as_ref();
                let task = current.and_then(|current| current.task_id.as_ref());
                let (Some(current), Some(task)) = (current, task) else {
                    return Err(anyhow!("series has no current occurrence to skip"));
                };
                let skipped = self.store.skip_recurrence(id, task, current.slot_on);
                Outcome::saved(said(&skipped.await?.message)).final_write()
            }
            Command::StopSeries(id, skip) => {
                let selected = self.selected.clone();
                let stopped = self.store.stop_recurrence(id, selected.as_ref(), *skip);
                Outcome::saved(said(&stopped.await?.message)).final_write()
            }
            Command::SetTemplate(id, description) => {
                let selected = self.selected.clone();
                let update = RecurrenceTemplateUpdate {
                    description: Some(description.clone()),
                    ..RecurrenceTemplateUpdate::default()
                };
                let saved = self
                    .store
                    .update_recurrence_template(id, update, selected.as_ref());
                Outcome::saved(said(&saved.await?.message)).final_write()
            }
            Command::ResolveConflict(target, value) => {
                let resolved = self
                    .store
                    .resolve_conflict_value((**target).clone(), value.clone());
                Outcome::saved(said(&resolved.await?.message)).final_write()
            }
            Command::Keep(id) => self.keep(id).await?,
            Command::Unkeep(id) => self.unkeep(id).await?,
            Command::SetStatus(id, status) => {
                let selection = self.selection(id).await?;
                self.store
                    .mutate_status_selection(&selection, *status, true)
                    .await?;
                let open = (self.store.tasks.iter())
                    .find(|item| item.task.id == *id && item.task.is_epic)
                    .filter(|_| status.is_terminal())
                    .map(|item| (item.task.title.clone(), epic_progress(item)));
                let change = match status {
                    TaskStatus::Done => "Marked done".to_string(),
                    TaskStatus::Canceled => "Canceled".to_string(),
                    status => format!("Moved to {}", status_label(*status)),
                };
                let saved = Toast::done(change).subject(self.subject(id));
                Outcome::saved(match open.filter(|(_, progress)| progress.open > 0) {
                    Some((title, progress)) => {
                        saved.detail(format!("{title} still has {} open tasks", progress.open))
                    }
                    None => saved,
                })
            }
            Command::SetPriority(id, priority) => {
                let selection = self.selection(id).await?;
                self.store
                    .mutate_priority_selection(&selection, PriorityMutation::Set(*priority))
                    .await?;
                let name = priority_label(*priority);
                Outcome::saved(
                    Toast::done(format!("Priority set to {name}")).subject(self.subject(id)),
                )
            }
            Command::SetTitle(id, title) => {
                let selection = self.selection(id).await?;
                self.store
                    .mutate_text_selection(&selection, TaskTextField::Title, title.clone())
                    .await?;
                Outcome::quiet()
            }
            Command::SetDescription(id, text) => {
                let selection = self.selection(id).await?;
                self.store
                    .mutate_text_selection(&selection, TaskTextField::Description, text.clone())
                    .await?;
                Outcome::quiet()
            }
            Command::SetSchedule {
                id,
                available_at,
                due_on,
            } => self.set_schedule(id, available_at, due_on).await?,
            Command::SetProject(id, project) => {
                let selection = self.selection(id).await?;
                self.store
                    .mutate_project_selection(&selection, project.clone())
                    .await?;
                let name = self
                    .store
                    .projects
                    .iter()
                    .find(|entry| entry.key == *project)
                    .map_or(project.as_str(), |entry| entry.name.as_str());
                Outcome::saved(Toast::done(format!("Moved to {name}")).subject(self.subject(id)))
            }
            Command::SetLabels(id, labels) => {
                let selection = self.selection(id).await?;
                self.store
                    .mutate_labels_selection(&selection, labels.clone(), Vec::new())
                    .await?;
                Outcome::quiet()
            }
            Command::SetDeleted(id, deleted) => {
                let selection = self.selection(id).await?;
                let subject = self.subject(id);
                self.store
                    .mutate_deleted_selection(&selection, *deleted, false)
                    .await?;
                if *deleted {
                    Outcome::saved(Toast::done("Deleted").subject(subject))
                        .with_go("Show deleted", Command::FilterDeleted(DeletedFilter::Show))
                } else {
                    Outcome::saved(Toast::done("Restored").subject(subject))
                }
            }
            Command::AddBlocker(id, other) => {
                let selection = self.selection(id).await?;
                self.store
                    .add_dependency_to_selection(&selection, other)
                    .await?;
                Outcome::quiet()
            }
            Command::RemoveBlocker(id, other) => {
                let selection = self.selection(id).await?;
                self.store
                    .remove_dependency_from_selection(&selection, other)
                    .await?;
                Outcome::saved(Toast::done("Blocker removed").subject(self.subject(id)))
            }
            Command::AddRelated(id, other) => {
                let selection = self.selection(id).await?;
                self.store
                    .add_related_to_selection(&selection, other)
                    .await?;
                Outcome::quiet()
            }
            Command::RemoveRelated(id, other) => {
                let selection = self.selection(id).await?;
                self.store
                    .remove_related_from_selection(&selection, other)
                    .await?;
                Outcome::saved(Toast::done("Link removed").subject(self.subject(id)))
            }
            Command::SetMetadata(id, key, value) => {
                let selection = self.selection(id).await?;
                let fields = self.store.metadata_fields().await?;
                let field = fields
                    .iter()
                    .find(|field| field.key == *key)
                    .ok_or_else(|| anyhow!("{key} is not a defined field"))?;
                self.store
                    .mutate_metadata(&selection, field, value.clone())
                    .await?;
                let change = if value.is_some() { "Saved" } else { "Removed" };
                Outcome::saved(Toast::done(format!("{change} {key}")).subject(self.subject(id)))
            }
            Command::AddNote(id, body) => {
                self.store.add_note_to_task(id, body.clone()).await?;
                Outcome::quiet()
            }
            Command::DeleteNote(id, note) => {
                self.store.delete_note(id, note).await?;
                Outcome::saved(Toast::done("Note deleted"))
            }
            Command::AddImage(id, image) => {
                self.add_image(id, image, String::new()).await?;
                Outcome::saved(Toast::done("Image added").subject(self.subject(id))).final_write()
            }
            Command::DeleteImage(id, image) => {
                self.store.delete_attachment(image).await?;
                Outcome::saved(Toast::done("Image removed").subject(self.subject(id))).final_write()
            }
            Command::SetImageAlt(id, image, alt) => {
                self.set_image_alt(id, image, alt).await?;
                Outcome::default()
            }
            Command::Undo => Outcome {
                message: Some(
                    self.undo()
                        .await?
                        .unwrap_or_else(|| Toast::info("Nothing to undo")),
                ),
                ..Outcome::default()
            },
            Command::Create(task) => self.create(task).await?,
            _ => Outcome::default(),
        };
        Ok(outcome)
    }

    async fn set_schedule(
        &mut self,
        id: &TaskId,
        available_at: &Option<Option<String>>,
        due_on: &Option<Option<String>>,
    ) -> Result<Outcome> {
        let selection = self.selection(id).await?;
        let today = Local::now().date_naive();
        let mut changes = Vec::new();
        if let Some(at) = available_at {
            self.store
                .mutate_date_selection(&selection, TaskDateField::Availability, at.clone(), true)
                .await?;
            changes.push(at.as_deref().map_or_else(
                || "available now".to_string(),
                |at| format!("hidden until {}", available_text(at, today).to_lowercase()),
            ));
        }
        if let Some(due) = due_on {
            let selection = self.selection(id).await?;
            self.store
                .mutate_date_selection(&selection, TaskDateField::Due, due.clone(), true)
                .await?;
            changes.push(due.as_deref().map_or_else(
                || "due date cleared".to_string(),
                |due| format!("due {}", due_text(due, today).to_lowercase()),
            ));
        }
        let outcome =
            Outcome::saved(Toast::done(capitalised(&changes.join(", "))).subject(self.subject(id)));
        Ok(match available_at {
            Some(Some(_)) => {
                outcome.with_go("View in Upcoming", Command::ShowView(TaskQuery::Upcoming))
            }
            _ => outcome,
        })
    }

    async fn undo(&mut self) -> Result<Option<Toast>> {
        let at = self
            .selected
            .as_ref()
            .and_then(|id| self.store.tasks.iter().position(|item| item.task.id == *id));
        let Some(undone) = self.store.undo_last(at).await? else {
            return Ok(None);
        };
        Ok(Some(self.undoable.pop().map_or_else(
            || said(&undone.message),
            |change| {
                Toast::done("Undone")
                    .subject(change.subject)
                    .detail(change.title)
            },
        )))
    }

    fn preview(&self, input: SearchInput) {
        let database = self.store.database();
        let workspace = self.store.active_workspace.id.clone();
        let replies = self.replies.clone();
        let request = self.handled;
        tokio::spawn(async move {
            let lookup = |project: Option<String>| {
                let database = database.clone();
                let workspace = workspace.clone();
                let text = input.text.trim().to_string();
                async move {
                    database
                        .search_task_preview_set_from_current_projection(
                            &workspace,
                            TaskSearchQuery {
                                text,
                                project,
                                metadata: Vec::new(),
                                has_metadata: Vec::new(),
                                missing_metadata: Vec::new(),
                                include_deleted: input.deleted,
                                limit: 50,
                            },
                        )
                        .await
                }
            };
            let Ok(found) = lookup(input.project.clone()).await else {
                return;
            };
            let elsewhere = match &input.project {
                Some(_) => lookup(None).await.map_or(0, |all| {
                    all.total_matches.saturating_sub(found.total_matches)
                }),
                None => 0,
            };
            let hits = found
                .items
                .iter()
                .map(|hit| SearchHit {
                    id: hit.task_id.clone(),
                    reference: hit.display_ref.clone(),
                    project: hit
                        .display_ref
                        .split('-')
                        .next()
                        .unwrap_or_default()
                        .to_string(),
                    project_key: hit.project_key.clone(),
                    title: hit.title.clone(),
                    status: status_of(&hit.status),
                    snippet: hit.snippet.clone(),
                    deleted: hit.deleted,
                    is_epic: hit.is_epic,
                })
                .collect();
            let body = ReplyBody::Search(SearchResults {
                hits,
                total: found.total_matches,
                elsewhere,
            });
            let _ = replies.send(Reply { request, body });
        });
    }

    /// Runs the configured natural language command off the actor, since it can
    /// take many seconds.
    fn intake(&self, input: String, project: Option<String>) {
        let intake =
            self.store
                .spawn_task_intake(self.config.agent.task_intake.clone(), input, project);
        let replies = self.replies.clone();
        let request = self.handled;
        tokio::spawn(async move {
            let body = match intake
                .await
                .map_err(anyhow::Error::from)
                .and_then(|run| run)
            {
                Ok(result) => ReplyBody::Intake(Box::new(intake_task(result))),
                Err(error) => ReplyBody::IntakeFailed(format!("{error:#}")),
            };
            let _ = replies.send(Reply { request, body });
        });
    }

    async fn launcher_catalog(&self) -> Result<Catalog> {
        let database = self.store.database();
        let mut places = Vec::new();
        for workspace in database.list_workspaces().await? {
            let projects = database
                .list_project_items(&workspace.id)
                .await?
                .into_iter()
                .map(|project| NavEntry {
                    name: project.name,
                    key: project.key,
                    prefix: project.prefix,
                    count: project.open_count,
                })
                .collect();
            places.push(Place {
                labels: database.list_labels(&workspace.id, None).await?,
                key: workspace.key,
                name: workspace.name,
                projects,
            });
        }
        Ok(Catalog {
            active: self.store.active_workspace.key.clone(),
            places,
            agent: launch::has_agent(&self.config.agent.task_intake),
        })
    }

    /// Starts `job` off the actor, or says why it cannot start. Saving jobs
    /// take turns and outlive the launcher; a review replaces the last one.
    async fn launch(&mut self, job: LaunchJob) {
        let id = job.id;
        let review = job.mode == LaunchMode::Review;
        let refused = match launch::refusal(&self.config.agent.task_intake, &job) {
            Some(reason) => Some(reason),
            None if !review && self.paused_by_join().await => Some(
                "Joining sync. Tasks can be added once synced tasks are downloaded.".to_string(),
            ),
            None => self.blob_dir().err().map(|error| format!("{error:#}")),
        };
        if let Some(reason) = refused {
            self.send(ReplyBody::Launch(LaunchEvent::Refused(id, reason)));
            return;
        }
        let env = LaunchEnv {
            database: self.store.database(),
            intake: self.config.agent.task_intake.clone(),
            blob_dir: self.blob_dir().unwrap_or_default(),
            policy: self.config.local.attachment_lifecycle.policy(),
        };
        let replies = self.replies.clone();
        let request = self.handled;
        if review {
            let task = tokio::spawn(async move {
                let body = ReplyBody::Launch(launch::run(&env, job).await);
                let _ = replies.send(Reply { request, body });
            });
            if let Some((_, previous)) = self.launch_review.replace((id, task)) {
                previous.abort();
            }
            return;
        }
        self.send(ReplyBody::Launch(LaunchEvent::Accepted(id)));
        let turns = self.launch_turns.clone();
        let wake = self.sync_wake.clone();
        tokio::spawn(async move {
            let Ok(_turn) = turns.acquire_owned().await else {
                return;
            };
            let event = launch::run(&env, job).await;
            if matches!(event, LaunchEvent::Created(_)) {
                wake.notify_one();
            }
            let _ = replies.send(Reply {
                request,
                body: ReplyBody::Launch(event),
            });
        });
    }

    async fn copy_report(&mut self, id: &TaskId) -> Result<String> {
        let blob_dir = self.blob_dir()?;
        let report = self
            .store
            .task_full_report(&blob_dir, id)
            .await?
            .ok_or_else(|| anyhow!("That task no longer exists"))?;
        Ok(aven_host::task_render::task_markdown(&report))
    }

    async fn handle(&mut self, request: Request) {
        self.handled = request.id;
        match request.command {
            Command::Refresh => self.outside_change().await,
            Command::RefreshView => {
                self.selected = None;
                self.outside_change().await;
            }
            Command::LoadDetail(id) => {
                self.selected = Some(id.clone());
                self.send_detail(&id).await;
            }
            Command::Peek(id) => self.send_detail(&id).await,
            Command::Preview(input) => self.preview(input),
            Command::Intake { input, project } => self.intake(input, project),
            Command::LoadLauncher => match self.launcher_catalog().await {
                Ok(catalog) => self.send(ReplyBody::Launcher(Box::new(catalog))),
                Err(error) => self.send(ReplyBody::Notice(failed(
                    "Could not load the launcher",
                    &error,
                ))),
            },
            Command::Launch(job) => self.launch(*job).await,
            Command::CancelLaunch(id) => {
                if let Some((_, review)) = self.launch_review.take_if(|(at, _)| *at == id) {
                    review.abort();
                }
            }
            Command::LoadSeries(id) => self.send_series(&id).await,
            Command::StartReview(exclude) => match self.review(&exclude).await {
                Ok((cards, more)) => self.send(ReplyBody::Review(cards, more)),
                Err(error) => {
                    self.send(ReplyBody::Notice(failed("Could not start triage", &error)))
                }
            },
            Command::PublishGist(id) => self.publish_gist(&id).await,
            Command::Sync(request) => self.sync_request(request).await,
            Command::LoadSettings => self.send_settings().await,
            Command::Backup => {
                let message = match self.backup().await {
                    Ok(path) => Toast::done("Backup written").detail(path.display().to_string()),
                    Err(error) => failed("Could not back up", &error),
                };
                self.send(ReplyBody::Notice(message));
            }
            Command::Export => {
                let message = match self.export().await {
                    Ok(path) => {
                        Toast::done("JSON export written").detail(path.display().to_string())
                    }
                    Err(error) => failed("Could not export", &error),
                };
                self.send(ReplyBody::Notice(message));
            }
            Command::Restore(archive) => self.restore(archive).await,
            Command::OpenDatabase(path) => {
                self.open_database(path).await;
            }
            Command::ReopenView {
                query,
                project,
                columns,
                selected,
            } => self.reopen_view(query, project, columns, selected).await,
            Command::Quit => self.quit().await,
            Command::FinishWelcome => {
                if let Err(error) = self.store.database().complete_onboarding().await {
                    tracing::warn!(%error, "could not record the welcome as done");
                }
                self.welcome = false;
                self.send_list().await;
            }
            Command::Settings(_) if self.paused_by_join().await => {
                self.send(ReplyBody::Notice(join_paused()));
            }
            Command::Settings(edit) => {
                match self.edit_settings(edit).await {
                    Ok(message) => {
                        self.outside_change().await;
                        self.send_settings().await;
                        self.send(ReplyBody::Notice(Toast::done(message)));
                    }
                    Err(error) => {
                        self.send_settings().await;
                        self.send(ReplyBody::Notice(failed("Could not save", &error)));
                    }
                }
                self.sync_wake.notify_one();
            }
            Command::CopyReport(id) => match self.copy_report(&id).await {
                Ok(text) => self.send(ReplyBody::Copied(text)),
                Err(error) => self.send(ReplyBody::Notice(failed(
                    "Could not copy the report",
                    &error,
                ))),
            },
            Command::SaveImage(image, path) => {
                let message = match self.save_image(&image, path).await {
                    Ok(()) => Toast::done("Saved a copy"),
                    Err(error) => failed("Could not save the image", &error),
                };
                self.send(ReplyBody::Notice(message));
            }
            command if command.is_navigation() => {
                if !matches!(command, Command::Goto(_)) {
                    self.selected = None;
                }
                match self.navigate(&command).await {
                    Ok(()) => self.send_list().await,
                    Err(error) => {
                        self.send(ReplyBody::SaveFailed(command, error.to_string()));
                        self.send_list().await;
                    }
                }
            }
            command => {
                match self.mutate(&command).await {
                    Ok(outcome) => {
                        // These save without refreshing the projection.
                        let refreshed = match command {
                            Command::AddNote(..)
                            | Command::Bulk(..)
                            | Command::Keep(_)
                            | Command::Unkeep(_)
                            | Command::SetDeleted(..)
                            | Command::DeleteNote(..)
                            | Command::AddImage(..)
                            | Command::DeleteImage(..)
                            | Command::SetImageAlt(..) => self.refresh().await,
                            _ => Ok(()),
                        };
                        match refreshed {
                            Ok(()) => self.send_list().await,
                            Err(error) => {
                                self.send(ReplyBody::SavedRefreshFailed(error.to_string()));
                            }
                        }
                        self.finish_write(&command, outcome);
                    }
                    Err(error) if mutation_committed(&error) || task_creation_committed(&error) => {
                        self.send(ReplyBody::SavedRefreshFailed(error.to_string()));
                    }
                    Err(error) => {
                        self.send(ReplyBody::SaveFailed(command, error.to_string()));
                        self.send_list().await;
                    }
                }
                self.sync_wake.notify_one();
            }
        }
    }

    pub(crate) async fn reopen_view(
        &mut self,
        query: Option<TaskQuery>,
        project: Option<String>,
        columns: bool,
        selected: Option<TaskId>,
    ) {
        let scope = project.map_or(TaskScopeTarget::Workspace, TaskScopeTarget::Project);
        let _ = self
            .store
            .show_scope_restoring(scope, &SelectionRestore::Default)
            .await;
        if let Some(query) = query {
            let _ = self.show_view(query).await;
            if columns && supports_columns(query) {
                let _ = self.set_columns(true).await;
            }
        }
        let selected = selected.filter(|id| self.listed(id));
        self.selected = selected.clone();
        self.send_list().await;
        if let Some(id) = selected {
            self.send(ReplyBody::Select(id));
        }
    }

    /// Every earlier write has returned by now, so its text is stored or handed
    /// back. A background round gets `QUIT_SYNC_BUDGET` to end, because `run_sync`
    /// only looks at the stop signal between rounds.
    async fn quit(&mut self) {
        self.sync_stop.notify_one();
        let background = self.background_sync.take();
        let rounds = async {
            if let Some(sync) = background {
                let _ = sync.await;
            }
            while self.sync_busy.load(Ordering::SeqCst) {
                sleep(QUIT_POLL).await;
            }
        };
        let _ = timeout(QUIT_SYNC_BUDGET, rounds).await;
        self.send(ReplyBody::Stopped);
    }

    fn finish_write(&mut self, command: &Command, outcome: Outcome) {
        let undo = outcome.undo && self.store.available_undo().is_some();
        if undo && !matches!(command, Command::Undo) {
            self.undoable.push(
                outcome
                    .message
                    .clone()
                    .unwrap_or_else(|| Toast::done("Last change")),
            );
        }
        if let Some(id) = outcome.created {
            self.send(ReplyBody::Created(id));
        }
        if let Some(id) = outcome.stop {
            self.send(ReplyBody::AskStop(id));
        }
        if let Some(message) = outcome.message {
            let action = outcome.go.map(ToastAction::Go);
            self.send(ReplyBody::Saved(message.action(action).undo(undo)));
        }
    }

    async fn save_image(&self, image: &str, path: PathBuf) -> Result<()> {
        self.store
            .save_attachment(&self.blob_dir()?, image, path)
            .await
            .map_err(|error| anyhow!("{error:?}"))?;
        Ok(())
    }

    async fn outside_change(&mut self) {
        match self.refresh().await {
            Ok(()) => self.send_list().await,
            Err(error) => self.send(ReplyBody::RefreshFailed(error.to_string())),
        }
    }

    async fn run(mut self, mut requests: mpsc::UnboundedReceiver<Request>) {
        let mut writes = OutsideWrites::open(&self.database).await.ok();
        let mut watched = self.database.clone();
        let mut poll = interval(POLL_INTERVAL);
        poll.set_missed_tick_behavior(MissedTickBehavior::Delay);
        // The first tick lets what launch asked for go ahead of a full refresh.
        let mut clock = interval_at(Instant::now() + POLL_INTERVAL, CLOCK_INTERVAL);
        clock.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                request = requests.recv() => {
                    let Some(request) = request else { break };
                    self.handle(request).await;
                    if watched != self.database {
                        watched = self.database.clone();
                        writes = OutsideWrites::open(&watched).await.ok();
                    }
                }
                _ = clock.tick() => self.outside_change().await,
                _ = poll.tick() => {
                    let Some(version) = writes.as_mut() else { continue };
                    if version.changed().await.unwrap_or(false) {
                        self.outside_change().await;
                    }
                }
            }
        }
    }
}

/// Runs background rounds until the app quits. The daemon's enable flag is not
/// read: a running app syncs on its own, and `NotSetUp` idles a local database.
async fn run_sync(
    database: Database,
    config: AppConfig,
    storage: Option<Arc<dyn ProtectedStorage>>,
    wake: Arc<Notify>,
    stop: Arc<Notify>,
    replies: mpsc::UnboundedSender<Reply>,
) {
    let interval = Duration::from_secs(config.sync_interval_seconds());
    let mut scheduler = SyncScheduler::new(interval, Instant::now());
    let mut sync = storage.map_or_else(DaemonSync::default, DaemonSync::with_storage);
    let report = |health: SyncHealth| {
        let _ = replies.send(Reply {
            request: 0,
            body: ReplyBody::Sync(health),
        });
    };
    loop {
        tokio::select! {
            biased;
            () = stop.notified() => return,
            () = wake.notified() => {
                sleep(SYNC_DEBOUNCE).await;
                scheduler.wake(false, Instant::now());
            }
            () = sleep_until(scheduler.next_sync()) => {
                scheduler.round_started();
                let round = sync.round(&database, &config, ROUND_BUDGET).await;
                let now = Instant::now();
                match round {
                    Ok(DaemonRound::Completed(outcome)) => {
                        scheduler.completed(outcome.more_work_ready(), now);
                        report(SyncHealth::Synced);
                        if outcome.received_changes > 0 {
                            let _ = replies.send(Reply {
                                request: 0,
                                body: ReplyBody::SyncArrived(outcome.received_changes),
                            });
                        }
                    }
                    Ok(DaemonRound::NotSetUp) => {
                        scheduler.not_set_up(now);
                        report(SyncHealth::NotSetUp);
                    }
                    Ok(DaemonRound::Deferred) => {
                        scheduler.deferred(now);
                        report(SyncHealth::Waiting);
                    }
                    Ok(DaemonRound::VaultDeleted) => {
                        scheduler.vault_deleted(now);
                        report(SyncHealth::VaultDeleted);
                    }
                    Err(error) => {
                        scheduler.failed(now);
                        report(SyncHealth::Failed(sync_ops::explain(
                            ErrorAction::General,
                            &error,
                            "Sync couldn't finish. Try again.",
                        )));
                    }
                }
            }
        }
    }
}

/// Opens the store on its thread, starts sync, and returns the first snapshot's
/// session. A database that does not open still gives a session, whose view
/// offers ways out, and the thread waits for one.
pub fn start(handle: tokio::runtime::Handle, launch: Launch) -> Result<(Session, Replies)> {
    prepare(handle, launch, None)?()
}

/// Starts the store while the main thread initializes the native platform.
/// The returned function waits for a fresh snapshot, never a persisted UI cache.
pub(crate) fn prepare(
    handle: tokio::runtime::Handle,
    launch: Launch,
    saved: Option<SavedView>,
) -> Result<impl FnOnce() -> Result<(Session, Replies)>> {
    let selected = saved
        .as_ref()
        .and_then(|view| view.selected.as_deref())
        .and_then(|id| id.parse().ok());
    let (requests_tx, mut requests_rx) = mpsc::unbounded_channel();
    let (replies_tx, replies_rx) = mpsc::unbounded_channel();
    let (opened_tx, opened_rx) = std::sync::mpsc::channel();
    let sync_wake = Arc::new(Notify::new());
    let runtime = handle.clone();
    let database = launch.database.clone();
    std::thread::Builder::new()
        .name("aven-store".to_string())
        .spawn(move || {
            runtime.block_on(async move {
                let mut path = launch.database.clone();
                let mut config = launch.config.clone();
                let mut opened = Some(opened_tx);
                let store = loop {
                    let view = opened.as_ref().and(saved.as_ref());
                    let error = match open_store_in_view(&path, &config, view).await {
                        Ok(store) => break store,
                        Err(error) => error,
                    };
                    let failure = (path.clone(), format!("{error:#}"));
                    match opened.take() {
                        Some(opened) => {
                            let _ = opened.send(Err(failure));
                        }
                        None => {
                            let _ = replies_tx.send(Reply {
                                request: 0,
                                body: ReplyBody::DatabaseFailed(failure.0, failure.1),
                            });
                        }
                    }
                    let Some(next) = recovery_target(&mut requests_rx, &launch, &replies_tx).await
                    else {
                        return;
                    };
                    config = sync_ops::config_for(&next, &launch.database, &launch.config);
                    path = next;
                };
                let welcome = welcome_due(&store.database()).await;
                match opened.take() {
                    Some(opened) => {
                        let mut first = snapshot(&store, Local::now());
                        first.welcome = welcome;
                        let _ = opened.send(Ok(first));
                    }
                    None => {
                        let _ = replies_tx.send(Reply {
                            request: 0,
                            body: ReplyBody::DatabaseOpened {
                                path: path.clone(),
                                tasks: store.tasks.len(),
                                previous: None,
                            },
                        });
                    }
                }
                let launch_config = launch.config;
                let mut actor = Actor::new(
                    store,
                    launch.database,
                    launch_config,
                    replies_tx,
                    sync_wake,
                    launch.storage,
                );
                actor.database = path;
                actor.config = config;
                actor.welcome = welcome;
                actor.spawn_background_sync();
                actor.send_list().await;
                actor.run(requests_rx).await;
            });
        })?;
    Ok(
        move || match opened_rx.recv().context("the store thread stopped")? {
            Ok(first) => {
                let mut session = Session::new_with_selection(first, requests_tx, selected);
                session.database = database;
                Ok((session, replies_rx))
            }
            Err((path, message)) => {
                let mut session = Session::new(Snapshot::default(), requests_tx);
                session.database_failed(path, message);
                Ok((session, replies_rx))
            }
        },
    )
}

/// Waits, with no database open, for the view to name one to try.
async fn recovery_target(
    requests: &mut mpsc::UnboundedReceiver<Request>,
    launch: &Launch,
    replies: &mpsc::UnboundedSender<Reply>,
) -> Option<PathBuf> {
    while let Some(request) = requests.recv().await {
        match request.command {
            Command::OpenDatabase(path) if path == launch.database => return Some(path),
            Command::OpenDatabase(path) => match sync_ops::check_database_path(&path) {
                Ok(()) => return Some(path),
                Err(error) => {
                    let _ = replies.send(Reply {
                        request: request.id,
                        body: ReplyBody::DatabaseFailed(path, error.to_string()),
                    });
                }
            },
            Command::Restore(archive) => {
                match sync_ops::restore_to_new(&launch.database, &launch.config, &archive).await {
                    Ok(path) => return Some(path),
                    Err(error) => {
                        let _ = replies.send(Reply {
                            request: request.id,
                            body: ReplyBody::Notice(failed("Could not restore", &error)),
                        });
                    }
                }
            }
            Command::Quit => {
                let _ = replies.send(Reply {
                    request: request.id,
                    body: ReplyBody::Stopped,
                });
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use aven_core::choices::TaskPriority;
    use aven_core::recurrence::RecurrenceSeriesState;
    use chrono::NaiveDate;

    #[tokio::test]
    async fn a_quit_waits_for_the_running_round_and_replies_last() {
        let (_directory, mut actor, mut replies) = actor("Keep").await;
        let finished = Arc::new(AtomicBool::new(false));
        let round = finished.clone();
        actor.background_sync = Some(tokio::spawn(async move {
            sleep(Duration::from_millis(50)).await;
            round.store(true, Ordering::SeqCst);
        }));
        actor.quit().await;
        assert!(finished.load(Ordering::SeqCst));
        let reply = replies.recv().await.unwrap();
        assert!(matches!(reply.body, ReplyBody::Stopped));
    }

    #[tokio::test]
    async fn a_quit_leaves_a_round_that_outlasts_its_budget() {
        let (_directory, mut actor, mut replies) = actor("Keep").await;
        actor.background_sync = Some(tokio::spawn(sleep(Duration::from_secs(60))));
        tokio::time::timeout(QUIT_SYNC_BUDGET + Duration::from_secs(2), actor.quit())
            .await
            .expect("the quit did not wait for the round");
        let reply = replies.recv().await.unwrap();
        assert!(matches!(reply.body, ReplyBody::Stopped));
    }

    #[tokio::test]
    async fn a_quit_stops_background_sync_between_rounds() {
        let (directory, mut actor, _replies) = actor("Keep").await;
        actor.config = AppConfig::default();
        let (replies, _received) = mpsc::unbounded_channel();
        actor.background_sync = Some(tokio::spawn(run_sync(
            actor.store.database(),
            AppConfig::default(),
            Some(Arc::new(
                aven_core::sync::client::keys::FileProtectedStorage::new(
                    directory.path().join("keys"),
                ),
            )),
            Arc::new(Notify::new()),
            actor.sync_stop.clone(),
            replies,
        )));
        tokio::time::timeout(Duration::from_secs(5), actor.quit())
            .await
            .expect("sync stopped");
    }

    #[tokio::test]
    async fn the_last_view_comes_back_and_what_is_gone_is_skipped() {
        let (_directory, mut actor, mut replies) = actor("Keep").await;
        let id = actor.store.tasks[0].task.id.clone();
        actor
            .reopen_view(
                Some(TaskQuery::Open),
                Some("no-such-project".to_string()),
                false,
                Some(id.clone()),
            )
            .await;
        assert_eq!(actor.store.view_state.query, TaskQuery::Open);
        assert_eq!(actor.selected, Some(id.clone()));
        let mut selected = None;
        while let Ok(reply) = replies.try_recv() {
            if let ReplyBody::Select(id) = reply.body {
                selected = Some(id);
            }
        }
        assert_eq!(selected, Some(id));
    }

    #[tokio::test]
    async fn sync_snapshots_keep_the_server_identity() {
        let (_directory, mut actor, _replies) = actor("Keep").await;
        for server in [
            Some("https://sync.aventasks.dev"),
            Some("https://sync.example.com:8443"),
            None,
        ] {
            actor.store.sync_status.server = server.map(str::to_string);
            let snapshot = arranged_snapshot(&actor.store, Local::now(), &Arrange::default());
            assert_eq!(snapshot.sync.server.as_deref(), server);
        }
    }

    #[tokio::test]
    async fn the_byline_names_an_update_only_after_creation() {
        let (_directory, actor, _replies) = actor("Keep").await;
        let mut item = actor.store.tasks[0].clone();
        item.task.created_at = "2026-10-23T10:00:00+00:00".to_string();
        item.task.updated_at = "2026-10-23T10:02:00+00:00".to_string();
        let now = parse_local_timestamp("2026-11-02T10:00:00+00:00").unwrap();
        let fresh = byline(&item, now);
        assert_eq!(fresh.created_ago, "10d ago");
        assert_eq!(fresh.source, None);
        assert_eq!(fresh.updated_ago, None);
        assert!(fresh.created_on.starts_with("Oct 23, 2026, "));

        item.task.updated_at = "2026-11-02T07:00:00+00:00".to_string();
        assert_eq!(byline(&item, now).updated_ago.as_deref(), Some("3h ago"));
    }

    #[tokio::test]
    async fn list_rows_include_creation_and_activity_before_details_load() {
        let (_directory, mut actor, _replies) = actor("Keep").await;
        let item = &mut actor.store.tasks[0];
        item.task.created_at = "2026-10-23T10:00:00+00:00".to_string();
        item.task.updated_at = "2026-11-02T07:00:00+00:00".to_string();
        item.task.source = TaskSource::Cli;
        let now = parse_local_timestamp("2026-11-02T10:00:00+00:00").unwrap();
        let expected = detail(item, now).row;
        let snapshot = arranged_snapshot(&actor.store, now, &Arrange::default());
        let row = snapshot.row(&expected.id).unwrap();
        assert_eq!(row.byline, expected.byline);
        assert_eq!(row.byline.created_ago, "10d ago");
        assert_eq!(row.byline.updated_ago.as_deref(), Some("3h ago"));
        assert_eq!(row.byline.source.as_deref(), Some("CLI"));
        assert_eq!(row.activity, expected.activity);
        assert_eq!(
            row.activity.last(),
            Some(&("Created from CLI".to_string(), "10d ago".to_string()))
        );
    }

    #[tokio::test]
    async fn a_task_made_here_names_no_source() {
        let (_directory, actor, _replies) = actor("Keep").await;
        let item = &actor.store.tasks[0];
        let lines = activity(item, Local::now());
        assert_eq!(lines.last().unwrap().0, "Created");
    }

    fn draft(title: &str) -> TaskDraft {
        TaskDraft {
            metadata: Vec::new(),
            title: title.to_string(),
            description: String::new(),
            project: None,
            status: Some("todo".to_string()),
            priority: "none".to_string(),
            source: TaskSource::Unknown,
            labels: Vec::new(),
            available_at: None,
            due_on: None,
            is_epic: false,
        }
    }

    #[test]
    fn saved_views_validate_query_and_layout() {
        for (name, query, columns) in [
            ("Open", TaskQuery::Open, true),
            ("Upcoming", TaskQuery::Upcoming, false),
            ("removed-view", TaskQuery::Queue, false),
        ] {
            let state = initial_view(Some(&SavedView {
                view: name.to_string(),
                project: Some("work".to_string()),
                columns: true,
                selected: None,
            }));
            assert_eq!(state.query, query);
            assert_eq!(state.scope, TaskScope::Project("work".to_string()));
            assert_eq!(state.layout == TaskLayout::Columns, columns);
        }
    }

    #[test]
    fn startup_loads_the_saved_view_before_exposing_a_session() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("db.sqlite");
        let selected = runtime.block_on(async {
            let mut store = open_store(&path, &AppConfig::default()).await.unwrap();
            store.create_task(draft("Not in Done"), None).await.unwrap();
            let mut done = draft("Previously completed");
            done.status = Some("done".to_string());
            store.create_task(done, None).await.unwrap();
            let mut done = draft("Another completed task");
            done.status = Some("done".to_string());
            store.create_task(done, None).await.unwrap();
            store
                .show_view_restoring(TaskQuery::Done, &SelectionRestore::Default)
                .await
                .unwrap();
            store.tasks[1].task.id.clone()
        });
        let finish = prepare(
            runtime.handle().clone(),
            Launch {
                database: path,
                config: AppConfig::default(),
                storage: None,
            },
            Some(SavedView {
                view: "Done".to_string(),
                project: Some("deleted-project".to_string()),
                columns: false,
                selected: Some(selected.to_string()),
            }),
        )
        .unwrap();
        let (mut session, mut replies) = finish().unwrap();
        assert!(session.database_error.is_none());
        assert_eq!(session.snapshot.view.query, TaskQuery::Done);
        assert_eq!(session.snapshot.view.project, None);
        assert_eq!(session.snapshot.tasks.len(), 2);
        assert!(
            session
                .snapshot
                .tasks
                .iter()
                .all(|task| task.status == TaskStatus::Done)
        );
        assert_ne!(session.snapshot.tasks[0].id, selected);
        assert_eq!(session.selected, Some(selected));
        session.quit();
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let reply = replies.recv().await.expect("store replies before stopping");
                    if matches!(reply.body, ReplyBody::Stopped) {
                        break;
                    }
                    session.apply(reply);
                }
            })
            .await
            .unwrap();
        });
        assert_eq!(session.snapshot.view.query, TaskQuery::Done);
    }

    #[test]
    fn preparing_does_not_wait_for_the_database_open_lock() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("db.sqlite");
        let lock =
            std::fs::File::create(directory.path().join("db.sqlite.aven-open.lock")).unwrap();
        lock.lock().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let handle = runtime.handle().clone();
        let (sent, received) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            assert!(
                sent.send(prepare(
                    handle,
                    Launch {
                        database: path,
                        config: AppConfig::default(),
                        storage: None,
                    },
                    None
                ))
                .is_ok()
            );
        });
        let prepared = received.recv_timeout(Duration::from_secs(2));
        drop(lock);
        let (mut session, mut replies) = prepared
            .expect("native initialization can proceed while SQLite is locked")
            .unwrap()()
        .unwrap();
        thread.join().unwrap();
        assert!(session.database_error.is_none());
        session.quit();
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), async {
                while !matches!(replies.recv().await.unwrap().body, ReplyBody::Stopped) {}
            })
            .await
            .unwrap();
        });
    }

    #[test]
    fn a_damaged_database_opens_the_gate_and_stays_as_it_was() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("db.sqlite");
        std::fs::write(&path, "not a database, and not empty either").unwrap();
        let before = std::fs::read(&path).unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let launch = Launch {
            database: path.clone(),
            config: AppConfig::default(),
            storage: Some(Arc::new(
                aven_core::sync::client::keys::FileProtectedStorage::new(
                    directory.path().join("keys"),
                ),
            )),
        };
        let (mut session, mut replies) = start(runtime.handle().clone(), launch).unwrap();
        assert_eq!(session.database_error.as_ref().unwrap().path, path);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        session.quit();
        let reply = runtime.block_on(replies.recv()).unwrap();
        assert!(matches!(reply.body, ReplyBody::Stopped));
    }

    #[tokio::test]
    async fn settings_edits_refuse_a_bad_digest_time() {
        let (_directory, mut actor, mut replies) = actor("One").await;
        let reminders = Reminders {
            digest_time: "25:00".to_string(),
            ..Reminders::default()
        };
        run(
            &mut actor,
            Command::Settings(SettingsEdit::Reminders(reminders)),
        )
        .await;
        assert!(
            bodies(&mut replies)
                .iter()
                .any(|body| matches!(body, ReplyBody::Notice(_)))
        );
        assert_ne!(actor.config.reminders.digest_time, "25:00");
    }

    #[tokio::test]
    async fn a_new_project_takes_its_prefix_or_leaves_nothing() {
        let (_directory, mut actor, mut replies) = actor("One").await;
        let new = |name: &str, prefix: &str| {
            Command::Settings(SettingsEdit::NewProject {
                name: name.to_string(),
                prefix: prefix.to_string(),
            })
        };
        run(&mut actor, new("Website", "WEB")).await;
        run(&mut actor, new("Mobile", "")).await;
        let prefix_of = |actor: &Actor, name: &str| {
            let project = actor
                .store
                .projects
                .iter()
                .find(|project| project.name == name);
            project.map(|project| project.prefix.clone())
        };
        assert_eq!(prefix_of(&actor, "Website").as_deref(), Some("WEB"));
        assert!(prefix_of(&actor, "Mobile").is_some());
        bodies(&mut replies);
        run(&mut actor, new("Website", "ZZ")).await;
        run(&mut actor, new("Other", "WEB")).await;
        run(&mut actor, new("Bad", "!")).await;
        assert_eq!(prefix_of(&actor, "Website").as_deref(), Some("WEB"));
        assert_eq!(prefix_of(&actor, "Other"), None);
        assert_eq!(prefix_of(&actor, "Bad"), None);
    }

    #[tokio::test]
    async fn deleting_a_project_refreshes_the_sidebar_settings_and_scope() {
        let (_directory, mut actor, mut replies) = actor("Keep").await;
        for name in ["Website", "Empty"] {
            run(
                &mut actor,
                Command::Settings(SettingsEdit::NewProject {
                    name: name.to_string(),
                    prefix: String::new(),
                }),
            )
            .await;
        }
        let mut task = draft("Keep project history");
        task.project = Some("website".to_string());
        actor.store.create_task(task, None).await.unwrap();
        run(
            &mut actor,
            Command::ShowProject(Some("website".to_string())),
        )
        .await;
        bodies(&mut replies);

        run(
            &mut actor,
            Command::Settings(SettingsEdit::DeleteProject("website".to_string())),
        )
        .await;
        let seen = bodies(&mut replies);
        let list = lists(&seen).pop().unwrap();
        assert_eq!(list.view.project, None);
        assert!(list.tasks.iter().any(|row| row.title == "Keep"));
        let retained = list
            .tasks
            .iter()
            .find(|row| row.title == "Keep project history")
            .unwrap();
        assert_eq!(retained.project_key, "website");
        assert!(!retained.deleted);
        assert!(!list.projects.iter().any(|project| project.key == "website"));
        assert!(list.projects.iter().any(|project| project.key == "empty"));
        let settings = seen
            .iter()
            .find_map(|body| match body {
                ReplyBody::Settings(settings) => Some(settings),
                _ => None,
            })
            .unwrap();
        assert!(
            !settings
                .projects
                .iter()
                .any(|project| project.key == "website")
        );

        run(
            &mut actor,
            Command::Settings(SettingsEdit::DeleteProject("empty".to_string())),
        )
        .await;
        assert!(
            !actor
                .store
                .projects
                .iter()
                .any(|project| project.key == "empty")
        );
        assert_eq!(actor.store.view_state.scope, TaskScope::Workspace);
        assert!(
            actor
                .edit_settings(SettingsEdit::DeleteProject("missing".to_string()))
                .await
                .is_err()
        );
        assert!(
            actor
                .store
                .tasks
                .iter()
                .any(|item| item.task.title == "Keep")
        );
    }

    #[tokio::test]
    async fn a_restore_waits_for_a_running_sync_operation() {
        let (directory, mut actor, mut replies) = actor("One").await;
        actor.sync_busy.store(true, Ordering::SeqCst);
        let archive = directory.path().join("backup.sqlite");
        run(&mut actor, Command::Restore(archive)).await;
        let found = bodies(&mut replies);
        assert!(
            found
                .iter()
                .any(|body| matches!(body, ReplyBody::Notice(toast) if toast.title == SYNC_BUSY))
        );
        let names = std::fs::read_dir(directory.path()).unwrap();
        let names: Vec<_> = names.map(|entry| entry.unwrap().file_name()).collect();
        assert!(
            !names
                .iter()
                .any(|name| name.to_string_lossy().contains("restored"))
        );
    }

    #[tokio::test]
    async fn settings_edits_reach_the_store() {
        let (_directory, mut actor, mut replies) = actor("One").await;
        let edit = SettingsEdit::NewLabel("Needs Review".to_string());
        run(&mut actor, Command::Settings(edit)).await;
        bodies(&mut replies);
        let data = actor.settings_data().await.unwrap();
        assert!(
            data.labels.iter().any(|(name, _)| name == "needs-review"),
            "{:?}",
            data.labels
        );
    }

    async fn actor(title: &str) -> (tempfile::TempDir, Actor, mpsc::UnboundedReceiver<Reply>) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("db.sqlite");
        let mut store = open_store(&path, &AppConfig::default()).await.unwrap();
        store.create_task(draft(title), None).await.unwrap();
        let (replies, received) = mpsc::unbounded_channel();
        let actor = Actor::new(
            store,
            path,
            AppConfig::default(),
            replies,
            Arc::new(Notify::new()),
            Some(Arc::new(
                aven_core::sync::client::keys::FileProtectedStorage::new(
                    directory.path().join("keys"),
                ),
            )),
        );
        (directory, actor, received)
    }

    fn bodies(replies: &mut mpsc::UnboundedReceiver<Reply>) -> Vec<ReplyBody> {
        std::iter::from_fn(|| replies.try_recv().ok())
            .map(|reply| reply.body)
            .collect()
    }

    fn status_of(actor: &Actor, title: &str) -> TaskStatus {
        let item = actor
            .store
            .tasks
            .iter()
            .find(|item| item.task.title == title);
        item.expect("a task with that title").task.status
    }

    fn saved_message(bodies: &[ReplyBody]) -> Option<Toast> {
        bodies.iter().find_map(|body| match body {
            ReplyBody::Saved(toast) => Some(toast.clone()),
            _ => None,
        })
    }

    fn new_task(title: &str, schedule: &str) -> NewTask {
        NewTask {
            title: title.to_string(),
            schedule: schedule.to_string(),
            priority: "none".to_string(),
            ..NewTask::default()
        }
    }

    #[tokio::test]
    async fn a_typed_schedule_saves_the_dates_it_means() {
        let (_directory, mut actor, _replies) = actor("One").await;
        actor
            .create(&new_task("Later", "tomorrow at 9am"))
            .await
            .unwrap();
        actor
            .create(&new_task("Dated", "due next friday"))
            .await
            .unwrap();
        let id = task_id(&actor, "Dated");
        let mut tasks = actor.store.tasks.iter();
        let task = &tasks.find(|item| item.task.id == id).unwrap().task;
        let due = task.due_on.clone().unwrap();
        assert!(due.parse::<NaiveDate>().is_ok(), "{due}");
    }

    #[tokio::test]
    async fn bulk_writes_only_the_ids_it_was_given() {
        let (_directory, mut actor, mut replies) = actor("One").await;
        for title in ["Two", "Three"] {
            actor.store.create_task(draft(title), None).await.unwrap();
        }
        let ids = vec![task_id(&actor, "One"), task_id(&actor, "Three")];
        run(
            &mut actor,
            Command::Bulk(ids, Bulk::Priority(TaskPriority::High)),
        )
        .await;
        let priority = |actor: &Actor, title: &str| {
            let item = actor
                .store
                .tasks
                .iter()
                .find(|item| item.task.title == title);
            item.unwrap().task.priority
        };
        assert_eq!(priority(&actor, "One"), TaskPriority::High);
        assert_eq!(priority(&actor, "Two"), TaskPriority::None);
        assert_eq!(priority(&actor, "Three"), TaskPriority::High);
        bodies(&mut replies);
    }

    #[tokio::test]
    async fn bulk_refuses_when_a_marked_task_is_gone() {
        let (_directory, mut actor, mut replies) = actor("One").await;
        let ids = vec![task_id(&actor, "One"), TaskId::new()];
        run(
            &mut actor,
            Command::Bulk(ids, Bulk::Status(TaskStatus::Done)),
        )
        .await;
        assert_eq!(status_of(&actor, "One"), TaskStatus::Todo);
        let bodies = bodies(&mut replies);
        assert!(format!("{bodies:?}").contains("A marked task is no longer available"));
    }

    #[tokio::test]
    async fn a_template_edit_leaves_the_current_task() {
        let (_directory, mut actor, mut replies) = actor("Other").await;
        let mut task = new_task("Review", "every friday at 09:00");
        task.description = "Old".to_string();
        run(&mut actor, Command::Create(Box::new(task))).await;
        run(
            &mut actor,
            Command::ShowRecurring(RecurrenceSeriesLifecycleFilter::All),
        )
        .await;
        let series = actor.store.recurrence_series[0].series.id.clone();
        run(
            &mut actor,
            Command::SetTemplate(series.clone(), "New".to_string()),
        )
        .await;

        let detail = actor.series_detail(&series).await.unwrap();
        assert_eq!(detail.description, "New");
        let current = detail.current.expect("the first occurrence").id;
        let pool = aven_core::test_support::pool(&actor.store.database());
        let description: String = sqlx::query_scalar("SELECT description FROM tasks WHERE id = ?")
            .bind(&current)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(description, "Old");
        bodies(&mut replies);
    }

    #[tokio::test]
    async fn a_series_pauses_resumes_and_stops_with_the_current_task_kept() {
        let (_directory, mut actor, mut replies) = actor("Other").await;
        let task = new_task("Review", "every friday at 09:00");
        run(&mut actor, Command::Create(Box::new(task))).await;
        run(
            &mut actor,
            Command::ShowRecurring(RecurrenceSeriesLifecycleFilter::All),
        )
        .await;
        let series = actor.store.recurrence_series[0].series.id.clone();
        let state = |detail: SeriesDetail| detail.state;

        run(&mut actor, Command::PauseSeries(series.clone())).await;
        let paused = actor.series_detail(&series).await.unwrap();
        assert_eq!(state(paused), RecurrenceSeriesState::Paused);
        run(&mut actor, Command::ResumeSeries(series.clone())).await;
        let resumed = actor.series_detail(&series).await.unwrap();
        assert_eq!(state(resumed), RecurrenceSeriesState::Active);
        run(&mut actor, Command::StopSeries(series.clone(), false)).await;
        let stopped = actor.series_detail(&series).await.unwrap();
        assert_eq!(state(stopped), RecurrenceSeriesState::Stopped);
        bodies(&mut replies);
    }

    #[tokio::test]
    async fn a_replacement_starts_from_the_series_rule_time_and_due_policy() {
        let (_directory, mut actor, mut replies) = actor("Other").await;
        let task = new_task("Review", "every Friday at 14:30, no due");
        run(&mut actor, Command::Create(Box::new(task))).await;
        run(
            &mut actor,
            Command::ShowRecurring(RecurrenceSeriesLifecycleFilter::All),
        )
        .await;
        let series = actor.store.recurrence_series[0].series.id.clone();
        let detail = actor.series_detail(&series).await.unwrap();
        assert_eq!(detail.rule, "weekly on fri");
        assert_eq!(detail.schedule, "Every Friday at 14:30, no due");

        let mut task = new_task("Replacement", &detail.schedule);
        task.replaces = Some(series);
        run(&mut actor, Command::Create(Box::new(task))).await;
        let bodies = bodies(&mut replies);
        assert!(!format!("{bodies:?}").contains("A replacement needs a repeating schedule"));
        let replacement = actor
            .store
            .recurrence_series
            .iter()
            .find(|row| row.series.title == "Replacement")
            .expect("the replacement series exists");
        assert_eq!(
            replacement.series.available_local_time,
            chrono::NaiveTime::from_hms_opt(14, 30, 0)
        );
        assert_eq!(replacement.series.due_policy, RecurrenceDuePolicy::None);
    }

    #[tokio::test]
    async fn a_replacement_without_a_repeating_schedule_is_refused() {
        let (_directory, mut actor, mut replies) = actor("Other").await;
        let task = new_task("Review", "every friday at 09:00");
        run(&mut actor, Command::Create(Box::new(task))).await;
        run(
            &mut actor,
            Command::ShowRecurring(RecurrenceSeriesLifecycleFilter::All),
        )
        .await;
        let series = actor.store.recurrence_series[0].series.id.clone();
        let mut task = new_task("Replacement", "tomorrow");
        task.replaces = Some(series.clone());
        run(&mut actor, Command::Create(Box::new(task))).await;
        let bodies = bodies(&mut replies);
        assert!(format!("{bodies:?}").contains("A replacement needs a repeating schedule"));
    }

    #[tokio::test]
    async fn an_epic_gains_and_loses_children_and_says_what_stays_open() {
        let (_directory, mut actor, mut replies) = actor("Loose").await;
        let epic = TaskDraft {
            is_epic: true,
            ..draft("Launch")
        };
        actor.store.create_task(epic, None).await.unwrap();
        actor.store.create_task(draft("Other"), None).await.unwrap();
        let (epic, loose) = (task_id(&actor, "Launch"), task_id(&actor, "Loose"));
        let other = task_id(&actor, "Other");
        run(&mut actor, Command::AddChild(epic.clone(), loose.clone())).await;
        run(&mut actor, Command::AddChild(epic.clone(), other.clone())).await;
        run(&mut actor, Command::RemoveChild(other)).await;
        run(
            &mut actor,
            Command::CreateChild(epic.clone(), "Fresh".to_string()),
        )
        .await;
        bodies(&mut replies);

        run(&mut actor, Command::SetStatus(epic, TaskStatus::Done)).await;
        let toast = saved_message(&bodies(&mut replies)).unwrap();
        assert_eq!(toast.title, "Marked done");
        assert_eq!(
            toast.detail.as_deref(),
            Some("Launch still has 2 open tasks")
        );
    }

    #[tokio::test]
    async fn resolving_a_conflict_writes_one_field_with_the_chosen_value() {
        let (_directory, mut actor, mut replies) = actor("Stored").await;
        let id = task_id(&actor, "Stored");
        let pool = aven_core::test_support::pool(&actor.store.database());
        for (field, local, remote) in [
            ("title", "Local title", "Remote title"),
            ("description", "Local body", "Remote body"),
        ] {
            sqlx::query(
                "INSERT INTO conflicts(task_id, field, base_version, local_value, remote_value,
                 local_change_id, remote_change_id, variant_a, variant_b, created_at, resolved)
                 VALUES (?, ?, NULL, ?, ?, NULL, ?, 'a', 'b', ?, 0)",
            )
            .bind(&id)
            .bind(field)
            .bind(local)
            .bind(remote)
            .bind(aven_core::ids::new_id())
            .bind(aven_core::ids::now())
            .execute(&pool)
            .await
            .unwrap();
        }
        actor.store.refresh(None).await.unwrap();
        let item = actor.store.tasks[0].clone();
        let targets = actor.store.conflict_targets_for(&item).await.unwrap();
        assert_eq!(targets.len(), 2);
        let title = targets
            .iter()
            .find(|target| target.field == "title")
            .unwrap();
        let command = Command::ResolveConflict(Box::new(title.clone()), "Remote title".to_string());
        run(&mut actor, command).await;

        let item = actor.store.tasks[0].clone();
        assert_eq!(item.task.title, "Remote title");
        let left = actor.store.conflict_targets_for(&item).await.unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].field, "description");
        bodies(&mut replies);
    }

    #[test]
    fn a_gist_goes_to_the_program_as_a_secret_and_reports_its_failure() {
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("gh");
        let log = directory.path().join("args");
        let body = format!(
            "#!/bin/sh\necho \"$@\" > {}\ncat >> {}\necho https://gist.example/1\n",
            log.display(),
            log.display()
        );
        std::fs::write(&script, body).unwrap();
        let mut permissions = std::fs::metadata(&script).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        std::fs::set_permissions(&script, permissions).unwrap();

        let url = create_secret_gist(script.as_os_str(), "# Report", "a.md", "Desc").unwrap();
        assert_eq!(url, "https://gist.example/1");
        let seen = std::fs::read_to_string(&log).unwrap();
        assert!(seen.starts_with("gist create --filename a.md --desc Desc -"));
        assert!(!seen.contains("--public"));
        assert!(seen.contains("# Report"));

        std::fs::write(
            &script,
            "#!/bin/sh\ncat > /dev/null\necho denied >&2\nexit 1\n",
        )
        .unwrap();
        let error = create_secret_gist(script.as_os_str(), "x", "a.md", "Desc").unwrap_err();
        assert_eq!(error.to_string(), "denied");
    }

    #[tokio::test]
    async fn a_refresh_failure_after_a_save_reports_saved() {
        let (_directory, mut actor, mut replies) = actor("Stored").await;
        let id = actor.store.tasks[0].task.id.clone();
        actor.store.fail_next_refresh();
        let command = Command::SetTitle(id, "Renamed".to_string());
        actor.handle(Request { id: 1, command }).await;

        let bodies = bodies(&mut replies);
        assert!(
            matches!(bodies[..], [ReplyBody::SavedRefreshFailed(_)]),
            "{bodies:?}"
        );
        actor.store.refresh(None).await.unwrap();
        assert_eq!(actor.store.tasks[0].task.title, "Renamed");
    }

    #[tokio::test]
    async fn a_finished_task_stays_until_selection_moves_on() {
        let (_directory, mut actor, mut replies) = actor("Stored").await;
        actor.store.create_task(draft("Next"), None).await.unwrap();
        let id = |store: &TuiStore, title: &str| {
            let item = store.tasks.iter().find(|item| item.task.title == title);
            item.expect("a task with that title").task.id.clone()
        };
        let (stored, next) = (id(&actor.store, "Stored"), id(&actor.store, "Next"));
        actor.selected = Some(stored.clone());
        let command = Command::SetStatus(stored.clone(), TaskStatus::Done);
        actor.handle(Request { id: 1, command }).await;
        actor.outside_change().await;
        assert!(actor.store.tasks.iter().any(|item| item.task.id == stored));

        actor.selected = Some(next);
        actor.outside_change().await;
        assert!(actor.store.tasks.iter().all(|item| item.task.id != stored));
        bodies(&mut replies);
    }

    #[tokio::test]
    async fn a_new_task_ignores_the_selected_tasks_project() {
        let (_directory, mut actor, mut replies) = actor("Stored").await;
        let elsewhere = TaskDraft {
            project: Some("other".to_string()),
            ..draft("Elsewhere")
        };
        actor.store.create_task(elsewhere, None).await.unwrap();
        let find = |store: &TuiStore, title: &str| {
            let item = store.tasks.iter().find(|item| item.task.title == title);
            item.expect("a task with that title").task.clone()
        };
        actor.selected = Some(find(&actor.store, "Elsewhere").id);
        let command = Command::Create(Box::new(NewTask::titled("Second")));
        actor.handle(Request { id: 1, command }).await;

        assert!(
            bodies(&mut replies)
                .iter()
                .any(|body| matches!(body, ReplyBody::Created(_)))
        );
        let first = actor.store.projects[0].key.clone();
        assert_ne!(first, "other");
        assert_eq!(find(&actor.store, "Second").project_key, first);
    }

    #[tokio::test]
    async fn a_refresh_failure_after_a_create_reports_saved_once() {
        let (_directory, mut actor, mut replies) = actor("Stored").await;
        actor.store.fail_next_refresh();
        let command = Command::Create(Box::new(NewTask::titled("Second")));
        actor.handle(Request { id: 1, command }).await;

        let bodies = bodies(&mut replies);
        let saved = bodies
            .iter()
            .filter(|body| matches!(body, ReplyBody::SavedRefreshFailed(_)));
        assert_eq!(saved.count(), 1, "{bodies:?}");
        assert!(
            !bodies
                .iter()
                .any(|body| matches!(body, ReplyBody::SaveFailed(..)))
        );
        actor.store.refresh(None).await.unwrap();
        assert_eq!(actor.store.tasks.len(), 2);
    }

    #[tokio::test]
    async fn a_write_that_cannot_run_reports_failure_and_can_be_retried() {
        let (_directory, mut actor, mut replies) = actor("Stored").await;
        let command = Command::SetTitle(TaskId::new(), "Nowhere".to_string());
        actor
            .handle(Request {
                id: 1,
                command: command.clone(),
            })
            .await;

        let bodies = bodies(&mut replies);
        assert!(
            matches!(&bodies[0], ReplyBody::SaveFailed(failed, _) if *failed == command),
            "{bodies:?}"
        );
        assert!(matches!(bodies[1], ReplyBody::List(_)), "{bodies:?}");
    }

    #[tokio::test]
    async fn an_outside_write_is_seen_by_data_version() {
        let (_directory, mut actor, _replies) = actor("Stored").await;
        let path = actor.database.clone();
        let mut writes = OutsideWrites::open(&path).await.unwrap();
        assert!(!writes.changed().await.unwrap());
        actor
            .store
            .create_task(draft("Second"), None)
            .await
            .unwrap();
        assert!(writes.changed().await.unwrap());
        assert!(!writes.changed().await.unwrap());
    }

    async fn run(actor: &mut Actor, command: Command) {
        let id = actor.handled + 1;
        actor.handle(Request { id, command }).await;
    }

    #[tokio::test]
    async fn launcher_jobs_start_apart_from_the_store() {
        let (_dir, mut actor, mut replies) = actor("Existing").await;
        run(&mut actor, Command::LoadLauncher).await;
        let catalog = bodies(&mut replies)
            .into_iter()
            .find_map(|body| match body {
                ReplyBody::Launcher(catalog) => Some(catalog),
                _ => None,
            });
        let catalog = catalog.expect("the launcher's catalog");
        let place = catalog
            .place(&catalog.active)
            .expect("the active workspace");
        assert!(!place.projects.is_empty());

        let job = |id, mode| LaunchJob {
            id,
            mode,
            workspace: catalog.active.clone(),
            project: None,
            labels: Vec::new(),
            text: "Buy milk".to_string(),
            images: Vec::new(),
        };
        run(
            &mut actor,
            Command::Launch(Box::new(job(1, LaunchMode::Create))),
        )
        .await;
        assert!(matches!(
            bodies(&mut replies)[..],
            [ReplyBody::Launch(LaunchEvent::Refused(1, _))]
        ));

        let plain = LaunchMode::Plain(launch::PlainTask {
            title: "Buy milk".to_string(),
            description: String::new(),
            priority: TaskPriority::None,
        });
        run(&mut actor, Command::Launch(Box::new(job(2, plain)))).await;
        let mut events = Vec::new();
        while events.len() < 2 {
            let reply = tokio::time::timeout(Duration::from_secs(5), replies.recv())
                .await
                .expect("the job should finish")
                .unwrap();
            if let ReplyBody::Launch(event) = reply.body {
                events.push(event);
            }
        }
        assert_eq!(events[0], LaunchEvent::Accepted(2));
        let LaunchEvent::Created(created) = &events[1] else {
            panic!("expected a created task, got {:?}", events[1]);
        };
        assert!(created.plain);
        assert_eq!(created.title, "Buy milk");
        assert!(actor.undoable.is_empty(), "launcher tasks stay out of undo");
    }

    fn task_id(actor: &Actor, title: &str) -> TaskId {
        let item = actor
            .store
            .tasks
            .iter()
            .find(|item| item.task.title == title);
        item.expect("a task with that title").task.id.clone()
    }

    fn lists(bodies: &[ReplyBody]) -> Vec<&Snapshot> {
        bodies
            .iter()
            .filter_map(|body| match body {
                ReplyBody::List(list) => Some(&**list),
                _ => None,
            })
            .collect()
    }

    fn detail_in(bodies: Vec<ReplyBody>) -> Box<Detail> {
        let details = bodies.into_iter().filter_map(|body| match body {
            ReplyBody::Detail(detail) => Some(detail),
            _ => None,
        });
        details.into_iter().next().expect("no detail")
    }

    fn last_toast(bodies: &[ReplyBody]) -> Option<(Toast, bool)> {
        bodies.iter().rev().find_map(|body| match body {
            ReplyBody::Saved(toast) => Some((toast.clone(), toast.undo)),
            _ => None,
        })
    }

    #[tokio::test]
    async fn views_filters_order_and_grouping_reach_the_list() {
        let (_directory, mut actor, mut replies) = actor("Stored").await;
        actor
            .store
            .create_task(
                TaskDraft {
                    labels: vec!["bug".to_string()],
                    priority: "high".to_string(),
                    ..draft("Labelled")
                },
                None,
            )
            .await
            .unwrap();
        run(&mut actor, Command::ShowView(TaskQuery::Open)).await;
        let bodies_seen = bodies(&mut replies);
        let list = lists(&bodies_seen)[0];
        assert_eq!(list.view.query, TaskQuery::Open);
        assert_eq!(list.ordered().count(), 2);
        assert_eq!(list.groups[0].name, "Todo");

        run(&mut actor, Command::FilterLabel(Some("bug".to_string()))).await;
        let bodies_seen = bodies(&mut replies);
        let list = lists(&bodies_seen)[0];
        assert_eq!(list.view.label.as_deref(), Some("bug"));
        assert_eq!(list.ordered().count(), 1);

        run(&mut actor, Command::ClearFilters).await;
        run(&mut actor, Command::SetGroup(GroupBy::Priority)).await;
        let bodies_seen = bodies(&mut replies);
        let list = lists(&bodies_seen).pop().unwrap();
        let names: Vec<_> = list
            .groups
            .iter()
            .map(|group| group.name.as_str())
            .collect();
        assert_eq!(names, ["High", "None"]);

        run(&mut actor, Command::SetOrder(ViewOrder::Title)).await;
        run(&mut actor, Command::SetGroup(GroupBy::None)).await;
        let bodies_seen = bodies(&mut replies);
        let list = lists(&bodies_seen).pop().unwrap();
        let titles: Vec<_> = list
            .ordered()
            .map(|row| list.tasks[row].title.as_str())
            .collect();
        assert_eq!(titles, ["Labelled", "Stored"]);
    }

    #[tokio::test]
    async fn columns_stay_with_their_view_and_list_every_lane() {
        let (_directory, mut actor, mut replies) = actor("Stored").await;
        run(&mut actor, Command::ShowView(TaskQuery::Open)).await;
        run(&mut actor, Command::SetColumns(true)).await;
        let bodies_seen = bodies(&mut replies);
        let list = lists(&bodies_seen).pop().unwrap();
        assert!(list.view.columns);
        let lanes: Vec<_> = list
            .groups
            .iter()
            .map(|group| group.name.as_str())
            .collect();
        assert_eq!(lanes, ["Inbox", "Backlog", "Todo", "Active", "Done"]);

        run(&mut actor, Command::ShowView(TaskQuery::Queue)).await;
        assert_eq!(actor.store.view_state.layout, TaskLayout::List);
        run(&mut actor, Command::ShowView(TaskQuery::Open)).await;
        assert_eq!(actor.store.view_state.layout, TaskLayout::Columns);
        bodies(&mut replies);
    }

    #[tokio::test]
    async fn ordering_the_queue_moves_to_open() {
        let (_directory, mut actor, _replies) = actor("Stored").await;
        run(&mut actor, Command::ShowView(TaskQuery::Queue)).await;

        run(&mut actor, Command::SetOrder(ViewOrder::Title)).await;

        assert_eq!(actor.store.view_state.query, TaskQuery::Open);
        assert_eq!(actor.store.view_state.order, TaskOrder::Title);
    }

    #[tokio::test]
    async fn a_new_order_starts_from_its_own_direction() {
        let (_directory, mut actor, _replies) = actor("Stored").await;
        run(&mut actor, Command::ShowView(TaskQuery::Open)).await;

        run(&mut actor, Command::SetOrder(ViewOrder::Updated)).await;
        run(&mut actor, Command::SetOrder(ViewOrder::Title)).await;
        assert_eq!(actor.store.sort_direction_label(), "asc");

        run(&mut actor, Command::ReverseOrder).await;
        let view = view_info(&actor.store, &actor.arrange);
        assert_eq!(view.order, ViewOrder::Title);
        assert!(view.descending);
    }

    #[tokio::test]
    async fn the_metadata_filter_keeps_rows_with_or_without_the_field() {
        let (_directory, mut actor, mut replies) = actor("Stored").await;
        let tagged = TaskDraft {
            metadata: metadata_inputs(&[("owner".to_string(), "Ada".to_string())]),
            ..draft("Tagged")
        };
        actor.store.create_task(tagged, None).await.unwrap();
        let id = task_id(&actor, "Tagged");
        let stored = task_id(&actor, "Stored");
        let command = Command::SetMetadata(stored, "owner".to_string(), Some("Bo".to_string()));
        run(&mut actor, command).await;
        let command = Command::SetMetadata(id.clone(), "owner".to_string(), None);
        run(&mut actor, command).await;
        let filter = MetaFilter {
            key: "owner".to_string(),
            present: true,
        };
        run(&mut actor, Command::FilterMetadata(Some(filter))).await;
        let bodies_seen = bodies(&mut replies);
        let list = lists(&bodies_seen).pop().unwrap();
        let rows: Vec<_> = list
            .ordered()
            .map(|row| list.tasks[row].id.clone())
            .collect();
        assert_eq!(rows, [task_id(&actor, "Stored")]);
        assert!(!rows.contains(&id));
        assert_eq!(list.fields, ["owner"]);
    }

    #[tokio::test]
    async fn switching_workspace_shows_its_tasks_and_drops_the_filters() {
        let (_directory, mut actor, mut replies) = actor("Stored").await;
        actor
            .store
            .create_workspace("Other".to_string())
            .await
            .unwrap();
        let other = actor
            .store
            .workspaces
            .iter()
            .find(|space| space.name == "Other");
        let key = other.unwrap().key.clone();
        let filter = Command::FilterPriority(Some(TaskPriority::High));
        run(&mut actor, filter).await;
        run(&mut actor, Command::SwitchWorkspace(key)).await;
        let seen = bodies(&mut replies);
        let list = lists(&seen).pop().unwrap();
        assert_eq!(list.workspace, "Other");
        assert!(list.tasks.is_empty());
        assert_eq!(list.view.priority, None);
    }

    #[tokio::test]
    async fn delete_shows_a_toast_undo_brings_it_back_and_a_filter_finds_it() {
        let (_directory, mut actor, mut replies) = actor("Stored").await;
        let id = task_id(&actor, "Stored");
        run(&mut actor, Command::SetDeleted(id.clone(), true)).await;
        let seen = bodies(&mut replies);
        let (toast, undo) = last_toast(&seen).unwrap();
        assert_eq!(toast.title, "Deleted");
        assert!(toast.subject.is_some());
        assert!(undo);
        assert!(actor.store.tasks.is_empty());

        run(&mut actor, Command::FilterDeleted(DeletedFilter::Show)).await;
        let seen = bodies(&mut replies);
        let list = lists(&seen).pop().unwrap();
        assert!(list.tasks[0].deleted);

        run(&mut actor, Command::Undo).await;
        let seen = bodies(&mut replies);
        let (toast, undo) = last_toast(&seen).unwrap();
        assert_eq!(toast.title, "Undone");
        assert_eq!(toast.detail.as_deref(), Some("Deleted"));
        assert!(toast.subject.is_some());
        assert!(!undo);
        let list = lists(&seen).pop().unwrap();
        assert!(!list.tasks[0].deleted);
    }

    #[tokio::test]
    async fn an_empty_undo_history_says_so() {
        let (_directory, mut actor, mut replies) = actor("Stored").await;
        run(&mut actor, Command::Undo).await;
        run(&mut actor, Command::Undo).await;
        let seen = bodies(&mut replies);
        let (toast, undo) = last_toast(&seen).unwrap();
        assert_eq!(toast.title, "Nothing to undo");
        assert!(!undo);
    }

    #[tokio::test]
    async fn search_finds_and_a_second_query_can_include_deleted_rows() {
        let (_directory, mut actor, mut replies) = actor("Needle one").await;
        actor.store.create_task(draft("Other"), None).await.unwrap();
        let needle = task_id(&actor, "Needle one");
        let input = SearchInput {
            text: "needle".to_string(),
            project: None,
            deleted: false,
        };
        run(&mut actor, Command::Search(input.clone())).await;
        let seen = bodies(&mut replies);
        let list = lists(&seen).pop().unwrap();
        assert_eq!(list.view.search.as_deref(), Some("needle"));
        assert_eq!(list.tasks.len(), 1);

        run(&mut actor, Command::SetDeleted(needle, true)).await;
        run(&mut actor, Command::Search(input.clone())).await;
        let seen = bodies(&mut replies);
        assert!(lists(&seen).pop().unwrap().tasks.is_empty());

        let deleted = SearchInput {
            deleted: true,
            ..input
        };
        run(&mut actor, Command::Search(deleted)).await;
        let seen = bodies(&mut replies);
        assert_eq!(lists(&seen).pop().unwrap().tasks.len(), 1);
    }

    #[tokio::test]
    async fn goto_widens_the_view_until_the_task_shows() {
        let (_directory, mut actor, mut replies) = actor("Stored").await;
        let id = task_id(&actor, "Stored");
        run(&mut actor, Command::SetStatus(id.clone(), TaskStatus::Done)).await;
        run(&mut actor, Command::ShowView(TaskQuery::Inbox)).await;
        assert!(actor.store.tasks.is_empty());
        bodies(&mut replies);

        run(&mut actor, Command::Goto(id.clone())).await;
        assert!(actor.listed(&id));
        assert_eq!(actor.store.view_state.query, TaskQuery::Done);
        let seen = bodies(&mut replies);
        assert!(seen.iter().any(|body| matches!(body, ReplyBody::Select(_))));
    }

    #[tokio::test]
    async fn goto_from_recent_actions_leaves_for_a_task_view() {
        let (_directory, mut actor, mut replies) = actor("Stored").await;
        let id = task_id(&actor, "Stored");
        run(&mut actor, Command::ShowView(TaskQuery::RecentActions)).await;
        bodies(&mut replies);

        run(&mut actor, Command::Goto(id.clone())).await;
        assert_ne!(actor.store.view_state.query, TaskQuery::RecentActions);
        assert!(actor.listed(&id));
    }

    #[tokio::test]
    async fn schedule_project_labels_links_and_notes_write_through_core() {
        let (_directory, mut actor, mut replies) = actor("Stored").await;
        actor
            .store
            .create_task(draft("Blocker"), None)
            .await
            .unwrap();
        let id = task_id(&actor, "Stored");
        let blocker = task_id(&actor, "Blocker");
        run(
            &mut actor,
            Command::SetSchedule {
                id: id.clone(),
                available_at: None,
                due_on: Some(Some("2031-05-01".to_string())),
            },
        )
        .await;
        run(
            &mut actor,
            Command::SetLabels(id.clone(), vec!["bug".to_string()]),
        )
        .await;
        run(&mut actor, Command::AddBlocker(id.clone(), blocker.clone())).await;
        run(&mut actor, Command::AddRelated(id.clone(), blocker.clone())).await;
        run(
            &mut actor,
            Command::AddNote(id.clone(), "Noted".to_string()),
        )
        .await;
        run(
            &mut actor,
            Command::SetDescription(id.clone(), "Body".to_string()),
        )
        .await;
        bodies(&mut replies);

        run(&mut actor, Command::LoadDetail(id.clone())).await;
        let seen = bodies(&mut replies);
        let detail = detail_in(seen);
        assert_eq!(detail.due_on.as_deref(), Some("2031-05-01"));
        assert_eq!(detail.row.labels, ["bug"]);
        assert_eq!(detail.blocked_by[0].id, blocker);
        assert_eq!(detail.related[0].id, blocker);
        assert_eq!(detail.description, "Body");
        let note = detail.notes[0].id.clone();

        run(
            &mut actor,
            Command::RemoveBlocker(id.clone(), blocker.clone()),
        )
        .await;
        run(
            &mut actor,
            Command::RemoveRelated(id.clone(), blocker.clone()),
        )
        .await;
        run(&mut actor, Command::DeleteNote(id.clone(), note)).await;
        bodies(&mut replies);
        run(&mut actor, Command::LoadDetail(id)).await;
        let seen = bodies(&mut replies);
        let detail = detail_in(seen);
        assert!(detail.blocked_by.is_empty() && detail.related.is_empty());
        assert!(detail.notes.is_empty());
    }

    fn png() -> Vec<u8> {
        let mut bytes = Vec::new();
        let picture = image::RgbImage::new(4, 4);
        picture
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        bytes
    }

    #[tokio::test]
    async fn an_image_lands_with_a_preview_and_changing_its_alt_keeps_the_picture() {
        let (_directory, mut actor, mut replies) = actor("Stored").await;
        let id = task_id(&actor, "Stored");
        let pending = PendingImage {
            name: "chart.png".to_string(),
            bytes: Arc::new(png()),
        };
        run(&mut actor, Command::AddImage(id.clone(), pending)).await;
        bodies(&mut replies);
        run(&mut actor, Command::LoadDetail(id.clone())).await;
        let seen = bodies(&mut replies);
        let detail = detail_in(seen);
        assert_eq!(detail.images[0].name, "chart.png");
        assert!(detail.images[0].preview.as_ref().unwrap().exists());
        let image = detail.images[0].id.clone();

        let command = Command::SetImageAlt(id.clone(), image, "A chart".to_string());
        run(&mut actor, command).await;
        bodies(&mut replies);
        run(&mut actor, Command::LoadDetail(id)).await;
        let seen = bodies(&mut replies);
        let detail = detail_in(seen);
        assert_eq!(detail.images.len(), 1);
        assert_eq!(detail.images[0].alt, "A chart");
    }

    #[tokio::test]
    async fn a_created_task_keeps_its_pasted_images() {
        let (_directory, mut actor, mut replies) = actor("Other").await;
        let mut task = NewTask::titled("Broken layout");
        task.images = vec![PendingImage {
            name: "pasted.png".to_string(),
            bytes: Arc::new(png()),
        }];
        run(&mut actor, Command::Create(Box::new(task))).await;
        bodies(&mut replies);
        let id = task_id(&actor, "Broken layout");
        run(&mut actor, Command::LoadDetail(id)).await;
        let detail = detail_in(bodies(&mut replies));
        assert_eq!(detail.images.len(), 1);
        assert_eq!(detail.images[0].name, "pasted.png");
        assert!(detail.images[0].preview.as_ref().unwrap().exists());
    }
}
