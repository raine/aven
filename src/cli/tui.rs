use crate::ids::WorkspaceId;
use std::path::PathBuf;

use clap::{ArgGroup, Args, Subcommand};

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
    Ready,
    Blocked,
    Overdue,
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
        conflicts_with_all = ["project", "all_projects", "view", "layout", "label", "priority", "add_task", "add_task_only"]
    )]
    pub(crate) task_ref: Option<String>,
    /// Start in this named view
    #[arg(long, value_enum, conflicts_with = "add_task_only")]
    pub(crate) view: Option<TuiViewArg>,
    /// Present the selected query as a list or columns
    #[arg(long, value_enum, conflicts_with = "add_task_only")]
    pub(crate) layout: Option<TuiLayoutArg>,
    /// Start in project scope; omit the value to infer from the current directory
    #[arg(short = 'p', long, num_args = 0..=1, default_missing_value = "", conflicts_with = "all_projects")]
    pub(crate) project: Option<String>,
    /// Start across all projects, ignoring the current directory
    #[arg(short = 'A', long)]
    pub(crate) all_projects: bool,
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
    /// Reloads the daemon's LaunchAgent from its plist on disk.
    #[command(name = "reload-daemon", hide = true)]
    ReloadDaemon,
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
