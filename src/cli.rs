use crate::ids::WorkspaceId;
use std::fmt::Write as _;
use std::net::SocketAddr;
use std::path::PathBuf;

use clap::builder::styling::{AnsiColor, Effects, Style, Styles};
use clap::{ArgGroup, Args, CommandFactory, FromArgMatches, Parser, Subcommand};

const ACCENT_STYLE: Style = AnsiColor::Magenta.on_default();
const HEADING_STYLE: Style = AnsiColor::Magenta.on_default().effects(Effects::BOLD);
const LITERAL_STYLE: Style = Style::new();
const PLACEHOLDER_STYLE: Style = Style::new().effects(Effects::DIMMED);
const DESCRIPTION_STYLE: Style = Style::new();

const STYLES: Styles = Styles::styled()
    .header(HEADING_STYLE)
    .usage(HEADING_STYLE)
    .literal(LITERAL_STYLE)
    .placeholder(PLACEHOLDER_STYLE)
    .context(DESCRIPTION_STYLE)
    .context_value(AnsiColor::Yellow.on_default())
    .valid(AnsiColor::Green.on_default())
    .invalid(AnsiColor::Red.on_default().effects(Effects::BOLD))
    .error(AnsiColor::Red.on_default().effects(Effects::BOLD));

const HELP_SECTIONS: &[HelpSection] = &[
    HelpSection {
        heading: "TASKS",
        commands: &[
            "add",
            "list",
            "search",
            "context",
            "show",
            "edit",
            "note",
            "note-delete",
            "dep",
            "related",
            "epic",
            "text",
            "bulk-update",
            "delete",
            "restore",
            "recur",
        ],
    },
    HelpSection {
        heading: "WORKSPACE",
        commands: &["workspace", "project", "label", "metadata"],
    },
    HelpSection {
        heading: "SYNC",
        commands: &["sync", "server", "conflict", "daemon"],
    },
    HelpSection {
        heading: "INTERACTIVE",
        commands: &["tui", "demo"],
    },
    HelpSection {
        heading: "AGENTS",
        commands: &["prime", "skill"],
    },
    HelpSection {
        heading: "SETUP",
        commands: &["config", "doctor", "update"],
    },
    HelpSection {
        heading: "ATTACHMENTS",
        commands: &["attachment"],
    },
    HelpSection {
        heading: "DATA SAFETY",
        commands: &["backup", "export", "import"],
    },
];

struct HelpSection {
    heading: &'static str,
    commands: &'static [&'static str],
}

pub(crate) fn parse() -> Cli {
    let mut command = Cli::command();
    let help = render_top_level_help(&command);
    command = command.override_help(help);
    let matches = command.get_matches();
    Cli::from_arg_matches(&matches).expect("clap validates matches")
}

fn render_top_level_help(command: &clap::Command) -> String {
    let mut help = String::new();
    writeln!(&mut help, "Local-first task manager").unwrap();
    writeln!(&mut help).unwrap();
    writeln!(
        &mut help,
        "{} aven {} {}",
        paint_heading("USAGE:"),
        paint("[OPTIONS]", LITERAL_STYLE),
        paint("[COMMAND]", PLACEHOLDER_STYLE)
    )
    .unwrap();
    writeln!(&mut help).unwrap();

    for section in HELP_SECTIONS {
        render_section(&mut help, command, section);
    }

    render_help_section(&mut help);
    render_options_section(&mut help);
    help
}

fn render_section(help: &mut String, command: &clap::Command, section: &HelpSection) {
    writeln!(help, "{}", paint_heading(section.heading)).unwrap();
    let width = help_row_width(section.commands.iter().copied());
    for name in section.commands {
        let about = command_about(command, name).unwrap_or_default();
        render_row(help, name, &paint(name, LITERAL_STYLE), &about, width);
    }
    writeln!(help).unwrap();
}

fn render_help_section(help: &mut String) {
    writeln!(help, "{}", paint_heading("HELP")).unwrap();
    render_row(
        help,
        "help",
        &paint("help", LITERAL_STYLE),
        "Print this message or the help of the given subcommand(s)",
        help_row_width(["help"]),
    );
    writeln!(help).unwrap();
}

fn render_options_section(help: &mut String) {
    writeln!(help, "{}", paint_heading("OPTIONS")).unwrap();
    let width = help_row_width([
        "--db <DB>",
        "--workspace <WORKSPACE>",
        "-V, --version",
        "-h, --help",
    ]);
    render_row(
        help,
        "--db <DB>",
        &format!(
            "{} {}",
            paint("--db", LITERAL_STYLE),
            paint("<DB>", PLACEHOLDER_STYLE)
        ),
        "Use a specific SQLite database path",
        width,
    );
    render_row(
        help,
        "--workspace <WORKSPACE>",
        &format!(
            "{} {}",
            paint("--workspace", LITERAL_STYLE),
            paint("<WORKSPACE>", PLACEHOLDER_STYLE)
        ),
        "Use a specific workspace by name or key",
        width,
    );
    render_row(
        help,
        "-V, --version",
        &format!(
            "{}, {}",
            paint("-V", LITERAL_STYLE),
            paint("--version", LITERAL_STYLE)
        ),
        "Print version",
        width,
    );
    render_row(
        help,
        "-h, --help",
        &format!(
            "{}, {}",
            paint("-h", LITERAL_STYLE),
            paint("--help", LITERAL_STYLE)
        ),
        "Print help",
        width,
    );
}

fn command_about(command: &clap::Command, name: &str) -> Option<String> {
    command
        .get_subcommands()
        .find(|subcommand| subcommand.get_name() == name)
        .and_then(|subcommand| subcommand.get_about())
        .map(|about| about.to_string())
}

fn help_row_width<'a>(names: impl IntoIterator<Item = &'a str>) -> usize {
    names
        .into_iter()
        .map(str::len)
        .max()
        .unwrap_or_default()
        .saturating_add(2)
}

fn render_row(
    help: &mut String,
    plain_name: &str,
    styled_name: &str,
    description: &str,
    width: usize,
) {
    write!(help, "  {styled_name}").unwrap();
    for _ in plain_name.len()..width {
        help.push(' ');
    }
    writeln!(help, "{}", paint(description, DESCRIPTION_STYLE)).unwrap();
}

fn paint_heading(text: &str) -> String {
    format!(
        "{} {}",
        paint("›", ACCENT_STYLE),
        paint(text, HEADING_STYLE)
    )
}

fn paint(text: &str, style: Style) -> String {
    format!("{}{}{}", style.render(), text, style.render_reset())
}

