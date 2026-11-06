use crate::launch::{Catalog, LaunchEvent, LaunchJob};
use crate::sync_ops::{SyncEvent, SyncRequest};
use crate::toast::Toast;
use anyhow::Result;
use aven_core::choices::{TaskPriority, TaskStatus};
use aven_core::ids::TaskId;
use aven_core::query::RecurrenceSeriesLifecycleFilter;
use aven_core::recurrence::{RecurrenceSeriesId, RecurrenceSeriesState};
use aven_core::sync::client::engine::LocalPhase;
use aven_core::sync::client::keys::ProtectedStorage;
use aven_host::config::{AppConfig, RemindersConfig};
use aven_host::store::{ConflictTarget, TaskQuery};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;

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
    pub project_key: String,
    pub title: String,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    pub reason: Option<(String, bool)>,
    pub blocked: bool,
    pub recurring: bool,
    pub progress: Option<String>,
    pub labels: Vec<String>,
    /// The due date, or the availability where the view orders by it.
    pub due: String,
    pub date_tone: DateTone,
    pub deleted: bool,
    pub is_epic: bool,
    /// How far an epic's children have got.
    pub epic: Option<EpicProgress>,
    pub created: Option<RowDate>,
    pub updated: Option<RowDate>,
    pub byline: Byline,
    pub activity: Vec<(String, String)>,
}

/// A moment a row shows as a date, with its full time for a closer look.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowDate {
    pub day: String,
    pub full: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpicProgress {
    pub done: usize,
    pub canceled: usize,
    pub open: usize,
    /// The first child in progress.
    pub current: Option<String>,
}

impl EpicProgress {
    pub fn total(&self) -> usize {
        self.done + self.canceled + self.open
    }

    pub fn counts(&self) -> String {
        let parts = [
            (self.done, "done"),
            (self.canceled, "canceled"),
            (self.open, "open"),
        ]
        .into_iter()
        .filter(|(count, _)| *count > 0)
        .map(|(count, word)| format!("{count} {word}"))
        .collect::<Vec<_>>();
        if parts.is_empty() {
            "No tasks".to_string()
        } else {
            parts.join(" · ")
        }
    }
}

/// A series' occurrence that is still in play.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Occurrence {
    pub id: TaskId,
    pub reference: String,
    pub slot: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeriesRow {
    pub id: RecurrenceSeriesId,
    pub reference: String,
    pub project: String,
    pub title: String,
    pub state: RecurrenceSeriesState,
    pub rule: String,
    pub current: Option<Occurrence>,
    pub after: Option<String>,
}

/// What the details panel shows for a selected series.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeriesDetail {
    pub id: RecurrenceSeriesId,
    pub reference: String,
    pub title: String,
    pub description: String,
    pub state: RecurrenceSeriesState,
    pub rule: String,
    /// The rule, time and due policy as the composer's schedule field reads them.
    pub schedule: String,
    pub due: String,
    pub timezone: String,
    pub after: Option<String>,
    pub current: Option<Occurrence>,
    /// Whether the current occurrence can still be skipped.
    pub skippable: bool,
    pub history: Vec<(String, String)>,
}

/// A task a review shows, with why it came up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewCard {
    pub id: TaskId,
    pub why: String,
    /// An inbox task has no status to keep.
    pub stale: bool,
}

