use std::net::SocketAddr;
use std::path::PathBuf;

use clap::builder::styling::{AnsiColor, Effects, Styles};
use clap::{Args, Parser, Subcommand};

const STYLES: Styles = Styles::styled()
    .header(AnsiColor::Green.on_default().effects(Effects::BOLD))
    .usage(AnsiColor::Green.on_default().effects(Effects::BOLD))
    .literal(AnsiColor::Cyan.on_default().effects(Effects::BOLD))
    .placeholder(AnsiColor::Cyan.on_default());

#[derive(Parser)]
#[command(name = "aven")]
#[command(about = "Local-first task manager")]
#[command(styles = STYLES)]
pub struct Cli {
    #[arg(long, global = true)]
    pub(crate) db: Option<PathBuf>,
    #[arg(long, global = true)]
    pub(crate) workspace: Option<String>,
    #[command(subcommand)]
    pub(crate) command: Commands,
}

#[derive(Subcommand)]
pub(crate) enum Commands {
    Add(AddArgs),
    Dep(DepCommand),
    Show(ShowArgs),
    List(ListArgs),
    BulkUpdate(BulkUpdateArgs),
    Prime(PrimeArgs),
    Update(UpdateArgs),
    Note(NoteArgs),
    Projects(SearchArgs),
    Labels(SearchArgs),
    Label(LabelCommand),
    Project(ProjectCommand),
    Delete(RefArgs),
    Restore(RefArgs),
    Conflict(ConflictCommand),
    Config(ConfigCommand),
    Daemon(DaemonArgs),
    Server(ServerArgs),
    Sync(SyncArgs),
    Workspace(WorkspaceCommand),
    Text(TextCommand),
    Skill,
    Doctor,
    Tmux(TmuxCommand),
    #[command(hide = true)]
    Internal(InternalCommand),
    Tui(TuiArgs),
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
