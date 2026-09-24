use std::net::SocketAddr;
use std::path::PathBuf;

use clap::{Args, Subcommand};

pub(super) const CONFLICT_EXAMPLES: &str = r#"Examples:
  aven conflict show APP-7KQ9
  aven conflict diff APP-7KQ9 description
  aven conflict resolve APP-7KQ9 description --use VARIANT_TOKEN
  aven conflict resolve APP-7KQ9 description --value-file resolved.md

Inspect both variants before resolving. Variant tokens come from `conflict show`.
--use takes precedence over explicit values. Without --use, supply exactly one
of --value, --value-file, or --value-stdin."#;

pub(super) const SERVER_HELP: &str = r#"Loopback binds may run without authentication. Private and public binds require
sync.auth_token in the configuration file. Public binds also require
--unsafe-public-bind. Aven does not provide TLS termination."#;

pub(super) const SYNC_HELP: &str = r#"The server URL comes from --server, AVEN_SYNC_SERVER, or sync.server_url, in
that order. Authentication and other sync settings live in the configuration
file. Run `aven config show` to inspect the active file and `aven doctor` to
diagnose routing and sync configuration."#;

pub(super) const PAIR_HELP: &str = r#"Pairing reads configuration and produces an invitation without opening a task
database or contacting the sync server. The invitation requires a nonempty
sync.auth_token and a phone-reachable HTTP or HTTPS server URL. Use --server
when the configured URL is loopback or available only from the desktop.

Use --copy on the local desktop to put the invitation on the clipboard instead
of displaying a QR code. The invitation contains credentials; clipboard history
and sharing services may retain it. SSH clipboard copying is not supported."#;

#[derive(Args)]
pub(crate) struct ConflictCommand {
    #[command(subcommand)]
    pub(crate) command: ConflictSubcommand,
}

