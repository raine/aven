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
#[command(name = "atm")]
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
    Show(ShowArgs),
    List(ListArgs),
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
    Daemon(DaemonCommand),
    Server(ServerArgs),
    Sync(SyncArgs),
    Workspace(WorkspaceCommand),
    Skill,
    Doctor,
    Tui,
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
    Path {
        #[command(subcommand)]
        command: ProjectPathSubcommand,
    },
}

#[derive(Subcommand)]
pub(crate) enum ProjectPathSubcommand {
    Add { project: String, path: PathBuf },
    Remove { project: String, path: PathBuf },
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

#[derive(Subcommand)]
pub(crate) enum ConfigSubcommand {
    Init,
    Show,
}

#[derive(Args)]
pub(crate) struct DaemonCommand {
    #[command(subcommand)]
    pub(crate) command: DaemonSubcommand,
}

#[derive(Subcommand)]
pub(crate) enum DaemonSubcommand {
    Run,
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
