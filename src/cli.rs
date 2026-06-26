use std::net::SocketAddr;
use std::path::PathBuf;

use clap::builder::styling::{Color, Effects, RgbColor, Styles};
use clap::{Args, Parser, Subcommand};

const ACCENT: Color = Color::Rgb(RgbColor(166, 139, 255));
const BLUE: Color = Color::Rgb(RgbColor(70, 128, 203));
const FG_MUTED: Color = Color::Rgb(RgbColor(191, 188, 180));
const RED: Color = Color::Rgb(RgbColor(239, 82, 86));
const GREEN: Color = Color::Rgb(RgbColor(137, 199, 82));

const STYLES: Styles = Styles::styled()
    .header(ACCENT.on_default().effects(Effects::BOLD))
    .usage(ACCENT.on_default().effects(Effects::BOLD))
    .literal(BLUE.on_default().effects(Effects::BOLD))
    .placeholder(FG_MUTED.on_default())
    .valid(GREEN.on_default())
    .invalid(RED.on_default());

const TOP_LEVEL_HELP: &str = concat!(
    "Local-first task manager\n\n",
    "\x1b[1;38;2;166;139;255mUsage:\x1b[0m aven [\x1b[1;38;2;70;128;203mOPTIONS\x1b[0m] <\x1b[38;2;191;188;180mCOMMAND\x1b[0m>\n\n",
    "\x1b[1;38;2;166;139;255mTask commands:\x1b[0m\n",
    "  \x1b[1;38;2;70;128;203madd\x1b[0m          Create a task\n",
    "  \x1b[1;38;2;70;128;203mdep\x1b[0m          Manage task dependencies\n",
    "  \x1b[1;38;2;70;128;203mshow\x1b[0m         Show task details\n",
    "  \x1b[1;38;2;70;128;203mlist\x1b[0m         List tasks\n",
    "  \x1b[1;38;2;70;128;203mbulk-update\x1b[0m  Update multiple tasks at once\n",
    "  \x1b[1;38;2;70;128;203mprime\x1b[0m        Generate agent-facing workspace context\n",
    "  \x1b[1;38;2;70;128;203mupdate\x1b[0m       Update task fields\n",
    "  \x1b[1;38;2;70;128;203mnote\x1b[0m         Append a note to a task\n",
    "  \x1b[1;38;2;70;128;203mdelete\x1b[0m       Delete a task\n",
    "  \x1b[1;38;2;70;128;203mrestore\x1b[0m      Restore a deleted task\n",
    "  \x1b[1;38;2;70;128;203mtext\x1b[0m         Safely edit long text fields\n\n",
    "\x1b[1;38;2;166;139;255mProject and label commands:\x1b[0m\n",
    "  \x1b[1;38;2;70;128;203mprojects\x1b[0m     List or search projects\n",
    "  \x1b[1;38;2;70;128;203mlabels\x1b[0m       List or search labels\n",
    "  \x1b[1;38;2;70;128;203mlabel\x1b[0m        Manage labels\n",
    "  \x1b[1;38;2;70;128;203mproject\x1b[0m      Manage projects\n",
    "  \x1b[1;38;2;70;128;203mworkspace\x1b[0m    Manage workspaces\n\n",
    "\x1b[1;38;2;166;139;255mConflict commands:\x1b[0m\n",
    "  \x1b[1;38;2;70;128;203mconflict\x1b[0m     Inspect and resolve sync conflicts\n\n",
    "\x1b[1;38;2;166;139;255mSetup and diagnostics:\x1b[0m\n",
    "  \x1b[1;38;2;70;128;203mconfig\x1b[0m       Manage local configuration\n",
    "  \x1b[1;38;2;70;128;203mskill\x1b[0m        Print a Claude Code skill primer\n",
    "  \x1b[1;38;2;70;128;203mdoctor\x1b[0m       Diagnose configuration and workspace state\n\n",
    "\x1b[1;38;2;166;139;255mSync and service commands:\x1b[0m\n",
    "  \x1b[1;38;2;70;128;203mdaemon\x1b[0m       Run or manage the background daemon\n",
    "  \x1b[1;38;2;70;128;203mserver\x1b[0m       Run the sync server\n",
    "  \x1b[1;38;2;70;128;203msync\x1b[0m         Sync with a remote server\n\n",
    "\x1b[1;38;2;166;139;255mInteractive commands:\x1b[0m\n",
    "  \x1b[1;38;2;70;128;203mtmux\x1b[0m         Open tmux popups\n",
    "  \x1b[1;38;2;70;128;203mtui\x1b[0m          Open the terminal UI\n\n",
    "\x1b[1;38;2;166;139;255mHelp:\x1b[0m\n",
    "  \x1b[1;38;2;70;128;203mhelp\x1b[0m         Print this message or the help of the given subcommand(s)\n\n",
    "\x1b[1;38;2;166;139;255mOptions:\x1b[0m\n",
    "      \x1b[1;38;2;70;128;203m--db\x1b[0m <\x1b[38;2;191;188;180mDB\x1b[0m>                Use a specific SQLite database path\n",
    "      \x1b[1;38;2;70;128;203m--workspace\x1b[0m <\x1b[38;2;191;188;180mWORKSPACE\x1b[0m>  Use a specific workspace by name or key\n",
    "  \x1b[1;38;2;70;128;203m-h\x1b[0m, \x1b[1;38;2;70;128;203m--help\x1b[0m                   Print help\n",
);

#[derive(Parser)]
#[command(name = "aven")]
#[command(about = "Local-first task manager")]
#[command(styles = STYLES)]
#[command(override_help = TOP_LEVEL_HELP)]
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
    /// Manage task dependencies
    Dep(DepCommand),
    /// Show task details
    Show(ShowArgs),
    /// List tasks
    List(ListArgs),
    /// Update multiple tasks at once
    BulkUpdate(BulkUpdateArgs),
    /// Generate agent-facing workspace context
    Prime(PrimeArgs),
    /// Update task fields
    Update(UpdateArgs),
    /// Append a note to a task
    Note(NoteArgs),
    /// Delete a task
    Delete(RefArgs),
    /// Restore a deleted task
    Restore(RefArgs),
    /// Safely edit long text fields
    Text(TextCommand),
    /// List or search projects
    Projects(SearchArgs),
    /// List or search labels
    Labels(SearchArgs),
    /// Manage labels
    Label(LabelCommand),
    /// Manage projects
    Project(ProjectCommand),
    /// Manage workspaces
    Workspace(WorkspaceCommand),
    /// Inspect and resolve sync conflicts
    Conflict(ConflictCommand),
    /// Manage local configuration
    Config(ConfigCommand),
    /// Print a Claude Code skill primer
    Skill,
    /// Diagnose configuration and workspace state
    Doctor,
    /// Run or manage the background daemon
    Daemon(DaemonArgs),
    /// Run the sync server
    Server(ServerArgs),
    /// Sync with a remote server
    Sync(SyncArgs),
    /// Open tmux popups
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
    pub(crate) ready: bool,
    #[arg(long)]
    pub(crate) blocked: bool,
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
    Create { name: String },
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