#[derive(Parser)]
#[command(name = "aven")]
#[command(about = "Local-first task manager")]
#[command(version)]
#[command(styles = STYLES)]
pub struct Cli {
    #[arg(long, global = true, help = "Use a specific SQLite database path")]
    pub(crate) db: Option<PathBuf>,
    #[arg(long, global = true, help = "Use a specific workspace by name or key")]
    pub(crate) workspace: Option<String>,
    #[command(subcommand)]
    pub(crate) command: Option<Commands>,
}

#[derive(Subcommand)]
pub(crate) enum Commands {
    /// Create a task
    Add(AddArgs),
    /// Inspect and modify task dependencies
    Dep(DepCommand),
    /// Inspect and modify related-task links
    Related(RelatedCommand),
    /// Inspect and modify epic membership
    Epic(EpicCommand),
    /// Show a task context snapshot
    Context(ContextArgs),
    /// Show task details
    Show(ShowArgs),
    /// List tasks
    List(ListArgs),
    /// Search tasks in the active workspace
    Search(TaskSearchArgs),
    /// Apply field updates across many tasks
    BulkUpdate(BulkUpdateArgs),
    /// Emit workspace context for AI agents
    Prime(PrimeArgs),
    /// Edit task fields
    Edit(TaskEditArgs),
    /// Check for and install an aven update
    Update(SelfUpdateArgs),
    /// Append a note to a task
    Note(NoteArgs),
    /// Delete a note from a task
    NoteDelete(NoteDeleteArgs),
    /// Delete a task
    Delete(RefArgs),
    /// Restore a deleted task
    Restore(RefArgs),
    /// Manage recurring task series
    Recur(RecurCommand),
    /// Get, diff, and set long text fields safely
    Text(TextCommand),
    /// Manage labels
    Label(LabelCommand),
    /// Inspect and rename metadata fields
    Metadata(MetadataCommand),
    /// Manage projects and their paths
    Project(ProjectCommand),
    /// Manage workspaces
    Workspace(WorkspaceCommand),
    /// Inspect and resolve sync conflicts
    Conflict(ConflictCommand),
    /// Manage local configuration
    Config(ConfigCommand),
    /// Back up or restore the SQLite database
    Backup(BackupCommand),
    /// Export user data as portable JSON
    Export(ExportArgs),
    /// Import portable JSON data
    Import(ImportArgs),
    /// Print or install the coding-agent skill
    Skill(SkillCommand),
    /// Diagnose startup, configuration, database, and workspace state without repairs
    Doctor(DoctorArgs),
    /// Manage task attachments
    Attachment(AttachmentCommand),
    /// Run or manage the background daemon
    Daemon(DaemonArgs),
    /// Run the sync server
    Server(ServerArgs),
    /// Sync with a remote server
    Sync(SyncArgs),
    /// Open the terminal UI
    Tui(TuiArgs),
    /// Explore aven with disposable sample tasks
    Demo,
    #[command(hide = true)]
    Internal(InternalCommand),
}

#[derive(Args)]
pub(crate) struct SkillCommand {
    #[command(subcommand)]
    pub(crate) command: Option<SkillSubcommand>,
}

#[derive(Subcommand)]
pub(crate) enum SkillSubcommand {
    /// Install the aven skill for coding agents
    Install(SkillInstallArgs),
}

#[derive(Args)]
pub(crate) struct SkillInstallArgs {
    /// Target a coding agent (default: all detected)
    #[arg(long = "agent", value_enum)]
    pub(crate) agent: Vec<CodingAgentArg>,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CodingAgentArg {
    Claude,
    Opencode,
    Codex,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TuiViewArg {
    Queue,
    All,
    Open,
    Inbox,
    Active,
    Backlog,
    Todo,
    Done,
    Upcoming,
    Conflicts,
    Epics,
    Recurring,
    RecentActions,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TuiLayoutArg {
    List,
    Columns,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TuiPriorityArg {
    None,
    Low,
    Medium,
    High,
    Urgent,
}

impl TuiPriorityArg {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Urgent => "urgent",
        }
    }
}

#[derive(Args, Default)]
#[command(group(
    ArgGroup::new("tui_composer")
        .args(["add_task", "add_task_only"])
        .multiple(false)
))]
pub(crate) struct TuiArgs {
    /// Open this task's detail directly
    #[arg(
        value_name = "TASK_REF",
        conflicts_with_all = ["project", "view", "layout", "label", "priority", "add_task", "add_task_only"]
    )]
    pub(crate) task_ref: Option<String>,
    /// Start in this named view
    #[arg(long, value_enum, conflicts_with = "add_task_only")]
    pub(crate) view: Option<TuiViewArg>,
    /// Present the selected query as a list or columns
    #[arg(long, value_enum, conflicts_with = "add_task_only")]
    pub(crate) layout: Option<TuiLayoutArg>,
    /// Start in project scope; omit the value to infer from the current directory
    #[arg(short = 'p', long, num_args = 0..=1, default_missing_value = "")]
    pub(crate) project: Option<String>,
    /// Apply an initial label filter
    #[arg(long, value_name = "LABEL", conflicts_with = "add_task_only")]
    pub(crate) label: Option<String>,
    /// Apply an initial priority filter
    #[arg(long, value_enum, conflicts_with = "add_task_only")]
    pub(crate) priority: Option<TuiPriorityArg>,
    /// Open the add-task composer over the selected view
    #[arg(long)]
    pub(crate) add_task: bool,
    /// Show only the add-task composer and exit after submission
    #[arg(long)]
    pub(crate) add_task_only: bool,
    /// Use natural-language input in the add-task composer
    #[arg(long, requires = "tui_composer")]
    pub(crate) natural: bool,
}

#[derive(Args)]
pub(crate) struct InternalCommand {
    #[command(subcommand)]
    pub(crate) command: InternalSubcommand,
}

#[derive(Subcommand)]
pub(crate) enum InternalSubcommand {
    #[command(name = "demo-snapshot", hide = true)]
    DemoSnapshot(InternalDemoSnapshotArgs),
    #[command(name = "natural-add", hide = true)]
    NaturalAdd(InternalNaturalAddArgs),
}

#[derive(Args)]
pub(crate) struct InternalDemoSnapshotArgs {
    #[arg(long)]
    pub(crate) output: PathBuf,
}