#[derive(Subcommand)]
pub(crate) enum ConflictSubcommand {
    /// List unresolved sync conflicts
    List {
        /// Restrict conflicts to a project by key or name
        #[arg(long)]
        project: Option<String>,
        /// Restrict conflicts to a field name
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
    Diff {
        /// Task ref with the conflict
        task_ref: String,
        /// Conflicted field name
        field: String,
    },
    /// Export conflicting values to files
    Export {
        /// Task ref with the conflict
        task_ref: String,
        /// Conflicted field name
        field: String,
        /// Directory to receive one file per variant
        #[arg(long)]
        dir: PathBuf,
    },
    /// Show conflict details for a task
    Show {
        /// Task or recurring-series ref with conflicts
        task_ref: String,
        /// Restrict output to one field
        #[arg(long)]
        field: Option<String>,
        #[arg(long, help = "Print machine-readable JSON")]
        json: bool,
    },
    /// Resolve a sync conflict
    #[command(after_long_help = CONFLICT_EXAMPLES)]
    Resolve {
        /// Task or recurring-series ref with the conflict
        task_ref: String,
        /// Conflicted field name
        field: String,
        /// Select an exact variant token printed by `conflict show`
        #[arg(long = "use")]
        use_variant: Option<String>,
        /// Resolve with this explicit value
        #[arg(long)]
        value: Option<String>,
        /// Read the explicit resolution value from a UTF-8 file
        #[arg(long)]
        value_file: Option<PathBuf>,
        /// Read the explicit resolution value from standard input
        #[arg(long)]
        value_stdin: bool,
    },
}

#[derive(Args)]
pub(crate) struct DaemonArgs {
    #[command(subcommand)]
    pub(crate) command: Option<DaemonSubcommand>,
}

#[derive(Subcommand)]
pub(crate) enum DaemonSubcommand {
    /// Report daemon installation and runtime health without changing it
    Status(StatusArgs),
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
#[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
pub(crate) struct ServerArgs {
    #[command(subcommand)]
    pub(crate) command: Option<ServerSubcommand>,
    /// Listen address; port 0 asks the OS to choose a free port
    #[arg(long, default_value = "127.0.0.1:0")]
    pub(crate) bind: SocketAddr,
    /// SQLite path; blobs use local.blob_dir or a path derived from this path
    #[arg(long, required = true)]
    pub(crate) data: Option<PathBuf>,
    /// Confirm an authenticated public bind without built-in TLS
    #[arg(long)]
    pub(crate) unsafe_public_bind: bool,
    /// Serve end-to-end encrypted sync from storage prepared by `server setup`
    #[arg(long, conflicts_with = "unsafe_public_bind")]
    pub(crate) encrypted: bool,
}

#[derive(Subcommand)]
pub(crate) enum ServerSubcommand {
    /// Prepare encrypted server storage and print its setup invitation
    #[command(after_long_help = SERVER_SETUP_HELP)]
    Setup(ServerSetupArgs),
}

#[derive(Args)]
pub(crate) struct ServerSetupArgs {
    /// SQLite path of the encrypted server storage
    #[arg(long)]
    pub(crate) data: PathBuf,
    /// Server URL that devices reach: HTTPS, or HTTP on a loopback address
    #[arg(long)]
    pub(crate) url: String,
}

pub(super) const SERVER_SETUP_HELP: &str = r#"The setup invitation lets one device claim this server and set up sync from
its database. It expires after one hour; running setup again replaces it until
a device has claimed the server. Serve the storage with
`aven server --encrypted --data PATH`. The encrypted server binds only loopback
addresses; put a TLS reverse proxy in front of it for other devices."#;

#[derive(Args)]
pub(crate) struct SyncArgs {
    #[command(subcommand)]
    pub(crate) command: Option<SyncSubcommand>,
    /// Override the configured sync server URL
    #[arg(long)]
    pub(crate) server: Option<String>,
    /// Emit the versioned sync result as JSON
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Subcommand)]
pub(crate) enum SyncSubcommand {
    /// Produce a pairing invitation for Aven iOS onboarding
    #[command(after_long_help = PAIR_HELP)]
    Pair(PairArgs),
    /// Report sync configuration, health, progress, and pending work
    Status(StatusArgs),
    /// Set up encrypted sync from this database with a server setup invitation
    #[command(after_long_help = SETUP_HELP)]
    Setup(SetupArgs),
    /// Invite another device to encrypted sync and wait until it joins
    #[command(after_long_help = INVITE_HELP)]
    Invite,
    /// Join encrypted sync from an empty database with a device invitation
    #[command(after_long_help = JOIN_HELP)]
    Join,
}

pub(super) const SETUP_HELP: &str = r#"Paste the invitation printed by `aven server setup`, or pipe it to standard
input. Setup previews this database and asks for confirmation; use --yes when
standard input is not a terminal. This database becomes the starting point of
the synced data. Afterwards it can no longer use plaintext sync, backup
restore, or import. Rerun the same command to resume an interrupted setup."#;

pub(super) const INVITE_HELP: &str = r#"The invitation is printed to standard output. Anyone with it can access all
synced data and manage devices. Keep this command running until the other
device joins; it stops when the invitation expires after ten minutes. Sync on
this device pauses until the invitation is used. An unused invitation stops
pausing sync once it expires, unless keys were already sent to the other device;
then sync resumes only after that device joins."#;

pub(super) const JOIN_HELP: &str = r#"Paste the invitation printed by `aven sync invite`, or pipe it to standard
input, while the inviting device waits. The database must be empty. Joining
downloads the synced data and then its images. Rerun the same command to resume
an interrupted join."#;

#[derive(Args)]
pub(crate) struct SetupArgs {
    /// Skip the confirmation prompt
    #[arg(long)]
    pub(crate) yes: bool,
}

#[derive(Args)]
pub(crate) struct PairArgs {
    /// Use a phone-reachable server URL for this invitation
    #[arg(long)]
    pub(crate) server: Option<String>,
    /// Copy the invitation to the local clipboard instead of displaying a QR code
    #[arg(long)]
    pub(crate) copy: bool,
}

#[derive(Args)]
pub(crate) struct StatusArgs {
    /// Emit the versioned status report as JSON
    #[arg(long)]
    pub(crate) json: bool,
}
