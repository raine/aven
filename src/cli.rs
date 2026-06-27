use std::fmt::Write as _;
use std::net::SocketAddr;
use std::path::PathBuf;

use clap::builder::styling::{AnsiColor, Effects, Style, Styles};
use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand};

const HEADING_STYLE: Style = AnsiColor::Blue.on_default().effects(Effects::BOLD);
const LITERAL_STYLE: Style = AnsiColor::Magenta.on_default();
const PLACEHOLDER_STYLE: Style = Style::new();
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
            "update",
            "note",
            "note-delete",
            "dep",
            "text",
            "bulk-update",
            "delete",
            "restore",
        ],
    },
    HelpSection {
        heading: "WORKSPACE",
        commands: &["workspace", "project", "label"],
    },
    HelpSection {
        heading: "SYNC",
        commands: &["sync", "server", "conflict", "daemon"],
    },
    HelpSection {
        heading: "INTERACTIVE",
        commands: &["tui", "tmux"],
    },
    HelpSection {
        heading: "AGENTS",
        commands: &["prime", "skill"],
    },
    HelpSection {
        heading: "SETUP",
        commands: &["config", "doctor"],
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
        paint("USAGE:", HEADING_STYLE),
        paint("[OPTIONS]", LITERAL_STYLE),
        paint("<COMMAND>", PLACEHOLDER_STYLE)
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
    writeln!(help, "{}", paint(section.heading, HEADING_STYLE)).unwrap();
    for name in section.commands {
        let about = command_about(command, name).unwrap_or_default();
        render_row(help, name, &paint(name, LITERAL_STYLE), &about, 13);
    }
    writeln!(help).unwrap();
}

fn render_help_section(help: &mut String) {
    writeln!(help, "{}", paint("HELP", HEADING_STYLE)).unwrap();
    render_row(
        help,
        "help",
        &paint("help", LITERAL_STYLE),
        "Print this message or the help of the given subcommand(s)",
        13,
    );
    writeln!(help).unwrap();
}

fn render_options_section(help: &mut String) {
    writeln!(help, "{}", paint("OPTIONS", HEADING_STYLE)).unwrap();
    render_row(
        help,
        "--db <DB>",
        &format!(
            "{} {}",
            paint("--db", LITERAL_STYLE),
            paint("<DB>", PLACEHOLDER_STYLE)
        ),
        "Use a specific SQLite database path",
        27,
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
        27,
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
        27,
    );
}