#[derive(Args)]
pub(crate) struct InternalNaturalAddArgs {
    #[arg(long)]
    pub(crate) workspace_id: WorkspaceId,
    #[arg(long)]
    pub(crate) project: Option<String>,
    #[arg(long, allow_hyphen_values = true)]
    pub(crate) input: String,
    #[arg(long, hide = true)]
    pub(crate) tui_undo: bool,
    #[arg(long, hide = true)]
    pub(crate) tui_pid: Option<std::num::NonZeroU32>,
}

#[derive(Args)]
pub(crate) struct AddArgs {
    pub(crate) title: String,
    #[arg(long)]
    pub(crate) project: Option<String>,
    #[arg(long)]
    pub(crate) description: Option<String>,
    #[arg(long)]
    pub(crate) description_file: Option<PathBuf>,
    #[arg(long)]
    pub(crate) description_stdin: bool,
    #[arg(long, default_value = "none")]
    pub(crate) priority: String,
    #[arg(long)]
    pub(crate) status: Option<String>,
    #[arg(long)]
    pub(crate) label: Vec<String>,
    #[arg(long, value_name = "KEY=VALUE")]
    pub(crate) metadata: Vec<String>,
    #[arg(long, help = "Create the task as an epic container")]
    pub(crate) epic: bool,
    #[arg(
        long,
        conflicts_with_all = [
            "description",
            "description_file",
            "description_stdin",
            "priority",
            "status",
            "label",
            "metadata",
            "epic",
            "available_at",
            "due",
            "repeat",
            "repeat_at",
            "repeat_due",
            "time_zone",
            "repeat_start_on"
        ]
    )]
    pub(crate) natural: bool,
    #[arg(long, value_name = "WHEN")]
    pub(crate) available_at: Option<String>,
    #[arg(long, value_name = "WHEN")]
    pub(crate) due: Option<String>,
    #[arg(long, value_name = "RULE")]
    pub(crate) repeat: Option<String>,
    #[arg(long, value_name = "HH:MM")]
    pub(crate) repeat_at: Option<String>,
    #[arg(long, value_name = "same-day|none")]
    pub(crate) repeat_due: Option<String>,
    #[arg(long, value_name = "IANA_ZONE")]
    pub(crate) time_zone: Option<String>,
    #[arg(long, value_name = "YYYY-MM-DD")]
    pub(crate) repeat_start_on: Option<String>,
}

#[derive(Args)]
pub(crate) struct ShowArgs {
    pub(crate) task_ref: String,
    #[arg(long)]
    pub(crate) full: bool,
    #[arg(long, help = "Print machine-readable JSON")]
    pub(crate) json: bool,
}

#[derive(Args)]
pub(crate) struct ContextArgs {
    pub(crate) task_ref: String,
    #[arg(long, help = "Print machine-readable JSON")]
    pub(crate) json: bool,
}

#[derive(Args)]
pub(crate) struct ListArgs {
    #[arg(long)]
    pub(crate) project: Option<String>,
    #[arg(long)]
    pub(crate) status: Option<String>,
    #[arg(long)]
    pub(crate) priority: Option<String>,
    #[arg(long)]
    pub(crate) label: Option<String>,
    #[arg(long, value_name = "KEY=VALUE")]
    pub(crate) metadata: Vec<String>,
    #[arg(long, value_name = "KEY")]
    pub(crate) has_metadata: Vec<String>,
    #[arg(long, value_name = "KEY")]
    pub(crate) missing_metadata: Vec<String>,
    #[arg(
        long,
        conflicts_with_all = ["status", "all", "deleted", "upcoming"]
    )]
    pub(crate) open: bool,
    #[arg(long)]
    pub(crate) all: bool,
    #[arg(long)]
    pub(crate) deleted: bool,
    #[arg(long)]
    pub(crate) ready: bool,
    #[arg(long)]
    pub(crate) blocked: bool,
    #[arg(long)]
    pub(crate) epics: bool,
    #[arg(long)]
    pub(crate) upcoming: bool,
    #[arg(long)]
    pub(crate) overdue: bool,
    #[arg(long, help = "Show individual recurring occurrences")]
    pub(crate) expand_recurring: bool,
    #[arg(
        long,
        value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(1..),
        help = "Maximum result count (must be at least 1)"
    )]
    pub(crate) limit: Option<usize>,
    #[arg(long, help = "Print machine-readable JSON")]
    pub(crate) json: bool,
}

#[derive(Args)]
pub(crate) struct TaskSearchArgs {
    pub(crate) query: Vec<String>,
    #[arg(long, help = "Restrict matches to a project by key or name")]
    pub(crate) project: Option<String>,
    #[arg(long, value_name = "KEY=VALUE")]
    pub(crate) metadata: Vec<String>,
    #[arg(long, value_name = "KEY")]
    pub(crate) has_metadata: Vec<String>,
    #[arg(long, value_name = "KEY")]
    pub(crate) missing_metadata: Vec<String>,
    #[arg(
        long,
        default_value_t = 50,
        value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(1..),
        help = "Maximum result count (must be at least 1)"
    )]
    pub(crate) limit: usize,
    #[arg(long, help = "Include deleted tasks")]
    pub(crate) all: bool,
    #[arg(long, help = "Show individual recurring occurrences")]
    pub(crate) expand_recurring: bool,
    #[arg(long, help = "Print machine-readable JSON")]
    pub(crate) json: bool,
}

#[derive(Args)]
pub(crate) struct RecurCommand {
    #[command(subcommand)]
    pub(crate) command: RecurSubcommand,
}

#[derive(Subcommand)]
pub(crate) enum RecurSubcommand {
    /// List recurring series
    List(RecurListArgs),
    /// Show a recurring series
    Show(RecurShowArgs),
    /// Show recurring series history
    History(RecurHistoryArgs),
    /// Edit the template used by future occurrences
    Edit(Box<RecurEditArgs>),
    /// Skip the current occurrence
    Skip(RecurRefArgs),
    /// Pause a recurring series
    Pause(RecurRefArgs),
    /// Resume a paused recurring series
    Resume(RecurRefArgs),
    /// Stop future scheduling
    Stop(RecurStopArgs),
}

#[derive(Args)]
pub(crate) struct RecurListArgs {
    #[arg(long, help = "Print machine-readable JSON")]
    pub(crate) json: bool,
}

#[derive(Args)]
pub(crate) struct RecurShowArgs {
    pub(crate) series_ref: String,
    #[arg(long, help = "Print machine-readable JSON")]
    pub(crate) json: bool,
}

