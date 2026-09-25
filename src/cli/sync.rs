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

pub(super) const SERVER_HELP: &str = r#"Prepare storage with `aven server setup` first. The server binds only
loopback addresses and does not terminate TLS; put a TLS reverse proxy in front
of it for other devices."#;

pub(super) const SYNC_HELP: &str = r#"Sync is end-to-end encrypted. Start it on one device with `aven sync setup`
and add other devices with `aven sync invite` and `aven sync join`. The server
is the one chosen during setup or join. Set sync.enabled to let the daemon sync
automatically."#;

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
    /// SQLite path of storage prepared by `server setup`
    #[arg(long, required = true)]
    pub(crate) data: Option<PathBuf>,
}

#[derive(Subcommand)]
pub(crate) enum ServerSubcommand {
    /// Prepare server storage and print its setup invitation
    #[command(after_long_help = SERVER_SETUP_HELP)]
    Setup(ServerSetupArgs),
}

#[derive(Args)]
pub(crate) struct ServerSetupArgs {
    /// SQLite path of the server storage
    #[arg(long)]
    pub(crate) data: PathBuf,
    /// Server URL that devices reach: HTTPS, or HTTP on a loopback address
    #[arg(long)]
    pub(crate) url: String,
}

pub(super) const SERVER_SETUP_HELP: &str = r#"The setup invitation lets one device claim this server and set up sync from
its database. It expires after one hour; until a device has claimed the
server, running setup again replaces it. The replacement keeps the server's
setup identity, so a device whose setup was interrupted resumes with the new
invitation. Serve the storage with `aven server --data PATH`. The server binds
only loopback addresses; put a TLS reverse proxy in front of it for other
devices."#;

#[derive(Args)]
pub(crate) struct SyncArgs {
    #[command(subcommand)]
    pub(crate) command: Option<SyncSubcommand>,
    /// Emit the versioned sync result as JSON
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Subcommand)]
pub(crate) enum SyncSubcommand {
    /// Report local sync state and pending work
    Status(StatusArgs),
    /// Set up sync from this database with a server setup invitation
    #[command(after_long_help = SETUP_HELP)]
    Setup(SetupArgs),
    /// Invite another device to sync and wait until it joins
    #[command(after_long_help = INVITE_HELP)]
    Invite,
    /// Join sync from an empty database with a device invitation
    #[command(after_long_help = JOIN_HELP)]
    Join(JoinArgs),
    /// List or remove the devices that take part in sync
    #[command(after_long_help = DEVICE_HELP)]
    Device(DeviceCommand),
}

pub(super) const DEVICE_HELP: &str = r#"Examples:
  aven sync device list --json
  aven sync device remove DEVICE_ID_OR_PREFIX --json

Both commands contact the server. Remove a device from any other device in
sync, using its full ID or a unique prefix of at least four hexadecimal
characters. Removal stops the device from syncing and rotates the keys for
future changes; it does not erase data the device already downloaded. Rerun an
interrupted removal with the same full device ID to resume it."#;

#[derive(Args)]
pub(crate) struct DeviceCommand {
    #[command(subcommand)]
    pub(crate) command: DeviceSubcommand,
}

#[derive(Subcommand)]
pub(crate) enum DeviceSubcommand {
    /// List devices in sync, marking the current device
    List {
        #[arg(long, help = "Print machine-readable JSON")]
        json: bool,
    },
    /// Remove another device from sync and rotate keys for future changes
    Remove {
        /// Full device ID, or a unique prefix of at least 4 hex characters
        device_id: String,
        #[arg(long, help = "Print machine-readable JSON")]
        json: bool,
    },
}

pub(super) const SETUP_HELP: &str = r#"Paste the invitation printed by `aven server setup`, or pipe it to standard
input. Setup previews this database and asks for confirmation; use --yes when
standard input is not a terminal. This database becomes the starting point of
the synced data. Afterwards it can no longer use backup restore or import. Rerun
the same command to resume an interrupted setup."#;

pub(super) const INVITE_HELP: &str = r#"The invitation is printed to standard output. Anyone with it can access all
synced data and manage devices. Keep this command running until the other
device joins; it stops when the invitation expires after ten minutes. Sync keeps
running meanwhile. If the invitation expires after keys may have been sent to a
device that never joined, the next sync changes keys before uploading new
changes; that device can still read anything it received before."#;

pub(super) const JOIN_HELP: &str = r#"Paste the invitation printed by `aven sync invite`, or pipe it to standard
input, while the inviting device waits. The database must be empty. Joining
downloads the synced data and then its images. Rerun the same command to resume
an interrupted join.

If the invitation expired before the inviting device added this device, create a
new invitation on that same device and pass it with --new-invitation. The
earlier invitation is kept, so an admission that already happened still
completes the join. Resuming can complete the join with any retained invitation."#;

#[derive(Args)]
pub(crate) struct JoinArgs {
    /// Continue an unfinished join with a new invitation from the same inviting device
    #[arg(long)]
    pub(crate) new_invitation: bool,
}

#[derive(Args)]
pub(crate) struct SetupArgs {
    /// Skip the confirmation prompt
    #[arg(long)]
    pub(crate) yes: bool,
}

#[derive(Args)]
pub(crate) struct StatusArgs {
    /// Emit the versioned status report as JSON
    #[arg(long)]
    pub(crate) json: bool,
}
