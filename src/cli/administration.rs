use std::path::PathBuf;

use clap::{Args, Subcommand};

pub(super) const AGENT_HELP: &str = r#"`prime` emits the coding-agent guidance plus live project work. `skill install`
installs the reusable guidance without live task context for detected or
selected agents."#;

pub(super) const CONFIG_HELP: &str = r#"`config get` and `config set` manage the listed scalar keys. `config show` prints
the configuration file path and contents for settings that require direct file
editing, including workspace routes, task-intake agents, TUI columns, and custom
commands."#;

pub(super) const CONFIG_SET_HELP: &str = r#"Accepted values:
  sync.enabled, update.automatic_checks       true | false
  sync.interval_seconds                       positive integer
  sync.qr_glyphs                              auto | sextant | half-block
  local.db_path                               nonempty path | null
  local.image_optimization                    off | paste | on

Use `aven config show` to locate settings managed by direct file editing."#;

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
    /// Target a coding agent; repeat for multiple (default: all detected)
    #[arg(long = "agent", value_enum)]
    pub(crate) agent: Vec<CodingAgentArg>,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CodingAgentArg {
    Claude,
    Opencode,
    Codex,
    Pi,
}

#[derive(Args)]
pub(crate) struct SelfUpdateArgs {
    #[arg(long, help = "Install an available direct update")]
    pub(crate) yes: bool,
}

#[derive(Args)]
pub(crate) struct LabelListArgs {
    /// Restrict labels to names containing this text
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
    /// Restrict projects to keys or names containing this text
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
pub(crate) struct LabelCommand {
    #[command(subcommand)]
    pub(crate) command: LabelSubcommand,
}

#[derive(Subcommand)]
pub(crate) enum LabelSubcommand {
    /// Create a label
    Create {
        /// Label name; normalization is applied before storage
        name: String,
    },
    /// Delete a label
    Delete {
        /// Label name to delete
        name: String,
    },
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
        /// Metadata key to inspect
        key: String,
        #[arg(long, help = "Print machine-readable JSON")]
        json: bool,
    },
    /// Rename a metadata field
    Rename {
        /// Existing metadata key
        key: String,
        /// Replacement metadata key
        new_key: String,
    },
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
        /// Project display name; its key is derived by normalization
        name: String,
        /// Map this directory to the project for inference
        #[arg(long)]
        path: Option<PathBuf>,
    },
    /// Delete a project
    Delete {
        /// Project key or name
        project: String,
    },
    /// List or search projects
    List(ProjectListArgs),
    /// Rename a project
    Rename {
        /// Existing project key or name
        project: String,
        /// Replacement display name
        new_name: String,
        /// Replacement task-ref prefix; otherwise derive it from the name
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
    Add {
        /// Project key or name
        project: String,
        /// Directory from which this project should be inferred
        path: PathBuf,
    },
    /// Remove a path mapping from a project
    Remove {
        /// Project key or name
        project: String,
        /// Mapped directory to remove
        path: PathBuf,
    },
    /// List project path mappings
    List {
        /// Optional project key or name to restrict the list
        project: Option<String>,
    },
}

#[derive(Args)]
pub(crate) struct WorkspaceCommand {
    #[command(subcommand)]
    pub(crate) command: WorkspaceSubcommand,
}

#[derive(Subcommand)]
pub(crate) enum WorkspaceSubcommand {
    /// List workspaces
    List,
    /// Create a workspace
    Create {
        /// Workspace display name; its key is derived by normalization
        name: String,
    },
    /// Rename a workspace
    Rename {
        /// Existing workspace key or name
        workspace: String,
        /// Replacement display name
        new_name: String,
    },
}

#[derive(Args)]
pub(crate) struct ConfigCommand {
    #[command(subcommand)]
    pub(crate) command: ConfigSubcommand,
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
    #[command(after_long_help = CONFIG_SET_HELP)]
    Set(ConfigSetArgs),
}

#[derive(Args)]
pub(crate) struct ConfigGetArgs {
    /// Configuration key to print
    pub(crate) key: ConfigKey,
}

#[derive(Args)]
pub(crate) struct ConfigSetArgs {
    /// Configuration key to update
    pub(crate) key: ConfigKey,
    /// New value, or null to clear an optional setting
    pub(crate) value: String,
}

#[derive(clap::ValueEnum, Clone, Copy)]
pub(crate) enum ConfigKey {
    #[value(name = "sync.enabled")]
    SyncEnabled,
    #[value(name = "sync.interval_seconds")]
    SyncIntervalSeconds,
    #[value(name = "sync.qr_glyphs")]
    SyncQrGlyphs,
    #[value(name = "update.automatic_checks")]
    UpdateAutomaticChecks,
    #[value(name = "local.db_path")]
    LocalDbPath,
    #[value(name = "local.image_optimization")]
    LocalImageOptimization,
}