#[derive(Args)]
pub(crate) struct RecurHistoryArgs {
    pub(crate) series_ref: String,
    #[arg(long, default_value_t = 0)]
    pub(crate) offset: usize,
    #[arg(
        long,
        default_value_t = 100,
        value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(1..=500),
        help = "Maximum result count (1-500)"
    )]
    pub(crate) limit: usize,
    #[arg(long, help = "Print machine-readable JSON")]
    pub(crate) json: bool,
}

#[derive(Args)]
pub(crate) struct RecurEditArgs {
    pub(crate) series_ref: String,
    #[arg(long)]
    pub(crate) title: Option<String>,
    #[arg(long)]
    pub(crate) description: Option<String>,
    #[arg(long)]
    pub(crate) description_file: Option<PathBuf>,
    #[arg(long)]
    pub(crate) description_stdin: bool,
    #[arg(long)]
    pub(crate) project: Option<String>,
    #[arg(long)]
    pub(crate) status: Option<String>,
    #[arg(long)]
    pub(crate) priority: Option<String>,
    #[arg(long, value_name = "LABEL")]
    pub(crate) label: Vec<String>,
    #[arg(long, value_name = "KEY=VALUE")]
    pub(crate) metadata: Vec<String>,
    #[arg(long, value_name = "KEY")]
    pub(crate) remove_metadata: Vec<String>,
    #[arg(long, value_name = "HH:MM|none")]
    pub(crate) repeat_at: Option<String>,
    #[arg(long, value_name = "same-day|none")]
    pub(crate) repeat_due: Option<String>,
}

#[derive(Args)]
pub(crate) struct RecurRefArgs {
    pub(crate) series_ref: String,
}

#[derive(Args)]
pub(crate) struct RecurStopArgs {
    pub(crate) series_ref: String,
    #[arg(long)]
    pub(crate) skip_current: bool,
}

#[derive(Args)]
pub(crate) struct DepCommand {
    #[command(subcommand)]
    pub(crate) command: DepSubcommand,
}

#[derive(Subcommand)]
pub(crate) enum DepSubcommand {
    /// Add a dependency to a task
    Add(DepAddArgs),
    /// Remove a dependency from a task
    Remove(DepRemoveArgs),
    /// List a task's dependencies
    List(DepListArgs),
}

#[derive(Args)]
pub(crate) struct DepAddArgs {
    pub(crate) task_ref: String,
    pub(crate) depends_on_ref: String,
}

#[derive(Args)]
pub(crate) struct DepRemoveArgs {
    pub(crate) task_ref: String,
    pub(crate) depends_on_ref: String,
}

#[derive(Args)]
pub(crate) struct DepListArgs {
    pub(crate) task_ref: String,
    #[arg(long, help = "Print machine-readable JSON")]
    pub(crate) json: bool,
}

#[derive(Args)]
pub(crate) struct RelatedCommand {
    #[command(subcommand)]
    pub(crate) command: RelatedSubcommand,
}

#[derive(Subcommand)]
pub(crate) enum RelatedSubcommand {
    /// Link two related tasks
    Add(RelatedMutationArgs),
    /// Unlink two related tasks
    Remove(RelatedMutationArgs),
    /// List a task's related tasks
    List(RelatedListArgs),
}

#[derive(Args)]
pub(crate) struct RelatedMutationArgs {
    pub(crate) task_ref: String,
    pub(crate) related_ref: String,
}

#[derive(Args)]
pub(crate) struct RelatedListArgs {
    pub(crate) task_ref: String,
    #[arg(long, help = "Print machine-readable JSON")]
    pub(crate) json: bool,
}

#[derive(Args)]
pub(crate) struct EpicCommand {
    #[command(subcommand)]
    pub(crate) command: EpicSubcommand,
}

#[derive(Subcommand)]
pub(crate) enum EpicSubcommand {
    /// Add a task to an epic
    Add(EpicAddArgs),
    /// Remove a task from an epic
    Remove(EpicRemoveArgs),
    /// List an epic's child tasks
    List(EpicListArgs),
}

#[derive(Args)]
pub(crate) struct EpicAddArgs {
    pub(crate) child_ref: String,
    pub(crate) epic_ref: String,
}

#[derive(Args)]
pub(crate) struct EpicRemoveArgs {
    pub(crate) child_ref: String,
    pub(crate) epic_ref: String,
}

#[derive(Args)]
pub(crate) struct EpicListArgs {
    pub(crate) epic_ref: String,
    #[arg(long, help = "Print machine-readable JSON")]
    pub(crate) json: bool,
}

#[derive(Args)]
pub(crate) struct BulkUpdateArgs {
    #[arg(long)]
    pub(crate) project: Option<String>,
    #[arg(long)]
    pub(crate) status: Option<String>,
    #[arg(long)]
    pub(crate) priority: Option<String>,
    #[arg(long)]
    pub(crate) filter_label: Option<String>,
    #[arg(long)]
    pub(crate) all: bool,
    #[arg(long)]
    pub(crate) include_deleted: bool,
    #[arg(long)]
    pub(crate) dry_run: bool,
    #[arg(long)]
    pub(crate) set_status: Option<String>,
    #[arg(long)]
    pub(crate) set_priority: Option<String>,
    #[arg(long)]
    pub(crate) set_project: Option<String>,
    #[arg(long)]
    pub(crate) label: Vec<String>,
    #[arg(long)]
    pub(crate) remove_label: Vec<String>,
    #[arg(long, value_name = "KEY=VALUE")]
    pub(crate) metadata: Vec<String>,
    #[arg(long, value_name = "KEY")]
    pub(crate) remove_metadata: Vec<String>,
}

#[derive(Args)]
pub(crate) struct PrimeArgs {
    #[arg(long)]
    pub(crate) project: Option<String>,
    #[arg(
        long,
        value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(1..),
        help = "Maximum result count (must be at least 1)"
    )]
    pub(crate) limit: Option<usize>,
    #[arg(long, help = "Print machine-readable JSON")]
    pub(crate) json: bool,
}

#[derive(Args)]
pub(crate) struct TaskEditArgs {
    pub(crate) task_ref: String,
    #[arg(long)]
    pub(crate) title: Option<String>,
    #[arg(long)]
    pub(crate) description: Option<String>,
    #[arg(long)]
    pub(crate) description_file: Option<PathBuf>,
    #[arg(long)]
    pub(crate) description_stdin: bool,
    #[arg(long)]
    pub(crate) project: Option<String>,
    #[arg(long)]
    pub(crate) status: Option<String>,
    #[arg(long)]
    pub(crate) priority: Option<String>,
    #[arg(long, value_name = "WHEN")]
    pub(crate) available_at: Option<String>,
    #[arg(long)]
    pub(crate) clear_available_at: bool,
    #[arg(long, value_name = "WHEN")]
    pub(crate) due: Option<String>,
    #[arg(long)]
    pub(crate) clear_due: bool,
    #[arg(long, value_name = "on|off")]
    pub(crate) epic: Option<String>,
    #[arg(long)]
    pub(crate) label: Vec<String>,
    #[arg(long)]
    pub(crate) remove_label: Vec<String>,
    #[arg(long, value_name = "KEY=VALUE")]
    pub(crate) metadata: Vec<String>,
    #[arg(long, value_name = "KEY")]
    pub(crate) remove_metadata: Vec<String>,
}