fn command_about(command: &clap::Command, name: &str) -> Option<String> {
    command
        .get_subcommands()
        .find(|subcommand| subcommand.get_name() == name)
        .and_then(|subcommand| subcommand.get_about())
        .map(|about| about.to_string())
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

fn paint(text: &str, style: Style) -> String {
    format!("{}{}{}", style.render(), text, style.render_reset())
}

#[derive(Parser)]
#[command(name = "aven")]
#[command(about = "Local-first task manager")]
#[command(styles = STYLES)]
pub struct Cli {
    #[arg(long, global = true, help = "Use a specific SQLite database path")]
    pub(crate) db: Option<PathBuf>,
    #[arg(long, global = true, help = "Use a specific workspace by name or key")]
    pub(crate) workspace: Option<String>,
    #[command(subcommand)]
    pub(crate) command: Commands,
}

#[derive(Subcommand)]
pub(crate) enum Commands {
    /// Create a task
    Add(AddArgs),
    /// Inspect and modify task dependencies
    Dep(DepCommand),
    /// Show a task context snapshot
    Context(ContextArgs),
    /// Show task details
    Show(ShowArgs),
    /// List tasks
    List(ListArgs),
    /// Search all tasks in the active workspace
    Search(TaskSearchArgs),
    /// Apply field updates across many tasks
    BulkUpdate(BulkUpdateArgs),
    /// Emit workspace context for AI agents
    Prime(PrimeArgs),
    /// Update task fields
    Update(UpdateArgs),
    /// Append a note to a task
    Note(NoteArgs),
    /// Delete a note from a task
    NoteDelete(NoteDeleteArgs),
    /// Delete a task
    Delete(RefArgs),
    /// Restore a deleted task
    Restore(RefArgs),
    /// Get, diff, and set long text fields safely
    Text(TextCommand),
    /// Manage labels
    Label(LabelCommand),
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
    /// Print a Claude Code skill primer
    Skill,
    /// Diagnose configuration and workspace state
    Doctor(DoctorArgs),
    /// Run or manage the background daemon
    Daemon(DaemonArgs),
    /// Run the sync server
    Server(ServerArgs),
    /// Sync with a remote server
    Sync(SyncArgs),
    /// Spawn tmux task-entry popups
    Tmux(TmuxCommand),
    /// Open the terminal UI
    Tui(TuiArgs),
    #[command(hide = true)]
    Internal(InternalCommand),
}

#[derive(Args)]
pub(crate) struct TuiArgs {
    #[arg(short = 'p', long, num_args = 0..=1, default_missing_value = "")]
    pub(crate) project: Option<String>,
    #[arg(long)]
    pub(crate) add_task: bool,
    #[arg(long)]
    pub(crate) add_task_only: bool,
    #[arg(long)]
    pub(crate) natural: bool,
}

#[derive(Args)]
pub(crate) struct TmuxCommand {
    #[command(subcommand)]
    pub(crate) command: TmuxSubcommand,
}

#[derive(Args)]
pub(crate) struct InternalCommand {
    #[command(subcommand)]
    pub(crate) command: InternalSubcommand,
}

#[derive(Subcommand)]
pub(crate) enum InternalSubcommand {
    #[command(name = "natural-add", hide = true)]
    NaturalAdd(InternalNaturalAddArgs),
}

#[derive(Args)]
pub(crate) struct InternalNaturalAddArgs {
    #[arg(long)]
    pub(crate) workspace_id: String,
    #[arg(long)]
    pub(crate) project: Option<String>,
    #[arg(long, allow_hyphen_values = true)]
    pub(crate) input: String,
}

#[derive(Subcommand)]
pub(crate) enum TmuxSubcommand {
    AddTaskPopup(TmuxAddTaskPopupArgs),
}

#[derive(Args)]
pub(crate) struct TmuxAddTaskPopupArgs {
    #[arg(short = 'p', long, num_args = 0..=1, default_missing_value = "")]
    pub(crate) project: Option<String>,
    #[arg(long, default_value = "80%")]
    pub(crate) width: String,
    #[arg(long, default_value = "80%")]
    pub(crate) height: String,
    #[arg(long)]
    pub(crate) print_binding: bool,
    #[arg(long)]
    pub(crate) natural: bool,
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
    pub(crate) label: Vec<String>,
    #[arg(long)]
    pub(crate) natural: bool,
}

#[derive(Args)]
pub(crate) struct ShowArgs {
    pub(crate) task_ref: String,
    #[arg(long)]
    pub(crate) full: bool,
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
    #[arg(long)]
    pub(crate) all: bool,
    #[arg(long)]
    pub(crate) deleted: bool,
    #[arg(long)]
    pub(crate) ready: bool,
    #[arg(long)]
    pub(crate) blocked: bool,
}

#[derive(Args)]
pub(crate) struct TaskSearchArgs {
    pub(crate) query: Vec<String>,
    #[arg(long, default_value_t = 50)]
    pub(crate) limit: usize,
    #[arg(long, help = "Include deleted tasks")]
    pub(crate) all: bool,
    #[arg(long, help = "Print machine-readable JSON")]
    pub(crate) json: bool,
}

#[derive(Args)]
pub(crate) struct DepCommand {
    #[command(subcommand)]
    pub(crate) command: DepSubcommand,
}

#[derive(Subcommand)]
pub(crate) enum DepSubcommand {
    Add(DepAddArgs),
    Remove(DepRemoveArgs),
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
}

#[derive(Args)]
pub(crate) struct PrimeArgs {
    #[arg(long)]
    pub(crate) project: Option<String>,
}

#[derive(Args)]
pub(crate) struct UpdateArgs {
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
    #[arg(long)]
    pub(crate) label: Vec<String>,
    #[arg(long)]
    pub(crate) remove_label: Vec<String>,
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
pub(crate) struct SearchArgs {
    #[arg(long)]
    pub(crate) search: Option<String>,
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
    Create {
        name: String,
    },
    /// Delete a label
    Delete {
        name: String,
    },
    /// List or search labels
    List(SearchArgs),
}

#[derive(Args)]
pub(crate) struct ProjectCommand {
    #[command(subcommand)]
    pub(crate) command: ProjectSubcommand,
}

#[derive(Subcommand)]
pub(crate) enum ProjectSubcommand {
    Create {
        name: String,
        #[arg(long)]
        path: Option<PathBuf>,
    },
    /// Delete a project
    Delete { project: String },
    /// List or search projects
    List(SearchArgs),
    Rename {
        project: String,
        new_name: String,
        #[arg(long)]
        prefix: Option<String>,
    },
    Path {
        #[command(subcommand)]
        command: ProjectPathSubcommand,
    },
}

#[derive(Subcommand)]
pub(crate) enum ProjectPathSubcommand {
    Add { project: String, path: PathBuf },
    Remove { project: String, path: PathBuf },
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
    #[arg(long)]
    pub(crate) integrity: bool,
}

#[derive(Subcommand)]
pub(crate) enum WorkspaceSubcommand {
    List,
    Create { name: String },
    Rename { workspace: String, new_name: String },
}

#[derive(Args)]
pub(crate) struct ConflictCommand {
    #[command(subcommand)]
    pub(crate) command: ConflictSubcommand,
}

#[derive(Subcommand)]
pub(crate) enum ConflictSubcommand {
    List {
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        field: Option<String>,
    },
    Diff {
        task_ref: String,
        field: String,
    },
    Export {
        task_ref: String,
        field: String,
        #[arg(long)]
        dir: PathBuf,
    },
    Show {
        task_ref: String,
        #[arg(long)]
        field: Option<String>,
    },
    Resolve {
        task_ref: String,
        field: String,
        #[arg(long)]
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
pub(crate) struct TextCommand {
    #[command(subcommand)]
    pub(crate) command: TextSubcommand,
}

#[derive(Subcommand)]
pub(crate) enum TextSubcommand {
    Get(TextGetArgs),
    Diff(TextDiffArgs),
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
    Init,
    Show,
}

#[derive(Args)]
pub(crate) struct DaemonArgs {
    #[command(subcommand)]
    pub(crate) command: Option<DaemonSubcommand>,
}

#[derive(Subcommand)]
pub(crate) enum DaemonSubcommand {
    Install,
    Uninstall,
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
