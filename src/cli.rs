use std::path::PathBuf;

use clap::{Parser, Subcommand};

mod administration;
mod data_safety;
mod help;
mod recurrence;
mod relationships;
mod sync;
mod tasks;
mod tui;

#[cfg(test)]
mod tests;

use administration::{AGENT_HELP, CONFIG_HELP};
use data_safety::{BACKUP_HELP, EXPORT_HELP, IMPORT_HELP};
use help::STYLES;
use recurrence::RECUR_HELP;
use sync::{CONFLICT_EXAMPLES, SERVER_HELP, SYNC_HELP};
use tasks::{
    ADD_EXAMPLES, BULK_UPDATE_EXAMPLES, EDIT_EXAMPLES, LIST_HELP, NOTE_HELP, TEXT_EXAMPLES,
};

pub(crate) use administration::{
    CodingAgentArg, ConfigCommand, ConfigKey, ConfigSubcommand, LabelCommand, LabelListArgs,
    LabelSubcommand, MetadataCommand, MetadataSubcommand, ProjectCommand, ProjectListArgs,
    ProjectPathSubcommand, ProjectSubcommand, SelfUpdateArgs, SkillCommand, SkillInstallArgs,
    SkillSubcommand, WorkspaceCommand, WorkspaceSubcommand,
};
pub(crate) use data_safety::{
    AttachmentAddArgs, AttachmentCommand, AttachmentDeleteArgs, AttachmentGetArgs,
    AttachmentListArgs, AttachmentPruneArgs, AttachmentSubcommand, BackupCommand,
    BackupRestoreArgs, BackupSubcommand, DoctorArgs, ExportArgs, ImportArgs,
};
pub(crate) use help::parse_from;
pub(crate) use recurrence::{
    RecurCommand, RecurEditArgs, RecurHistoryArgs, RecurListArgs, RecurRefArgs, RecurShowArgs,
    RecurStopArgs, RecurSubcommand,
};
pub(crate) use relationships::{
    DepCommand, DepSubcommand, EpicCommand, EpicSubcommand, RelatedCommand, RelatedSubcommand,
};
pub(crate) use sync::{
    ConflictCommand, ConflictSubcommand, DaemonArgs, DaemonSubcommand, DeviceSubcommand,
    ServerArgs, ServerSetupArgs, ServerSubcommand, SetupArgs, SyncArgs, SyncSubcommand,
};
pub(crate) use tasks::{
    AddArgs, BulkUpdateArgs, ContextArgs, ListArgs, NoteArgs, NoteDeleteArgs, PrimeArgs, RefArgs,
    ShowArgs, TaskEditArgs, TaskSearchArgs, TextCommand, TextSubcommand,
};
#[cfg(test)]
pub(crate) use tui::TuiPriorityArg;
pub(crate) use tui::{
    InternalCommand, InternalDemoSnapshotArgs, InternalNaturalAddArgs, InternalSubcommand, TuiArgs,
    TuiLayoutArg, TuiViewArg,
};

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
    #[command(after_long_help = ADD_EXAMPLES)]
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
    #[command(after_long_help = LIST_HELP)]
    List(ListArgs),
    /// Search tasks in the active workspace
    Search(TaskSearchArgs),
    /// Apply field updates across many tasks
    #[command(after_long_help = BULK_UPDATE_EXAMPLES)]
    BulkUpdate(BulkUpdateArgs),
    /// Emit workspace context for AI agents
    #[command(after_long_help = AGENT_HELP)]
    Prime(PrimeArgs),
    /// Edit task fields
    #[command(after_long_help = EDIT_EXAMPLES)]
    Edit(TaskEditArgs),
    /// Check for and install an aven update
    Update(SelfUpdateArgs),
    /// Append a note to a task
    #[command(after_long_help = NOTE_HELP)]
    Note(NoteArgs),
    /// Delete a note from a task
    NoteDelete(NoteDeleteArgs),
    /// Delete a task
    Delete(RefArgs),
    /// Restore a deleted task
    Restore(RefArgs),
    /// Manage recurring task series
    #[command(after_long_help = RECUR_HELP)]
    Recur(RecurCommand),
    /// Get, diff, and set long text fields safely
    #[command(after_long_help = TEXT_EXAMPLES)]
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
    #[command(after_long_help = CONFLICT_EXAMPLES)]
    Conflict(ConflictCommand),
    /// Manage local configuration
    #[command(after_long_help = CONFIG_HELP)]
    Config(ConfigCommand),
    /// Back up or restore local data
    #[command(after_long_help = BACKUP_HELP)]
    Backup(BackupCommand),
    /// Export user data as portable JSON
    #[command(after_long_help = EXPORT_HELP)]
    Export(ExportArgs),
    /// Import portable JSON data
    #[command(after_long_help = IMPORT_HELP)]
    Import(ImportArgs),
    /// Print or install the coding-agent skill
    #[command(after_long_help = AGENT_HELP)]
    Skill(SkillCommand),
    /// Diagnose startup, configuration, database, and workspace state without repairs
    Doctor(DoctorArgs),
    /// Manage task attachments
    Attachment(AttachmentCommand),
    /// Run or manage the background daemon
    Daemon(DaemonArgs),
    /// Run the sync server
    #[command(after_long_help = SERVER_HELP)]
    Server(ServerArgs),
    /// Sync with a remote server
    #[command(after_long_help = SYNC_HELP)]
    Sync(SyncArgs),
    /// Open the terminal UI
    Tui(TuiArgs),
    /// Explore aven with disposable sample tasks
    Demo,
    #[command(hide = true)]
    Internal(InternalCommand),
}

#[cfg(test)]
use help::HELP_SECTIONS;