#[derive(Args)]
pub(crate) struct SelfUpdateArgs {
    #[arg(long, help = "Install an available direct update")]
    pub(crate) yes: bool,
}

#[derive(Args)]
pub(crate) struct NoteArgs {
    pub(crate) task_ref: String,
    pub(crate) text: Option<String>,
    #[arg(long)]
    pub(crate) file: Option<PathBuf>,
    #[arg(long)]
    pub(crate) stdin: bool,
}

#[derive(Args)]
pub(crate) struct NoteDeleteArgs {
    pub(crate) task_ref: String,
    pub(crate) note_id: String,
}

#[derive(Args)]
pub(crate) struct LabelListArgs {
    #[arg(long)]
    pub(crate) search: Option<String>,
    #[arg(
        long,
        value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(1..),
        help = "Maximum result count (must be at least 1)"
    )]
    pub(crate) limit: Option<usize>,
    #[arg(long, help = "Print machine-readable JSON")]
    pub(crate) json: bool,
}

#[derive(Args)]
pub(crate) struct ProjectListArgs {
    #[arg(long)]
    pub(crate) search: Option<String>,
    #[arg(
        long,
        value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(1..),
        help = "Maximum result count (must be at least 1)"
    )]
    pub(crate) limit: Option<usize>,
    #[arg(long, help = "Print machine-readable JSON")]
    pub(crate) json: bool,
}

#[derive(Args)]
pub(crate) struct RefArgs {
    pub(crate) task_ref: String,
}

#[derive(Args)]
pub(crate) struct LabelCommand {
    #[command(subcommand)]
    pub(crate) command: LabelSubcommand,
}

#[derive(Subcommand)]
pub(crate) enum LabelSubcommand {
    /// Create a label
    Create { name: String },
    /// Delete a label
    Delete { name: String },
    /// List or search labels
    List(LabelListArgs),
}

#[derive(Args)]
pub(crate) struct MetadataCommand {
    #[command(subcommand)]
    pub(crate) command: MetadataSubcommand,
}

#[derive(Subcommand)]
pub(crate) enum MetadataSubcommand {
    /// List metadata fields and their usage
    List {
        #[arg(long, help = "Print machine-readable JSON")]
        json: bool,
    },
    /// Show a metadata field
    Show {
        key: String,
        #[arg(long, help = "Print machine-readable JSON")]
        json: bool,
    },
    /// Rename a metadata field
    Rename { key: String, new_key: String },
}

#[derive(Args)]
pub(crate) struct ProjectCommand {
    #[command(subcommand)]
    pub(crate) command: ProjectSubcommand,
}

#[derive(Subcommand)]
pub(crate) enum ProjectSubcommand {
    /// Create a project
    Create {
        name: String,
        #[arg(long)]
        path: Option<PathBuf>,
    },
    /// Delete a project
    Delete { project: String },
    /// List or search projects
    List(ProjectListArgs),
    /// Rename a project
    Rename {
        project: String,
        new_name: String,
        #[arg(long)]
        prefix: Option<String>,
    },
    /// Manage project path mappings
    Path {
        #[command(subcommand)]
        command: ProjectPathSubcommand,
    },
}

#[derive(Subcommand)]
pub(crate) enum ProjectPathSubcommand {
    /// Add a path mapping to a project
    Add { project: String, path: PathBuf },
    /// Remove a path mapping from a project
    Remove { project: String, path: PathBuf },
    /// List project path mappings
    List { project: Option<String> },
}

#[derive(Args)]
pub(crate) struct WorkspaceCommand {
    #[command(subcommand)]
    pub(crate) command: WorkspaceSubcommand,
}

#[derive(Args)]
pub(crate) struct BackupCommand {
    #[command(subcommand)]
    pub(crate) command: Option<BackupSubcommand>,
    #[arg(long)]
    pub(crate) output: Option<PathBuf>,
}

#[derive(Subcommand)]
pub(crate) enum BackupSubcommand {
    /// Restore the database from a backup
    Restore(BackupRestoreArgs),
}

#[derive(Args)]
pub(crate) struct BackupRestoreArgs {
    pub(crate) path: PathBuf,
    #[arg(long)]
    pub(crate) yes: bool,
}

#[derive(Args)]
pub(crate) struct ExportArgs {
    #[arg(long)]
    pub(crate) output: PathBuf,
}

#[derive(Args)]
pub(crate) struct ImportArgs {
    pub(crate) path: PathBuf,
    #[arg(long)]
    pub(crate) yes: bool,
}

#[derive(Args)]
pub(crate) struct DoctorArgs {
    /// Run deeper read-only SQLite, relationship, and attachment checks
    #[arg(long)]
    pub(crate) integrity: bool,
    /// Print machine-readable JSON
    #[arg(long)]
    pub(crate) json: bool,
    /// Exit nonzero when the report contains error-level findings
    #[arg(long)]
    pub(crate) fail_on_error: bool,
}

#[derive(Subcommand)]
pub(crate) enum WorkspaceSubcommand {
    /// List workspaces
    List,
    /// Create a workspace
    Create { name: String },
    /// Rename a workspace
    Rename { workspace: String, new_name: String },
}

#[derive(Args)]
pub(crate) struct ConflictCommand {
    #[command(subcommand)]
    pub(crate) command: ConflictSubcommand,
}

#[derive(Subcommand)]
pub(crate) enum ConflictSubcommand {
    /// List unresolved sync conflicts
    List {
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        field: Option<String>,
        #[arg(
            long,
            value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(1..),
            help = "Maximum result count (must be at least 1)"
        )]
        limit: Option<usize>,
        #[arg(long, help = "Print machine-readable JSON")]
        json: bool,
    },
    /// Show a conflict as a text diff
    Diff { task_ref: String, field: String },
    /// Export conflicting values to files
    Export {
        task_ref: String,
        field: String,
        #[arg(long)]
        dir: PathBuf,
    },
    /// Show conflict details for a task
    Show {
        task_ref: String,
        #[arg(long)]
        field: Option<String>,
        #[arg(long, help = "Print machine-readable JSON")]
        json: bool,
    },
    /// Resolve a sync conflict
    Resolve {
        task_ref: String,
        field: String,
        #[arg(long = "use")]
        use_variant: Option<String>,
        #[arg(long)]
        value: Option<String>,
        #[arg(long)]
        value_file: Option<PathBuf>,
        #[arg(long)]
        value_stdin: bool,
    },
}

