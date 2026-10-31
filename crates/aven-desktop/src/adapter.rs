//! The bridge between GPUI views and `aven-host`, with no GPUI types.
//!
//! One thread owns the `TuiStore` and runs requests in order, so its replies keep
//! their order too. Outside writes surface through SQLite's `data_version`, which
//! changes when another connection commits. A background task runs sync rounds.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use aven_core::choices::{TaskPriority, TaskSource, TaskStatus};
use aven_core::db::Database;
use aven_core::ids::TaskId;
use aven_core::operations::TaskDraft;
use aven_core::query::TaskListItem;
use aven_core::queue::QueueReason;
use aven_core::sync::client::keys::ProtectedStorage;
use aven_host::config::AppConfig;
use aven_host::store::task_selection::TaskSelection;
use aven_host::store::{
    PriorityMutation, TaskListRow, TaskTextField, TaskViewState, TuiStore, mutation_committed,
    task_creation_committed,
};
use aven_host::sync::encrypted::{DaemonRound, DaemonSync};
use aven_host::sync::scheduler::{ROUND_BUDGET, SyncScheduler};
use chrono::{DateTime, Local, NaiveDate};
use sqlx::ConnectOptions as _;
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};
use tokio::sync::{Notify, mpsc};
use tokio::time::{Instant, MissedTickBehavior, interval, sleep, sleep_until};

use crate::session::Session;

/// How often outside writes are looked for.
const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// Dates and availability change with the clock, with no write to notice.
const CLOCK_INTERVAL: Duration = Duration::from_secs(60);
/// Coalesces a burst of local changes into one sync round.
const SYNC_DEBOUNCE: Duration = Duration::from_millis(700);