/// What every marked task receives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bulk {
    Status(TaskStatus),
    Priority(TaskPriority),
    Schedule {
        available_at: Option<Option<String>>,
        due_on: Option<Option<String>>,
    },
    Project(String),
    Labels {
        selected: Vec<String>,
        partial: Vec<String>,
    },
    Deleted(bool),
    Epic(TaskId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub name: String,
    pub hint: String,
    pub rows: Vec<usize>,
    /// The status a task takes when dropped on this column.
    pub lane: Option<TaskStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavEntry {
    pub name: String,
    pub key: String,
    /// The ref prefix of a project.
    pub prefix: String,
    pub count: i64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GroupBy {
    #[default]
    Status,
    Project,
    Priority,
    None,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DeletedFilter {
    #[default]
    Hide,
    Show,
    Only,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ViewOrder {
    #[default]
    Priority,
    Due,
    Available,
    Created,
    Updated,
    Project,
    Title,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaFilter {
    pub key: String,
    pub present: bool,
}

/// How the list on screen was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewInfo {
    pub query: TaskQuery,
    /// The key of the project in scope.
    pub project: Option<String>,
    pub columns: bool,
    pub order: ViewOrder,
    pub descending: bool,
    pub group: GroupBy,
    pub label: Option<String>,
    pub priority: Option<TaskPriority>,
    pub deleted: DeletedFilter,
    pub metadata: Option<MetaFilter>,
    pub search: Option<String>,
    /// Which recurring series are listed.
    pub lifecycle: RecurrenceSeriesLifecycleFilter,
    /// Whether closed rows are listed where the view can include them.
    pub closed: bool,
}

impl Default for ViewInfo {
    fn default() -> Self {
        Self {
            query: TaskQuery::Queue,
            project: None,
            columns: false,
            order: ViewOrder::default(),
            descending: false,
            group: GroupBy::default(),
            label: None,
            priority: None,
            deleted: DeletedFilter::default(),
            metadata: None,
            search: None,
            lifecycle: RecurrenceSeriesLifecycleFilter::default(),
            closed: false,
        }
    }
}

impl ViewInfo {
    /// Whether closed rows belong to this list, rather than joining the end group.
    pub fn lists_closed(&self) -> bool {
        self.columns
            || self.search.is_some()
            || matches!(self.query, TaskQuery::Done | TaskQuery::Search)
    }

    pub fn has_filters(&self) -> bool {
        self.label.is_some()
            || self.priority.is_some()
            || self.deleted != DeletedFilter::Hide
            || self.metadata.is_some()
    }
}

/// Views that can show as columns.
pub fn supports_columns(query: TaskQuery) -> bool {
    matches!(
        query,
        TaskQuery::Ready
            | TaskQuery::Inbox
            | TaskQuery::Blocked
            | TaskQuery::Overdue
            | TaskQuery::Open
            | TaskQuery::Backlog
            | TaskQuery::Todo
            | TaskQuery::Active
            | TaskQuery::Done
    )
}

/// One entry of the workspace's task list, for pickers and the palette.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogTask {
    pub id: TaskId,
    pub reference: String,
    pub project: String,
    pub project_key: String,
    pub title: String,
    pub status: TaskStatus,
    pub is_epic: bool,
}

/// One line of the Recent actions ledger, newest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LedgerLine {
    Day {
        label: String,
        count: usize,
    },
    /// Actions with less than a pause between them.
    Burst {
        label: String,
        range: String,
    },
    Run(LedgerRun),
}

/// Consecutive actions on one entity, read as a single row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerRun {
    /// The newest action's change, which keeps the row selected across reloads.
    pub key: String,
    pub clock: String,
    pub task: Option<TaskId>,
    pub reference: String,
    pub project_key: String,
    pub title: String,
    pub closed: bool,
    pub deleted: bool,
    /// Newest first.
    pub steps: Vec<LedgerStep>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerStep {
    pub mark: StepMark,
    /// How the action reads on its own, before its value.
    pub lead: String,
    /// The value the action set, which reads emphasised.
    pub value: Option<String>,
    /// What the action names, such as a label.
    pub chip: Option<String>,
    /// How the action reads as one step of a run.
    pub short: String,
    /// The status the action left the task in, which colours its value.
    pub tone: Option<TaskStatus>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepMark {
    Status(TaskStatus),
    Priority(TaskPriority),
    Created,
    Icon(&'static str),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ledger {
    pub lines: Vec<LedgerLine>,
    /// Actions and distinct tasks recorded today.
    pub today: (usize, usize),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub workspace: String,
    pub tasks: Vec<TaskRow>,
    pub groups: Vec<Group>,
    pub series: Vec<SeriesRow>,
    pub workspaces: Vec<NavEntry>,
    pub projects: Vec<NavEntry>,
    /// Task counts by view, in scope. The sidebar shows only some of them.
    pub views: Vec<(&'static str, i64)>,
    pub view: ViewInfo,
    pub labels: Vec<String>,
    pub fields: Vec<String>,
    pub recent: Ledger,
    pub sync: SyncView,
    /// The database has no tasks and has never been through the welcome.
    pub welcome: bool,
}

/// What the store knows about sync, merged from this database's state and the
/// daemon's record, which the app only reads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncView {
    pub phase: SyncPhase,
    pub server: Option<String>,
    pub pending: usize,
    pub conflicts: usize,
    /// Unix seconds of the daemon's last completed round.
    pub daemon_synced_at: Option<i64>,
    pub access_refused: bool,
    pub vault_deleted: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SyncPhase {
    #[default]
    Local,
    SetupIncomplete,
    JoinIncomplete,
    SetUp,
}

impl From<LocalPhase> for SyncPhase {
    fn from(phase: LocalPhase) -> Self {
        match phase {
            LocalPhase::NotSetUp => Self::Local,
            LocalPhase::SetupIncomplete => Self::SetupIncomplete,
            LocalPhase::JoinIncomplete => Self::JoinIncomplete,
            LocalPhase::SetUp => Self::SetUp,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceEntry {
    pub key: String,
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectEntry {
    pub key: String,
    pub name: String,
    pub prefix: String,
}

/// Rows of a settings tab, read when settings open and after each edit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SettingsData {
    pub workspaces: Vec<WorkspaceEntry>,
    pub projects: Vec<ProjectEntry>,
    pub labels: Vec<(String, usize)>,
    pub fields: Vec<(String, usize)>,
    pub commands: Vec<(String, String)>,
    pub reminders: Reminders,
    /// The agent command configured for task intake, if any.
    pub agent: Option<AgentSetup>,
    pub background: BackgroundStatus,
}

/// What runs while Aven is quit: the LaunchAgent that hosts sync and
/// reminders then.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BackgroundStatus {
    /// This app is the installed one in `/Applications`, which keeps the agent
    /// in line with the switches. Other copies only show what runs.
    pub managed: bool,
    /// `sync.enabled`, which only the agent reads: the app always syncs.
    pub sync_when_quit: bool,
    pub reminders: bool,
    /// The intent and switches ask for the agent.
    pub wanted: bool,
    pub agent: AgentShown,
    /// Unix seconds of the agent's last completed sync.
    pub last_sync: Option<i64>,
    /// Notifications from the last week that weren't shown because
    /// notifications were off.
    pub notifications_off: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AgentShown {
    /// Not checked: this copy of the app doesn't manage the agent.
    #[default]
    Unknown,
    Running,
    Off,
    /// The switches want it, but it isn't running.
    NotRunning,
    /// The user turned Aven off in System Settings > General > Login Items &
    /// Extensions.
    TurnedOff,
}

/// The agent command task intake runs, as the config sets it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentSetup {
    pub command: String,
    pub args: Vec<String>,
    /// Whether `command` names an executable, by path or on `PATH`.
    pub runnable: bool,
}

/// The note the settings' agent test sends.
pub const AGENT_TEST_NOTE: &str = "pay rent friday, it\u{2019}s important";

/// What the agent wrote for the test note, and how long it took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTest {
    pub task: Box<NewTask>,
    pub elapsed: std::time::Duration,
}

/// The shared config's reminder settings, as the Reminders tab edits them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reminders {
    pub enabled: bool,
    pub digest_time: String,
    pub due_digest: bool,
    pub available: bool,
}

impl From<&RemindersConfig> for Reminders {
    fn from(config: &RemindersConfig) -> Self {
        Self {
            enabled: config.enabled,
            digest_time: config.digest_time.clone(),
            due_digest: config.due_digest,
            available: config.available,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsEdit {
    NewWorkspace(String),
    RenameWorkspace(String, String),
    NewProject {
        name: String,
        prefix: String,
    },
    EditProject {
        key: String,
        name: String,
        prefix: String,
    },
    DeleteProject(String),
    NewLabel(String),
    RenameLabel(String, String),
    RenameField(String, String),
    Reminders(Reminders),
    /// Sync while Aven is quit, or not.
    SyncWhenQuit(bool),
    /// Run the agent for the switches that are on.
    TurnOnBackground,
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
    pub id: String,
    pub at: String,
    pub body: String,
}

/// Another task a task points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub id: TaskId,
    pub reference: String,
    pub project: String,
    pub project_key: String,
    pub title: String,
    pub status: TaskStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub id: String,
    pub name: String,
    pub alt: String,
    /// A local preview. Without one the image's bytes are still on their way.
    pub preview: Option<PathBuf>,
}

/// What the details panel shows beyond the row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Byline {
    pub created_ago: String,
    pub created_on: String,
    pub source: Option<String>,
    /// Absent until the task changes after it was created.
    pub updated_ago: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detail {
    pub task_id: TaskId,
    /// A task outside the list has no row there.
    pub row: TaskRow,
    pub description: String,
    pub available: String,
    pub available_at: Option<String>,
    pub due_on: Option<String>,
    pub project: String,
    pub project_name: String,
    pub notes: Vec<Note>,
    pub images: Vec<Image>,
    pub epic: Option<Link>,
    pub blocked_by: Vec<Link>,
    pub blocks: Vec<Link>,
    pub related: Vec<Link>,
    pub metadata: Vec<(String, String)>,
    /// Children in core's order. Epics have them, other tasks do not.
    pub children: Vec<Child>,
    pub conflicts: Vec<ConflictTarget>,
    /// The series a recurring task came from, with its ref.
    pub series: Option<(RecurrenceSeriesId, String)>,
}

/// An epic's child as the epic's section reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Child {
    pub link: Link,
    pub group: ChildGroup,
    pub note: String,
    pub due: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildGroup {
    Progress,
    Ready,
    Waiting,
    Finished,
}

/// Bytes waiting to become an attachment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingImage {
    pub name: String,
    pub bytes: Arc<Vec<u8>>,
}

/// A task as the composer submits it. `schedule` is the text the user typed, so
/// a failed create returns it unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewTask {
    pub title: String,
    pub description: String,
    pub project: Option<String>,
    pub status: Option<String>,
    pub priority: String,
    pub labels: Vec<String>,
    pub metadata: Vec<(String, String)>,
    pub is_epic: bool,
    pub schedule: String,
    pub images: Vec<PendingImage>,
    /// The series this one takes over from.
    pub replaces: Option<RecurrenceSeriesId>,
    /// The name for the project to create when the workspace has none.
    pub first_project: Option<String>,
}

impl NewTask {
    pub fn titled(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            priority: TaskPriority::None.to_string(),
            ..Self::default()
        }
    }
}

/// What a toast's second button does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Go {
    pub label: &'static str,
    pub command: Command,
}

/// What the task list search dialog asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchInput {
    pub text: String,
    pub project: Option<String>,
    pub deleted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub id: TaskId,
    pub reference: String,
    pub project: String,
    pub project_key: String,
    pub title: String,
    pub status: TaskStatus,
    pub snippet: Option<String>,
    pub deleted: bool,
    pub is_epic: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchResults {
    pub hits: Vec<SearchHit>,
    pub total: usize,
    pub elsewhere: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Refresh,
    /// A refresh the user asked for, which also lets go of the selected row.
    RefreshView,
    LoadDetail(TaskId),
    /// Details of a task opened from another one, leaving the selection alone.
    Peek(TaskId),
    Preview(SearchInput),
    /// Runs the agent once on the test note, saving nothing.
    TestAgent,
    /// Every workspace with its projects and labels, for the launcher.
    LoadLauncher,
    /// Starts a launcher submission, which runs apart from the store.
    Launch(Box<LaunchJob>),
    /// Stops the review with this id, if it still runs.
    CancelLaunch(u64),
    CopyReport(TaskId),
    ShowView(TaskQuery),
    ShowProject(Option<String>),
    SwitchWorkspace(String),
    FilterLabel(Option<String>),
    FilterPriority(Option<TaskPriority>),
    FilterDeleted(DeletedFilter),
    FilterMetadata(Option<MetaFilter>),
    ClearFilters,
    SetOrder(ViewOrder),
    ReverseOrder,
    SetGroup(GroupBy),
    SetColumns(bool),
    Search(SearchInput),
    /// Reveals a task, widening the view as far as it takes.
    Goto(TaskId),
    SetStatus(TaskId, TaskStatus),
    SetPriority(TaskId, TaskPriority),
    SetTitle(TaskId, String),
    SetDescription(TaskId, String),
    SetSchedule {
        id: TaskId,
        available_at: Option<Option<String>>,
        due_on: Option<Option<String>>,
    },
    SetProject(TaskId, String),
    SetLabels(TaskId, Vec<String>),
    SetDeleted(TaskId, bool),
    AddBlocker(TaskId, TaskId),
    RemoveBlocker(TaskId, TaskId),
    AddRelated(TaskId, TaskId),
    RemoveRelated(TaskId, TaskId),
    SetMetadata(TaskId, String, Option<String>),
    AddNote(TaskId, String),
    DeleteNote(TaskId, String),
    AddImage(TaskId, PendingImage),
    DeleteImage(TaskId, String),
    SetImageAlt(TaskId, String, String),
    SaveImage(String, PathBuf),
    Undo,
    Create(Box<NewTask>),
    /// A task Add Task's agent job `job` read for the workspace with key
    /// `workspace`, created without review. It saves nothing if the window
    /// has since moved to another workspace or database.
    CreateUnreviewed {
        kept: Box<crate::draft::Kept>,
        task: Box<NewTask>,
    },
    /// A write to exactly these tasks, whatever the list holds by then.
    Bulk(Vec<TaskId>, Bulk),
    CreateChild(TaskId, String),
    AddChild(TaskId, TaskId),
    RemoveChild(TaskId),
    SetClosed(bool),
    ShowRecurring(RecurrenceSeriesLifecycleFilter),
    /// Lists a series, widening the lifecycle filter if it is hidden.
    OpenSeries(RecurrenceSeriesId),
    LoadSeries(RecurrenceSeriesId),
    PauseSeries(RecurrenceSeriesId),
    ResumeSeries(RecurrenceSeriesId),
    SkipSeries(RecurrenceSeriesId),
    StopSeries(RecurrenceSeriesId, bool),
    SetTemplate(RecurrenceSeriesId, String),
    /// One field of one task. Conflicts never resolve through marks.
    ResolveConflict(Box<ConflictTarget>, String),
    StartReview(Vec<TaskId>),
    Keep(TaskId),
    Unkeep(TaskId),
    PublishGist(TaskId),
    /// Sync work a person started. Never goes through a retry toast, which
    /// would keep invitation text around.
    Sync(SyncRequest),
    LoadSettings,
    Settings(SettingsEdit),
    Backup,
    Export,
    /// Restores an archive to a new database and opens it.
    Restore(PathBuf),
    OpenDatabase(PathBuf),
    /// Goes back to what the last run showed, skipping what no longer exists.
    ReopenView {
        query: Option<TaskQuery>,
        project: Option<String>,
        columns: bool,
        selected: Option<TaskId>,
    },
    /// Records that the welcome is done without adding a task.
    FinishWelcome,
    /// Finishes queued work and stops sync between rounds.
    Quit,
}

impl Command {
    /// Whether this changes stored data, which may change what undo takes back.
    pub fn is_write(&self) -> bool {
        self.is_mutation() && !self.is_navigation() && *self != Self::RefreshView
    }

    /// Whether this only changes what the list shows.
    pub fn is_navigation(&self) -> bool {
        matches!(
            self,
            Self::ShowView(_)
                | Self::ShowProject(_)
                | Self::SwitchWorkspace(_)
                | Self::FilterLabel(_)
                | Self::FilterPriority(_)
                | Self::FilterDeleted(_)
                | Self::FilterMetadata(_)
                | Self::ClearFilters
                | Self::SetOrder(_)
                | Self::ReverseOrder
                | Self::SetGroup(_)
                | Self::SetColumns(_)
                | Self::Search(_)
                | Self::Goto(_)
                | Self::SetClosed(_)
                | Self::ShowRecurring(_)
                | Self::OpenSeries(_)
        )
    }

    /// Whether lists from before this command must be dropped.
    pub fn is_mutation(&self) -> bool {
        !matches!(
            self,
            Self::Refresh
                | Self::LoadDetail(_)
                | Self::Peek(_)
                | Self::Preview(_)
                | Self::TestAgent
                | Self::LoadLauncher
                | Self::Launch(_)
                | Self::CancelLaunch(_)
                | Self::CopyReport(..)
                | Self::SaveImage(..)
                | Self::LoadSeries(_)
                | Self::StartReview(_)
                | Self::PublishGist(_)
                | Self::Sync(_)
                | Self::LoadSettings
                | Self::Settings(_)
                | Self::Backup
                | Self::Export
                | Self::Restore(_)
                | Self::OpenDatabase(_)
                | Self::ReopenView { .. }
                | Self::Quit
        )
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
    Series(Box<SeriesDetail>),
    /// A page of tasks to review, with how many wait beyond it.
    Review(Vec<ReviewCard>, usize),
    /// A series to ask about stopping, now its replacement exists.
    AskStop(RecurrenceSeriesId),
    /// Selects a series the list now shows.
    SelectSeries(RecurrenceSeriesId),
    Catalog(Vec<CatalogTask>),
    Search(SearchResults),
    /// Text a task's report or similar produced for the clipboard.
    Copied(String),
    /// The agent test's result, or why it failed.
    AgentTest(Result<AgentTest, String>),
    Launcher(Box<Catalog>),
    Launch(LaunchEvent),
    Created(TaskId),
    /// A write landed, with the toast that says so. Its `undo` is set when the
    /// store's undo would take this write back.
    Saved(Toast),
    /// A task the view should select.
    Select(TaskId),
    /// A notice that is not about a write.
    Notice(Toast),
    /// Queued work is done and sync has stopped, so the app can exit.
    Stopped,
    /// The write did not happen, so the command can run again.
    SaveFailed(Command, String),
    /// The write happened but the view is behind. Only a refresh may follow.
    SavedRefreshFailed(String),
    RefreshFailed(String),
    Sync(SyncHealth),
    /// A background round brought changes in.
    SyncArrived(usize),
    SyncEvent(SyncEvent),
    Settings(Box<SettingsData>),
    /// The agent changed after settings were sent.
    Background(BackgroundStatus),
    /// Another database is open now, with how many tasks the last one held.
    DatabaseOpened {
        path: PathBuf,
        tasks: usize,
        previous: Option<(PathBuf, usize)>,
    },
    /// The database did not open. The one before it, if any, stays open.
    DatabaseFailed(PathBuf, String),
    /// The published gist's URL, or why there is none.
    Gist(Result<String, String>),
}

#[derive(Debug)]
pub struct Reply {
    pub request: u64,
    pub body: ReplyBody,
}

pub struct Launch {
    pub database: PathBuf,
    pub config: AppConfig,
    /// The file `config` was read from, read again whenever it changes.
    /// `None` keeps `config` as given.
    pub config_file: Option<PathBuf>,
    /// Replaces the installation's protected key storage. `None` keeps it, so
    /// each host mode decides on Keychain access itself.
    pub storage: Option<Arc<dyn ProtectedStorage>>,
    /// The running version, which the background host status records.
    pub version: String,
}

pub type Replies = mpsc::UnboundedReceiver<Reply>;

/// How the view arranges rows beyond what the store's view state holds.
#[derive(Debug, Clone, Default)]
pub struct Arrange {
    pub group: GroupBy,
    pub order: ViewOrder,
    pub metadata: Option<MetaFilter>,
    /// Tasks the metadata filter lets through.
    pub allowed: Option<HashSet<TaskId>>,
}