#[derive(Args)]
pub(crate) struct ConfigCommand {
    #[command(subcommand)]
    pub(crate) command: ConfigSubcommand,
}

#[derive(Args)]
pub(crate) struct AttachmentCommand {
    #[command(subcommand)]
    pub(crate) command: AttachmentSubcommand,
}

#[derive(Subcommand)]
pub(crate) enum AttachmentSubcommand {
    /// Attach a file to a task
    Add(AttachmentAddArgs),
    /// List attachments for a task
    List(AttachmentListArgs),
    /// Get attachment metadata and optionally write bytes
    Get(AttachmentGetArgs),
    /// Delete (tombstone) an attachment
    Delete(AttachmentDeleteArgs),
    /// Inspect or prune eligible attachment blobs
    Prune(AttachmentPruneArgs),
}

#[derive(Args)]
pub(crate) struct AttachmentAddArgs {
    pub(crate) task_ref: String,
    pub(crate) path: PathBuf,
    /// Alternative text for the image
    #[arg(long)]
    pub(crate) alt: Option<String>,
    /// Override the filename stored in metadata
    #[arg(long)]
    pub(crate) filename: Option<String>,
    /// Declared media type, checked against the image bytes
    #[arg(long = "media-type")]
    pub(crate) media_type: Option<String>,
    /// Optimize supported image formats before storing bytes
    #[arg(long, conflicts_with = "no_optimize")]
    pub(crate) optimize: bool,
    /// Preserve attachment bytes exactly
    #[arg(long)]
    pub(crate) no_optimize: bool,
    /// Print machine-readable JSON
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Args)]
pub(crate) struct AttachmentListArgs {
    pub(crate) task_ref: String,
    /// Include deleted (tombstoned) attachments
    #[arg(long)]
    pub(crate) all: bool,
    /// Print machine-readable JSON
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Args)]
pub(crate) struct AttachmentGetArgs {
    pub(crate) attachment_id: String,
    /// Write bytes to this path
    #[arg(long)]
    pub(crate) output: Option<PathBuf>,
    /// Include deleted attachments
    #[arg(long)]
    pub(crate) all: bool,
    /// Print machine-readable JSON
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Args)]
pub(crate) struct AttachmentDeleteArgs {
    pub(crate) attachment_id: String,
    /// Print machine-readable JSON
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Args)]
pub(crate) struct AttachmentPruneArgs {
    /// Apply deletion. The default is a dry run.
    #[arg(long, conflicts_with = "dry_run")]
    pub(crate) apply: bool,
    /// Inspect eligible blobs without deleting them
    #[arg(long)]
    pub(crate) dry_run: bool,
    /// Print machine-readable JSON
    #[arg(long)]
    pub(crate) json: bool,
}

impl AttachmentSubcommand {
    pub(crate) fn wakes_daemon(&self) -> bool {
        matches!(self, Self::Add(_) | Self::Delete(_))
    }
}

#[derive(Args)]
pub(crate) struct TextCommand {
    #[command(subcommand)]
    pub(crate) command: TextSubcommand,
}

#[derive(Subcommand)]
pub(crate) enum TextSubcommand {
    /// Read a long text field
    Get(TextGetArgs),
    /// Compare a long text field with a file
    Diff(TextDiffArgs),
    /// Update a long text field safely
    Set(TextSetArgs),
}

#[derive(Args)]
pub(crate) struct TextGetArgs {
    pub(crate) task_ref: String,
    pub(crate) field: String,
    #[arg(long)]
    pub(crate) raw: bool,
    #[arg(long)]
    pub(crate) output: Option<PathBuf>,
}

#[derive(Args)]
pub(crate) struct TextDiffArgs {
    pub(crate) task_ref: String,
    pub(crate) field: String,
    #[arg(long)]
    pub(crate) file: PathBuf,
}

#[derive(Args)]
pub(crate) struct TextSetArgs {
    pub(crate) task_ref: String,
    pub(crate) field: String,
    #[arg(long)]
    pub(crate) file: Option<PathBuf>,
    #[arg(long)]
    pub(crate) stdin: bool,
    #[arg(long)]
    pub(crate) if_sha256: String,
}

#[derive(Subcommand)]
pub(crate) enum ConfigSubcommand {
    /// Create the local configuration file
    Init,
    /// Show the local configuration file
    Show,
    /// Get a configuration value
    Get(ConfigGetArgs),
    /// Set a configuration value
    Set(ConfigSetArgs),
}

#[derive(Args)]
pub(crate) struct ConfigGetArgs {
    pub(crate) key: ConfigKey,
}

#[derive(Args)]
pub(crate) struct ConfigSetArgs {
    pub(crate) key: ConfigKey,
    /// New value, or null to clear an optional setting
    pub(crate) value: String,
}

#[derive(clap::ValueEnum, Clone, Copy)]
pub(crate) enum ConfigKey {
    #[value(name = "sync.enabled")]
    SyncEnabled,
    #[value(name = "sync.server_url")]
    SyncServerUrl,
    #[value(name = "sync.interval_seconds")]
    SyncIntervalSeconds,
    #[value(name = "update.automatic_checks")]
    UpdateAutomaticChecks,
    #[value(name = "local.db_path")]
    LocalDbPath,
    #[value(name = "local.image_optimization")]
    LocalImageOptimization,
}

#[derive(Args)]
pub(crate) struct DaemonArgs {
    #[command(subcommand)]
    pub(crate) command: Option<DaemonSubcommand>,
}

#[derive(Subcommand)]
pub(crate) enum DaemonSubcommand {
    /// Install the background daemon
    Install(DaemonInstallArgs),
    /// Uninstall the background daemon
    Uninstall,
    /// Restart the background daemon
    Restart,
    /// Repair the background daemon installation
    Repair(DaemonRepairArgs),
}

#[derive(Args)]
pub(crate) struct DaemonInstallArgs {
    #[arg(
        long,
        value_name = "PATH",
        help = "Write this executable path into the LaunchAgent"
    )]
    pub(crate) program: Option<PathBuf>,
}