/// The prototype's name and hint for each store group.
const GROUP_NAMES: [(&str, &str, &str); 10] = [
    (
        "needs action",
        "Needs action",
        "Conflicts, urgent, stale, due today or overdue",
    ),
    (
        "available",
        "Available",
        "Deferred work that has just arrived",
    ),
    ("focus", "Focus", "Active work and high priority todos"),
    ("soon", "Medium priority", "Todos at medium priority"),
    ("triage", "Triage", "Inbox tasks"),
    ("blocked", "Blocked", "Waiting on open dependencies"),
    ("later", "Later", "Backlog and low priority todos"),
    (
        "epics",
        "Epics",
        "Containers, separate from actionable work",
    ),
    ("done", "Done", ""),
    ("canceled", "Canceled", ""),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateTone {
    Plain,
    Soon,
    Late,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRow {
    pub id: TaskId,
    pub reference: String,
    /// The project prefix of `reference`.
    pub project: String,
    pub title: String,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    pub reason: Option<(String, bool)>,
    pub blocked: bool,
    pub recurring: bool,
    pub progress: Option<String>,
    pub labels: Vec<String>,
    pub due: String,
    pub date_tone: DateTone,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub name: String,
    pub hint: String,
    pub rows: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavEntry {
    pub name: String,
    pub key: String,
    /// The ref prefix of a project.
    pub prefix: String,
    pub count: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub workspace: String,
    pub tasks: Vec<TaskRow>,
    pub groups: Vec<Group>,
    pub workspaces: Vec<NavEntry>,
    pub projects: Vec<NavEntry>,
    pub views: Vec<(&'static str, i64)>,
}

impl Snapshot {
    pub fn row(&self, id: &TaskId) -> Option<&TaskRow> {
        self.tasks.iter().find(|row| &row.id == id)
    }

    /// Task indexes in display order.
    pub fn ordered(&self) -> impl Iterator<Item = usize> + '_ {
        self.groups
            .iter()
            .flat_map(|group| group.rows.iter().copied())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub at: String,
    pub body: String,
}

/// What the details panel shows beyond the row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detail {
    pub task_id: TaskId,
    pub description: String,
    pub available: String,
    pub project: String,
    pub notes: Vec<Note>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Refresh,
    LoadDetail(TaskId),
    SetStatus(TaskId, TaskStatus),
    SetPriority(TaskId, TaskPriority),
    SetTitle(TaskId, String),
    AddNote(TaskId, String),
    Create(String),
}

impl Command {
    pub fn is_mutation(&self) -> bool {
        !matches!(self, Self::Refresh | Self::LoadDetail(_))
    }
}

#[derive(Debug)]
pub struct Request {
    pub id: u64,
    pub command: Command,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncHealth {
    NotSetUp,
    Synced,
    Waiting,
    Failed(String),
    VaultDeleted,
}

#[derive(Debug)]
pub enum ReplyBody {
    List(Box<Snapshot>),
    Detail(Box<Detail>),
    Created(TaskId),
    /// The write did not happen, so the command can run again.
    SaveFailed(Command, String),
    /// The write happened but the view is behind. Only a refresh may follow.
    SavedRefreshFailed(String),
    RefreshFailed(String),
    Sync(SyncHealth),
}

#[derive(Debug)]
pub struct Reply {
    pub request: u64,
    pub body: ReplyBody,
}

pub struct Launch {
    pub database: PathBuf,
    pub config: AppConfig,
    pub storage: Arc<dyn ProtectedStorage>,
}

pub type Replies = mpsc::UnboundedReceiver<Reply>;

/// Matches the prototype's `dueText`.
pub fn due_text(due: &str, today: NaiveDate) -> String {
    let Ok(date) = NaiveDate::parse_from_str(due, "%Y-%m-%d") else {
        return due.to_string();
    };
    match (date - today).num_days() {
        0 => "Today".to_string(),
        1 => "Tomorrow".to_string(),
        -1 => "Yesterday".to_string(),
        _ => date.format("%b %-d").to_string(),
    }
}

/// The reason a queue row shows, with whether it reads as a problem. The prototype
/// leaves urgency and dates to the priority icon and the date column.
fn reason_text(item: &TaskListItem) -> Option<(String, bool)> {
    match item.queue.reason? {
        QueueReason::Conflict => {
            let label = item
                .conflicts
                .first()
                .map_or("Field", |conflict| &conflict.field);
            Some((format!("{label} conflict"), true))
        }
        QueueReason::Stale { days } => Some((format!("No activity for {days}d"), false)),
        QueueReason::AvailableAfterDeferral => {
            let at = item.task.available_at.as_deref().and_then(local_clock);
            Some((format!("Available since {}", at.unwrap_or_default()), false))
        }
        QueueReason::Urgent
        | QueueReason::DueToday
        | QueueReason::Overdue { .. }
        | QueueReason::Blocked { .. } => None,
    }
}

fn local_time(stamp: &str) -> Option<DateTime<Local>> {
    let parsed = DateTime::parse_from_rfc3339(stamp).ok()?;
    Some(parsed.with_timezone(&Local))
}

fn local_clock(stamp: &str) -> Option<String> {
    Some(local_time(stamp)?.format("%H:%M").to_string())
}

/// Open tasks warn from the day before they are due.
fn date_tone(item: &TaskListItem, today: NaiveDate) -> DateTone {
    let due = item.task.due_on.as_deref();
    let due = due.and_then(|due| NaiveDate::parse_from_str(due, "%Y-%m-%d").ok());
    let closed = matches!(item.task.status, TaskStatus::Done | TaskStatus::Canceled);
    match due.filter(|_| !closed).map(|due| (due - today).num_days()) {
        Some(days) if days < 0 => DateTone::Late,
        Some(days) if days <= 1 => DateTone::Soon,
        _ => DateTone::Plain,
    }
}

fn task_row(item: &TaskListItem, today: NaiveDate) -> TaskRow {
    let task = &item.task;
    let prefix = item.display_ref.split('-').next().unwrap_or_default();
    TaskRow {
        id: task.id.clone(),
        reference: item.display_ref.clone(),
        project: prefix.to_string(),
        title: task.title.clone(),
        status: task.status,
        priority: task.priority,
        reason: reason_text(item),
        blocked: item.unresolved_blocker_count > 0,
        recurring: item.recurrence.is_some(),
        progress: item
            .epic_rollup
            .as_ref()
            .map(|rollup| format!("{}/{}", rollup.done, rollup.total)),
        labels: item.labels.clone(),
        due: task
            .due_on
            .as_deref()
            .map(|due| due_text(due, today))
            .unwrap_or_default(),
        date_tone: date_tone(item, today),
    }
}

fn group_text(label: &str) -> (String, String) {
    GROUP_NAMES
        .iter()
        .find(|(group, ..)| *group == label)
        .map_or_else(
            || (label.to_string(), String::new()),
            |(_, name, hint)| ((*name).to_string(), (*hint).to_string()),
        )
}

/// Reads the store's projection. `now` formats dates, so scene runs pass their anchor.
pub fn snapshot(store: &TuiStore, now: DateTime<Local>) -> Snapshot {
    let today = now.date_naive();
    let tasks: Vec<TaskRow> = store
        .tasks
        .iter()
        .map(|item| task_row(item, today))
        .collect();
    let mut groups: Vec<Group> = Vec::new();
    for row in &store.task_list_view().rows {
        match row {
            TaskListRow::Group(group) => {
                let (name, hint) = group_text(&group.label);
                groups.push(Group {
                    name,
                    hint,
                    rows: Vec::with_capacity(group.count),
                });
            }
            TaskListRow::Task { task_index } | TaskListRow::EpicChild { task_index, .. } => {
                if let Some(group) = groups.last_mut() {
                    group.rows.push(*task_index);
                }
            }
        }
    }
    let counts = &store.counts;
    Snapshot {
        workspace: store.active_workspace.name.clone(),
        tasks,
        groups,
        workspaces: store
            .workspaces
            .iter()
            .map(|workspace| NavEntry {
                name: workspace.name.clone(),
                key: workspace.key.clone(),
                prefix: String::new(),
                count: store
                    .workspace_open_counts
                    .get(&workspace.id)
                    .copied()
                    .unwrap_or_default(),
            })
            .collect(),
        projects: store
            .projects
            .iter()
            .map(|project| NavEntry {
                name: project.name.clone(),
                key: project.key.clone(),
                prefix: project.prefix.clone(),
                count: project.open_count,
            })
            .collect(),
        views: vec![
            ("queue", counts.open),
            ("ready", counts.ready),
            ("inbox", counts.inbox),
            ("upcoming", counts.upcoming),
            ("overdue", counts.overdue),
            ("blocked", counts.blocked),
            ("epics", counts.epics),
            ("recurring", counts.recurring),
            ("done", 0),
            ("conflicts", counts.conflicts),
        ],
    }
}

pub fn detail(item: &TaskListItem) -> Detail {
    let stamp = |stamp: &str| {
        local_time(stamp).map_or_else(
            || stamp.to_string(),
            |time| time.format("%b %-d, %H:%M").to_string(),
        )
    };
    Detail {
        task_id: item.task.id.clone(),
        description: item.task.description.clone(),
        available: item
            .task
            .available_at
            .as_deref()
            .map_or_else(|| "Now".to_string(), stamp),
        project: item.task.project_key.clone(),
        notes: item
            .notes
            .iter()
            .map(|note| Note {
                at: stamp(&note.created_at),
                body: note.body.clone(),
            })
            .collect(),
    }
}

/// Opens the store a launch resolves to: the queue's selected workspace on the
/// default view.
pub async fn open_store(path: &Path, config: &AppConfig) -> Result<TuiStore> {
    let database = Database::open(path).await?;
    let workspace = database.restore_queue_workspace().await?;
    TuiStore::new_with_view_state_and_config(
        database,
        workspace,
        TaskViewState::default(),
        config.clone(),
    )
    .await
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

struct Actor {
    store: TuiStore,
    database: PathBuf,
    replies: mpsc::UnboundedSender<Reply>,
    sync_wake: Arc<Notify>,
    handled: u64,
    /// The task the view has selected, which refreshes keep in the list.
    selected: Option<TaskId>,
}

impl Actor {
    fn send(&self, body: ReplyBody) {
        let _ = self.replies.send(Reply {
            request: self.handled,
            body,
        });
    }

    fn send_list(&self) {
        let list = snapshot(&self.store, Local::now());
        self.send(ReplyBody::List(Box::new(list)));
    }

    /// A desktop launch has no directory to infer a project from, so a new task
    /// joins the selected task's project, or the first one.
    fn project_for_new_task(&self) -> Option<String> {
        let selected = self.selected.as_ref().and_then(|id| {
            let item = self.store.tasks.iter().find(|item| item.task.id == *id)?;
            Some(item.task.project_key.clone())
        });
        selected.or_else(|| {
            self.store
                .projects
                .first()
                .map(|project| project.key.clone())
        })
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

    fn selection(&self, id: &TaskId) -> Result<TaskSelection> {
        let index = self.store.tasks.iter().position(|item| &item.task.id == id);
        index
            .and_then(|index| {
                TaskSelection::resolve(&self.store.tasks, &BTreeSet::new(), Some(index))
            })
            .ok_or_else(|| anyhow!("That task is no longer in this view"))
    }

    /// Runs a write and returns the task it created, if any. An error that
    /// `mutation_committed` or `task_creation_committed` accepts means the
    /// write stands.
    async fn mutate(&mut self, command: &Command) -> Result<Option<TaskId>> {
        match command {
            Command::SetStatus(id, status) => {
                let selection = self.selection(id)?;
                self.store
                    .mutate_status_selection(&selection, *status, true)
                    .await?;
            }
            Command::SetPriority(id, priority) => {
                let selection = self.selection(id)?;
                self.store
                    .mutate_priority_selection(&selection, PriorityMutation::Set(*priority))
                    .await?;
            }
            Command::SetTitle(id, title) => {
                let selection = self.selection(id)?;
                self.store
                    .mutate_text_selection(&selection, TaskTextField::Title, title.clone())
                    .await?;
            }
            Command::AddNote(id, body) => {
                self.store.add_note_to_task(id, body.clone()).await?;
            }
            Command::Create(title) => {
                let draft = TaskDraft {
                    title: title.clone(),
                    description: String::new(),
                    project: self.project_for_new_task(),
                    status: None,
                    priority: TaskPriority::None.to_string(),
                    source: TaskSource::Tui,
                    labels: Vec::new(),
                    metadata: Vec::new(),
                    available_at: None,
                    due_on: None,
                    is_epic: false,
                };
                let (_, selected) = self.store.create_task(draft, None).await?;
                let row = selected.and_then(|index| self.store.tasks.get(index));
                return Ok(row.map(|item| item.task.id.clone()));
            }
            Command::Refresh | Command::LoadDetail(_) => {}
        }
        Ok(None)
    }

    async fn handle(&mut self, request: Request) {
        self.handled = request.id;
        match request.command {
            Command::Refresh => self.outside_change().await,
            Command::LoadDetail(id) => {
                self.selected = Some(id.clone());
                match self.store.load_task_item(&id).await {
                    Ok(Some(item)) => self.send(ReplyBody::Detail(Box::new(detail(&item)))),
                    Ok(None) => {}
                    Err(error) => self.send(ReplyBody::RefreshFailed(error.to_string())),
                }
            }
            command => {
                match self.mutate(&command).await {
                    Ok(created) => {
                        // A note saves without refreshing the projection.
                        let refreshed = match command {
                            Command::AddNote(..) => self.refresh().await,
                            _ => Ok(()),
                        };
                        match refreshed {
                            Ok(()) => self.send_list(),
                            Err(error) => {
                                self.send(ReplyBody::SavedRefreshFailed(error.to_string()));
                            }
                        }
                        if let Some(id) = created {
                            self.send(ReplyBody::Created(id));
                        }
                    }
                    Err(error) if mutation_committed(&error) || task_creation_committed(&error) => {
                        self.send(ReplyBody::SavedRefreshFailed(error.to_string()));
                    }
                    Err(error) => {
                        self.send(ReplyBody::SaveFailed(command, error.to_string()));
                        self.send_list();
                    }
                }
                self.sync_wake.notify_one();
            }
        }
    }

    async fn outside_change(&mut self) {
        match self.refresh().await {
            Ok(()) => self.send_list(),
            Err(error) => self.send(ReplyBody::RefreshFailed(error.to_string())),
        }
    }

    async fn run(mut self, mut requests: mpsc::UnboundedReceiver<Request>) {
        let mut writes = OutsideWrites::open(&self.database).await.ok();
        let mut poll = interval(POLL_INTERVAL);
        poll.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut clock = interval(CLOCK_INTERVAL);
        clock.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                request = requests.recv() => {
                    let Some(request) = request else { break };
                    self.handle(request).await;
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
    storage: Arc<dyn ProtectedStorage>,
    wake: Arc<Notify>,
    replies: mpsc::UnboundedSender<Reply>,
) {
    let interval = Duration::from_secs(config.sync_interval_seconds());
    let mut scheduler = SyncScheduler::new(interval, Instant::now());
    let mut sync = DaemonSync::with_storage(storage);
    let report = |health: SyncHealth| {
        let _ = replies.send(Reply {
            request: 0,
            body: ReplyBody::Sync(health),
        });
    };
    loop {
        tokio::select! {
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
                        report(SyncHealth::Failed(error.to_string()));
                    }
                }
            }
        }
    }
}

/// Opens the store on its thread, starts sync, and returns the first snapshot's
/// session.
pub fn start(handle: tokio::runtime::Handle, launch: Launch) -> Result<(Session, Replies)> {
    let (requests_tx, requests_rx) = mpsc::unbounded_channel();
    let (replies_tx, replies_rx) = mpsc::unbounded_channel();
    let (opened_tx, opened_rx) = std::sync::mpsc::channel();
    let sync_wake = Arc::new(Notify::new());
    let runtime = handle.clone();
    std::thread::Builder::new()
        .name("aven-store".to_string())
        .spawn(move || {
            runtime.clone().block_on(async move {
                let store = match open_store(&launch.database, &launch.config).await {
                    Ok(store) => store,
                    Err(error) => {
                        let _ = opened_tx.send(Err(error));
                        return;
                    }
                };
                if launch.config.sync_is_allowed() {
                    runtime.spawn(run_sync(
                        store.database(),
                        launch.config.clone(),
                        launch.storage.clone(),
                        sync_wake.clone(),
                        replies_tx.clone(),
                    ));
                }
                let _ = opened_tx.send(Ok(snapshot(&store, Local::now())));
                Actor {
                    store,
                    database: launch.database,
                    replies: replies_tx,
                    sync_wake,
                    handled: 0,
                    selected: None,
                }
                .run(requests_rx)
                .await;
            });
        })?;
    let first = opened_rx.recv().context("the store thread stopped")??;
    Ok((Session::new(first, requests_tx), replies_rx))
}

/// A generated list for layout measurement.
#[cfg(feature = "capture")]
pub fn bench_snapshot(tasks: usize, groups: Vec<Group>) -> Snapshot {
    Snapshot {
        workspace: "Bench".to_string(),
        tasks: (0..tasks)
            .map(|index| TaskRow {
                id: TaskId::new(),
                reference: format!("BEN-{index:04}"),
                project: "BEN".to_string(),
                title: format!("Generated task number {index}"),
                status: TaskStatus::Todo,
                priority: TaskPriority::None,
                reason: None,
                blocked: false,
                recurring: false,
                progress: None,
                labels: vec!["bench".to_string()],
                due: String::new(),
                date_tone: DateTone::Plain,
            })
            .collect(),
        groups,
        ..Snapshot::default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Prefs {
    pub nav_width: f32,
    pub detail_width: f32,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            nav_width: 228.,
            detail_width: 380.,
        }
    }
}

impl Prefs {
    /// Desktop-local preferences beside the database.
    pub fn path(database: &Path) -> PathBuf {
        database.with_file_name("desktop-prefs.json")
    }

    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        std::fs::write(path, serde_json::to_vec_pretty(self)?).context("write preferences")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    async fn actor(title: &str) -> (tempfile::TempDir, Actor, mpsc::UnboundedReceiver<Reply>) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("db.sqlite");
        let mut store = open_store(&path, &AppConfig::default()).await.unwrap();
        store.create_task(draft(title), None).await.unwrap();
        let (replies, received) = mpsc::unbounded_channel();
        let actor = Actor {
            store,
            database: path,
            replies,
            sync_wake: Arc::new(Notify::new()),
            handled: 0,
            selected: None,
        };
        (directory, actor, received)
    }

    fn bodies(replies: &mut mpsc::UnboundedReceiver<Reply>) -> Vec<ReplyBody> {
        std::iter::from_fn(|| replies.try_recv().ok())
            .map(|reply| reply.body)
            .collect()
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
    async fn a_new_task_joins_the_selected_tasks_project() {
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
        let command = Command::Create("Second".to_string());
        actor.handle(Request { id: 1, command }).await;

        assert!(
            bodies(&mut replies)
                .iter()
                .any(|body| matches!(body, ReplyBody::Created(_)))
        );
        assert_eq!(find(&actor.store, "Second").project_key, "other");
    }

    #[tokio::test]
    async fn a_refresh_failure_after_a_create_reports_saved_once() {
        let (_directory, mut actor, mut replies) = actor("Stored").await;
        actor.store.fail_next_refresh();
        let command = Command::Create("Second".to_string());
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

    #[test]
    fn due_text_matches_the_prototype_around_today() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 14).unwrap();
        for (due, expected) in [
            ("2026-10-14", "Today"),
            ("2026-10-15", "Tomorrow"),
            ("2026-10-13", "Yesterday"),
            ("2026-10-02", "Oct 2"),
            ("2026-11-20", "Nov 20"),
        ] {
            assert_eq!(due_text(due, today), expected);
        }
    }
}