#[derive(Args)]
pub(crate) struct DaemonRepairArgs {
    #[arg(long, help = "Succeed without changes when the LaunchAgent is absent")]
    pub(crate) if_installed: bool,
    #[arg(
        long,
        value_name = "PATH",
        help = "Write this executable path into the LaunchAgent"
    )]
    pub(crate) program: Option<PathBuf>,
}
#[derive(Args)]
pub(crate) struct ServerArgs {
    #[arg(long, default_value = "127.0.0.1:0")]
    pub(crate) bind: SocketAddr,
    #[arg(long)]
    pub(crate) data: PathBuf,
    #[arg(long)]
    pub(crate) unsafe_public_bind: bool,
}

#[derive(Args)]
pub(crate) struct SyncArgs {
    #[arg(long)]
    pub(crate) server: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn help_rows_align_to_each_section_longest_command() {
        let commands = ["add", "command-name-that-exceeds-fixed-width"];
        let width = help_row_width(commands);
        let mut rendered = String::new();

        for command in commands {
            render_row(&mut rendered, command, command, "description", width);
        }

        let rows = rendered.lines().collect::<Vec<_>>();
        let description_columns = rows
            .iter()
            .map(|row| row.find("description").unwrap())
            .collect::<Vec<_>>();
        assert_eq!(description_columns[0], description_columns[1]);
        assert!(rows[1].contains("command-name-that-exceeds-fixed-width  description"));
    }

    #[test]
    fn top_level_help_sections_match_visible_commands() {
        let command = Cli::command();
        let visible = command
            .get_subcommands()
            .filter(|subcommand| !subcommand.is_hide_set())
            .map(clap::Command::get_name)
            .collect::<BTreeSet<_>>();
        let listed_entries = HELP_SECTIONS
            .iter()
            .flat_map(|section| section.commands.iter().copied())
            .collect::<Vec<_>>();
        let mut seen = BTreeSet::new();
        let duplicates = listed_entries
            .iter()
            .copied()
            .filter(|name| !seen.insert(*name))
            .collect::<Vec<_>>();
        let listed = listed_entries.into_iter().collect::<BTreeSet<_>>();
        let missing = visible.difference(&listed).copied().collect::<Vec<_>>();
        let invalid = listed.difference(&visible).copied().collect::<Vec<_>>();

        assert!(
            missing.is_empty() && invalid.is_empty() && duplicates.is_empty(),
            "top-level HELP_SECTIONS drifted\nmissing visible commands: {}\nlisted names without visible commands: {}\nduplicate commands: {}",
            missing.join(", "),
            invalid.join(", "),
            duplicates.join(", ")
        );
    }

    #[test]
    fn visible_subcommands_have_help_descriptions() {
        fn collect_missing(command: &clap::Command, parent_path: &str, missing: &mut Vec<String>) {
            for subcommand in command
                .get_subcommands()
                .filter(|subcommand| !subcommand.is_hide_set())
            {
                let path = format!("{parent_path} {}", subcommand.get_name());
                let description = subcommand
                    .get_about()
                    .map(|about| about.to_string())
                    .unwrap_or_default();
                if description.trim().is_empty() {
                    missing.push(path.clone());
                }
                collect_missing(subcommand, &path, missing);
            }
        }

        let mut missing = Vec::new();
        collect_missing(&Cli::command(), "aven", &mut missing);

        assert!(
            missing.is_empty(),
            "visible subcommands missing help descriptions: {}",
            missing.join(", ")
        );
    }

    #[test]
    fn omitted_command_is_accepted_for_default_tui_launch() {
        let parsed = Cli::try_parse_from(["aven"]).unwrap();
        assert!(parsed.command.is_none());

        let parsed = Cli::try_parse_from(["aven", "--db", "local.db"]).unwrap();
        assert!(parsed.command.is_none());
    }

    #[test]
    fn application_update_and_task_edit_are_distinct_commands() {
        let update = Cli::try_parse_from(["aven", "update"]).unwrap();
        assert!(matches!(
            update.command,
            Some(Commands::Update(SelfUpdateArgs { yes: false }))
        ));

        let edit = Cli::try_parse_from(["aven", "edit", "APP-1234", "--status", "active"]).unwrap();
        assert!(matches!(edit.command, Some(Commands::Edit(_))));
        assert!(Cli::try_parse_from(["aven", "edit"]).is_err());
        assert!(Cli::try_parse_from(["aven", "update", "APP-1234"]).is_err());
    }

    #[test]
    fn conflict_resolve_parses_use_variant() {
        let parsed = Cli::try_parse_from([
            "aven", "conflict", "resolve", "APP-1234", "title", "--use", "remote",
        ])
        .unwrap();
        let Some(Commands::Conflict(ConflictCommand {
            command:
                ConflictSubcommand::Resolve {
                    task_ref,
                    field,
                    use_variant,
                    value,
                    value_file,
                    value_stdin,
                },
        })) = parsed.command
        else {
            panic!("expected conflict resolve command");
        };
        assert_eq!(task_ref, "APP-1234");
        assert_eq!(field, "title");
        assert_eq!(use_variant.as_deref(), Some("remote"));
        assert_eq!(value, None);
        assert_eq!(value_file, None);
        assert!(!value_stdin);

        assert!(
            Cli::try_parse_from([
                "aven",
                "conflict",
                "resolve",
                "APP-1234",
                "title",
                "--use-variant",
                "remote",
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from(["aven", "conflict", "resolve", "APP-1234", "title", "--use",])
                .is_err()
        );
    }

    #[test]
    fn label_and_project_lists_parse_command_specific_arguments() {
        let label = Cli::try_parse_from([
            "aven", "label", "list", "--search", "bug", "--limit", "3", "--json",
        ])
        .unwrap();
        let Some(Commands::Label(LabelCommand {
            command: LabelSubcommand::List(label_args),
        })) = label.command
        else {
            panic!("expected label list command");
        };
        assert_eq!(label_args.search.as_deref(), Some("bug"));
        assert_eq!(label_args.limit, Some(3));
        assert!(label_args.json);

        let project = Cli::try_parse_from([
            "aven", "project", "list", "--search", "agent", "--limit", "5", "--json",
        ])
        .unwrap();
        let Some(Commands::Project(ProjectCommand {
            command: ProjectSubcommand::List(project_args),
        })) = project.command
        else {
            panic!("expected project list command");
        };
        assert_eq!(project_args.search.as_deref(), Some("agent"));
        assert_eq!(project_args.limit, Some(5));
        assert!(project_args.json);
    }

    #[test]
    fn result_limits_preserve_defaults_and_validate_explicit_values() {
        let search = Cli::try_parse_from(["aven", "search", "task"]).unwrap();
        let Some(Commands::Search(search_args)) = search.command else {
            panic!("expected search command");
        };
        assert_eq!(search_args.limit, 50);

        let history = Cli::try_parse_from(["aven", "recur", "history", "RCR-1234"]).unwrap();
        let Some(Commands::Recur(RecurCommand {
            command: RecurSubcommand::History(history_args),
        })) = history.command
        else {
            panic!("expected recurrence history command");
        };
        assert_eq!(history_args.limit, 100);

        let list = Cli::try_parse_from(["aven", "list"]).unwrap();
        let Some(Commands::List(list_args)) = list.command else {
            panic!("expected list command");
        };
        assert_eq!(list_args.limit, None);

        for arguments in [
            vec!["aven", "list", "--limit", "0"],
            vec!["aven", "search", "task", "--limit", "0"],
            vec!["aven", "recur", "history", "RCR-1234", "--limit", "0"],
            vec!["aven", "prime", "--limit", "0"],
            vec!["aven", "label", "list", "--limit", "0"],
            vec!["aven", "project", "list", "--limit", "0"],
            vec!["aven", "conflict", "list", "--limit", "0"],
        ] {
            let error = Cli::try_parse_from(arguments).err().unwrap();
            assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
        }

        assert!(Cli::try_parse_from(["aven", "search", "task", "--limit", "1"]).is_ok());
        assert!(
            Cli::try_parse_from(["aven", "recur", "history", "RCR-1234", "--limit", "500",])
                .is_ok()
        );
        for limit in ["501", "900"] {
            let error =
                Cli::try_parse_from(["aven", "recur", "history", "RCR-1234", "--limit", limit])
                    .err()
                    .unwrap();
            assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
        }
    }

    #[test]
    fn internal_workspace_ids_are_validated_by_clap() {
        let parsed = Cli::try_parse_from([
            "aven",
            "internal",
            "natural-add",
            "--workspace-id",
            "0123456789ABCDEF",
            "--input",
            "task",
        ])
        .unwrap();
        let Some(Commands::Internal(InternalCommand {
            command: InternalSubcommand::NaturalAdd(args),
        })) = parsed.command
        else {
            panic!("expected internal natural-add command");
        };
        assert_eq!(args.workspace_id.as_str(), "0123456789ABCDEF");

        assert!(
            Cli::try_parse_from([
                "aven",
                "internal",
                "natural-add",
                "--workspace-id",
                "invalid",
                "--input",
                "task",
            ])
            .is_err()
        );
    }

    #[test]
    fn tui_launch_arguments_compose_browse_state() {
        let parsed = Cli::try_parse_from([
            "aven",
            "tui",
            "--project",
            "app",
            "--view",
            "all",
            "--layout",
            "columns",
            "--label",
            "bug",
            "--priority",
            "urgent",
            "--add-task",
            "--natural",
        ])
        .unwrap();
        let Some(Commands::Tui(args)) = parsed.command else {
            panic!("expected tui command");
        };
        assert_eq!(args.project.as_deref(), Some("app"));
        assert_eq!(args.view, Some(TuiViewArg::All));
        assert_eq!(args.layout, Some(TuiLayoutArg::Columns));
        assert_eq!(args.label.as_deref(), Some("bug"));
        assert_eq!(args.priority, Some(TuiPriorityArg::Urgent));
        assert!(args.add_task);
        assert!(args.natural);
    }

    #[test]
    fn tui_launch_parses_all_typed_values() {
        for view in [
            "queue",
            "all",
            "open",
            "inbox",
            "active",
            "backlog",
            "todo",
            "done",
            "upcoming",
            "conflicts",
            "epics",
            "recurring",
            "recent-actions",
        ] {
            assert!(Cli::try_parse_from(["aven", "tui", "--view", view]).is_ok());
        }
        for layout in ["list", "columns"] {
            assert!(Cli::try_parse_from(["aven", "tui", "--layout", layout]).is_ok());
        }
        for priority in ["none", "low", "medium", "high", "urgent"] {
            assert!(Cli::try_parse_from(["aven", "tui", "--priority", priority]).is_ok());
        }
        assert!(Cli::try_parse_from(["aven", "tui", "--view", "search"]).is_err());
        assert!(Cli::try_parse_from(["aven", "tui", "--priority", "critical"]).is_err());
    }

    #[test]
    fn tui_launch_parses_task_and_optional_project_value() {
        let parsed = Cli::try_parse_from(["aven", "tui", "APP-1234"]).unwrap();
        let Some(Commands::Tui(args)) = parsed.command else {
            panic!("expected tui command");
        };
        assert_eq!(args.task_ref.as_deref(), Some("APP-1234"));
        assert_eq!(args.project, None);

        let parsed = Cli::try_parse_from(["aven", "tui", "-p", "app"]).unwrap();
        let Some(Commands::Tui(args)) = parsed.command else {
            panic!("expected tui command");
        };
        assert_eq!(args.project.as_deref(), Some("app"));
        assert_eq!(args.task_ref, None);

        let parsed = Cli::try_parse_from(["aven", "tui", "-p", "--view", "inbox"]).unwrap();
        let Some(Commands::Tui(args)) = parsed.command else {
            panic!("expected tui command");
        };
        assert_eq!(args.project.as_deref(), Some(""));
        assert_eq!(args.view, Some(TuiViewArg::Inbox));
    }

    #[test]
    fn tui_launch_rejects_conflicting_targets_and_modes() {
        for arguments in [
            vec!["aven", "tui", "APP-1234", "--view", "open"],
            vec!["aven", "tui", "APP-1234", "--project", "app"],
            vec!["aven", "tui", "APP-1234", "--label", "bug"],
            vec!["aven", "tui", "APP-1234", "--priority", "high"],
            vec!["aven", "tui", "APP-1234", "--add-task"],
            vec!["aven", "tui", "--add-task", "--add-task-only"],
            vec!["aven", "tui", "--add-task-only", "--view", "inbox"],
            vec!["aven", "tui", "--add-task-only", "--label", "bug"],
            vec!["aven", "tui", "--natural"],
        ] {
            assert!(Cli::try_parse_from(arguments).is_err());
        }

        assert!(
            Cli::try_parse_from([
                "aven",
                "tui",
                "--add-task-only",
                "--project",
                "app",
                "--natural",
            ])
            .is_ok()
        );
    }
}
